Set-StrictMode -Version Latest

$script:InvariantCulture = [System.Globalization.CultureInfo]::InvariantCulture
$script:BenchmarkParserVersion = 3

function Assert-TrustedScalar {
    param(
        [AllowEmptyString()] [string] $Value,
        [Parameter(Mandatory = $true)] [string] $Field,
        [int] $MaximumLength = 160,
        [switch] $AllowEmpty
    )

    if (-not $AllowEmpty -and [string]::IsNullOrWhiteSpace($Value)) {
        throw "$Field must not be empty."
    }
    if ($Value.Length -gt $MaximumLength) { throw "$Field exceeds $MaximumLength characters." }
    if ($Value -match '[\x00-\x1f\x7f]') { throw "$Field contains a control character." }
    return $Value
}

function Assert-IdentifierScalar {
    param(
        [Parameter(Mandatory = $true)] [string] $Value,
        [Parameter(Mandatory = $true)] [string] $Field,
        [Parameter(Mandatory = $true)]
        [ValidateSet('Effort', 'Harness', 'Config', 'Version')]
        [string] $Kind,
        [string[]] $AllowedValues
    )

    $limits = @{ Effort = 24; Harness = 64; Config = 150; Version = 30 }
    Assert-TrustedScalar $Value $Field $limits[$Kind] | Out-Null
    if ($Value -ne $Value.Trim()) { throw "$Field has leading or trailing whitespace." }
    if ($Value -match '(?i)(?:[a-z][a-z0-9+.-]*:)?//') { throw "$Field contains a URI-like value." }

    if ($AllowedValues -and $Value -cnotin $AllowedValues) {
        throw "$Field is not an allowlisted $($Kind.ToLowerInvariant()) value."
    }

    $valid = switch ($Kind) {
        'Effort' { $Value -match '^[A-Za-z][A-Za-z0-9]*(?:[ -][A-Za-z0-9]+)?$' }
        'Harness' { $Value -match '^[A-Za-z0-9][A-Za-z0-9._-]*$' }
        'Config' { $Value -match '^[A-Za-z0-9][A-Za-z0-9._+:/-]*$' }
        'Version' { $Value -match '^[0-9]+(?:\.[0-9]+){1,3}$' }
    }
    if (-not $valid) { throw "$Field does not match the $($Kind.ToLowerInvariant()) identifier grammar." }
    return $Value
}

function Assert-SourceModelScalar {
    param(
        [Parameter(Mandatory = $true)] [string] $Value,
        [Parameter(Mandatory = $true)] [string] $Field,
        [Parameter(Mandatory = $true)] $Source
    )

    Assert-TrustedScalar $Value $Field 100 | Out-Null
    if ($Value -ne $Value.Trim()) { throw "$Field has leading or trailing whitespace." }
    if ($Value -match '(?i)(?:[a-z][a-z0-9+.-]*:)?//') { throw "$Field contains a URI-like value." }

    foreach ($pattern in @($Source.modelPatterns)) {
        if ([regex]::IsMatch(
                $Value,
                [string]$pattern,
                [System.Text.RegularExpressions.RegexOptions]::CultureInvariant,
                [timespan]::FromMilliseconds(100))) {
            return $Value
        }
    }
    throw "$Field is not an allowlisted model family for source $($Source.id)."
}

function Convert-ToBoundedDecimal {
    param(
        [Parameter(Mandatory = $true)] $Value,
        [Parameter(Mandatory = $true)] [string] $Field,
        [decimal] $Minimum,
        [decimal] $Maximum
    )

    try { $number = [Convert]::ToDecimal($Value, $script:InvariantCulture) }
    catch { throw "$Field must be numeric." }
    if ($number -lt $Minimum -or $number -gt $Maximum) {
        throw "$Field must be between $Minimum and $Maximum."
    }
    return $number
}

function Convert-ToBoundedInteger {
    param(
        [Parameter(Mandatory = $true)] $Value,
        [Parameter(Mandatory = $true)] [string] $Field,
        [int] $Minimum,
        [int] $Maximum
    )

    try { $number = [Convert]::ToInt32($Value, $script:InvariantCulture) }
    catch { throw "$Field must be an integer." }
    if ($number -lt $Minimum -or $number -gt $Maximum) {
        throw "$Field must be between $Minimum and $Maximum."
    }
    return $number
}

