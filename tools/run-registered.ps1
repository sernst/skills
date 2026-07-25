param(
    [Parameter(Mandatory = $true)] [ValidateSet('format','format-check','lint','build','release-build','test','coverage','docs','deny','check','metadata','build-target','test-target','package','advisory','live-smoke')] [string] $Recipe,
    [string[]] $Arguments = @()
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    $clis = @(Get-Content clis/registry.just | ForEach-Object { if ($_ -match '^\s*mod\s+([a-z0-9]+(?:-[a-z0-9]+)*)\s+''\1''\s*$') { $Matches[1] } })
    if (-not $clis.Count) { throw 'No registered CLI modules were found.' }
    foreach ($cli in $clis) {
        Write-Host "==> $cli::$Recipe"
        & just "$cli::$Recipe" @Arguments
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }
} finally { Pop-Location }
