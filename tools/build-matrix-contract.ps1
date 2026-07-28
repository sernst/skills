function Get-CanonicalFullBuildTargets {
    @(
        [ordered]@{ runner='macos-15-intel'; target='x86_64-apple-darwin'; archive='tar.gz'; native=$true; zig=$false; musl=$false; msvc_arm64=$false },
        [ordered]@{ runner='macos-15'; target='aarch64-apple-darwin'; archive='tar.gz'; native=$true; zig=$false; musl=$false; msvc_arm64=$false },
        [ordered]@{ runner='windows-2025'; target='x86_64-pc-windows-msvc'; archive='zip'; native=$true; zig=$false; musl=$false; msvc_arm64=$false },
        [ordered]@{ runner='windows-2025'; target='aarch64-pc-windows-msvc'; archive='zip'; native=$false; zig=$false; musl=$false; msvc_arm64=$true },
        [ordered]@{ runner='ubuntu-24.04'; target='x86_64-unknown-linux-gnu'; archive='tar.gz'; native=$true; zig=$false; musl=$false; msvc_arm64=$false },
        [ordered]@{ runner='ubuntu-24.04'; target='aarch64-unknown-linux-gnu'; archive='tar.gz'; native=$false; zig=$true; musl=$false; msvc_arm64=$false },
        [ordered]@{ runner='ubuntu-24.04'; target='x86_64-unknown-linux-musl'; archive='tar.gz'; native=$true; zig=$false; musl=$true; msvc_arm64=$false },
        [ordered]@{ runner='ubuntu-24.04'; target='aarch64-unknown-linux-musl'; archive='tar.gz'; native=$false; zig=$true; musl=$false; msvc_arm64=$false }
    )
}

function Throw-BuildContractViolation {
    param(
        [Parameter(Mandatory = $true)] [string] $Reason,
        [Parameter(Mandatory = $true)] [string] $Message
    )
    throw "[$Reason] $Message"
}

function Assert-TargetEntries {
    param(
        [Parameter(Mandatory = $true)] [object[]] $Actual,
        [Parameter(Mandatory = $true)] [object[]] $Expected,
        [Parameter(Mandatory = $true)] [string] $Profile
    )

    if ($Actual.Count -ne $Expected.Count) {
        Throw-BuildContractViolation 'MATRIX_COUNT' "The $Profile build profile must contain exactly $($Expected.Count) targets."
    }
    $properties = @('runner', 'target', 'archive', 'native', 'zig', 'musl', 'msvc_arm64')
    for ($index = 0; $index -lt $Expected.Count; $index++) {
        if ($null -eq $Actual[$index] -or $Actual[$index] -isnot [System.Collections.IDictionary]) {
            Throw-BuildContractViolation 'MATRIX_TYPE' "The $Profile build profile entry $index must be a mapping."
        }
        $actualKeys = @($Actual[$index].Keys | ForEach-Object { [string]$_ })
        foreach ($actualKey in $actualKeys) {
            if ($actualKey -notin $properties) {
                Throw-BuildContractViolation 'MATRIX_EXTRA_KEY' "The $Profile build profile entry $index contains unexpected '$actualKey'."
            }
        }
        foreach ($property in $properties) {
            if (-not $Actual[$index].Contains($property)) {
                Throw-BuildContractViolation 'MATRIX_MISSING_KEY' "The $Profile build profile entry $index is missing '$property'."
            }
            $value = $Actual[$index][$property]
            $expectedType = if ($property -in @('native', 'zig', 'musl', 'msvc_arm64')) {
                [bool]
            } else {
                [string]
            }
            if ($null -eq $value -or $value.GetType() -ne $expectedType) {
                Throw-BuildContractViolation 'MATRIX_TYPE' "The $Profile build profile entry $index has an invalid '$property' type."
            }
        }
    }
    $targetIds = @($Actual | ForEach-Object { $_['target'] })
    if (@($targetIds | Sort-Object -Unique).Count -ne $targetIds.Count) {
        Throw-BuildContractViolation 'MATRIX_DUPLICATE_TARGET' "The $Profile build profile contains duplicate target IDs."
    }
    for ($index = 0; $index -lt $Expected.Count; $index++) {
        foreach ($property in $properties) {
            if ($Actual[$index][$property] -cne $Expected[$index][$property]) {
                Throw-BuildContractViolation 'MATRIX_VALUE' "The $Profile build profile entry $index has incorrect '$property' metadata."
            }
        }
    }
}

function Assert-CanonicalFullBuildTargets {
    param([Parameter(Mandatory = $true)] [object[]] $Targets)
    Assert-TargetEntries -Actual $Targets -Expected @(Get-CanonicalFullBuildTargets) -Profile 'Full'
}

