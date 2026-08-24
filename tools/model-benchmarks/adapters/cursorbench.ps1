function ConvertFrom-CursorBenchBenchmark {
    param(
        [Parameter(Mandatory = $true)] [string] $Content,
        [Parameter(Mandatory = $true)] $Source
    )

    $versionMatch = [regex]::Match($Content, [string]$Source.versionPattern)
    if (-not $versionMatch.Success) { throw 'cursorbench version marker is missing or changed.' }
    $version = Assert-TrustedScalar $versionMatch.Groups[1].Value 'cursorbench.version' 30
    $dateMatch = [regex]::Match($Content, 'cursorbench-changelog-([0-9]{4}-[0-9]{2}-[0-9]{2})')
    if (-not $dateMatch.Success) { throw 'cursorbench source-update timestamp is missing or changed.' }
    try { [void][datetime]::ParseExact($dateMatch.Groups[1].Value, 'yyyy-MM-dd', $script:InvariantCulture) }
    catch { throw 'cursorbench source-update timestamp is invalid.' }

    $renderedContent = [regex]::Replace($Content, '<script\b[^>]*>.*?</script>', '', 'Singleline')
    $tables = [regex]::Matches($renderedContent, '<table\b.*?</table>', 'Singleline')
    $uniqueTables = @($tables | ForEach-Object Value | Sort-Object -Unique)
    if ($uniqueTables.Count -ne 1) { throw "cursorbench expected one unique rendered table; found $($uniqueTables.Count)." }
    $htmlRows = [regex]::Matches($uniqueTables[0], '<tr\b.*?</tr>', 'Singleline')
    if ($htmlRows.Count -lt 2) { throw 'cursorbench rendered table has no data rows.' }
    $headers = @([regex]::Matches($htmlRows[0].Value, '<th\b[^>]*>(.*?)</th>', 'Singleline') |
        ForEach-Object { Get-HtmlCellText $_.Groups[1].Value })
    $headerText = $headers -join '|'
    if ($headerText -notmatch 'Model' -or $headerText -notmatch 'Score' -or $headerText -notmatch 'Cost / task') {
        throw 'cursorbench rendered table headers changed.'
    }

    $effortLabels = @($Source.effortLabels | Sort-Object Length -Descending)
    $rows = for ($index = 1; $index -lt $htmlRows.Count; $index++) {
        $cells = @([regex]::Matches($htmlRows[$index].Value, '<td\b[^>]*>(.*?)</td>', 'Singleline') |
            ForEach-Object { Get-HtmlCellText $_.Groups[1].Value })
        if ($cells.Count -ne 6) { throw "cursorbench row $index has $($cells.Count) cells; expected 6." }
        $combinedModel = Assert-TrustedScalar $cells[1] "cursorbench.row[$index].model" 120
        $effort = 'default'
        $model = $combinedModel
        foreach ($label in $effortLabels) {
            $suffix = " $label"
            if ($combinedModel.EndsWith($suffix, [StringComparison]::Ordinal)) {
                $model = $combinedModel.Substring(0, $combinedModel.Length - $suffix.Length)
                $effort = [string]$label
                break
            }
        }
        $model = Assert-TrustedScalar $model "cursorbench.row[$index].model" 100
        $effort = Assert-TrustedScalar $effort "cursorbench.row[$index].effort" 40
        if ($cells[2] -notmatch '^([0-9]+(?:\.[0-9]+)?)\s*%$') { throw "cursorbench row $index score changed format." }
        $score = Convert-ToBoundedDecimal $Matches[1] "cursorbench.row[$index].score" 0 100
        if ($cells[3] -notmatch '^\$\s*([0-9]+(?:\.[0-9]+)?)$') { throw "cursorbench row $index cost changed format." }
        $cost = Convert-ToBoundedDecimal $Matches[1] "cursorbench.row[$index].cost" 0 10000
        [pscustomobject]@{
            Model = $model
            Effort = $effort
            Harness = Assert-TrustedScalar ([string]$Source.harness) 'cursorbench.harness' 100
            Config = Assert-TrustedScalar ([string]$Source.config) 'cursorbench.config' 150
            Score = $score
            Cost = $cost
            CiLow = $null
            CiHigh = $null
            SampleCount = $null
            RunCount = $null
            Pareto = $false
        }
    }
    return [pscustomobject]@{
        Id = 'cursorbench'
        DisplayName = 'CursorBench'
        Version = $version
        PublishedAt = $dateMatch.Groups[1].Value
        TaskCount = $null
        ScoreLabel = Assert-TrustedScalar ([string]$Source.scoreLabel) 'cursorbench.scoreLabel' 30
        Rows = @($rows)
    }
}
