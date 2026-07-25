param(
    [Parameter(Mandatory = $true)] [string] $Tag,
    [Parameter(Mandatory = $true)] [string] $ArtifactDirectory
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'release-version.ps1')
$version = (Get-Content (Join-Path $repoRoot 'VERSION') -Raw).Trim()
if ($Tag -ne "v$version") { throw "Tag $Tag does not match VERSION $version." }
if (-not (Test-StrictSemVer $version)) { throw 'VERSION is not strict SemVer without build metadata.' }
$commit = (git -C $repoRoot rev-parse HEAD).Trim()
$registry = Join-Path $repoRoot 'clis/registry.just'
$clis = @(Get-Content $registry | ForEach-Object { if ($_ -match '^\s*mod\s+([a-z0-9]+(?:-[a-z0-9]+)*)\s+''\1''\s*$') { $Matches[1] } })
if (-not $clis.Count) { throw 'No registered CLIs were found.' }
$targets = @(
    'x86_64-apple-darwin', 'aarch64-apple-darwin',
    'x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc',
    'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu',
    'x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl'
)
$expected = @()
foreach ($cli in $clis) {
    foreach ($target in $targets) {
        $suffix = if ($target -match 'windows') { 'zip' } else { 'tar.gz' }
        $expected += "$cli-v$version-$target.$suffix"
    }
}
$directories = @(Get-ChildItem $ArtifactDirectory -Directory)
if ($directories) { throw "Unexpected directories in release inputs: $($directories.Name -join ', ')." }
$archives = @(Get-ChildItem $ArtifactDirectory -File | Sort-Object Name)
$actual = @($archives.Name)
$duplicates = @($actual | Group-Object | Where-Object Count -ne 1)
if ($duplicates) { throw "Duplicate release archives found: $($duplicates.Name -join ', ')." }
$missing = @($expected | Where-Object { $_ -notin $actual })
$unexpected = @($actual | Where-Object { $_ -notin $expected })
if ($missing -or $unexpected) { throw "Release asset set mismatch. Missing: [$($missing -join ', ')]. Unexpected: [$($unexpected -join ', ')]." }
$items = foreach ($archive in $archives) {
    $cli = @($clis | Where-Object { $archive.Name.StartsWith("$_-v$version-") })
    if ($cli.Count -ne 1) { throw "Could not uniquely determine CLI for $($archive.Name)." }
    $target = $archive.Name.Substring("$($cli[0])-v$version-".Length) -replace '\.(tar\.gz|zip)$', ''
    [ordered]@{ cli = $cli[0]; target = $target; filename = $archive.Name; sha256 = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant(); validation = if ($target -in @('x86_64-apple-darwin','aarch64-apple-darwin','x86_64-pc-windows-msvc','x86_64-unknown-linux-gnu','x86_64-unknown-linux-musl')) { 'native-tested' } else { 'cross-compiled' } }
}
$manifest = [ordered]@{ schema_version = 1; tag = $Tag; version = $version; commit = $commit; artifacts = @($items) }
$manifestPath = Join-Path $ArtifactDirectory 'release-manifest.json'
$manifest | ConvertTo-Json -Depth 5 | Set-Content -NoNewline $manifestPath
$sumFiles = @(@($archives.FullName) + $manifestPath | Sort-Object { Split-Path $_ -Leaf })
$sumFiles | ForEach-Object { "{0}  {1}" -f (Get-FileHash $_ -Algorithm SHA256).Hash.ToLowerInvariant(), (Split-Path $_ -Leaf) } | Set-Content (Join-Path $ArtifactDirectory 'SHA256SUMS')