function Convert-ToMarkdownScalar {
    param([Parameter(Mandatory = $true)] [string] $Value)
    # Encode HTML and URI punctuation before escaping Markdown syntax. Even
    # registry-authored values therefore remain inert if rendered as table data.
    $escaped = $Value.Replace('&', '&amp;').Replace('<', '&lt;').Replace('>', '&gt;')
    $escaped = $escaped.Replace('"', '&quot;').Replace("'", '&#39;')
    $escaped = $escaped.Replace(':', '&#58;').Replace('/', '&#47;')
    foreach ($character in @('\', '|', '`', '*', '_', '[', ']', '(', ')', '!')) {
        $escaped = $escaped.Replace($character, "\$character")
    }
    return $escaped
}

function Get-Sha256Hex {
    param([Parameter(Mandatory = $true)] [string] $Value)
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Value)
    return [Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
}

function Get-HtmlCellText {
    param([Parameter(Mandatory = $true)] [AllowEmptyString()] [string] $Html)
    $withoutComments = [regex]::Replace($Html, '<!--.*?-->', '', 'Singleline')
    $withoutTags = [regex]::Replace($withoutComments, '<[^>]+>', ' ')
    $decoded = [System.Net.WebUtility]::HtmlDecode($withoutTags)
    return [regex]::Replace($decoded, '\s+', ' ').Trim()
}

function Assert-SourceRows {
    param(
        [Parameter(Mandatory = $true)] $Source,
        [Parameter(Mandatory = $true)] [object[]] $Rows
    )

    $minimumRows = Convert-ToBoundedInteger $Source.minimumRows "$($Source.id).minimumRows" 1 10000
    $maximumRows = Convert-ToBoundedInteger $Source.maximumRows "$($Source.id).maximumRows" $minimumRows 10000
    if ($Rows.Count -lt $minimumRows -or $Rows.Count -gt $maximumRows) {
        throw "$($Source.id) returned $($Rows.Count) rows; expected $minimumRows..$maximumRows."
    }

    $seen = @{}
    foreach ($row in $Rows) {
        $key = "$($row.Model)`u{001f}$($row.Effort)`u{001f}$($row.Harness)`u{001f}$($row.Config)"
        if ($seen.ContainsKey($key)) { throw "$($Source.id) returned a duplicate model/effort/config row." }
        $seen[$key] = $true
    }
}

. (Join-Path $PSScriptRoot 'adapters/deepswe.ps1')
. (Join-Path $PSScriptRoot 'adapters/cursorbench.ps1')

function Read-BenchmarkRegistry {
    param([Parameter(Mandatory = $true)] [string] $Path)

    try { $registry = Get-Content -Raw -LiteralPath $Path | ConvertFrom-Json }
    catch { throw "Benchmark registry is invalid JSON: $($_.Exception.Message)" }
    if ($registry.schemaVersion -ne 1) { throw 'Benchmark registry schemaVersion must be 1.' }
    if (-not $registry.sources) { throw 'Benchmark registry has no sources.' }

    $allowedAdapters = @('deepswe-json', 'cursorbench-html')
    $seen = @{}
    foreach ($source in @($registry.sources)) {
        $id = Assert-TrustedScalar ([string]$source.id) 'source.id' 40
        if ($id -notmatch '^[a-z0-9]+(?:-[a-z0-9]+)*$') { throw "Invalid source id: $id" }
        if ($seen.ContainsKey($id)) { throw "Duplicate source id: $id" }
        $seen[$id] = $true
        if ($source.adapter -notin $allowedAdapters) { throw "Source $id uses a non-allowlisted adapter." }
        foreach ($field in @('url', 'canonicalUrl')) {
            $uri = [Uri]$source.$field
            if ($uri.Scheme -ne 'https') { throw "Source $id $field must be HTTPS." }
        }
        Assert-TrustedScalar ([string]$source.scope) "$id.scope" 180 | Out-Null
        Assert-TrustedScalar ([string]$source.caveat) "$id.caveat" 220 | Out-Null

        $modelPatterns = @($source.modelPatterns)
        if (-not $modelPatterns.Count -or $modelPatterns.Count -gt 20) {
            throw "Source $id must define 1..20 modelPatterns."
        }
        foreach ($pattern in $modelPatterns) {
            $pattern = Assert-TrustedScalar ([string]$pattern) "$id.modelPatterns" 200
            if (-not $pattern.StartsWith('^', [StringComparison]::Ordinal) -or
                -not $pattern.EndsWith('$', [StringComparison]::Ordinal)) {
                throw "Source $id modelPatterns must be anchored."
            }
            try {
                [void][regex]::new(
                    $pattern,
                    [System.Text.RegularExpressions.RegexOptions]::CultureInvariant,
                    [timespan]::FromMilliseconds(100))
            } catch {
                throw "Source $id has an invalid model pattern: $($_.Exception.Message)"
            }
        }

        if ($source.adapter -eq 'deepswe-json') {
            Assert-IdentifierScalar ([string]$source.harness) "$id.harness" Harness | Out-Null
            $template = Assert-TrustedScalar ([string]$source.configTemplate) "$id.configTemplate" 100
            if ($template -cne 'mini_swe_agent_{model}_{effort}') {
                throw "Source $id configTemplate is not the reviewed DeepSWE structure."
            }
        } elseif ($source.adapter -eq 'cursorbench-html') {
            Assert-TrustedScalar ([string]$source.harness) "$id.harness" 100 | Out-Null
            Assert-TrustedScalar ([string]$source.config) "$id.config" 150 | Out-Null
        }
    }
    return $registry
}

function Invoke-BenchmarkFetch {
    param([Parameter(Mandatory = $true)] $Source)

    $lastError = $null
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        try {
            $response = Invoke-WebRequest -UseBasicParsing -Uri $Source.url -TimeoutSec 30
            if ($response.StatusCode -ne 200) { throw "HTTP $($response.StatusCode)" }
            return [string]$response.Content
        } catch {
            $lastError = $_
            $statusCode = $null
            $responseProperty = $_.Exception.psobject.Properties['Response']
            if ($responseProperty -and $responseProperty.Value -and $responseProperty.Value.StatusCode) {
                $statusCode = [int]$responseProperty.Value.StatusCode
            }
            if ($statusCode -ge 400 -and $statusCode -lt 500 -and $statusCode -notin @(408, 429)) {
                throw "$($Source.id) fetch failed with non-retryable HTTP $statusCode."
            }
            if ($attempt -lt 3) {
                Write-Warning "$($Source.id) fetch attempt $attempt failed; retrying."
                Start-Sleep -Seconds ([Math]::Pow(2, $attempt - 1))
            }
        }
    }
    throw "$($Source.id) fetch failed after 3 attempts: $($lastError.Exception.Message)"
}

