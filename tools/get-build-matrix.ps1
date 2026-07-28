param(
    [switch] $ValidateOnly,
    [ValidateSet('Full', 'Pr')] [string] $Profile = 'Full'
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'build-matrix-contract.ps1')
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
$fullTargets = @(
    @{ runner='macos-15-intel'; target='x86_64-apple-darwin'; archive='tar.gz'; native=$true; zig=$false; musl=$false; msvc_arm64=$false },
    @{ runner='macos-15'; target='aarch64-apple-darwin'; archive='tar.gz'; native=$true; zig=$false; musl=$false; msvc_arm64=$false },
    @{ runner='windows-2025'; target='x86_64-pc-windows-msvc'; archive='zip'; native=$true; zig=$false; musl=$false; msvc_arm64=$false },
    @{ runner='windows-2025'; target='aarch64-pc-windows-msvc'; archive='zip'; native=$false; zig=$false; musl=$false; msvc_arm64=$true },
    @{ runner='ubuntu-24.04'; target='x86_64-unknown-linux-gnu'; archive='tar.gz'; native=$true; zig=$false; musl=$false; msvc_arm64=$false },
    @{ runner='ubuntu-24.04'; target='aarch64-unknown-linux-gnu'; archive='tar.gz'; native=$false; zig=$true; musl=$false; msvc_arm64=$false },
    @{ runner='ubuntu-24.04'; target='x86_64-unknown-linux-musl'; archive='tar.gz'; native=$true; zig=$false; musl=$true; msvc_arm64=$false },
    @{ runner='ubuntu-24.04'; target='aarch64-unknown-linux-musl'; archive='tar.gz'; native=$false; zig=$true; musl=$false; msvc_arm64=$false }
)
$prTargetNames = @(
    'x86_64-pc-windows-msvc',
    'aarch64-apple-darwin',
    'aarch64-unknown-linux-musl'
)
$prTargets = @($fullTargets | Where-Object { $_.target -in $prTargetNames })

Assert-CanonicalFullBuildTargets -Targets $fullTargets
Assert-CanonicalPrBuildTargets -Targets $prTargets

foreach ($workflowName in @('build.yml', 'pr.yml')) {
    $workflowPath = Join-Path $repoRoot ".github/workflows/$workflowName"
    $workflow = Get-Content $workflowPath -Raw
    Assert-WorkflowTargetSetupOrder -WorkflowName $workflowName -Workflow $workflow
}
$prWorkflow = Get-Content (Join-Path $repoRoot '.github/workflows/pr.yml') -Raw
$mainWorkflow = Get-Content (Join-Path $repoRoot '.github/workflows/security-and-live.yml') -Raw
Assert-LiveSmokeWorkflowContract -PrWorkflow $prWorkflow -MainWorkflow $mainWorkflow
$liveSmokeHelper = Get-Content (Join-Path $repoRoot 'tools/live-github-smoke.sh') -Raw
Assert-LiveSmokeHelperContract -Helper $liveSmokeHelper
$skillManagerJustfile = Get-Content (Join-Path $repoRoot 'clis/skill-manager/Justfile') -Raw
$localLiveSmokeWrapper = Get-Content (Join-Path $repoRoot 'tools/live-github-smoke.ps1') -Raw
Assert-LocalLiveSmokeContract -Justfile $skillManagerJustfile -Wrapper $localLiveSmokeWrapper

if ($ValidateOnly) {
    & (Join-Path $PSScriptRoot 'test-build-matrix-contract.ps1')
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    Write-Output "Validated $($entries.Count) registered CLI(s) and Full/Pr build profiles."
    exit 0
}

$targets = if ($Profile -eq 'Pr') { $prTargets } else { $fullTargets }
$include = foreach ($cli in $entries) {
    foreach ($target in $targets) {
        [ordered]@{ cli=$cli; runner=$target.runner; target=$target.target; archive=$target.archive; native=$target.native; zig=$target.zig; musl=$target.musl; msvc_arm64=$target.msvc_arm64 }
    }
}
[ordered]@{ include=@($include) } | ConvertTo-Json -Depth 4 -Compress
