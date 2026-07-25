$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'release-version.ps1')
Push-Location $repoRoot
try {
    if (git status --porcelain) { throw 'Release tags require a clean worktree.' }
    $version = (Get-Content VERSION -Raw).Trim()
    if (-not (Test-StrictSemVer $version)) { throw 'VERSION is not a strict SemVer version without build metadata.' }
    if (-not (Select-String -Path CHANGELOG.md -Pattern "^## $([regex]::Escape($version))\b" -Quiet)) { throw 'CHANGELOG does not contain the current version heading.' }
    $tag = "v$version"
    if (git rev-parse -q --verify "refs/tags/$tag") { throw "Tag $tag already exists." }
    pwsh -File tools/get-build-matrix.ps1 -ValidateOnly
    if ($LASTEXITCODE -ne 0) { throw 'CLI registry validation failed.' }
    git fetch origin main --tags
    git merge-base --is-ancestor HEAD origin/main
    if ($LASTEXITCODE -ne 0) { throw 'HEAD must be reachable from origin/main.' }
    $packages = Get-ChildItem clis -Filter Cargo.toml -Recurse | ForEach-Object { Select-String -Path $_.FullName -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1 }
    foreach ($package in $packages) { if ($package.Matches[0].Groups[1].Value -ne $version) { throw "Package version mismatch in $($package.Path)." } }
    git tag -a $tag -m "Release $tag"
    Write-Host "Created annotated tag $tag. Review it, then run: git push origin $tag"
} finally { Pop-Location }
