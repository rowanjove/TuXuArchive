param(
    [Parameter(Mandatory)][string[]]$BinaryPath,
    [Parameter(Mandatory)][string]$ReportPath
)
$ErrorActionPreference = "Stop"

# dumpbin ships with the MSVC toolchain already required to build this application.
$dumpbinCommand = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
$dumpbin = if ($dumpbinCommand) { $dumpbinCommand.Source } else { $null }
if (!$dumpbin) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path -LiteralPath $vswhere) {
        $installation = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($LASTEXITCODE -ne 0) { throw "vswhere 查询失败" }
        if ($installation) {
            $dumpbin = Get-ChildItem -Path (Join-Path $installation 'VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe') |
                Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
        }
    }
}
if (!$dumpbin) { throw "找不到 MSVC dumpbin.exe，无法验证交付文件的 DLL 依赖" }

$systemDlls = @(
    'advapi32.dll', 'bcrypt.dll', 'bcryptprimitives.dll', 'combase.dll',
    'comctl32.dll', 'crypt32.dll', 'dwmapi.dll', 'gdi32.dll', 'imm32.dll',
    'kernel32.dll', 'ntdll.dll', 'ole32.dll', 'oleaut32.dll', 'propsys.dll',
    'rpcrt4.dll', 'secur32.dll', 'shell32.dll', 'shlwapi.dll', 'user32.dll',
    'userenv.dll', 'uxtheme.dll', 'version.dll', 'winhttp.dll', 'winmm.dll',
    'ws2_32.dll', 'wtsapi32.dll'
)
$rows = @()
$failures = @()
foreach ($binary in $BinaryPath) {
    $resolved = (Resolve-Path -LiteralPath $binary).Path
    $output = @(& $dumpbin /nologo /dependents $resolved 2>&1)
    if ($LASTEXITCODE -ne 0) { throw "dumpbin 检查失败：$resolved`n$($output -join [char]10)" }
    $dependencies = @($output | ForEach-Object {
        if ("$_" -match '^\s+([A-Za-z0-9_.-]+\.dll)\s*$') { $Matches[1].ToLowerInvariant() }
    } | Sort-Object -Unique)
    if (!$dependencies.Count) { throw "未解析到 DLL 依赖，拒绝把空结果视为通过：$resolved" }
    $runtime = @($dependencies | Where-Object {
        $_ -match '^(vcruntime|msvcp|msvcr\d|concrt|vcomp|mfc|mfcm|atl\d|ucrtbase|ucrtbased|api-ms-win-crt-)'
    })
    $unexpected = @($dependencies | Where-Object {
        $_ -notin $systemDlls -and $_ -notmatch '^(api|ext)-ms-win-'
    })
    if ($runtime.Count) { $failures += "$resolved 仍依赖动态运行库：$($runtime -join ', ')" }
    if ($unexpected.Count) { $failures += "$resolved 存在未批准 DLL：$($unexpected -join ', ')" }
    $rows += [ordered]@{
        path = $resolved
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $resolved).Hash.ToLowerInvariant()
        dependencies = $dependencies
        dynamicRuntimeDependencies = $runtime
        unexpectedDependencies = $unexpected
    }
}
$report = [ordered]@{
    passed = ($failures.Count -eq 0)
    scope = 'Normal and delay-load PE imports of shipped binaries; excludes Windows/WebView2 internals and runtime LoadLibrary calls'
    tool = $dumpbin
    binaries = $rows
    failures = $failures
}
[IO.File]::WriteAllText([IO.Path]::GetFullPath($ReportPath), (ConvertTo-Json -Depth 8 -InputObject $report), (New-Object Text.UTF8Encoding $false))
if ($failures.Count) { throw ($failures -join [char]10) }
Write-Output "DLL dependency check passed: $($BinaryPath.Count) binaries"
