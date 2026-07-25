param(
    [Parameter(Mandatory = $true)] [string] $Cli,
    [Parameter(Mandatory = $true)] [string] $Target,
    [string] $Version = '',
    [string] $HostBinary = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Version) { $Version = (Get-Content (Join-Path $repoRoot 'VERSION') -Raw).Trim() }
$extension = if ($Target -match 'windows') { '.exe' } else { '' }
$binary = Join-Path $repoRoot "clis/$Cli/target/$Target/release/$Cli$extension"
if (-not (Test-Path $binary)) { throw "Expected release binary was not found: $binary" }
if (-not $HostBinary) {
    $hostExtension = if ($IsWindows) { '.exe' } else { '' }
    $HostBinary = Join-Path $repoRoot "clis/$Cli/target/release/$Cli$hostExtension"
}
if (-not (Test-Path $HostBinary)) { throw "Expected host binary was not found: $HostBinary" }
$dist = Join-Path $repoRoot 'dist'
$root = "$Cli-v$Version-$Target"
$staging = Join-Path $dist $root
New-Item -ItemType Directory -Force -Path $staging | Out-Null
Copy-Item $binary (Join-Path $staging "$Cli$extension") -Force
Copy-Item (Join-Path $repoRoot 'LICENSE') (Join-Path $staging 'LICENSE') -Force
Copy-Item (Join-Path $repoRoot "clis/$Cli/README.md") (Join-Path $staging 'README.md') -Force

# The CLI owns deterministic generators, so package contents do not depend on
# host tooling or shell completion packages.
$completions = @(
    @('bash', (Join-Path $staging "$Cli.bash")),
    @('zsh', (Join-Path $staging "_$Cli")),
    @('fish', (Join-Path $staging "$Cli.fish")),
    @('powershell', (Join-Path $staging "$Cli.ps1"))
)
foreach ($completion in $completions) {
    $content = & $HostBinary generate-completions --shell $completion[0] | Out-String
    if ($LASTEXITCODE -ne 0) { throw 'CLI documentation generation failed.' }
    [System.IO.File]::WriteAllText($completion[1], $content, [System.Text.UTF8Encoding]::new($false))
}
& $HostBinary generate-man --output (Join-Path $staging "$Cli.1")
if ($LASTEXITCODE -ne 0) { throw 'CLI manual generation failed.' }

$extension = if ($Target -match 'windows') { 'zip' } else { 'tar.gz' }
$archive = Join-Path $dist "$root.$extension"
Remove-Item -LiteralPath $archive -Force -ErrorAction SilentlyContinue
Push-Location $dist
try {
    if ($Target -match 'windows') { Compress-Archive -Path $root -DestinationPath $archive -Force }
    else { tar -czf $archive $root }
} finally { Pop-Location }
Write-Output $archive