function ConvertFrom-BenchmarkSource {
    param(
        [Parameter(Mandatory = $true)] $Source,
        [Parameter(Mandatory = $true)] [string] $Content
    )

    switch ($Source.adapter) {
        'deepswe-json' { return ConvertFrom-DeepSweBenchmark -Content $Content -Source $Source }
        'cursorbench-html' { return ConvertFrom-CursorBenchBenchmark -Content $Content -Source $Source }
        default { throw "Unsupported adapter: $($Source.adapter)" }
    }
}

function Set-ParetoMarkers {
    param([Parameter(Mandatory = $true)] [object[]] $Rows)
    foreach ($row in $Rows) {
        $dominated = $false
        foreach ($candidate in $Rows) {
            if ([object]::ReferenceEquals($row, $candidate)) { continue }
            if ($candidate.Cost -le $row.Cost -and $candidate.Score -ge $row.Score -and
                ($candidate.Cost -lt $row.Cost -or $candidate.Score -gt $row.Score)) {
                $dominated = $true
                break
            }
        }
        $row.Pareto = -not $dominated
    }
}

function Get-SemanticBenchmarkText {
    param([Parameter(Mandatory = $true)] $Benchmark)
    $lines = @("$($Benchmark.Id)|$($Benchmark.Version)|$($Benchmark.PublishedAt)|$($Benchmark.ScoreLabel)|$($Benchmark.TaskCount)")
    foreach ($row in $Benchmark.Rows) {
        $lines += @($row.Model, $row.Effort, $row.Harness, $row.Config,
            $row.Score.ToString('0.################', $script:InvariantCulture),
            $row.Cost.ToString('0.################', $script:InvariantCulture),
            $row.CiLow, $row.CiHigh, $row.SampleCount, $row.RunCount, $row.Pareto) -join '|'
    }
    return $lines -join "`n"
}

function Format-OptionalInteger {
    param($Value)
    if ($null -eq $Value) { return $null }
    return ([int]$Value).ToString('N0', $script:InvariantCulture)
}

