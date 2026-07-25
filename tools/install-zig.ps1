param([Parameter(Mandatory = $true)] [string] $Version)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$installRoot = Join-Path $repoRoot ".tools/zig-$Version"
$zig = Join-Path $installRoot 'zig.exe'
if (Test-Path $zig) {
    $installed = (& $zig version).Trim()
    if ($installed -ne $Version) { throw "Existing Zig version $installed does not equal $Version." }
    Write-Host "Zig $Version is already installed at $installRoot"
    exit 0
}
$archive = Join-Path ([System.IO.Path]::GetTempPath()) "zig-$Version.zip"
$url = "https://ziglang.org/download/$Version/zig-x86_64-windows-$Version.zip"
Invoke-WebRequest -Uri $url -OutFile $archive -MaximumRetryCount 3
$expanded = Join-Path ([System.IO.Path]::GetTempPath()) "zig-expand-$Version"
New-Item -ItemType Directory -Force $expanded | Out-Null
Expand-Archive -LiteralPath $archive -DestinationPath $expanded -Force
$source = Get-ChildItem $expanded -Directory | Select-Object -First 1
if (-not $source) { throw 'Downloaded Zig archive did not contain the expected directory.' }
New-Item -ItemType Directory -Force (Split-Path -Parent $installRoot) | Out-Null
Move-Item -LiteralPath $source.FullName -Destination $installRoot
$actual = (& $zig version).Trim()
if ($actual -ne $Version) { throw "Installed Zig version $actual does not equal $Version." }
Write-Host "Zig $Version installed at $installRoot"
