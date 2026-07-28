$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'build-matrix-contract.ps1')

function Copy-Targets {
    param([Parameter(Mandatory = $true)] [object[]] $Targets)
    @($Targets | ForEach-Object {
        $copy = [ordered]@{}
        foreach ($key in $_.Keys) { $copy[$key] = $_[$key] }
        $copy
    })
}

function Assert-Rejected {
    param(
        [Parameter(Mandatory = $true)] [scriptblock] $Operation,
        [Parameter(Mandatory = $true)] [string] $Reason,
        [Parameter(Mandatory = $true)] [string] $Case
    )
    try {
        & $Operation
    } catch {
        if ($_.Exception.Message.StartsWith("[$Reason]", [StringComparison]::Ordinal)) {
            return
        }
        throw "Guardrail rejected '$Case' for the wrong reason. Expected [$Reason], got: $($_.Exception.Message)"
    }
    throw "Guardrail accepted invalid case: $Case"
}

function New-WorkflowFixture {
    param(
        [string[]] $BeforeHost = @(),
        [string[]] $Setup = @(),
        [string[]] $AfterTarget = @()
    )
    $lines = @('jobs:', '  package:', '    steps:')
    $stepNumber = 0
    foreach ($command in $BeforeHost) {
        $lines += "      - name: Fixture before $stepNumber"
        $lines += "        run: $command"
        $stepNumber++
    }
    $lines += '      - name: Fixture host'
    $lines += '        run: pwsh -File tools/run-build-matrix-entry.ps1 -Cli sample -Phase Host'
    foreach ($command in $Setup) {
        $lines += "      - name: Fixture setup $stepNumber"
        $lines += "        run: $command"
        $stepNumber++
    }
    $lines += '      - name: Fixture target'
    $lines += '        run: pwsh -File tools/run-build-matrix-entry.ps1 -Cli sample -Target sample -Native true -Zig false -Phase Target'
    foreach ($command in $AfterTarget) {
        $lines += "      - name: Fixture after $stepNumber"
        $lines += "        run: $command"
        $stepNumber++
    }
    $lines -join [Environment]::NewLine
}

$canonicalFull = @(Get-CanonicalFullBuildTargets)
Assert-CanonicalFullBuildTargets -Targets $canonicalFull
$canonicalPr = @($canonicalFull | Where-Object {
    $_.target -in @('x86_64-pc-windows-msvc', 'aarch64-apple-darwin', 'aarch64-unknown-linux-musl')
})
Assert-CanonicalPrBuildTargets -Targets $canonicalPr

$missing = @(Copy-Targets -Targets $canonicalFull)[0..6]
Assert-Rejected { Assert-CanonicalFullBuildTargets -Targets $missing } 'MATRIX_COUNT' 'Full missing target'
$duplicate = @(Copy-Targets -Targets $canonicalFull)
$duplicate[7] = @(Copy-Targets -Targets @($duplicate[0]))[0]
Assert-Rejected { Assert-CanonicalFullBuildTargets -Targets $duplicate } 'MATRIX_DUPLICATE_TARGET' 'Full duplicate target'
$replaced = @(Copy-Targets -Targets $canonicalFull)
$replaced[7]['target'] = 'replacement-target'
Assert-Rejected { Assert-CanonicalFullBuildTargets -Targets $replaced } 'MATRIX_VALUE' 'Full replacement target'
$reordered = @(Copy-Targets -Targets $canonicalFull)
$swap = $reordered[0]
$reordered[0] = $reordered[1]
$reordered[1] = $swap
Assert-Rejected { Assert-CanonicalFullBuildTargets -Targets $reordered } 'MATRIX_VALUE' 'Full target order'
foreach ($property in @('runner', 'archive', 'native', 'zig', 'musl', 'msvc_arm64')) {
    $drifted = @(Copy-Targets -Targets $canonicalFull)
    $drifted[0][$property] = if ($drifted[0][$property] -is [bool]) { -not $drifted[0][$property] } else { 'drifted' }
    Assert-Rejected { Assert-CanonicalFullBuildTargets -Targets $drifted } 'MATRIX_VALUE' "Full $property drift"
}
$extra = @(Copy-Targets -Targets $canonicalFull)
$extra[0]['unexpected'] = 'value'
Assert-Rejected { Assert-CanonicalFullBuildTargets -Targets $extra } 'MATRIX_EXTRA_KEY' 'Full extra key'
$missingField = @(Copy-Targets -Targets $canonicalFull)
$missingField[0].Remove('archive')
Assert-Rejected { Assert-CanonicalFullBuildTargets -Targets $missingField } 'MATRIX_MISSING_KEY' 'Full missing field'
$stringBoolean = @(Copy-Targets -Targets $canonicalFull)
$stringBoolean[0]['native'] = 'true'
Assert-Rejected { Assert-CanonicalFullBuildTargets -Targets $stringBoolean } 'MATRIX_TYPE' 'Full string boolean'
$nullValue = @(Copy-Targets -Targets $canonicalFull)
$nullValue[0]['runner'] = $null
Assert-Rejected { Assert-CanonicalFullBuildTargets -Targets $nullValue } 'MATRIX_TYPE' 'Full null value'
$wrongType = @(Copy-Targets -Targets $canonicalFull)
$wrongType[0]['archive'] = 42
Assert-Rejected { Assert-CanonicalFullBuildTargets -Targets $wrongType } 'MATRIX_TYPE' 'Full wrong type'