function Format-BenchmarkUncertainty {
    param([Parameter(Mandatory = $true)] $Row)
    $parts = @()
    if ($null -ne $Row.CiLow -and $null -ne $Row.CiHigh) {
        $parts += "95% CI $($Row.CiLow.ToString('0.00', $script:InvariantCulture))–$($Row.CiHigh.ToString('0.00', $script:InvariantCulture))%"
    }
    if ($null -ne $Row.SampleCount) { $parts += "n=$(Format-OptionalInteger $Row.SampleCount)" }
    if ($null -ne $Row.RunCount) { $parts += "runs=$(Format-OptionalInteger $Row.RunCount)" }
    if (-not $parts.Count) { return '—' }
    return $parts -join '; '
}

function New-BenchmarkSnapshot {
    param(
        [Parameter(Mandatory = $true)] $Registry,
        [Parameter(Mandatory = $true)] [object[]] $Benchmarks,
        [Parameter(Mandatory = $true)] [datetimeoffset] $RetrievedAt
    )

    $sourceHashes = @{}
    $sourceTexts = foreach ($benchmark in $Benchmarks) {
        $sourceText = Get-SemanticBenchmarkText $benchmark
        $sourceHashes[$benchmark.Id] = Get-Sha256Hex $sourceText
        $sourceText
    }
    $semanticHash = Get-Sha256Hex ("parser=$script:BenchmarkParserVersion`n" + ($sourceTexts -join "`n"))
    $builder = [System.Text.StringBuilder]::new()
    [void]$builder.AppendLine('# Model benchmark snapshot')
    [void]$builder.AppendLine()
    [void]$builder.AppendLine('Generated supporting evidence for maestro model/effort selection. Compare only')
    [void]$builder.AppendLine('within a source and version; task-specific judgment and the current roster remain')
    [void]$builder.AppendLine('authoritative. `★` marks the point-estimate cost/performance Pareto frontier.')
    [void]$builder.AppendLine()
    [void]$builder.AppendLine("- Retrieved after semantic change: ``$($RetrievedAt.ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'))``")
    [void]$builder.AppendLine("- Parser version: ``$script:BenchmarkParserVersion``")
    [void]$builder.AppendLine("- Normalized SHA-256: ``$semanticHash``")
    [void]$builder.AppendLine('- Scores and costs are source-reported; no composite or cross-source ranking is calculated.')

    foreach ($benchmark in $Benchmarks) {
        $source = @($Registry.sources | Where-Object id -eq $benchmark.Id)[0]
        [void]$builder.AppendLine()
        [void]$builder.AppendLine("## $($benchmark.DisplayName)")
        [void]$builder.AppendLine()
        $published = if ($benchmark.PublishedAt) { " · source updated ``$($benchmark.PublishedAt)``" } else { '' }
        $tasks = if ($benchmark.TaskCount) { " · tasks ``$($benchmark.TaskCount)``" } else { '' }
        [void]$builder.AppendLine("Source: [$($benchmark.DisplayName)]($($source.canonicalUrl)) · version ``$($benchmark.Version)``$published$tasks · normalized SHA-256 ``$($sourceHashes[$benchmark.Id])``")
        [void]$builder.AppendLine()
        [void]$builder.AppendLine("Metric: ``$($benchmark.ScoreLabel)`` · $($source.scope) $($source.caveat)")
        [void]$builder.AppendLine()
        [void]$builder.AppendLine('| model | effort | harness / config | score | avg cost/task | uncertainty / sample | Pareto |')
        [void]$builder.AppendLine('| --- | --- | --- | ---: | ---: | --- | :---: |')
        foreach ($row in $benchmark.Rows) {
            $model = Convert-ToMarkdownScalar $row.Model
            $effort = Convert-ToMarkdownScalar $row.Effort
            $harnessConfig = Convert-ToMarkdownScalar "$($row.Harness) / $($row.Config)"
            $score = $row.Score.ToString('0.00', $script:InvariantCulture) + '%'
            $cost = '$' + $row.Cost.ToString('0.000', $script:InvariantCulture)
            $uncertainty = Format-BenchmarkUncertainty $row
            $pareto = if ($row.Pareto) { '★' } else { '' }
            [void]$builder.AppendLine("| $model | $effort | $harnessConfig | $score | $cost | $uncertainty | $pareto |")
        }
    }
    $content = $builder.ToString().Replace("`r`n", "`n")
    return [pscustomobject]@{ Content = $content; SemanticHash = $semanticHash }
}

