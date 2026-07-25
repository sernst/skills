param([Parameter(Mandatory = $true)] [string] $Tag)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'release-version.ps1')
Push-Location $repoRoot
try {
    if (-not $Tag.StartsWith('v') -or -not (Test-StrictSemVer $Tag.Substring(1))) { throw 'Tag must be v followed by strict SemVer without build metadata.' }
    $tagObject = git cat-file -t $Tag
    if ($tagObject.Trim() -ne 'tag') { throw 'Releases require an annotated tag.' }
    $isShallow = (git rev-parse --is-shallow-repository).Trim()
    if ($isShallow -eq 'true') { git fetch --unshallow origin }
    git fetch origin main --tags
    git merge-base --is-ancestor "$Tag^{commit}" origin/main
    if ($LASTEXITCODE -ne 0) { throw 'The tagged commit is not reachable from origin/main.' }
    $version = (Get-Content VERSION -Raw).Trim()
    if ($Tag -ne "v$version") { throw 'Tag and VERSION disagree.' }
    if (-not (Select-String CHANGELOG.md -Pattern "^## $([regex]::Escape($version))\b" -Quiet)) { throw 'CHANGELOG does not contain this version.' }
    Get-ChildItem clis -Filter Cargo.toml -Recurse | ForEach-Object {
        $pattern = '^version\s*=\s*"' + [regex]::Escape($version) + '"\s*$'
        if (-not (Select-String $_.FullName -Pattern $pattern -Quiet)) { throw "Version mismatch in $($_.FullName)." }
    }
} finally { Pop-Location }