$prExtra = @(Copy-Targets -Targets $canonicalPr)
$prExtra[0]['unexpected'] = $true
Assert-Rejected { Assert-CanonicalPrBuildTargets -Targets $prExtra } 'MATRIX_EXTRA_KEY' 'Pr extra key'
$prMissing = @(Copy-Targets -Targets $canonicalPr)
$prMissing[0].Remove('runner')
Assert-Rejected { Assert-CanonicalPrBuildTargets -Targets $prMissing } 'MATRIX_MISSING_KEY' 'Pr missing field'
$prStringBoolean = @(Copy-Targets -Targets $canonicalPr)
$prStringBoolean[0]['zig'] = 'false'
Assert-Rejected { Assert-CanonicalPrBuildTargets -Targets $prStringBoolean } 'MATRIX_TYPE' 'Pr string boolean'
$prNull = @(Copy-Targets -Targets $canonicalPr)
$prNull[0]['target'] = $null
Assert-Rejected { Assert-CanonicalPrBuildTargets -Targets $prNull } 'MATRIX_TYPE' 'Pr null value'

$setupCommands = @(
    'rustup target add sample-target',
    'cargo install cargo-zigbuild --version 1.0.0 --locked',
    'bash tools/install-zig.sh 1.0.0',
    'sudo apt-get install --yes musl-tools',
    'pwsh -File tools/enable-msvc-arm64.ps1'
)
$validFixture = New-WorkflowFixture -Setup $setupCommands
Assert-WorkflowTargetSetupOrder -WorkflowName 'valid fixture' -Workflow $validFixture
$alternateRust = @($setupCommands)
$alternateRust[0] = 'pwsh -File tools/install-ci-tools.ps1 -Mode Base -Phase Toolchain -Target sample-target'
Assert-WorkflowTargetSetupOrder -WorkflowName 'alternate Rust fixture' -Workflow (New-WorkflowFixture -Setup $alternateRust)

$validJob = $validFixture -replace '^jobs:\r?\n', ''
$unrelatedSetupFixture = @(
    'jobs:',
    '  quality:',
    '    steps:',
    '      - shell: pwsh',
    '        run: rustup target add unrelated-target'
) -join [Environment]::NewLine
$unrelatedSetupFixture += [Environment]::NewLine + $validJob
Assert-WorkflowTargetSetupOrder -WorkflowName 'unrelated setup fixture' -Workflow $unrelatedSetupFixture

$splitPhaseFixture = @(
    'jobs:',
    '  host_job:',
    '    steps:',
    '      - run: pwsh -File tools/run-build-matrix-entry.ps1 -Cli sample -Phase Host',
    '  target_job:',
    '    steps:',
    '      - run: pwsh -File tools/run-build-matrix-entry.ps1 -Cli sample -Target sample -Native true -Zig false -Phase Target'
) -join [Environment]::NewLine
Assert-Rejected {
    Assert-WorkflowTargetSetupOrder -WorkflowName 'split phase fixture' -Workflow $splitPhaseFixture
} 'CROSS_JOB_PHASES' 'Host and Target split across jobs'

