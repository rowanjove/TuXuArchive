param([string]$OutputDirectory = "")
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location -LiteralPath $root

function Invoke-Native {
    param([string]$File, [string[]]$ArgumentList)
    & $File @ArgumentList
    if ($LASTEXITCODE -ne 0) { throw "$File 退出码 $LASTEXITCODE" }
}
function Get-Sha256([string]$Path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant() }
function Write-Utf8([string]$Path, [string]$Text) {
    [IO.File]::WriteAllText($Path, $Text, (New-Object Text.UTF8Encoding $false))
}

if (!$OutputDirectory) {
    $OutputDirectory = Join-Path $root ("dist-release\" + (Get-Date -Format "yyyyMMdd-HHmmss"))
}
$release = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $release) { throw "输出目录已存在：$release" }
New-Item -ItemType Directory -Path $release | Out-Null

$staging = Join-Path $release "staging"
$portableRoot = Join-Path $staging "TuXuArchive"
New-Item -ItemType Directory -Path $portableRoot -Force | Out-Null

$sourceFiles = @()
foreach ($dir in @("src", "crates", "src-tauri", "launcher", "tests", ".cargo", "scripts")) {
    $p = Join-Path $root $dir
    if (Test-Path -LiteralPath $p) {
        $sourceFiles += Get-ChildItem -LiteralPath $p -File -Recurse | Where-Object { $_.FullName -notmatch '\\gen\\' }
    }
}
foreach ($name in @("index.html", "Cargo.toml", "Cargo.lock", "package.json", "package-lock.json", "build-release.ps1", "tsconfig.json", "vite.config.ts")) {
    $p = Join-Path $root $name
    if (Test-Path -LiteralPath $p) {
        $sourceFiles += Get-Item -LiteralPath $p
    }
}
$sourceRows = @($sourceFiles | Sort-Object FullName | ForEach-Object { [ordered]@{path=$_.FullName.Substring($root.Length).TrimStart('\');sha256=Get-Sha256 $_.FullName} })
Write-Utf8 (Join-Path $release "source-manifest.json") (ConvertTo-Json -Depth 5 -InputObject $sourceRows)
$sourceHash = Get-Sha256 (Join-Path $release "source-manifest.json")

$target = "x86_64-pc-windows-msvc"
Invoke-Native "npx.cmd" @("tauri", "build", "--no-bundle", "--target", $target, "--", "--locked")
$builtDirectory = Join-Path $root "target\$target\release"
$built = Join-Path $builtDirectory "TuXuArchive.exe"
if (!(Test-Path -LiteralPath $built)) { throw "未找到主程序：$built" }
Copy-Item -LiteralPath $built -Destination (Join-Path $portableRoot "TuXuArchive.exe")

$note = @'
图序归档 (TuXuArchive) v1.0.0 — 通用图片批量分类、重命名与归档工具

运行方式：
- 绿色免安装 ZIP：解压后直接运行 TuXuArchive.exe；
- 免安装单文件 EXE：双击 TuXuArchive_v1.0_x64.exe 即可自动校验并启动。

系统要求：
- Windows 10 / Windows 11 (x64)
- 已静态链接 MSVC 运行库（+crt-static）与内置 SQLite，无需安装 VC++ Redistributable、Rust 或 Node。
- 界面依赖系统自带 Microsoft Edge WebView2 Runtime。

数据与独立运行说明：
- 自解压运行时目录：%LOCALAPPDATA%\TuXuArchive\runtime\<指纹>
- 任务草稿、历史与配置目录：%LOCALAPPDATA%\TuXuArchive\data
- 进程互斥锁与数据目录完全独立，卸载或删除程序不会删除原图片及已归档文件。

核心使用流程：
1. 导入与位置：拖入或选择源图片文件夹，设置输出根目录与任务名称（全程只读不改原图）；
2. 分类与命名：自由选择输出目录层级与命名规则（支持日期、分类、标签、原文件名、自定义前后缀及连续编号），支持按时间、数量、预设方案或手动多选分组；
3. 预览与调整：实时对照原图与目标路径，支持批量移动分类、打标签、重排或进入大图精修模式；
4. 核对与安全归档：零覆盖检查通过后执行原子复制，支持中断恢复、清单导出及基于文件身份与 SHA-256 校验的安全撤销。
'@
Write-Utf8 (Join-Path $portableRoot "README.txt") $note

$manifest = @("TuXuArchive.exe", "README.txt") | ForEach-Object { "$(Get-Sha256 (Join-Path $portableRoot $_))  $_" }
Write-Utf8 (Join-Path $portableRoot "manifest.sha256") (($manifest -join [char]10) + [char]10)

Add-Type -AssemblyName System.IO.Compression.FileSystem
$payload = Join-Path $release "payload.zip"
[IO.Compression.ZipFile]::CreateFromDirectory($portableRoot, $payload, [IO.Compression.CompressionLevel]::Optimal, $false)
$portableZip = Join-Path $release "TuXuArchive_v1.0_Portable_x64.zip"
[IO.Compression.ZipFile]::CreateFromDirectory($staging, $portableZip, [IO.Compression.CompressionLevel]::Optimal, $false)

$previousZipPath = $env:TUXU_ZIP_PATH
try {
    $env:TUXU_ZIP_PATH = $payload
    Invoke-Native "cargo" @("build", "-p", "tuxu-archive-launcher", "--release", "--target", $target, "--locked")
} finally {
    $env:TUXU_ZIP_PATH = $previousZipPath
}
Copy-Item -LiteralPath (Join-Path $builtDirectory "TuXuArchive_v1_0_x64.exe") -Destination (Join-Path $release "TuXuArchive_v1.0_x64.exe")

$shippedBinaries = @((Join-Path $release "TuXuArchive_v1.0_x64.exe"))
$shippedBinaries += @(Get-ChildItem -LiteralPath $portableRoot -Recurse -File | Where-Object { $_.Extension -in @('.exe', '.dll') } | Select-Object -ExpandProperty FullName)
& (Join-Path $root "scripts\check-windows-dependencies.ps1") -BinaryPath $shippedBinaries -ReportPath (Join-Path $release "dependency-report.json")

$sourceChanged = @($sourceRows | Where-Object { (Get-Sha256 (Join-Path $root $_.path)) -ne $_.sha256 })
if ($sourceChanged.Count) { throw "构建期间源码发生变化，请重新构建" }

$buildInfo = [ordered]@{
    product = "TuXuArchive"
    displayName = "图序归档"
    version = "1.0.0"
    builtAt = (Get-Date).ToString("o")
    target = $target
    crt = "static (+crt-static)"
    dependencyReportSha256 = Get-Sha256 (Join-Path $release "dependency-report.json")
    sourceManifestSha256 = $sourceHash
    cargoLockSha256 = Get-Sha256 (Join-Path $root "Cargo.lock")
    npmLockSha256 = Get-Sha256 (Join-Path $root "package-lock.json")
    rust = (& rustc --version)
    cargo = (& cargo --version)
    node = (& node --version)
    npm = (& npm.cmd --version)
    tauri = (& npx.cmd tauri --version)
}
Write-Utf8 (Join-Path $release "build-info.json") (ConvertTo-Json -Depth 5 -InputObject $buildInfo)

$sums = @("TuXuArchive_v1.0_Portable_x64.zip", "TuXuArchive_v1.0_x64.exe") | ForEach-Object { "$(Get-Sha256 (Join-Path $release $_))  $_" }
Write-Utf8 (Join-Path $release "SHA256SUMS.txt") (($sums -join [char]10) + [char]10)

Write-Output "Release build completed: $release"
Get-ChildItem -LiteralPath $release -File | Select-Object Name, Length
