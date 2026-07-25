param(
    [ValidateSet('Base','Quality','Advisory')] [string] $Mode = 'Base',
    [string] $Target = ''
)

$ErrorActionPreference = 'Stop'
$version = { param($name) (& (Join-Path $PSScriptRoot 'get-tool-version.ps1') -Name $name).Trim() }
$rust = & $version 'rust'
rustup toolchain install $rust --profile minimal
rustup default $rust
if ($Target) { rustup target add $Target }
if ($Mode -eq 'Quality') {
    rustup component add rustfmt clippy
    cargo install just --version (& $version 'just') --locked
    cargo install cargo-llvm-cov --version (& $version 'cargo-llvm-cov') --locked
    cargo install cargo-deny --version (& $version 'cargo-deny') --locked
    cargo install zizmor --version (& $version 'workflow-audit') --locked
} elseif ($Mode -eq 'Advisory') {
    cargo install just --version (& $version 'just') --locked
    cargo install cargo-deny --version (& $version 'cargo-deny') --locked
}