$secondValidJob = $validJob -replace '^  package:', '  preflight:'
$duplicateJobFixture = 'jobs:' + [Environment]::NewLine + $validJob + [Environment]::NewLine + $secondValidJob
Assert-Rejected {
    Assert-WorkflowTargetSetupOrder -WorkflowName 'duplicate qualifying jobs fixture' -Workflow $duplicateJobFixture
} 'AMBIGUOUS_PHASE_JOB' 'duplicate qualifying phase jobs'

$unrelatedPhaseFixture = @(
    'jobs:',
    '  unrelated:',
    '    steps:',
    '      - run: pwsh -File tools/run-build-matrix-entry.ps1 -Cli sample -Phase Host'
) -join [Environment]::NewLine
$unrelatedPhaseFixture += [Environment]::NewLine + $validJob
Assert-Rejected {
    Assert-WorkflowTargetSetupOrder -WorkflowName 'unrelated phase fixture' -Workflow $unrelatedPhaseFixture
} 'AMBIGUOUS_PHASE_JOB' 'phase marker in unrelated job'

foreach ($command in @($setupCommands + $alternateRust[0])) {
    $base = if ($command -eq $alternateRust[0]) { $alternateRust } else { $setupCommands }
    $without = @($base | Where-Object { $_ -ne $command })
    Assert-Rejected {
        Assert-WorkflowTargetSetupOrder -WorkflowName 'moved before fixture' -Workflow (
            New-WorkflowFixture -BeforeHost @($command) -Setup $without
        )
    } 'ORDER_BEFORE_HOST' "setup before Host: $command"
    Assert-Rejected {
        Assert-WorkflowTargetSetupOrder -WorkflowName 'moved after fixture' -Workflow (
            New-WorkflowFixture -Setup $without -AfterTarget @($command)
        )
    } 'ORDER_AFTER_TARGET' "setup after Target: $command"
    Assert-Rejected {
        Assert-WorkflowTargetSetupOrder -WorkflowName 'duplicate fixture' -Workflow (
            New-WorkflowFixture -Setup @($base + $command)
        )
    } 'DUPLICATE_SETUP' "duplicate setup: $command"
}
$unnamedDuplicate = $validFixture + [Environment]::NewLine +
    '      - run: rustup target add duplicate-target'
Assert-Rejected {
    Assert-WorkflowTargetSetupOrder -WorkflowName 'unnamed duplicate fixture' -Workflow $unnamedDuplicate
} 'DUPLICATE_SETUP' 'unnamed duplicate setup step'
$shellFirstDuplicate = @(
    'jobs:',
    '  package:',
    '    steps:',
    '      - shell: pwsh',
    '        run: |',
    '          rustup target add early-target'
) -join [Environment]::NewLine
$shellFirstDuplicate += [Environment]::NewLine + ($validFixture -replace '^jobs:\r?\n  package:\r?\n    steps:\r?\n', '')
Assert-Rejected {
    Assert-WorkflowTargetSetupOrder -WorkflowName 'shell-first duplicate fixture' -Workflow $shellFirstDuplicate
} 'DUPLICATE_SETUP' 'shell-first pre-Host duplicate Rust setup'
$missingSetupCommands = @($setupCommands | Where-Object { $_ -ne 'sudo apt-get install --yes musl-tools' })
Assert-Rejected {
    Assert-WorkflowTargetSetupOrder -WorkflowName 'missing setup fixture' -Workflow (
        New-WorkflowFixture -Setup $missingSetupCommands
    )
} 'MISSING_SETUP' 'missing setup command'
Assert-Rejected {
    Assert-WorkflowTargetSetupOrder -WorkflowName 'duplicate Host fixture' -Workflow (
        $validFixture + [Environment]::NewLine +
        '      - name: Duplicate host' + [Environment]::NewLine +
        '        run: pwsh -File tools/run-build-matrix-entry.ps1 -Cli sample -Phase Host'
    )
} 'DUPLICATE_HOST_PHASE' 'duplicate Host phase'
Assert-Rejected {
    Assert-WorkflowTargetSetupOrder -WorkflowName 'duplicate Target fixture' -Workflow (
        $validFixture + [Environment]::NewLine +
        '      - name: Duplicate target' + [Environment]::NewLine +
        '        run: pwsh -File tools/run-build-matrix-entry.ps1 -Cli sample -Target sample -Native true -Zig false -Phase Target'
    )
} 'DUPLICATE_TARGET_PHASE' 'duplicate Target phase'

Write-Output 'Build matrix and workflow guardrail self-tests passed.'
