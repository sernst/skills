function ConvertFrom-DeepSweBenchmark {
    param(
        [Parameter(Mandatory = $true)] [string] $Content,
        [Parameter(Mandatory = $true)] $Source
    )

    try { $document = $Content | ConvertFrom-Json }
    catch { throw "deepswe JSON is invalid: $($_.Exception.Message)" }
    foreach ($field in @('generated_at', 'n_tasks_in_set', 'rows')) {
        if ($field -notin $document.psobject.Properties.Name) { throw "deepswe schema is missing $field." }
    }
    try { $publishedAt = [datetimeoffset]::Parse([string]$document.generated_at, $script:InvariantCulture) }
    catch { throw 'deepswe generated_at is not an ISO timestamp.' }
    $taskCount = Convert-ToBoundedInteger $document.n_tasks_in_set 'deepswe.n_tasks_in_set' 1 100000

    $rows = foreach ($inputRow in @($document.rows)) {
        foreach ($field in @('model', 'harness', 'reasoning_effort', 'config', $Source.scoreField,
                $Source.costField, 'ci_lo', 'ci_hi', 'n_attempted', 'n_runs')) {
            if ($field -notin $inputRow.psobject.Properties.Name) { throw "deepswe row is missing $field." }
        }
        $model = Assert-IdentifierScalar ([string]$inputRow.model) 'deepswe.model' Model
        $harness = Assert-IdentifierScalar ([string]$inputRow.harness) 'deepswe.harness' Harness
        $effortValue = [string]$inputRow.reasoning_effort
        if ([string]::IsNullOrWhiteSpace($effortValue)) { $effortValue = 'default' }
        $effort = Assert-IdentifierScalar $effortValue 'deepswe.reasoning_effort' Effort @($Source.effortLabels)
        $config = Assert-IdentifierScalar ([string]$inputRow.config) 'deepswe.config' Config
        $scoreRatio = Convert-ToBoundedDecimal $inputRow.($Source.scoreField) "deepswe.$($Source.scoreField)" 0 1
        $cost = Convert-ToBoundedDecimal $inputRow.($Source.costField) "deepswe.$($Source.costField)" 0 10000
        $ciLow = Convert-ToBoundedDecimal $inputRow.ci_lo 'deepswe.ci_lo' 0 1
        $ciHigh = Convert-ToBoundedDecimal $inputRow.ci_hi 'deepswe.ci_hi' 0 1
        if ($ciLow -gt $scoreRatio -or $ciHigh -lt $scoreRatio -or $ciLow -gt $ciHigh) {
            throw 'deepswe confidence interval does not contain the score.'
        }
        [pscustomobject]@{
            Model = $model
            Effort = $effort
            Harness = $harness
            Config = $config
            Score = $scoreRatio * 100
            Cost = $cost
            CiLow = $ciLow * 100
            CiHigh = $ciHigh * 100
            SampleCount = Convert-ToBoundedInteger $inputRow.n_attempted 'deepswe.n_attempted' 1 1000000
            RunCount = Convert-ToBoundedInteger $inputRow.n_runs 'deepswe.n_runs' 1 10000
            Pareto = $false
        }
    }
    return [pscustomobject]@{
        Id = 'deepswe'
        DisplayName = 'DeepSWE'
        Version = Assert-IdentifierScalar ([string]$Source.version) 'deepswe.version' Version
        PublishedAt = $publishedAt.ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        TaskCount = $taskCount
        ScoreLabel = Assert-TrustedScalar ([string]$Source.scoreLabel) 'deepswe.scoreLabel' 30
        Rows = @($rows)
    }
}