function Assert-CanonicalPrBuildTargets {
    param([Parameter(Mandatory = $true)] [object[]] $Targets)
    $full = @(Get-CanonicalFullBuildTargets)
    $expectedNames = @(
        'aarch64-apple-darwin',
        'x86_64-pc-windows-msvc',
        'aarch64-unknown-linux-musl'
    )
    $expected = @($full | Where-Object { $_.target -in $expectedNames })
    Assert-TargetEntries -Actual $Targets -Expected $expected -Profile 'Pr'
}

function Get-LeadingSpaceCount {
    param([Parameter(Mandatory = $true)] [AllowEmptyString()] [string] $Line)
    if ($Line -match '^(?<spaces>[ ]*)') { return $Matches['spaces'].Length }
    0
}

function Get-StepRunText {
    param(
        [Parameter(Mandatory = $true)] [AllowEmptyString()] [string[]] $Lines,
        [Parameter(Mandatory = $true)] [int] $StepIndent
    )

    $commands = @()
    for ($index = 0; $index -lt $Lines.Count; $index++) {
        $line = $Lines[$index]
        $match = [regex]::Match($line, '^[ ]*-\s+run:\s*(?<value>.*)$')
        if (-not $match.Success) {
            $match = [regex]::Match($line, '^[ ]*run:\s*(?<value>.*)$')
            if (-not $match.Success -or (Get-LeadingSpaceCount $line) -ne $StepIndent + 2) {
                continue
            }
        }
        $value = $match.Groups['value'].Value.Trim()
        if ($value -notmatch '^[|>][+-]?\s*(?:#.*)?$') {
            $commands += $value
            continue
        }
        $runIndent = Get-LeadingSpaceCount $line
        $block = @()
        for ($blockIndex = $index + 1; $blockIndex -lt $Lines.Count; $blockIndex++) {
            $blockLine = $Lines[$blockIndex]
            if ($blockLine.Trim().Length -gt 0 -and (Get-LeadingSpaceCount $blockLine) -le $runIndent) {
                break
            }
            $block += $blockLine
            $index = $blockIndex
        }
        $commands += ($block -join [Environment]::NewLine)
    }
    $commands -join [Environment]::NewLine
}

function Remove-ShellInlineComment {
    param([Parameter(Mandatory = $true)] [AllowEmptyString()] [string] $Line)

    $singleQuoted = $false
    $doubleQuoted = $false
    $escaped = $false
    for ($index = 0; $index -lt $Line.Length; $index++) {
        $character = $Line[$index]
        if ($escaped) {
            $escaped = $false
            continue
        }
        if ($character -eq '\' -and -not $singleQuoted) {
            $escaped = $true
            continue
        }
        if ($character -eq "'" -and -not $doubleQuoted) {
            $singleQuoted = -not $singleQuoted
            continue
        }
        if ($character -eq '"' -and -not $singleQuoted) {
            $doubleQuoted = -not $doubleQuoted
            continue
        }
        if ($character -eq '#' -and -not $singleQuoted -and -not $doubleQuoted) {
            $startsComment = $index -eq 0 -or [char]::IsWhiteSpace($Line[$index - 1]) -or
                $Line[$index - 1] -in @(';', '|', '&', '(', ')')
            if ($startsComment) {
                return $Line.Substring(0, $index).TrimEnd()
            }
        }
    }
    $Line
}

function Get-CommentFreeLines {
    param([Parameter(Mandatory = $true)] [AllowEmptyString()] [string] $Text)

    $lines = @()
    foreach ($line in @([regex]::Split($Text, '\r?\n'))) {
        $withoutComment = Remove-ShellInlineComment -Line $line
        if ($withoutComment.Trim().Length -gt 0) {
            $lines += $withoutComment
        }
    }
    @($lines)
}

function ConvertTo-CommentFreeText {
    param([Parameter(Mandatory = $true)] [AllowEmptyString()] [string] $Text)
    @(Get-CommentFreeLines -Text $Text) -join [Environment]::NewLine
}

