param([Parameter(Mandatory = $true)] [string] $Name)

$versionsPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'TOOL_VERSIONS'
$match = Get-Content $versionsPath | Where-Object { $_ -match "^$([regex]::Escape($Name))=" } | Select-Object -First 1
if (-not $match) { throw "Unknown pinned tool '$Name'." }
($match -split '=', 2)[1]
