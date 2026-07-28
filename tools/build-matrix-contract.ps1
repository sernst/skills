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

function ConvertTo-WorkflowStep {
    param(
        [Parameter(Mandatory = $true)] [AllowEmptyString()] [string[]] $Lines,
        [Parameter(Mandatory = $true)] [int] $Start,
        [Parameter(Mandatory = $true)] [int] $End,
        [Parameter(Mandatory = $true)] [int] $Indent
    )
    $stepLines = @($Lines[$Start..($End - 1)])
    [pscustomobject]@{
        Run = Get-StepRunText -Lines $stepLines -StepIndent $Indent
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
        foreach ($match in [regex]::Matches($Steps[$stepIndex].Run, $Pattern, [Text.RegularExpressions.RegexOptions]::IgnoreCase)) {
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
