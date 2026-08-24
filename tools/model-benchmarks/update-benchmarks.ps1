<#
.SYNOPSIS
Refreshes or checks the generated maestro model-benchmark snapshot.

.DESCRIPTION
Refresh writes an atomic snapshot only after every enabled source validates.
Check never writes: it exits 0 when current, 2 when an update is available,
and 1 on fetch, schema, security, or limit failure.
#>
[CmdletBinding()]
param(
    [ValidateSet('Refresh', 'Check')] [string] $Mode = 'Refresh',
    [string] $RegistryPath = (Join-Path $PSScriptRoot 'sources.json'),
    [string] $OutputPath = (Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'skills/running-as-maestro/references/benchmark-snapshot.md'),
    [string] $FixtureRoot,
    [datetimeoffset] $RetrievedAt = [datetimeoffset]::UtcNow
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'benchmark-lib.ps1')

try {
    $result = Invoke-BenchmarkUpdate `
        -RegistryPath $RegistryPath `
        -OutputPath $OutputPath `
        -FixtureRoot $FixtureRoot `
        -RetrievedAt $RetrievedAt `
        -Check:($Mode -eq 'Check')
    if ($Mode -eq 'Check' -and $result.Changed) { exit 2 }
    exit 0
} catch {
    Write-Error "Benchmark refresh failed without modifying the snapshot: $($_.Exception.Message)"
    exit 1
}
