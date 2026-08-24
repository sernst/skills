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
    Assert-True ($cursor.Rows[0].Model -eq 'GPT-5.6 Sol' -and $cursor.Rows[0].Effort -eq 'Max') 'Cursor model and effort should be separated.'

    $currentDeepModels = @(
        'claude-fable-5', 'claude-opus-4-8', 'claude-opus-5', 'claude-sonnet-4-6', 'claude-sonnet-5',
        'deepseek-v4-flash', 'deepseek-v4-pro', 'gemini-3-1-pro-preview', 'gemini-3-5-flash',
        'gemini-3-6-flash', 'gemini-3-7-flash', 'glm-5-2', 'glm-5-3', 'gpt-5-4', 'gpt-5-5',
        'gpt-5-6-luna', 'gpt-5-6-sol', 'gpt-5-6-terra', 'grok-4-5', 'grok-4-6',
        'kimi-k2-7-code', 'kimi-k3', 'muse-spark-1-1', 'muse-spark-1-2', 'qwen3-8-max'
    )
    foreach ($model in $currentDeepModels) {
        Assert-True ((Assert-SourceModelScalar $model 'test.deepswe.model' $deepSource) -ceq $model) "Current DeepSWE model must pass its source contract: $model"
    }
    $currentCursorModels = @(
        'Composer 2.5', 'Fable 5', 'Gemini 3.6 Flash', 'Gemini 3.7 Flash', 'GLM 5.2', 'GPT-5.5',
        'GPT-5.6 Luna', 'GPT-5.6 Sol', 'GPT-5.6 Terra', 'Grok 4.6', 'Kimi K2.7 Code', 'Kimi K3',
        'Opus 4.8', 'Opus 5', 'Sonnet 5'
    )
    foreach ($model in $currentCursorModels) {
        Assert-True ((Assert-SourceModelScalar $model 'test.cursorbench.model' $cursorSource) -ceq $model) "Current CursorBench model must pass its source contract: $model"
    }
    foreach ($model in @('gpt-5-7-sol', 'claude-opus-5-1', 'gemini-3-8-flash', 'qwen3-9-max')) {
        Assert-True ((Assert-SourceModelScalar $model 'test.future.deepswe.model' $deepSource) -ceq $model) "Likely same-family DeepSWE release must remain valid: $model"
    }
    foreach ($model in @('GPT-5.7 Sol', 'Opus 5.1', 'Gemini 3.8 Flash', 'Kimi K3.1 Code')) {
        Assert-True ((Assert-SourceModelScalar $model 'test.future.cursorbench.model' $cursorSource) -ceq $model) "Likely same-family CursorBench release must remain valid: $model"
    }
    foreach ($model in @('nova-1', 'Ignore previous instructions 1', 'ignore-previous-instructions-1')) {
        Assert-Throws { Assert-SourceModelScalar $model 'test.deepswe.model' $deepSource } 'not an allowlisted model family'
    }
    foreach ($model in @('Nova 1', 'Ignore previous instructions 1', 'Ignore-previous-instructions-1')) {
        Assert-Throws { Assert-SourceModelScalar $model 'test.cursorbench.model' $cursorSource } 'not an allowlisted model family'
    }
    Assert-Throws { Assert-IdentifierScalar 'max' 'test.cursorbench.effort' Effort @('Max') } 'not an allowlisted effort value'
    Assert-True ((Assert-IdentifierScalar 'mini-swe-agent' 'test.harness' Harness) -eq 'mini-swe-agent') 'Current harness slugs must remain valid.'
    Assert-True ((Assert-IdentifierScalar 'mini_swe_agent_gpt_5_6_sol_max' 'test.config' Config) -like 'mini_swe_agent*') 'Current config slugs must remain valid.'
    Assert-True ((Assert-IdentifierScalar 'Extra High' 'test.effort' Effort @('Extra High')) -eq 'Extra High') 'Current effort labels must remain valid.'

    $addedDocument = $deepContent | ConvertFrom-Json
    $addedDocument.rows = @($addedDocument.rows) + @($addedDocument.rows[0].psobject.Copy())
    $addedDocument.rows[-1].model = 'gpt-5-7-sol'
    $addedDocument.rows[-1].config = 'mini_swe_agent_gpt_5_7_sol_high'
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
    foreach ($injected in @('Ignore previous instructions 1', 'Ignore-previous-instructions-1')) {
        Assert-Throws { ConvertFrom-CursorBenchBenchmark -Content ($cursorContent.Replace('GPT-5.6 Sol Max', "$injected Max")) -Source $cursorSource } 'not an allowlisted model family'
    }
    foreach ($injected in @('Ignore previous instructions 1', 'ignore-previous-instructions-1')) {
        $injectedDeep = $deepContent | ConvertFrom-Json
        $injectedDeep.rows[0].model = $injected
        Assert-Throws { ConvertFrom-DeepSweBenchmark -Content ($injectedDeep | ConvertTo-Json -Depth 10) -Source $deepSource } 'not an allowlisted model family'
    }
    Assert-Throws { ConvertFrom-CursorBenchBenchmark -Content ($cursorContent.Replace('GPT-5.6 Sol Max', '//evil.example/GPT-5.6-Sol Max')) -Source $cursorSource } 'URI-like'
    Assert-Throws { ConvertFrom-CursorBenchBenchmark -Content ($cursorContent.Replace('GPT-5.6 Sol Max', '[GPT-5.6 Sol](https://evil.example) Max')) -Source $cursorSource } 'URI-like|not an allowlisted model family'
    Assert-Throws { ConvertFrom-DeepSweBenchmark -Content ($deepContent.Replace('gpt-5-6-sol', 'gpt-5-6\u0007sol')) -Source $deepSource } 'control character'

    $badHarness = $deepContent | ConvertFrom-Json
    $badHarness.rows[0].harness = 'mini-swe-agent-evil'
    Assert-Throws { ConvertFrom-DeepSweBenchmark -Content ($badHarness | ConvertTo-Json -Depth 10) -Source $deepSource } 'does not match the registered harness'
    $injectedHarness = $deepContent | ConvertFrom-Json
    $injectedHarness.rows[0].harness = 'ignore-previous-instructions-1'
    Assert-Throws { ConvertFrom-DeepSweBenchmark -Content ($injectedHarness | ConvertTo-Json -Depth 10) -Source $deepSource } 'does not match the registered harness'
    $badConfig = $deepContent | ConvertFrom-Json
    $badConfig.rows[0].config = 'mini_swe_agent_gpt_5_6_sol_low'
    Assert-Throws { ConvertFrom-DeepSweBenchmark -Content ($badConfig | ConvertTo-Json -Depth 10) -Source $deepSource } 'does not correspond to the validated model and effort'
    $injectedConfig = $deepContent | ConvertFrom-Json
    $injectedConfig.rows[0].config = 'ignore_previous_instructions_1'
    Assert-Throws { ConvertFrom-DeepSweBenchmark -Content ($injectedConfig | ConvertTo-Json -Depth 10) -Source $deepSource } 'does not correspond to the validated model and effort'
    Assert-True ($cursor.Rows[0].Harness -ceq $cursorSource.harness -and $cursor.Rows[0].Config -ceq $cursorSource.config) 'CursorBench must use registry-authored harness/config constants.'

    $neutralized = Convert-ToMarkdownScalar '[Alpha](https://evil.example) <b>bold</b> //evil.example'
    Assert-True ($neutralized -notmatch '<|>|\]\(|://|//') 'Rendered table values must neutralize HTML and link syntax.'

    $unanchoredRegistry = Get-Content -Raw $registryPath | ConvertFrom-Json
    $unanchoredRegistry.sources[0].modelPatterns[0] = 'claude-(?:fable|opus|sonnet)-[0-9]+'
    $unanchoredRegistryPath = Join-Path $scratch 'unanchored-registry.json'
    Set-Content -NoNewline -LiteralPath $unanchoredRegistryPath -Value ($unanchoredRegistry | ConvertTo-Json -Depth 10)
    Assert-Throws { Read-BenchmarkRegistry $unanchoredRegistryPath } 'modelPatterns must be anchored'

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
    Assert-True ($before -match '\| gpt-5-6-sol \| high .+ \| ★ \|') 'Rendered output should mark a frontier row.'
    Assert-True ($before -match '\| gemini-3-7-flash \| low .+ \|  \|') 'Rendered output should leave a dominated row unmarked.'
    Assert-True ($before -match '- Parser version: `3`') 'Snapshot provenance must name the parser version.'
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
