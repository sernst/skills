param(
    [Parameter(Mandatory = $true)] [string] $Cli,
    [string] $Target = '',
    [ValidateSet('', 'true', 'false')] [string] $Native = '',
    [ValidateSet('', 'true', 'false')] [string] $Zig = '',
    [ValidateSet('All', 'Host', 'Target')] [string] $Phase = 'All'
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $repoRoot "clis/$Cli/Cargo.toml"

function Invoke-HostBuild {
    # Build before target-specific toolchain setup. The host binary generates
    # completions and documentation during packaging.
    cargo build --locked --release --manifest-path $manifest
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

function Invoke-TargetBuild {
    if (-not $Target -or $Native -notin @('true', 'false') -or $Zig -notin @('true', 'false')) {
        throw 'Target, Native, and Zig are required for the Target and All phases.'
    }
    $isNative = $Native -eq 'true'
    $usesZig = $Zig -eq 'true'

    if ($usesZig) {
        cargo zigbuild --locked --release --target $Target --manifest-path $manifest
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        cargo zigbuild --locked --all-features --target $Target --manifest-path $manifest --tests
    } else {
        cargo build --locked --release --target $Target --manifest-path $manifest
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        if ($isNative) {
            cargo test --locked --all-features --target $Target --manifest-path $manifest
        } else {
            cargo test --locked --all-features --target $Target --manifest-path $manifest --no-run
        }
    }
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    $version = (Get-Content (Join-Path $repoRoot 'VERSION') -Raw).Trim()
    $targetExtension = if ($Target -match 'windows') { '.exe' } else { '' }
    $binary = Join-Path $repoRoot "clis/$Cli/target/$Target/release/$Cli$targetExtension"
    & (Join-Path $PSScriptRoot 'verify-target.ps1') -Path $binary -Target $Target
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    if ($isNative) {
        $actualVersion = (& $binary --version).Trim()
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        if ($actualVersion -ne "$Cli $version") {
            throw "Native binary version '$actualVersion' does not equal '$Cli $version'."
        }
    }

    $hostExtension = if ($IsWindows) { '.exe' } else { '' }
    $hostBinary = Join-Path $repoRoot "clis/$Cli/target/release/$Cli$hostExtension"
    & (Join-Path $PSScriptRoot 'package-cli.ps1') `
        -Cli $Cli `
        -Target $Target `
        -HostBinary $hostBinary
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

if ($Phase -in @('All', 'Host')) { Invoke-HostBuild }
if ($Phase -in @('All', 'Target')) { Invoke-TargetBuild }