function Invoke-BenchmarkUpdate {
    param(
        [Parameter(Mandatory = $true)] [string] $RegistryPath,
        [Parameter(Mandatory = $true)] [string] $OutputPath,
        [string] $FixtureRoot,
        [datetimeoffset] $RetrievedAt = [datetimeoffset]::UtcNow,
        [switch] $Check
    )

    $registry = Read-BenchmarkRegistry $RegistryPath
    $enabledSources = @($registry.sources | Where-Object enabled)
    if (-not $enabledSources.Count) { throw 'Benchmark registry has no enabled sources.' }
    Write-Host "Benchmark refresh plan: $($enabledSources.Count) allowlisted sources -> $OutputPath"

    $benchmarks = @()
    foreach ($source in $enabledSources) {
        Write-Host "Fetching $($source.id) from $($source.url)"
        if ($FixtureRoot) {
            $extension = if ($source.adapter -eq 'deepswe-json') { 'json' } else { 'html' }
            $fixturePath = Join-Path $FixtureRoot "$($source.id).$extension"
            if (-not (Test-Path -LiteralPath $fixturePath -PathType Leaf)) { throw "Missing fixture: $fixturePath" }
            $content = Get-Content -Raw -LiteralPath $fixturePath
        } else {
            $content = Invoke-BenchmarkFetch $source
        }
        $benchmark = ConvertFrom-BenchmarkSource -Source $source -Content $content
        Assert-SourceRows -Source $source -Rows $benchmark.Rows
        Set-ParetoMarkers $benchmark.Rows
        $benchmark.Rows = @($benchmark.Rows | Sort-Object @{Expression='Score';Descending=$true}, @{Expression='Cost';Descending=$false}, Model, Effort)
        $benchmarks += $benchmark
        Write-Host "Validated $($benchmark.Rows.Count) $($source.id) rows."
    }

    $totalRows = ($benchmarks | ForEach-Object { $_.Rows.Count } | Measure-Object -Sum).Sum
    $maximumRows = Convert-ToBoundedInteger $registry.snapshot.maximumRows 'snapshot.maximumRows' 1 10000
    if ($totalRows -gt $maximumRows) { throw "Snapshot has $totalRows rows; maximum is $maximumRows." }

    $snapshot = New-BenchmarkSnapshot -Registry $registry -Benchmarks $benchmarks -RetrievedAt $RetrievedAt
    $bytes = [System.Text.Encoding]::UTF8.GetByteCount($snapshot.Content)
    $maximumBytes = Convert-ToBoundedInteger $registry.snapshot.maximumBytes 'snapshot.maximumBytes' 1024 1048576
    if ($bytes -gt $maximumBytes) { throw "Snapshot is $bytes bytes; maximum is $maximumBytes." }

    $existingHash = $null
    if (Test-Path -LiteralPath $OutputPath -PathType Leaf) {
        $existing = Get-Content -Raw -LiteralPath $OutputPath
        $match = [regex]::Match($existing, 'Normalized SHA-256: `([a-f0-9]{64})`')
        if ($match.Success) { $existingHash = $match.Groups[1].Value }
    }
    if ($existingHash -eq $snapshot.SemanticHash) {
        Write-Host "Unchanged: $totalRows rows; no file written."
        return [pscustomobject]@{ Changed = $false; Rows = $totalRows; Bytes = $bytes; Hash = $snapshot.SemanticHash }
    }

    if ($Check) {
        Write-Host "Update available: $totalRows rows, $bytes bytes."
        return [pscustomobject]@{ Changed = $true; Rows = $totalRows; Bytes = $bytes; Hash = $snapshot.SemanticHash }
    }

    $outputDirectory = Split-Path -Parent $OutputPath
    if (-not (Test-Path -LiteralPath $outputDirectory -PathType Container)) {
        New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
    }
    $temporaryPath = Join-Path $outputDirectory ".benchmark-snapshot.$([Guid]::NewGuid().ToString('N')).tmp"
    try {
        [System.IO.File]::WriteAllText($temporaryPath, $snapshot.Content, [System.Text.UTF8Encoding]::new($false))
        Move-Item -LiteralPath $temporaryPath -Destination $OutputPath -Force
    } finally {
        if (Test-Path -LiteralPath $temporaryPath) { Remove-Item -LiteralPath $temporaryPath -Force }
    }
    Write-Host "Updated: $totalRows rows, $bytes bytes -> $OutputPath"
    return [pscustomobject]@{ Changed = $true; Rows = $totalRows; Bytes = $bytes; Hash = $snapshot.SemanticHash }
}
