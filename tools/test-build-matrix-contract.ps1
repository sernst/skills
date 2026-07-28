$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'build-matrix-contract.ps1')
. (Join-Path $PSScriptRoot 'live-smoke-paths.ps1')

function Copy-Targets {
    param([Parameter(Mandatory = $true)] [object[]] $Targets)
    @($Targets | ForEach-Object {
        $copy = [ordered]@{}
        foreach ($key in $_.Keys) { $copy[$key] = $_[$key] }
        $copy
    })
}

function New-SmokeEscapeFixturePath {
    param([Parameter(Mandatory = $true)] [string] $CanonicalTempRoot)

    $parent = [IO.Directory]::GetParent($CanonicalTempRoot)
    if ($null -ne $parent) {
        return [IO.Path]::Combine($parent.FullName, 'skill-manager-live-smoke-escape')
    }
    [IO.Path]::Combine($CanonicalTempRoot, 'outside', 'skill-manager-live-smoke-escape')
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

$duplicateRealpathCommands = @(
    [pscustomobject]@{ Source = '/usr/bin/realpath' },
    [pscustomobject]@{ Source = '/bin/realpath' }
)
$selectedRealpathCommand = Select-SmokeApplicationCommand -Commands $duplicateRealpathCommands
if (
    $selectedRealpathCommand -is [array] -or
    $selectedRealpathCommand.Source -is [array] -or
    $selectedRealpathCommand.Source -cne '/usr/bin/realpath'
) {
    throw 'Duplicate realpath applications must resolve to one deterministic executable.'
}
if ($null -ne (Select-SmokeApplicationCommand -Commands @())) {
    throw 'An absent realpath application must remain absent.'
}

$canonicalTempRoot = Resolve-SmokeCanonicalExistingPath -Path ([IO.Path]::GetTempPath())
if ($IsWindows) {
    $providerTempRoot = (Resolve-Path -LiteralPath ([IO.Path]::GetTempPath())).ProviderPath
    $verbatimTempRoot = if ($providerTempRoot.StartsWith('\\', [StringComparison]::Ordinal)) {
        '\\?\UNC\' + $providerTempRoot.Substring(2)
    } else {
        '\\?\' + $providerTempRoot
    }
    $canonicalVerbatimTempRoot = Resolve-SmokeCanonicalExistingPath -Path $verbatimTempRoot
    if (-not $canonicalTempRoot.Equals($canonicalVerbatimTempRoot, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Canonical cleanup paths differ between normal and Windows verbatim spellings.'
    }
} elseif ((Test-Path -LiteralPath '/var') -and (Test-Path -LiteralPath '/private/var')) {
    $canonicalVar = Resolve-SmokeCanonicalExistingPath -Path '/var'
    $canonicalPrivateVar = Resolve-SmokeCanonicalExistingPath -Path '/private/var'
    if ($canonicalVar -cne $canonicalPrivateVar) {
        throw 'Canonical cleanup paths differ between /var and /private/var.'
    }
}
$validContainedSmoke = Join-Path $canonicalTempRoot 'skill-manager-live-smoke-fixture'
Assert-SmokePathContained `
    -CanonicalTempRoot $canonicalTempRoot `
    -CanonicalSmokeRoot $validContainedSmoke
$escapedSmoke = New-SmokeEscapeFixturePath -CanonicalTempRoot $canonicalTempRoot
Assert-Rejected {
    Assert-SmokePathContained `
        -CanonicalTempRoot $canonicalTempRoot `
        -CanonicalSmokeRoot $escapedSmoke
} 'SMOKE_PATH_ESCAPE' 'live-smoke cleanup lexical escape'
$canonicalPathRoot = [IO.Path]::GetPathRoot($canonicalTempRoot)
$rootEscapedSmoke = New-SmokeEscapeFixturePath -CanonicalTempRoot $canonicalPathRoot
Assert-Rejected {
    Assert-SmokePathContained `
        -CanonicalTempRoot $canonicalPathRoot `
        -CanonicalSmokeRoot $rootEscapedSmoke
} 'SMOKE_PATH_ESCAPE' 'live-smoke cleanup escape from a filesystem root'
$nestedSmoke = Join-Path $canonicalTempRoot 'nested/skill-manager-live-smoke-escape'
Assert-Rejected {
    Assert-SmokePathContained `
        -CanonicalTempRoot $canonicalTempRoot `
        -CanonicalSmokeRoot $nestedSmoke
} 'SMOKE_PATH_ESCAPE' 'live-smoke cleanup must be a direct child'

$validPrSmoke = @(
    'jobs:',
    '  preflight:',
    '    steps:',
    '      - name: Fixture host',
    '        run: pwsh -File tools/run-build-matrix-entry.ps1 -Cli sample -Phase Host',
    '      - name: Fixture target',
    '        run: pwsh -File tools/run-build-matrix-entry.ps1 -Cli sample -Target sample -Native false -Zig true -Phase Target',
    '      - name: Exercise fork head through a live GitHub source',
    "        if: matrix.target == 'aarch64-unknown-linux-musl'",
    '        shell: bash',
    '        env:',
    '          SMOKE_REPOSITORY: ${{ github.event.pull_request.head.repo.full_name }}',
    '          SMOKE_REF: ${{ github.event.pull_request.head.sha }}',
    '        run: |',
    '          set -euo pipefail',
    '          smoke_source_url="https://github.com/${SMOKE_REPOSITORY}/tree/${SMOKE_REF}/skills"',
    '          smoke_stage=$(sudo mktemp -d /tmp/skill-manager-live-stage.XXXXXXXX)',
    '          trap ''sudo rm -rf -- "${smoke_stage:?}" || true'' EXIT',
    '          sudo chmod 0755 "$smoke_stage"',
    '          sudo install -o root -g root -m 0755 "${GITHUB_WORKSPACE}/tools/live-github-smoke.sh" "$smoke_stage/run-smoke"',
    '          sudo install -o root -g root -m 0755 "${GITHUB_WORKSPACE}/clis/skill-manager/target/release/skill-manager" "$smoke_stage/skill-manager"',
    '          sudo useradd --no-create-home --shell /bin/bash skill-manager-smoke',
    '          trap ''sudo userdel skill-manager-smoke || true; sudo rm -rf -- "${smoke_stage:?}" || true'' EXIT',
    '          sudo --user skill-manager-smoke --set-home /bin/bash \',
    '            "$smoke_stage/run-smoke" \',
    '            "$smoke_stage/skill-manager" \',
    '            "$smoke_source_url"'
) -join [Environment]::NewLine
$validMainSmoke = @(
    'jobs:',
    '  github-source-smoke:',
    '    steps:',
    '      - name: Exercise GitHub source at an exact commit',
    '        run: |',
    '          set -euo pipefail',
    '          smoke_source_url="https://github.com/${GITHUB_REPOSITORY}/tree/${GITHUB_SHA}/skills"',
    '          smoke_stage=$(sudo mktemp -d /tmp/skill-manager-live-stage.XXXXXXXX)',
    '          trap ''sudo rm -rf -- "${smoke_stage:?}" || true'' EXIT',
    '          sudo chmod 0755 "$smoke_stage"',
    '          sudo install -o root -g root -m 0755 "${GITHUB_WORKSPACE}/tools/live-github-smoke.sh" "$smoke_stage/run-smoke"',
    '          sudo install -o root -g root -m 0755 "${GITHUB_WORKSPACE}/clis/skill-manager/target/release/skill-manager" "$smoke_stage/skill-manager"',
    '          sudo useradd --no-create-home --shell /bin/bash skill-manager-smoke',
    '          trap ''sudo userdel skill-manager-smoke || true; sudo rm -rf -- "${smoke_stage:?}" || true'' EXIT',
    '          sudo --user skill-manager-smoke --set-home /bin/bash \',
    '            "$smoke_stage/run-smoke" \',
    '            "$smoke_stage/skill-manager" \',
    '            "$smoke_source_url"'
) -join [Environment]::NewLine
Assert-LiveSmokeWorkflowContract -PrWorkflow $validPrSmoke -MainWorkflow $validMainSmoke
$crlfContinuation = "sudo --user smoke /bin/bash \`r`n  `"/absolute/helper.sh`""
if (
    (ConvertTo-ExecutableShellScript -Script $crlfContinuation) -cne
    'sudo --user smoke /bin/bash "/absolute/helper.sh"'
) {
    throw 'Guardrail did not preserve a bare terminal backslash continuation across CRLF.'
}

Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke.Replace('/tools/live-github-smoke.sh', '/tools/not-live-smoke.sh')) `
        -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_COMMAND_SET' 'missing PR live smoke'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke.Replace(
            '          sudo --user skill-manager-smoke --set-home /bin/bash \',
            '          # sudo --user skill-manager-smoke --set-home /bin/bash \'
        )) `
        -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_COMMAND_SET' 'commented-out PR sudo boundary'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke.Replace(
            '          sudo --user skill-manager-smoke --set-home /bin/bash \',
            '          echo ignored # sudo --user skill-manager-smoke --set-home /bin/bash \'
        )) `
        -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_COMMAND_SET' 'inline-commented PR sudo boundary'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke.Replace(
            '          sudo --user skill-manager-smoke --set-home /bin/bash \',
            '          sudo --user skill-manager-smoke --set-home /bin/bash \ # not a continuation'
        )) `
        -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_COMMAND_SET' 'backslash before an inline comment is not a continuation'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke.Replace(
            '          sudo install -o root -g root -m 0755 "${GITHUB_WORKSPACE}/tools/live-github-smoke.sh" "$smoke_stage/run-smoke"',
            '          # sudo install -o root -g root -m 0755 "${GITHUB_WORKSPACE}/tools/live-github-smoke.sh" "$smoke_stage/run-smoke"'
        )) `
        -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_COMMAND_SET' 'commented-out PR helper path'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke.Replace(
            '          sudo install -o root -g root -m 0755 "${GITHUB_WORKSPACE}/clis/skill-manager/target/release/skill-manager" "$smoke_stage/skill-manager"',
            '          # sudo install -o root -g root -m 0755 "${GITHUB_WORKSPACE}/clis/skill-manager/target/release/skill-manager" "$smoke_stage/skill-manager"'
        )) `
        -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_COMMAND_SET' 'commented-out PR binary path'
foreach ($stagingMutation in @(
    [pscustomobject]@{
        Case = 'stage created without root ownership'
        Old = '          smoke_stage=$(sudo mktemp -d /tmp/skill-manager-live-stage.XXXXXXXX)'
        New = '          smoke_stage=$(mktemp -d /tmp/skill-manager-live-stage.XXXXXXXX)'
    },
    [pscustomobject]@{
        Case = 'stage blocks unprivileged traversal'
        Old = '          sudo chmod 0755 "$smoke_stage"'
        New = '          sudo chmod 0700 "$smoke_stage"'
    },
    [pscustomobject]@{
        Case = 'staged helper is not root-owned'
        Old = '          sudo install -o root -g root -m 0755 "${GITHUB_WORKSPACE}/tools/live-github-smoke.sh" "$smoke_stage/run-smoke"'
        New = '          sudo install -o runner -g runner -m 0755 "${GITHUB_WORKSPACE}/tools/live-github-smoke.sh" "$smoke_stage/run-smoke"'
    },
    [pscustomobject]@{
        Case = 'staged helper is not executable'
        Old = '          sudo install -o root -g root -m 0755 "${GITHUB_WORKSPACE}/tools/live-github-smoke.sh" "$smoke_stage/run-smoke"'
        New = '          sudo install -o root -g root -m 0644 "${GITHUB_WORKSPACE}/tools/live-github-smoke.sh" "$smoke_stage/run-smoke"'
    },
    [pscustomobject]@{
        Case = 'staging cleanup is not armed before copies'
        Old = '          trap ''sudo rm -rf -- "${smoke_stage:?}" || true'' EXIT'
        New = '          trap ''true'' EXIT'
    },
    [pscustomobject]@{
        Case = 'user cleanup drops staging cleanup'
        Old = '          trap ''sudo userdel skill-manager-smoke || true; sudo rm -rf -- "${smoke_stage:?}" || true'' EXIT'
        New = '          trap ''sudo userdel skill-manager-smoke || true'' EXIT'
    },
    [pscustomobject]@{
        Case = 'unprivileged helper executes from the workspace'
        Old = '            "$smoke_stage/run-smoke" \'
        New = '            "${GITHUB_WORKSPACE}/tools/live-github-smoke.sh" \'
    },
    [pscustomobject]@{
        Case = 'unprivileged binary executes from the workspace'
        Old = '            "$smoke_stage/skill-manager" \'
        New = '            "${GITHUB_WORKSPACE}/clis/skill-manager/target/release/skill-manager" \'
    },
    [pscustomobject]@{
        Case = 'unprivileged helper executes from runner home'
        Old = '            "$smoke_stage/run-smoke" \'
        New = '            "/home/runner/run-smoke" \'
    }
)) {
    Assert-Rejected {
        Assert-LiveSmokeWorkflowContract `
            -PrWorkflow ($validPrSmoke.Replace($stagingMutation.Old, $stagingMutation.New)) `
            -MainWorkflow $validMainSmoke
    } 'LIVE_SMOKE_COMMAND_SET' $stagingMutation.Case
}
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke.Replace(
            '          SMOKE_REF: ${{ github.event.pull_request.head.sha }}',
            '          # SMOKE_REF: ${{ github.event.pull_request.head.sha }}'
        )) `
        -MainWorkflow $validMainSmoke
} 'PR_LIVE_SMOKE_SOURCE' 'commented-out PR source ref'
$commentedSecretPrSmoke = $validPrSmoke.Replace(
    '          SMOKE_REF: ${{ github.event.pull_request.head.sha }}',
    "          SMOKE_REF: `${{ github.event.pull_request.head.sha }}`n          # GH_TOKEN: `${{ secrets.GITHUB_TOKEN }}"
)
Assert-LiveSmokeWorkflowContract -PrWorkflow $commentedSecretPrSmoke -MainWorkflow $validMainSmoke
$commentedHelperPrSmoke = $validPrSmoke + [Environment]::NewLine + @(
    '      # - run: bash tools/live-github-smoke.sh',
    '      - run: echo ok # bash "${GITHUB_WORKSPACE}"/tools/live-github-smoke.sh'
) -join [Environment]::NewLine
Assert-LiveSmokeWorkflowContract -PrWorkflow $commentedHelperPrSmoke -MainWorkflow $validMainSmoke
$commentedHelperMainSmoke = $validMainSmoke + [Environment]::NewLine + @(
    '      # - run: bash tools/live-github-smoke.sh',
    '      - run: echo ok # bash "${GITHUB_WORKSPACE}"/tools/live-github-smoke.sh'
) -join [Environment]::NewLine
Assert-LiveSmokeWorkflowContract -PrWorkflow $validPrSmoke -MainWorkflow $commentedHelperMainSmoke
foreach ($alternateHelperCommand in @(
    'bash tools/live-github-smoke.sh',
    'bash "${GITHUB_WORKSPACE}"/tools/live-github-smoke.sh',
    'bash tools/live-github-"smoke".sh',
    'HELPER=tools/live-github-smoke.sh; bash "$HELPER"',
    'HELPER=tools/live-github-smoke.sh'
)) {
    Assert-Rejected {
        Assert-LiveSmokeWorkflowContract `
            -PrWorkflow ($validPrSmoke + [Environment]::NewLine + "      - run: $alternateHelperCommand") `
            -MainWorkflow $validMainSmoke
    } 'LIVE_SMOKE_COMMAND_SET' "alternate PR live-smoke helper reference: $alternateHelperCommand"
    Assert-Rejected {
        Assert-LiveSmokeWorkflowContract `
            -PrWorkflow $validPrSmoke `
            -MainWorkflow ($validMainSmoke + [Environment]::NewLine + "      - run: $alternateHelperCommand")
    } 'LIVE_SMOKE_COMMAND_SET' "alternate main live-smoke helper reference: $alternateHelperCommand"
}
$splitStemHelperCommand = 'HELPER_STEM=live-github-smoke; bash "${GITHUB_WORKSPACE}/tools/${HELPER_STEM}.sh"'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke + [Environment]::NewLine + "      - run: $splitStemHelperCommand") `
        -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_COMMAND_SET' 'split-stem PR live-smoke helper reconstruction'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow $validPrSmoke `
        -MainWorkflow ($validMainSmoke + [Environment]::NewLine + "      - run: $splitStemHelperCommand")
} 'LIVE_SMOKE_COMMAND_SET' 'split-stem main live-smoke helper reconstruction'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke + [Environment]::NewLine + '      - run: "${GITHUB_WORKSPACE}/tools/live-github-smoke.sh"') `
        -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_COMMAND_SET' 'duplicate PR live smoke'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke.Replace("matrix.target == 'aarch64-unknown-linux-musl'", "matrix.target == 'x86_64-pc-windows-msvc'")) `
        -MainWorkflow $validMainSmoke
} 'PR_LIVE_SMOKE_CONDITION' 'wrong PR live-smoke matrix condition'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke.Replace('github.event.pull_request.head.sha', 'github.sha')) `
        -MainWorkflow $validMainSmoke
} 'PR_LIVE_SMOKE_SOURCE' 'PR live smoke using merge SHA'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke.Replace(' -Phase Target', ' -Phase Final')) `
        -MainWorkflow $validMainSmoke
} 'PR_LIVE_SMOKE_ORDER' 'PR live smoke without preceding Target phase'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke.Replace(
            '          sudo --user skill-manager-smoke --set-home /bin/bash \',
            "          sudo --user skill-manager-smoke just live-smoke`n          sudo --user skill-manager-smoke --set-home /bin/bash \"
        )) `
        -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_COMMAND_SET' 'bare sudo just in PR live smoke'
foreach ($boundaryCommand in @(
    'echo ok; sudo --user skill-manager-smoke just live-smoke',
    'true && sudo --user skill-manager-smoke just live-smoke',
    'false || sudo --user skill-manager-smoke just live-smoke',
    'printf x | sudo --user skill-manager-smoke just live-smoke'
)) {
    Assert-Rejected {
        Assert-LiveSmokeWorkflowContract `
            -PrWorkflow ($validPrSmoke.Replace(
                '          sudo --user skill-manager-smoke --set-home /bin/bash \',
                "          $boundaryCommand`n          sudo --user skill-manager-smoke --set-home /bin/bash \"
            )) `
            -MainWorkflow $validMainSmoke
    } 'LIVE_SMOKE_COMMAND_SET' "sudo just after shell boundary: $boundaryCommand"
}
$quotedSudoPrSmoke = $validPrSmoke.Replace(
    '          sudo --user skill-manager-smoke --set-home /bin/bash \',
    "          echo 'ok; sudo --user skill-manager-smoke just live-smoke'`n          sudo --user skill-manager-smoke --set-home /bin/bash \"
)
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract -PrWorkflow $quotedSudoPrSmoke -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_COMMAND_SET' 'quoted sudo text is an unapproved extra command'
$parenthesizedSudoPrSmoke = $validPrSmoke.Replace(
    '          sudo --user skill-manager-smoke --set-home /bin/bash \',
    '          (sudo --user skill-manager-smoke --set-home /bin/bash \'
).Replace(
    '            "$smoke_source_url"',
    '            "$smoke_source_url")'
)
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract -PrWorkflow $parenthesizedSudoPrSmoke -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_COMMAND_SET' 'parenthesized sudo invocation'
$bracedSudoPrSmoke = $validPrSmoke.Replace(
    '          sudo --user skill-manager-smoke --set-home /bin/bash \',
    '          { sudo --user skill-manager-smoke --set-home /bin/bash \'
).Replace(
    '            "$smoke_source_url"',
    '            "$smoke_source_url"; }'
)
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract -PrWorkflow $bracedSudoPrSmoke -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_COMMAND_SET' 'braced sudo invocation'
foreach ($sudoPrefix in @('command sudo', "'sudo'")) {
    $prefixedSudoPrSmoke = $validPrSmoke.Replace(
        '          sudo --user skill-manager-smoke --set-home /bin/bash \',
        "          $sudoPrefix --user skill-manager-smoke --set-home /bin/bash \"
    )
    Assert-Rejected {
        Assert-LiveSmokeWorkflowContract -PrWorkflow $prefixedSudoPrSmoke -MainWorkflow $validMainSmoke
    } 'LIVE_SMOKE_COMMAND_SET' "alternate sudo executable form: $sudoPrefix"
}
foreach ($extraCommand in @('echo unexpected', 'true')) {
    $extraCommandPrSmoke = $validPrSmoke.Replace(
        '          smoke_source_url=',
        "          $extraCommand`n          smoke_source_url="
    )
    Assert-Rejected {
        Assert-LiveSmokeWorkflowContract -PrWorkflow $extraCommandPrSmoke -MainWorkflow $validMainSmoke
    } 'LIVE_SMOKE_COMMAND_SET' "unapproved extra command: $extraCommand"
}
foreach ($unsafeSudo in @(
    'sudo -E --user skill-manager-smoke true',
    'sudo --preserve-env=PATH --user skill-manager-smoke true',
    'sudo --user skill-manager-smoke PATH=/workspace/bin true'
)) {
    Assert-Rejected {
        Assert-LiveSmokeWorkflowContract `
            -PrWorkflow ($validPrSmoke.Replace(
                '          sudo --user skill-manager-smoke --set-home /bin/bash \',
                "          $unsafeSudo`n          sudo --user skill-manager-smoke --set-home /bin/bash \"
            )) `
            -MainWorkflow $validMainSmoke
    } 'LIVE_SMOKE_COMMAND_SET' "unsafe sudo environment: $unsafeSudo"
}
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke.Replace(
            '          SMOKE_REF: ${{ github.event.pull_request.head.sha }}',
            "          SMOKE_REF: `${{ github.event.pull_request.head.sha }}`n          GH_TOKEN: `${{ secrets.GITHUB_TOKEN }}"
        )) `
        -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_TOKEN' 'secret token in PR live smoke'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow ($validPrSmoke.Replace(
            '"${GITHUB_WORKSPACE}/clis/skill-manager/target/release/skill-manager"',
            '"clis/skill-manager/target/release/skill-manager"'
        )) `
        -MainWorkflow $validMainSmoke
} 'LIVE_SMOKE_COMMAND_SET' 'relative PR smoke binary'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow $validPrSmoke `
        -MainWorkflow ($validMainSmoke.Replace('/tools/live-github-smoke.sh', '/tools/not-live-smoke.sh'))
} 'LIVE_SMOKE_COMMAND_SET' 'missing main live smoke'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow $validPrSmoke `
        -MainWorkflow ($validMainSmoke + [Environment]::NewLine + '      - run: "${GITHUB_WORKSPACE}/tools/live-github-smoke.sh"')
} 'LIVE_SMOKE_COMMAND_SET' 'duplicate main live smoke'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow $validPrSmoke `
        -MainWorkflow ($validMainSmoke.Replace('${GITHUB_SHA}', '${GITHUB_REF}'))
} 'MAIN_LIVE_SMOKE_SOURCE' 'main live smoke using branch ref'
Assert-Rejected {
    Assert-LiveSmokeWorkflowContract `
        -PrWorkflow $validPrSmoke `
        -MainWorkflow ($validMainSmoke.Replace('--user skill-manager-smoke', '--user root'))
} 'LIVE_SMOKE_COMMAND_SET' 'main live smoke running as root'

$validLiveSmokeHelper = @'
#!/usr/bin/env bash
set -euo pipefail
"$skill_manager" --json source add "$source_url" live-github-smoke
"$skill_manager" --json status --refresh
'@
Assert-LiveSmokeHelperContract -Helper $validLiveSmokeHelper
Assert-Rejected {
    Assert-LiveSmokeHelperContract -Helper ($validLiveSmokeHelper.Replace(' live-github-smoke', ''))
} 'LIVE_SMOKE_HELPER_SOURCE' 'live-smoke helper missing noninteractive source name'
Assert-Rejected {
    Assert-LiveSmokeHelperContract -Helper (
        $validLiveSmokeHelper + [Environment]::NewLine + '"$skill_manager" --version'
    )
} 'LIVE_SMOKE_HELPER_CALLS' 'live-smoke helper with an extra CLI call'
Assert-Rejected {
    Assert-LiveSmokeHelperContract -Helper ($validLiveSmokeHelper.Replace(
        '"$skill_manager" --json source add "$source_url" live-github-smoke',
        'eval "$skill_manager --json source add $source_url live-github-smoke"'
    ))
} 'LIVE_SMOKE_HELPER_EVAL' 'live-smoke helper using eval'

$validLocalJustfile = @'
live-smoke url:
    pwsh -File ../../tools/live-github-smoke.ps1 -SourceUrl {{quote(url)}}
'@
$validLocalWrapper = @'
. (Join-Path $PSScriptRoot 'live-smoke-paths.ps1')
$smokeRoot = Join-Path $root "skill-manager-live-smoke-$id"
[Environment]::SetEnvironmentVariable('SKILL_MANAGER_HOME', $smokeRoot, 'Process')
try {
    & $resolvedBinary --json source add $SourceUrl live-github-smoke
    & $resolvedBinary --json status --refresh
} finally {
    $canonicalTempRoot = Resolve-SmokeCanonicalExistingPath -Path $systemTempRoot
    $canonicalSmokeRoot = Resolve-SmokeCanonicalExistingPath -Path $smokeRoot
    Assert-SmokePathContained -CanonicalTempRoot $canonicalTempRoot -CanonicalSmokeRoot $canonicalSmokeRoot
    Remove-Item -LiteralPath $canonicalSmokeRoot -Recurse -Force
}
'@
Assert-LocalLiveSmokeContract -Justfile $validLocalJustfile -Wrapper $validLocalWrapper
Assert-Rejected {
    Assert-LocalLiveSmokeContract `
        -Justfile ($validLocalJustfile.Replace(
            'pwsh -File ../../tools/live-github-smoke.ps1 -SourceUrl {{quote(url)}}',
            'bash ../../tools/live-github-smoke.sh "$PWD/target/release/skill-manager" {{quote(url)}}'
        )) `
        -Wrapper $validLocalWrapper
} 'LOCAL_LIVE_SMOKE_RECIPE' 'Windows-incompatible local live-smoke recipe'
Assert-Rejected {
    Assert-LocalLiveSmokeContract `
        -Justfile $validLocalJustfile `
        -Wrapper ($validLocalWrapper.Replace(' live-github-smoke', ''))
} 'LOCAL_LIVE_SMOKE_SOURCE' 'local wrapper missing source name'
Assert-Rejected {
    Assert-LocalLiveSmokeContract `
        -Justfile $validLocalJustfile `
        -Wrapper ($validLocalWrapper.Replace(
            "    Remove-Item -LiteralPath `$canonicalSmokeRoot -Recurse -Force",
            '    Write-Output skipped-cleanup'
        ))
} 'LOCAL_LIVE_SMOKE_ISOLATION' 'local wrapper missing cleanup'

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
