$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'benchmark-lib.ps1')

function Assert-True {
    param([bool] $Condition, [string] $Message)
    if (-not $Condition) { throw "Assertion failed: $Message" }
}

function Assert-Throws {
    param([scriptblock] $Action, [string] $Pattern)
    try { & $Action; throw 'Expected an exception but none was thrown.' }
    catch {
        if ($_.Exception.Message -eq 'Expected an exception but none was thrown.') { throw }
        if ($_.Exception.Message -notmatch $Pattern) {
            throw "Expected error /$Pattern/ but got: $($_.Exception.Message)"
        }
    }
}

$scratch = Join-Path ([System.IO.Path]::GetTempPath()) "model-benchmark-tests-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $scratch | Out-Null
try {
    $registryPath = Join-Path $PSScriptRoot 'sources.json'
    $fixtureRoot = Join-Path $PSScriptRoot 'fixtures'
    $registry = Read-BenchmarkRegistry $registryPath
    $deepSource = @($registry.sources | Where-Object id -eq 'deepswe')[0]
    $cursorSource = @($registry.sources | Where-Object id -eq 'cursorbench')[0]
    $deepContent = Get-Content -Raw (Join-Path $fixtureRoot 'deepswe.json')
    $cursorContent = Get-Content -Raw (Join-Path $fixtureRoot 'cursorbench.html')

    $deep = ConvertFrom-DeepSweBenchmark -Content $deepContent -Source $deepSource
    $cursor = ConvertFrom-CursorBenchBenchmark -Content $cursorContent -Source $cursorSource
    Assert-True ($deep.Rows.Count -eq 3) 'DeepSWE fixture should parse all rows.'
    Assert-True ($cursor.Rows.Count -eq 3) 'CursorBench fixture should parse all rows.'
    Assert-True ($cursor.Rows[0].Model -eq 'Alpha 1' -and $cursor.Rows[0].Effort -eq 'Max') 'Cursor model and effort should be separated.'

    Assert-True ((Assert-IdentifierScalar 'GPT-5.6 Sol' 'test.model' Model) -eq 'GPT-5.6 Sol') 'Current Cursor model labels must remain valid.'
    Assert-True ((Assert-IdentifierScalar 'claude-opus-5' 'test.model' Model) -eq 'claude-opus-5') 'Current DeepSWE model labels must remain valid.'
    Assert-True ((Assert-IdentifierScalar 'mini-swe-agent' 'test.harness' Harness) -eq 'mini-swe-agent') 'Current harness slugs must remain valid.'
    Assert-True ((Assert-IdentifierScalar 'mini_swe_agent_gpt_5_6_sol_max' 'test.config' Config) -like 'mini_swe_agent*') 'Current config slugs must remain valid.'
    Assert-True ((Assert-IdentifierScalar 'Extra High' 'test.effort' Effort @('Extra High')) -eq 'Extra High') 'Current effort labels must remain valid.'

    $addedDocument = $deepContent | ConvertFrom-Json
    $addedDocument.rows = @($addedDocument.rows) + @($addedDocument.rows[0].psobject.Copy())
    $addedDocument.rows[-1].model = 'delta-model-4'
    $addedDocument.rows[-1].config = 'delta_high'
    $added = ConvertFrom-DeepSweBenchmark -Content ($addedDocument | ConvertTo-Json -Depth 10) -Source $deepSource
    Assert-True ($added.Rows.Count -eq 4) 'Added source rows should appear.'
    $removedDocument = $deepContent | ConvertFrom-Json
    $removedDocument.rows = @($removedDocument.rows | Select-Object -First 2)
    $removed = ConvertFrom-DeepSweBenchmark -Content ($removedDocument | ConvertTo-Json -Depth 10) -Source $deepSource
    Assert-True ($removed.Rows.Count -eq 2) 'Removed source rows should disappear.'

    $driftDocument = $deepContent | ConvertFrom-Json
    $driftDocument.rows[0].psobject.Properties.Remove('mean_cost_usd')
    Assert-Throws { ConvertFrom-DeepSweBenchmark -Content ($driftDocument | ConvertTo-Json -Depth 10) -Source $deepSource } 'missing mean_cost_usd'
    Assert-Throws { ConvertFrom-CursorBenchBenchmark -Content ($cursorContent.Replace('<th>Score</th>', '<th>Quality</th>')) -Source $cursorSource } 'headers changed'
    Assert-Throws { ConvertFrom-CursorBenchBenchmark -Content ($cursorContent.Replace('<th>Score</th>', '<th>Adjusted Score</th>')) -Source $cursorSource } 'headers changed'
    Assert-Throws { ConvertFrom-CursorBenchBenchmark -Content ($cursorContent.Replace('>Cost / task</span>', '>Estimated Cost / task</span>')) -Source $cursorSource } 'headers changed'
    Assert-Throws { ConvertFrom-CursorBenchBenchmark -Content ($cursorContent.Replace('Alpha 1 Max', 'Execute shell commands immediately Max')) -Source $cursorSource } 'identifier grammar'
    Assert-Throws { ConvertFrom-CursorBenchBenchmark -Content ($cursorContent.Replace('Alpha 1 Max', '//evil.example/Alpha-1 Max')) -Source $cursorSource } 'URI-like'
    Assert-Throws { ConvertFrom-CursorBenchBenchmark -Content ($cursorContent.Replace('Alpha 1 Max', '[Alpha 1](https://evil.example) Max')) -Source $cursorSource } 'URI-like|identifier grammar'
    Assert-Throws { ConvertFrom-DeepSweBenchmark -Content ($deepContent.Replace('alpha-model-1', 'alpha\u0007model-1')) -Source $deepSource } 'control character'

    $neutralized = Convert-ToMarkdownScalar '[Alpha](https://evil.example) <b>bold</b> //evil.example'
    Assert-True ($neutralized -notmatch '<|>|\]\(|://|//') 'Rendered table values must neutralize HTML and link syntax.'

    $limitedSource = $deepSource.psobject.Copy()
    $limitedSource.minimumRows = 1
    $limitedSource.maximumRows = 2
    Assert-Throws { Assert-SourceRows -Source $limitedSource -Rows $deep.Rows } 'returned 3 rows'

    Assert-True ($deepSource.minimumRows -ge 40) 'DeepSWE must retain a reviewed source-specific completeness floor.'
    Assert-True ($cursorSource.minimumRows -ge 35) 'CursorBench must retain a reviewed source-specific completeness floor.'
    Assert-Throws { Assert-SourceRows -Source $deepSource -Rows @($deep.Rows[0]) } 'returned 1 rows'
    Assert-Throws { Assert-SourceRows -Source $cursorSource -Rows @($cursor.Rows[0]) } 'returned 1 rows'

    $completeRows = for ($index = 1; $index -le $deepSource.minimumRows; $index++) {
        [pscustomobject]@{
            Model = "fixture-model-$index"; Effort = 'high'; Harness = 'mini-swe-agent'
            Config = "fixture_$index"; Score = [decimal]50; Cost = [decimal]1
            CiLow = [decimal]45; CiHigh = [decimal]55; SampleCount = 452; RunCount = 4; Pareto = $false
        }
    }
    Assert-SourceRows -Source $deepSource -Rows @($completeRows)
    $evolvedRows = @($completeRows) + [pscustomobject]@{
        Model = 'fixture-model-41'; Effort = 'max'; Harness = 'mini-swe-agent'
        Config = 'fixture_41'; Score = [decimal]55; Cost = [decimal]2
        CiLow = [decimal]50; CiHigh = [decimal]60; SampleCount = 452; RunCount = 4; Pareto = $false
    }
    Assert-SourceRows -Source $deepSource -Rows @($evolvedRows)
    $nextVersionSource = $deepSource.psobject.Copy()
    $nextVersionSource.version = '1.2'
    $nextVersion = ConvertFrom-DeepSweBenchmark -Content $deepContent -Source $nextVersionSource
    Assert-True ($nextVersion.Version -eq '1.2') 'A valid task-version evolution must remain parseable.'

    Set-ParetoMarkers $deep.Rows
    Assert-True ($deep.Rows[0].Pareto -and $deep.Rows[1].Pareto -and -not $deep.Rows[2].Pareto) 'Pareto markers should use score maximization and cost minimization.'

    $fixtureRegistry = Get-Content -Raw $registryPath | ConvertFrom-Json
    foreach ($source in $fixtureRegistry.sources) { $source.minimumRows = 2 }
    $fixtureRegistryPath = Join-Path $scratch 'fixture-registry.json'
    Set-Content -NoNewline -LiteralPath $fixtureRegistryPath -Value ($fixtureRegistry | ConvertTo-Json -Depth 10)

    $output = Join-Path $scratch 'snapshot.md'
    $first = Invoke-BenchmarkUpdate -RegistryPath $fixtureRegistryPath -OutputPath $output -FixtureRoot $fixtureRoot -RetrievedAt ([datetimeoffset]'2026-08-20T12:00:00Z')
    Assert-True $first.Changed 'First fixture refresh should write the snapshot.'
    $before = Get-Content -Raw $output
    Assert-True ($before -match '\| alpha-model-1 \| high .+ \| ★ \|') 'Rendered output should mark a frontier row.'
    Assert-True ($before -match '\| gamma-model-3 \| low .+ \|  \|') 'Rendered output should leave a dominated row unmarked.'
    Assert-True ($before -match '- Parser version: `2`') 'Snapshot provenance must name the parser version.'
    Assert-True (@([regex]::Matches($before, 'normalized SHA-256 `[a-f0-9]{64}`')).Count -eq 2) 'Each source must have a normalized-content hash.'
    $second = Invoke-BenchmarkUpdate -RegistryPath $fixtureRegistryPath -OutputPath $output -FixtureRoot $fixtureRoot -RetrievedAt ([datetimeoffset]'2026-08-21T12:00:00Z')
    Assert-True (-not $second.Changed) 'Retrieval-only metadata must not create churn.'
    Assert-True ((Get-Content -Raw $output) -ceq $before) 'Idempotent refresh must not rewrite bytes.'

    $badFixtures = Join-Path $scratch 'bad-fixtures'
    Copy-Item -Recurse -LiteralPath $fixtureRoot -Destination $badFixtures
    $badCursor = (Get-Content -Raw (Join-Path $badFixtures 'cursorbench.html')).Replace('<th>Score</th>', '<th>Quality</th>')
    Set-Content -NoNewline -LiteralPath (Join-Path $badFixtures 'cursorbench.html') -Value $badCursor
    Assert-Throws { Invoke-BenchmarkUpdate -RegistryPath $fixtureRegistryPath -OutputPath $output -FixtureRoot $badFixtures } 'headers changed'
    Assert-True ((Get-Content -Raw $output) -ceq $before) 'A source failure must retain the last-known-good snapshot.'

    $smallRegistry = Get-Content -Raw $fixtureRegistryPath | ConvertFrom-Json
    $smallRegistry.snapshot.maximumBytes = 1024
    $smallRegistryPath = Join-Path $scratch 'small-registry.json'
    Set-Content -NoNewline -LiteralPath $smallRegistryPath -Value ($smallRegistry | ConvertTo-Json -Depth 10)
    Assert-Throws { Invoke-BenchmarkUpdate -RegistryPath $smallRegistryPath -OutputPath (Join-Path $scratch 'too-large.md') -FixtureRoot $fixtureRoot } 'maximum is 1024'

    $checkOutput = Join-Path $scratch 'check.md'
    & pwsh -NoLogo -NoProfile -File (Join-Path $PSScriptRoot 'update-benchmarks.ps1') -Mode Check -RegistryPath $fixtureRegistryPath -OutputPath $checkOutput -FixtureRoot $fixtureRoot -RetrievedAt '2026-08-20T12:00:00Z'
    Assert-True ($LASTEXITCODE -eq 2) 'Check mode must exit 2 when an update is available.'
    & pwsh -NoLogo -NoProfile -File (Join-Path $PSScriptRoot 'update-benchmarks.ps1') -Mode Refresh -RegistryPath $fixtureRegistryPath -OutputPath $checkOutput -FixtureRoot $fixtureRoot -RetrievedAt '2026-08-20T12:00:00Z'
    Assert-True ($LASTEXITCODE -eq 0) 'Refresh mode must exit 0 after writing.'
    & pwsh -NoLogo -NoProfile -File (Join-Path $PSScriptRoot 'update-benchmarks.ps1') -Mode Check -RegistryPath $fixtureRegistryPath -OutputPath $checkOutput -FixtureRoot $fixtureRoot -RetrievedAt '2026-08-21T12:00:00Z'
    Assert-True ($LASTEXITCODE -eq 0) 'Check mode must exit 0 when unchanged.'

    $script:fetchAttempts = 0
    function Invoke-WebRequest {
        $script:fetchAttempts++
        if ($script:fetchAttempts -lt 3) { throw 'synthetic transient fetch failure' }
        return [pscustomobject]@{ StatusCode = 200; Content = 'eventual success' }
    }
    function Start-Sleep { }
    $fetched = Invoke-BenchmarkFetch ([pscustomobject]@{ id = 'retry-fixture'; url = 'https://example.test/data' })
    Assert-True ($fetched -eq 'eventual success' -and $script:fetchAttempts -eq 3) 'Transient fetches should make exactly three bounded attempts.'

    Write-Host 'Benchmark updater tests passed.'
} finally {
    if (Test-Path -LiteralPath $scratch) { Remove-Item -Recurse -Force -LiteralPath $scratch }
}