function ConvertTo-ExecutableShellScript {
    param([Parameter(Mandatory = $true)] [AllowEmptyString()] [string] $Script)

    $commands = @()
    $pending = ''
    foreach ($physicalLine in @([regex]::Split($Script, '\r?\n'))) {
        $physical = $physicalLine.TrimEnd()
        $continued = $physical.EndsWith('\', [StringComparison]::Ordinal) -or
            $physical.EndsWith('`', [StringComparison]::Ordinal)
        $command = (Remove-ShellInlineComment -Line $physical).Trim()
        if ($command.Length -eq 0) {
            if ($pending.Length -gt 0) {
                $commands += $pending
                $pending = ''
            }
            continue
        }
        if ($continued) {
            if ($command.EndsWith('\', [StringComparison]::Ordinal) -or $command.EndsWith('`', [StringComparison]::Ordinal)) {
                $command = $command.Substring(0, $command.Length - 1).TrimEnd()
            } else {
                $continued = $false
            }
        }
        $pending = if ($pending.Length -gt 0) { "$pending $command" } else { $command }
        if (-not $continued) {
            $commands += $pending
            $pending = ''
        }
    }
    if ($pending.Length -gt 0) {
        $commands += $pending
    }
    $commands -join [Environment]::NewLine
}

function Split-ExecutableShellCommands {
    param([Parameter(Mandatory = $true)] [AllowEmptyString()] [string] $Script)

    $commands = [Collections.Generic.List[string]]::new()
    $current = [Text.StringBuilder]::new()
    $singleQuoted = $false
    $doubleQuoted = $false
    $escaped = $false

    $addCurrentShellCommand = {
        $command = $current.ToString().Trim()
        if ($command.Length -gt 0) {
            $commands.Add($command)
        }
        [void]$current.Clear()
    }

    for ($index = 0; $index -lt $Script.Length; $index++) {
        $character = $Script[$index]
        if ($escaped) {
            [void]$current.Append($character)
            $escaped = $false
            continue
        }
        if ($character -eq '\' -and -not $singleQuoted) {
            [void]$current.Append($character)
            $escaped = $true
            continue
        }
        if ($character -eq "'" -and -not $doubleQuoted) {
            $singleQuoted = -not $singleQuoted
            [void]$current.Append($character)
            continue
        }
        if ($character -eq '"' -and -not $singleQuoted) {
            $doubleQuoted = -not $doubleQuoted
            [void]$current.Append($character)
            continue
        }
        if (-not $singleQuoted -and -not $doubleQuoted) {
            if ($character -eq "`n" -or $character -eq "`r" -or $character -eq ';') {
                . $addCurrentShellCommand
                continue
            }
            if ($character -eq '&' -or $character -eq '|') {
                . $addCurrentShellCommand
                if ($index + 1 -lt $Script.Length -and $Script[$index + 1] -eq $character) {
                    $index++
                }
                continue
            }
        }
        [void]$current.Append($character)
    }
    . $addCurrentShellCommand
    @($commands)
}

function ConvertTo-WorkflowStep {
    param(
        [Parameter(Mandatory = $true)] [AllowEmptyString()] [string[]] $Lines,
        [Parameter(Mandatory = $true)] [int] $Start,
        [Parameter(Mandatory = $true)] [int] $End,
        [Parameter(Mandatory = $true)] [int] $Indent
    )
    $stepLines = @($Lines[$Start..($End - 1)])
    $text = $stepLines -join [Environment]::NewLine
    $run = Get-StepRunText -Lines $stepLines -StepIndent $Indent
    [pscustomobject]@{
        Run = $run
        Text = $text
        ActiveText = ConvertTo-CommentFreeText -Text $text
        Executable = ConvertTo-ExecutableShellScript -Script $run
        Position = $Start
    }
}

function Get-WorkflowSteps {
    param([Parameter(Mandatory = $true)] [string] $Workflow)

    $lines = @([regex]::Split($Workflow, '\r?\n'))
    $steps = @()
    $stepsIndent = -1
    $stepIndent = -1
    $stepStart = -1

    for ($lineIndex = 0; $lineIndex -lt $lines.Count; $lineIndex++) {
        $line = $lines[$lineIndex]
        $trimmed = $line.Trim()
        $indent = Get-LeadingSpaceCount $line

        if ($stepsIndent -ge 0) {
            if ($trimmed.Length -gt 0 -and -not $trimmed.StartsWith('#') -and $indent -le $stepsIndent) {
                if ($stepStart -ge 0) {
                    $steps += ConvertTo-WorkflowStep -Lines $lines -Start $stepStart -End $lineIndex -Indent $stepIndent
                }
                $stepsIndent = -1
                $stepIndent = -1
                $stepStart = -1
            } elseif ($line -match '^[ ]*-\s+\S' -and ($stepIndent -lt 0 -or $indent -eq $stepIndent)) {
                if ($stepStart -ge 0) {
                    $steps += ConvertTo-WorkflowStep -Lines $lines -Start $stepStart -End $lineIndex -Indent $stepIndent
                }
                $stepIndent = $indent
                $stepStart = $lineIndex
                continue
            }
        }

        if ($stepsIndent -lt 0 -and $line -match '^[ ]*steps:\s*(?:#.*)?$') {
            $stepsIndent = $indent
        }
    }
    if ($stepsIndent -ge 0 -and $stepStart -ge 0) {
        $steps += ConvertTo-WorkflowStep -Lines $lines -Start $stepStart -End $lines.Count -Indent $stepIndent
    }
    @($steps)
}

function ConvertTo-WorkflowJob {
    param(
        [Parameter(Mandatory = $true)] [AllowEmptyString()] [string[]] $Lines,
        [Parameter(Mandatory = $true)] [int] $Start,
        [Parameter(Mandatory = $true)] [int] $End,
        [Parameter(Mandatory = $true)] [string] $Name
    )
    $jobText = @($Lines[$Start..($End - 1)]) -join [Environment]::NewLine
    [pscustomobject]@{
        Name = $Name
        Steps = @(Get-WorkflowSteps -Workflow $jobText)
    }
}

function Get-WorkflowJobs {
    param([Parameter(Mandatory = $true)] [string] $Workflow)

    $lines = @([regex]::Split($Workflow, '\r?\n'))
    $jobs = @()
    $jobsIndent = -1
    $jobStart = -1
    $jobName = ''

    for ($lineIndex = 0; $lineIndex -lt $lines.Count; $lineIndex++) {
        $line = $lines[$lineIndex]
        $trimmed = $line.Trim()
        $indent = Get-LeadingSpaceCount $line

        if ($jobsIndent -ge 0) {
            if ($trimmed.Length -gt 0 -and -not $trimmed.StartsWith('#') -and $indent -le $jobsIndent) {
                if ($jobStart -ge 0) {
                    $jobs += ConvertTo-WorkflowJob -Lines $lines -Start $jobStart -End $lineIndex -Name $jobName
                }
                $jobsIndent = -1
                $jobStart = -1
                $jobName = ''
            } elseif (
                $indent -eq $jobsIndent + 2 -and
                $line -match '^[ ]*(?<name>[A-Za-z0-9_-]+):\s*(?:#.*)?$'
            ) {
                if ($jobStart -ge 0) {
                    $jobs += ConvertTo-WorkflowJob -Lines $lines -Start $jobStart -End $lineIndex -Name $jobName
                }
                $jobStart = $lineIndex
                $jobName = $Matches['name']
                continue
            }
        }

        if ($jobsIndent -lt 0 -and $line -match '^[ ]*jobs:\s*(?:#.*)?$') {
            $jobsIndent = $indent
        }
    }
    if ($jobsIndent -ge 0 -and $jobStart -ge 0) {
        $jobs += ConvertTo-WorkflowJob -Lines $lines -Start $jobStart -End $lines.Count -Name $jobName
    }
    @($jobs)
}

function Find-StepCommandOccurrences {
    param(
        [Parameter(Mandatory = $true)] [object[]] $Steps,
        [Parameter(Mandatory = $true)] [string] $Pattern
    )

    $occurrences = @()
    for ($stepIndex = 0; $stepIndex -lt $Steps.Count; $stepIndex++) {
        foreach ($match in [regex]::Matches($Steps[$stepIndex].Executable, $Pattern, [Text.RegularExpressions.RegexOptions]::IgnoreCase)) {
            $occurrences += [pscustomobject]@{
                StepIndex = $stepIndex
                Position = $match.Index
            }
        }
    }
    @($occurrences)
}

function Assert-WorkflowTargetSetupOrder {
    param(
        [Parameter(Mandatory = $true)] [string] $WorkflowName,
        [Parameter(Mandatory = $true)] [string] $Workflow
    )

    $jobs = @(Get-WorkflowJobs -Workflow $Workflow)
    if (-not $jobs.Count) {
        Throw-BuildContractViolation 'WORKFLOW_NO_STEPS' "Workflow '$WorkflowName' contains no jobs."
    }
    $phaseJobs = @()
    for ($jobIndex = 0; $jobIndex -lt $jobs.Count; $jobIndex++) {
        $jobHost = @(Find-StepCommandOccurrences -Steps $jobs[$jobIndex].Steps -Pattern 'run-build-matrix-entry\.ps1[\s\S]*?-Phase\s+Host\b')
        $jobTarget = @(Find-StepCommandOccurrences -Steps $jobs[$jobIndex].Steps -Pattern 'run-build-matrix-entry\.ps1[\s\S]*?-Phase\s+Target\b')
        if ($jobHost.Count -gt 0 -or $jobTarget.Count -gt 0) {
            $phaseJobs += [pscustomobject]@{
                Job = $jobs[$jobIndex]
                Host = $jobHost
                Target = $jobTarget
            }
        }
    }
    if ($phaseJobs.Count -eq 0) {
        Throw-BuildContractViolation 'MISSING_HOST_PHASE' "Workflow '$WorkflowName' must invoke the Host phase."
    }
    if ($phaseJobs.Count -gt 1) {
        $completeJobs = @($phaseJobs | Where-Object { $_.Host.Count -gt 0 -and $_.Target.Count -gt 0 })
        if (
            $completeJobs.Count -eq 0 -and
            @($phaseJobs | Where-Object { $_.Host.Count -gt 0 }).Count -gt 0 -and
            @($phaseJobs | Where-Object { $_.Target.Count -gt 0 }).Count -gt 0
        ) {
            Throw-BuildContractViolation 'CROSS_JOB_PHASES' "Workflow '$WorkflowName' splits Host and Target across jobs."
        }
        Throw-BuildContractViolation 'AMBIGUOUS_PHASE_JOB' "Workflow '$WorkflowName' has phase markers in multiple jobs."
    }

    $steps = @($phaseJobs[0].Job.Steps)
    $hostOccurrences = @($phaseJobs[0].Host)
    $targetOccurrences = @($phaseJobs[0].Target)
    if ($hostOccurrences.Count -eq 0) {
        Throw-BuildContractViolation 'MISSING_HOST_PHASE' "Workflow '$WorkflowName' must invoke the Host phase."
    }
    if ($hostOccurrences.Count -gt 1) {
        Throw-BuildContractViolation 'DUPLICATE_HOST_PHASE' "Workflow '$WorkflowName' must invoke the Host phase exactly once."
    }
    if ($targetOccurrences.Count -eq 0) {
        Throw-BuildContractViolation 'MISSING_TARGET_PHASE' "Workflow '$WorkflowName' must invoke the Target phase."
    }
    if ($targetOccurrences.Count -gt 1) {
        Throw-BuildContractViolation 'DUPLICATE_TARGET_PHASE' "Workflow '$WorkflowName' must invoke the Target phase exactly once."
    }
    if ($hostOccurrences[0].StepIndex -ge $targetOccurrences[0].StepIndex) {
        Throw-BuildContractViolation 'PHASE_ORDER' "Workflow '$WorkflowName' must invoke the Host phase before the Target phase."
    }

    $setupPatterns = [ordered]@{
        'Rust target' = 'rustup\s+target\s+add\b|install-ci-tools\.ps1[\s\S]*?\s-Target\b'
        'cargo-zigbuild' = 'cargo\s+install\s+cargo-zigbuild\b'
        'Zig' = 'install-zig\.(?:ps1|sh)\b'
        'musl' = '\bmusl-tools\b'
        'MSVC ARM64' = 'enable-msvc-arm64\.ps1\b'
    }
    foreach ($setup in $setupPatterns.GetEnumerator()) {
        $occurrences = @(Find-StepCommandOccurrences -Steps $steps -Pattern $setup.Value)
        if ($occurrences.Count -eq 0) {
            Throw-BuildContractViolation 'MISSING_SETUP' "Workflow '$WorkflowName' must configure $($setup.Key)."
        }
        if ($occurrences.Count -gt 1) {
            Throw-BuildContractViolation 'DUPLICATE_SETUP' "Workflow '$WorkflowName' must configure $($setup.Key) exactly once."
        }
        if ($occurrences[0].StepIndex -le $hostOccurrences[0].StepIndex) {
            Throw-BuildContractViolation 'ORDER_BEFORE_HOST' "Workflow '$WorkflowName' must configure $($setup.Key) after Host."
        }
        if ($occurrences[0].StepIndex -ge $targetOccurrences[0].StepIndex) {
            Throw-BuildContractViolation 'ORDER_AFTER_TARGET' "Workflow '$WorkflowName' must configure $($setup.Key) before Target."
        }
    }
}

function Get-NamedWorkflowSteps {
    param(
        [Parameter(Mandatory = $true)] [string] $Workflow,
        [Parameter(Mandatory = $true)] [string] $StepName
    )

    $results = @()
    $namePattern = '(?m)^[ ]*-\s+name:\s*' + [regex]::Escape($StepName) + '\s*$'
    foreach ($job in @(Get-WorkflowJobs -Workflow $Workflow)) {
        for ($stepIndex = 0; $stepIndex -lt $job.Steps.Count; $stepIndex++) {
            if ([regex]::IsMatch($job.Steps[$stepIndex].ActiveText, $namePattern)) {
                $results += [pscustomobject]@{
                    Job = $job
                    Step = $job.Steps[$stepIndex]
                    StepIndex = $stepIndex
                }
            }
        }
    }
    @($results)
}

function Get-LiveSmokeHelperStemOccurrenceCount {
    param([Parameter(Mandatory = $true)] [string] $Workflow)

    $activeText = ConvertTo-CommentFreeText -Text $Workflow
    $quoteIndependentText = $activeText.Replace("'", '').Replace('"', '')
    [regex]::Matches(
        $quoteIndependentText,
        '(?<![A-Za-z0-9_-])live-github-smoke(?![A-Za-z0-9_-])',
        [Text.RegularExpressions.RegexOptions]::IgnoreCase
    ).Count
}

function Assert-ExactLiveSmokeCommandSet {
    param(
        [Parameter(Mandatory = $true)] [string] $WorkflowName,
        [Parameter(Mandatory = $true)] [object] $Step,
        [Parameter(Mandatory = $true)] [string] $SourceCommand
    )

    if (
        $Step.ActiveText -match '(?i)(?:secrets\.|github\.token|GITHUB_TOKEN|GH_TOKEN|ACTIONS_RUNTIME_TOKEN|ID_TOKEN)' -or
        $Step.Executable -match '(?i)(?:secrets\.|github\.token|GITHUB_TOKEN|GH_TOKEN|ACTIONS_RUNTIME_TOKEN|ID_TOKEN)'
    ) {
        Throw-BuildContractViolation 'LIVE_SMOKE_TOKEN' "Workflow '$WorkflowName' must not expose a secret or token to the live smoke."
    }

    $expected = @(
        'set -euo pipefail',
        $SourceCommand,
        'smoke_stage=$(sudo mktemp -d /tmp/skill-manager-live-stage.XXXXXXXX)',
        'trap ''sudo rm -rf -- "${smoke_stage:?}" || true'' EXIT',
        'sudo chmod 0755 "$smoke_stage"',
        (
            'sudo install -o root -g root -m 0755 ' +
            '"${GITHUB_WORKSPACE}/tools/live-github-smoke.sh" ' +
            '"$smoke_stage/run-smoke"'
        ),
        (
            'sudo install -o root -g root -m 0755 ' +
            '"${GITHUB_WORKSPACE}/clis/skill-manager/target/release/skill-manager" ' +
            '"$smoke_stage/skill-manager"'
        ),
        'sudo useradd --no-create-home --shell /bin/bash skill-manager-smoke',
        'trap ''sudo userdel skill-manager-smoke || true; sudo rm -rf -- "${smoke_stage:?}" || true'' EXIT',
        (
            'sudo --user skill-manager-smoke --set-home /bin/bash ' +
            '"$smoke_stage/run-smoke" ' +
            '"$smoke_stage/skill-manager" ' +
            '"$smoke_source_url"'
        )
    )
    $actual = @(Split-ExecutableShellCommands -Script $Step.Executable)
    $directWorkspaceExecutions = @($actual | Where-Object {
        $_ -match '^sudo --user skill-manager-smoke --set-home /bin/bash\b' -and
        $_ -match '(?:GITHUB_WORKSPACE|/home/runner)'
    })
    if ($directWorkspaceExecutions.Count -gt 0) {
        Throw-BuildContractViolation 'LIVE_SMOKE_COMMAND_SET' "Workflow '$WorkflowName' must not execute smoke inputs from the runner workspace as the unprivileged user."
    }
    if ($actual.Count -ne $expected.Count) {
        Throw-BuildContractViolation 'LIVE_SMOKE_COMMAND_SET' "Workflow '$WorkflowName' live smoke must contain only the approved command sequence."
    }
    for ($index = 0; $index -lt $expected.Count; $index++) {
        if ($actual[$index] -cne $expected[$index]) {
            Throw-BuildContractViolation 'LIVE_SMOKE_COMMAND_SET' "Workflow '$WorkflowName' live smoke command $index differs from the approved sequence."
        }
    }
}

function Assert-LiveSmokeWorkflowContract {
    param(
        [Parameter(Mandatory = $true)] [string] $PrWorkflow,
        [Parameter(Mandatory = $true)] [string] $MainWorkflow
    )

    $prSmokes = @(Get-NamedWorkflowSteps `
        -Workflow $PrWorkflow `
        -StepName 'Exercise fork head through a live GitHub source')
    if ($prSmokes.Count -eq 0) {
        Throw-BuildContractViolation 'MISSING_PR_LIVE_SMOKE' 'The PR workflow must invoke the shared live-smoke helper exactly once.'
    }
    if ($prSmokes.Count -gt 1) {
        Throw-BuildContractViolation 'DUPLICATE_PR_LIVE_SMOKE' 'The PR workflow must invoke the shared live-smoke helper exactly once.'
    }
    $prSmoke = $prSmokes[0]
    if ($prSmoke.Job.Name -cne 'preflight') {
        Throw-BuildContractViolation 'PR_LIVE_SMOKE_JOB' 'The PR live smoke must run in the existing preflight job.'
    }
    if ($prSmoke.Step.ActiveText -notmatch '(?m)^[ ]*if:\s*matrix\.target\s*==\s*''aarch64-unknown-linux-musl''\s*$') {
        Throw-BuildContractViolation 'PR_LIVE_SMOKE_CONDITION' 'The PR live smoke must run only for aarch64-unknown-linux-musl.'
    }
    if (
        $prSmoke.Step.ActiveText -notmatch '(?m)^[ ]*SMOKE_REPOSITORY:\s*\$\{\{\s*github\.event\.pull_request\.head\.repo\.full_name\s*\}\}\s*$' -or
        $prSmoke.Step.ActiveText -notmatch '(?m)^[ ]*SMOKE_REF:\s*\$\{\{\s*github\.event\.pull_request\.head\.sha\s*\}\}\s*$' -or
        $prSmoke.Step.Executable -notmatch '(?m)^smoke_source_url="https://github\.com/\$\{SMOKE_REPOSITORY\}/tree/\$\{SMOKE_REF\}/skills"\s*$'
    ) {
        Throw-BuildContractViolation 'PR_LIVE_SMOKE_SOURCE' 'The PR live smoke must use the public fork head repository and exact head SHA.'
    }
    $targetSteps = @(Find-StepCommandOccurrences -Steps $prSmoke.Job.Steps -Pattern 'run-build-matrix-entry\.ps1[\s\S]*?-Phase\s+Target\b')
    if (-not $targetSteps.Count -or $prSmoke.StepIndex -le $targetSteps[-1].StepIndex) {
        Throw-BuildContractViolation 'PR_LIVE_SMOKE_ORDER' 'The PR live smoke must run after the Target phase.'
    }
    Assert-ExactLiveSmokeCommandSet `
        -WorkflowName 'pr.yml' `
        -Step $prSmoke.Step `
        -SourceCommand 'smoke_source_url="https://github.com/${SMOKE_REPOSITORY}/tree/${SMOKE_REF}/skills"'
    if ((Get-LiveSmokeHelperStemOccurrenceCount -Workflow $PrWorkflow) -ne 1) {
        Throw-BuildContractViolation 'LIVE_SMOKE_COMMAND_SET' 'The PR workflow must reference the live-smoke helper stem only in the approved command sequence.'
    }

    $mainSmokes = @(Get-NamedWorkflowSteps `
        -Workflow $MainWorkflow `
        -StepName 'Exercise GitHub source at an exact commit')
    if ($mainSmokes.Count -eq 0) {
        Throw-BuildContractViolation 'MISSING_MAIN_LIVE_SMOKE' 'The main workflow must invoke the shared live-smoke helper exactly once.'
    }
    if ($mainSmokes.Count -gt 1) {
        Throw-BuildContractViolation 'DUPLICATE_MAIN_LIVE_SMOKE' 'The main workflow must invoke the shared live-smoke helper exactly once.'
    }
    $mainSmoke = $mainSmokes[0]
    if ($mainSmoke.Job.Name -cne 'github-source-smoke') {
        Throw-BuildContractViolation 'MAIN_LIVE_SMOKE_JOB' 'The main live smoke must run in the github-source-smoke job.'
    }
    if ($mainSmoke.Step.Executable -notmatch '(?m)^smoke_source_url="https://github\.com/\$\{GITHUB_REPOSITORY\}/tree/\$\{GITHUB_SHA\}/skills"\s*$') {
        Throw-BuildContractViolation 'MAIN_LIVE_SMOKE_SOURCE' 'The main live smoke must use GITHUB_REPOSITORY and the exact GITHUB_SHA.'
    }
    Assert-ExactLiveSmokeCommandSet `
        -WorkflowName 'security-and-live.yml' `
        -Step $mainSmoke.Step `
        -SourceCommand 'smoke_source_url="https://github.com/${GITHUB_REPOSITORY}/tree/${GITHUB_SHA}/skills"'
    if ((Get-LiveSmokeHelperStemOccurrenceCount -Workflow $MainWorkflow) -ne 1) {
        Throw-BuildContractViolation 'LIVE_SMOKE_COMMAND_SET' 'The main workflow must reference the live-smoke helper stem only in the approved command sequence.'
    }
}

function Assert-LiveSmokeHelperContract {
    param([Parameter(Mandatory = $true)] [string] $Helper)

    if ($Helper -notmatch '(?m)^set -euo pipefail\s*$') {
        Throw-BuildContractViolation 'LIVE_SMOKE_HELPER_STRICT' 'The live-smoke helper must enable strict Bash error handling.'
    }
    if ($Helper -match '(?m)^[ ]*eval\b') {
        Throw-BuildContractViolation 'LIVE_SMOKE_HELPER_EVAL' 'The live-smoke helper must not use eval.'
    }
    $calls = @([regex]::Matches($Helper, '(?m)^[ ]*"\$skill_manager"\s+'))
    if ($calls.Count -ne 2) {
        Throw-BuildContractViolation 'LIVE_SMOKE_HELPER_CALLS' 'The live-smoke helper must make exactly two skill-manager calls.'
    }
    $sourceCall = [regex]::Match(
        $Helper,
        '(?m)^[ ]*"\$skill_manager"\s+--json\s+source\s+add\s+"\$source_url"\s+live-github-smoke\s*$'
    )
    if (-not $sourceCall.Success) {
        Throw-BuildContractViolation 'LIVE_SMOKE_HELPER_SOURCE' 'The live-smoke helper must add the exact URL with an explicit noninteractive source name.'
    }
    $statusCall = [regex]::Match(
        $Helper,
        '(?m)^[ ]*"\$skill_manager"\s+--json\s+status\s+--refresh\s*$'
    )
    if (-not $statusCall.Success) {
        Throw-BuildContractViolation 'LIVE_SMOKE_HELPER_STATUS' 'The live-smoke helper must refresh status after adding the source.'
    }
    if ($sourceCall.Index -ge $statusCall.Index) {
        Throw-BuildContractViolation 'LIVE_SMOKE_HELPER_ORDER' 'The live-smoke helper must add the source before refreshing status.'
    }
}

function Assert-LocalLiveSmokeContract {
    param(
        [Parameter(Mandatory = $true)] [string] $Justfile,
        [Parameter(Mandatory = $true)] [string] $Wrapper
    )

    if (
        $Justfile -notmatch (
            '(?m)^[ ]*pwsh\s+-File\s+\.\./\.\./tools/live-github-smoke\.ps1\s+' +
            '-SourceUrl\s+\{\{quote\(url\)\}\}\s*$'
        ) -or
        $Justfile -match '(?im)^[ ]*(?:bash\b|.*\$PWD)'
    ) {
        Throw-BuildContractViolation 'LOCAL_LIVE_SMOKE_RECIPE' 'The local live-smoke recipe must use the cross-platform PowerShell wrapper.'
    }
    if ($Wrapper -match '(?im)^[ ]*(?:Invoke-Expression|iex)\b') {
        Throw-BuildContractViolation 'LOCAL_LIVE_SMOKE_EVAL' 'The local live-smoke wrapper must not evaluate command strings.'
    }
    $calls = @([regex]::Matches($Wrapper, '(?m)^[ ]*&\s+\$resolvedBinary\s+'))
    if ($calls.Count -ne 2) {
        Throw-BuildContractViolation 'LOCAL_LIVE_SMOKE_CALLS' 'The local live-smoke wrapper must make exactly two skill-manager calls.'
    }
    if ($Wrapper -notmatch '(?m)^[ ]*&\s+\$resolvedBinary\s+--json\s+source\s+add\s+\$SourceUrl\s+live-github-smoke\s*$') {
        Throw-BuildContractViolation 'LOCAL_LIVE_SMOKE_SOURCE' 'The local live-smoke wrapper must add the URL with an explicit source name.'
    }
    if ($Wrapper -notmatch '(?m)^[ ]*&\s+\$resolvedBinary\s+--json\s+status\s+--refresh\s*$') {
        Throw-BuildContractViolation 'LOCAL_LIVE_SMOKE_STATUS' 'The local live-smoke wrapper must refresh status.'
    }
    if (
        $Wrapper -notmatch 'skill-manager-live-smoke-' -or
        $Wrapper -notmatch 'live-smoke-paths\.ps1' -or
        $Wrapper -notmatch 'SetEnvironmentVariable\(''SKILL_MANAGER_HOME''' -or
        $Wrapper -notmatch '\bfinally\s*\{' -or
        $Wrapper -notmatch 'Resolve-SmokeCanonicalExistingPath\s+-Path\s+\$systemTempRoot' -or
        $Wrapper -notmatch 'Resolve-SmokeCanonicalExistingPath\s+-Path\s+\$smokeRoot' -or
        $Wrapper -notmatch 'Assert-SmokePathContained' -or
        $Wrapper -notmatch 'Remove-Item\s+-LiteralPath\s+\$canonicalSmokeRoot\s+-Recurse\s+-Force'
    ) {
        Throw-BuildContractViolation 'LOCAL_LIVE_SMOKE_ISOLATION' 'The local live-smoke wrapper must isolate and clean up its temporary home.'
    }
}
