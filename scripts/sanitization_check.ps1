param(
    [string]$ProjectDir = (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path))
)
$ErrorActionPreference = "Stop"

$patterns = @(
    "电网", "国网", "南网", "南方电网", "供电",
    "无人机", "飞手", "巡检", "杆塔", "塔型", "塔号", "塔基", "塔头", "节号",
    "导线", "地线", "绝缘子", "防鸟刺", "引流线", "横担", "小号侧", "大号侧", "光缆",
    "同塔双回", "耐张塔", "直线塔", "\bstst\b",
    "26241", "C:\\Users\\", "D:\\stst",
    "InspectionPhotoOrganizer", "inspection-photo-organizer", "local\.inspection",
    "IPO_DATA_DIR", "IPO_NO_UI", "IPO_ZIP_PATH", "IPO_TEST_", "\.ipo-"
)

$regex = ($patterns -join "|")
$excludeRegex = '\\(target|node_modules|dist|dist-release|dist-candidate|\.git)\\|sanitization_check\.ps1$|DESENSITIZATION_REPORT\.md$'

$files = Get-ChildItem -LiteralPath $ProjectDir -Recurse -File | Where-Object {
    $_.FullName -notmatch $excludeRegex -and
    $_.Extension -in @('.rs', '.ts', '.js', '.html', '.css', '.json', '.toml', '.ps1', '.py', '.md', '.txt')
}

$findings = @()
foreach ($f in $files) {
    $rel = $f.FullName.Substring($ProjectDir.Length).TrimStart('\')
    $lines = Get-Content -LiteralPath $f.FullName -Encoding UTF8 -ErrorAction SilentlyContinue
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        if ($line -match $regex -and $line -notmatch 'const forbidden') {
            $findings += [ordered]@{
                file = $rel
                line = $i + 1
                match = $Matches[0]
                text = $line.Trim()
            }
        }
    }
}

Write-Output ("Scanned files: " + $files.Count)
Write-Output ("Findings: " + $findings.Count)
if ($findings.Count -gt 0) {
    $findings | Format-Table -AutoSize
    exit 1
}
Write-Output "SANITIZATION_CHECK_PASSED"
