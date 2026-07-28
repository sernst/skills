param(
    [Parameter(Mandatory = $true)] [string] $SourceUrl,
    [string] $SkillManagerBinary = ''
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'live-smoke-paths.ps1')

if ($SourceUrl -cnotmatch '^https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/tree/[0-9A-Fa-f]{40}/skills/?$') {
    throw 'SourceUrl must be a public GitHub skills path at an exact commit.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $SkillManagerBinary) {
    $binaryName = if ($IsWindows) { 'skill-manager.exe' } else { 'skill-manager' }
    $SkillManagerBinary = Join-Path $repoRoot "clis/skill-manager/target/release/$binaryName"
}
if (-not [IO.Path]::IsPathFullyQualified($SkillManagerBinary)) {
    throw 'SkillManagerBinary must be an absolute path.'
}
$resolvedBinary = (Resolve-Path -LiteralPath $SkillManagerBinary -ErrorAction Stop).Path
if (-not (Test-Path -LiteralPath $resolvedBinary -PathType Leaf)) {
    throw "Skill-manager binary is not a file: $resolvedBinary"
}

$systemTempRoot = [IO.Path]::GetTempPath()
$smokeRoot = Join-Path $systemTempRoot "skill-manager-live-smoke-$([guid]::NewGuid().ToString('N'))"
$environmentNames = @(
    'HOME',
    'USERPROFILE',
    'SKILL_MANAGER_HOME',
    'XDG_CACHE_HOME',
    'TMPDIR',
    'TEMP',
    'TMP',
    'GIT_TERMINAL_PROMPT'
)
$previousEnvironment = @{}
foreach ($name in $environmentNames) {
    $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$locationPushed = $false

try {
    $smokeHome = Join-Path $smokeRoot 'home'
    $smokeTemp = Join-Path $smokeRoot 'tmp'
    $smokeWork = Join-Path $smokeRoot 'work'
    New-Item -ItemType Directory -Path $smokeHome, $smokeTemp, $smokeWork -ErrorAction Stop | Out-Null

    foreach ($name in @('HOME', 'USERPROFILE')) {
        [Environment]::SetEnvironmentVariable($name, $smokeHome, 'Process')
    }
    [Environment]::SetEnvironmentVariable('SKILL_MANAGER_HOME', $smokeHome, 'Process')
    [Environment]::SetEnvironmentVariable('XDG_CACHE_HOME', (Join-Path $smokeHome '.cache'), 'Process')
    foreach ($name in @('TMPDIR', 'TEMP', 'TMP')) {
        [Environment]::SetEnvironmentVariable($name, $smokeTemp, 'Process')
    }
    [Environment]::SetEnvironmentVariable('GIT_TERMINAL_PROMPT', '0', 'Process')

    Push-Location $smokeWork
    $locationPushed = $true
    & $resolvedBinary --json source add $SourceUrl live-github-smoke
    if ($LASTEXITCODE -ne 0) {
        throw "skill-manager source add failed with exit code $LASTEXITCODE."
    }
    & $resolvedBinary --json status --refresh
    if ($LASTEXITCODE -ne 0) {
        throw "skill-manager status failed with exit code $LASTEXITCODE."
    }
} finally {
    if ($locationPushed) {
        Pop-Location
    }
    foreach ($name in $environmentNames) {
        [Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], 'Process')
    }

    if (Test-Path -LiteralPath $smokeRoot) {
        $canonicalTempRoot = Resolve-SmokeCanonicalExistingPath -Path $systemTempRoot
        $canonicalSmokeRoot = Resolve-SmokeCanonicalExistingPath -Path $smokeRoot
        Assert-SmokePathContained `
            -CanonicalTempRoot $canonicalTempRoot `
            -CanonicalSmokeRoot $canonicalSmokeRoot
        Remove-Item -LiteralPath $canonicalSmokeRoot -Recurse -Force
    }
}
