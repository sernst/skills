param(
    [Parameter(Mandatory = $true)] [string] $Cli,
    [Parameter(Mandatory = $true)] [string] $Target,
    [switch] $PrebuiltTarget
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $repoRoot "clis/$Cli/Cargo.toml"
cargo build --locked --release --manifest-path $manifest
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if (-not $PrebuiltTarget) {
    if ($Target -match '^aarch64-unknown-linux-(?:gnu|musl)$') {
        cargo zigbuild --locked --release --target $Target --manifest-path $manifest
    } else {
        cargo build --locked --release --target $Target --manifest-path $manifest
    }
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
$hostExtension = if ($IsWindows) { '.exe' } else { '' }
$hostBinary = Join-Path $repoRoot "clis/$Cli/target/release/$Cli$hostExtension"
& (Join-Path $PSScriptRoot 'package-cli.ps1') -Cli $Cli -Target $Target -HostBinary $hostBinary
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
