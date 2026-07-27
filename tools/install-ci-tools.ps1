param(
    [ValidateSet('Base','Quality','Advisory')] [string] $Mode = 'Base',
    [string] $Target = '',
    [ValidateSet('All','Toolchain','Tools')] [string] $Phase = 'All'
)

$ErrorActionPreference = 'Stop'
$version = { param($name) (& (Join-Path $PSScriptRoot 'get-tool-version.ps1') -Name $name).Trim() }

# Keep direct/local callers on Cargo's normal install root. Actions uses an
# isolated, cacheable root that never includes ~/.cargo credentials.
if ($env:GITHUB_ACTIONS -eq 'true' -and -not $env:CARGO_INSTALL_ROOT) {
    $repositoryRoot = Split-Path -Parent $PSScriptRoot
    $env:CARGO_INSTALL_ROOT = Join-Path $repositoryRoot '.tools/cargo-install'
}
if ($env:CARGO_INSTALL_ROOT) {
    $cargoBin = Join-Path $env:CARGO_INSTALL_ROOT 'bin'
    New-Item -ItemType Directory -Force -Path $cargoBin | Out-Null
    if ($env:GITHUB_PATH) {
        Add-Content -LiteralPath $env:GITHUB_PATH -Value $cargoBin -Encoding utf8
    }
    $env:PATH = "$cargoBin$([IO.Path]::PathSeparator)$env:PATH"
}

if ($Phase -in @('All', 'Toolchain')) {
    $rust = & $version 'rust'
    rustup toolchain install $rust --profile minimal
    rustup default $rust
    if ($Target) { rustup target add $Target }
    if ($Mode -eq 'Quality') {
        rustup component add rustfmt clippy
    }
}

if ($Phase -in @('All', 'Tools')) {
    if ($Mode -eq 'Quality') {
        cargo install just --version (& $version 'just') --locked
        cargo install cargo-llvm-cov --version (& $version 'cargo-llvm-cov') --locked
        cargo install cargo-deny --version (& $version 'cargo-deny') --locked
        cargo install zizmor --version (& $version 'workflow-audit') --locked
    } elseif ($Mode -eq 'Advisory') {
        cargo install just --version (& $version 'just') --locked
        cargo install cargo-deny --version (& $version 'cargo-deny') --locked
    }
}
