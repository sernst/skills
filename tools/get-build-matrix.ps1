param([switch] $ValidateOnly)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$registryPath = Join-Path $repoRoot 'clis/registry.just'
$repositoryVersion = (Get-Content (Join-Path $repoRoot 'VERSION') -Raw).Trim()
$entries = @()
foreach ($line in Get-Content $registryPath) {
    if ($line -match '^\s*mod\s+([a-z0-9]+(?:-[a-z0-9]+)*)\s+''([^'']+)''\s*$') {
        $id = $Matches[1]
        $directory = $Matches[2]
        if ($id -ne $directory) { throw "Registry module '$id' must use the identical directory '$id'." }
        $component = Join-Path $repoRoot "clis/$directory"
        $manifest = Join-Path $component 'Cargo.toml'
        if (-not (Test-Path $manifest)) { throw "Registered CLI '$id' has no Cargo.toml." }
        $metadata = cargo metadata --manifest-path $manifest --locked --no-deps --format-version 1 | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0) { throw "Could not read Cargo metadata for '$id'." }
        $package = @($metadata.packages | Where-Object { $_.manifest_path -eq $manifest.Replace('\','/') -or $_.name -eq $id })
        if ($package.Count -ne 1 -or $package[0].name -ne $id) { throw "Registered CLI '$id' must have Cargo package name '$id'." }
        if ($package[0].version -ne $repositoryVersion) { throw "Registered CLI '$id' version $($package[0].version) does not equal VERSION $repositoryVersion." }
        if (-not @($package[0].targets | Where-Object { $_.name -eq $id -and $_.kind -contains 'bin' })) { throw "Registered CLI '$id' must expose executable '$id'." }
        if (-not (Test-Path (Join-Path $component 'Justfile'))) { throw "Registered CLI '$id' has no Justfile." }
        $entries += $id
    } elseif ($line -match '^\s*mod\s+') {
        throw "Invalid registry entry: $line"
    }
}
if (-not $entries.Count) { throw 'The CLI registry is empty.' }
if (@($entries | Sort-Object -Unique).Count -ne $entries.Count) { throw 'The CLI registry contains duplicate IDs.' }
if ($ValidateOnly) { Write-Output "Validated $($entries.Count) registered CLI(s)."; exit 0 }

$targets = @(
    @{ runner='macos-15-intel'; target='x86_64-apple-darwin'; native=$true; zig=$false; musl=$false; msvc_arm64=$false },
    @{ runner='macos-15'; target='aarch64-apple-darwin'; native=$true; zig=$false; musl=$false; msvc_arm64=$false },
    @{ runner='windows-2025'; target='x86_64-pc-windows-msvc'; native=$true; zig=$false; musl=$false; msvc_arm64=$false },
    @{ runner='windows-2025'; target='aarch64-pc-windows-msvc'; native=$false; zig=$false; musl=$false; msvc_arm64=$true },
    @{ runner='ubuntu-24.04'; target='x86_64-unknown-linux-gnu'; native=$true; zig=$false; musl=$false; msvc_arm64=$false },
    @{ runner='ubuntu-24.04'; target='aarch64-unknown-linux-gnu'; native=$false; zig=$true; musl=$false; msvc_arm64=$false },
    @{ runner='ubuntu-24.04'; target='x86_64-unknown-linux-musl'; native=$true; zig=$false; musl=$true; msvc_arm64=$false },
    @{ runner='ubuntu-24.04'; target='aarch64-unknown-linux-musl'; native=$false; zig=$true; musl=$false; msvc_arm64=$false }
)
$include = foreach ($cli in $entries) {
    foreach ($target in $targets) {
        [ordered]@{ cli=$cli; runner=$target.runner; target=$target.target; native=$target.native; zig=$target.zig; musl=$target.musl; msvc_arm64=$target.msvc_arm64 }
    }
}
[ordered]@{ include=@($include) } | ConvertTo-Json -Depth 4 -Compress
