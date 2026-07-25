param([switch] $SkipRustup)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$versions = @{}
Get-Content (Join-Path $repoRoot 'TOOL_VERSIONS') | Where-Object { $_ -match '=' } | ForEach-Object {
    $pair = $_ -split '=', 2
    $versions[$pair[0]] = $pair[1]
}

if (-not $SkipRustup) {
    rustup toolchain install $versions.rust --profile minimal
    rustup component add rustfmt clippy --toolchain $versions.rust
}

cargo install just --version $versions.just --locked
cargo install cargo-llvm-cov --version $versions.'cargo-llvm-cov' --locked
cargo install cargo-deny --version $versions.'cargo-deny' --locked
cargo install cargo-zigbuild --version $versions.'cargo-zigbuild' --locked
cargo install zizmor --version $versions.'workflow-audit' --locked
& (Join-Path $PSScriptRoot 'install-zig.ps1') -Version $versions.zig
