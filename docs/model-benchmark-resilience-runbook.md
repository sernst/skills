# Model benchmark ingestion resilience runbook

This runbook covers the single code PR that lets the scheduled model-benchmark
refresh accept future, syntactically safe model labels without family-registry
edits. It does not change the `skill-manager` CLI version.

## Merge order

| position | pull request | version | contents |
| ---: | --- | --- | --- |
| 1 | [PR link — fill after publication](PR_LINK_TO_BE_FILLED) | no CLI version change | Bounded family-independent model labels, reviewed CursorBench `Minimal` effort, row-scoped failure diagnostics, tests, and the refreshed benchmark snapshot |

This human-authored code PR must be merged by fast-forward only. The existing
scheduled snapshot bot continues to use its established generated-file-only
squash auto-merge workflow; that separate automation is unchanged.

## Pre-merge gates

Run these commands from a clean repository checkout in PowerShell. If any
command fails, or if `git status --short` prints a path, do not merge.

```powershell
git fetch origin
git switch main
git status --short
git rev-parse HEAD
git rev-parse origin/main
git merge-base --is-ancestor origin/main origin/codex/benchmark-model-resilience
git log --merges origin/main..origin/codex/benchmark-model-resilience
git switch --detach origin/codex/benchmark-model-resilience
just check
python -m unittest discover -s tools/model_benchmarks/tests -v
git switch main
git status --short
gh pr checks PR_LINK_TO_BE_FILLED --watch
```

The two `rev-parse` commands must print the same commit. The ancestry command
must exit `0`, the merge log must print nothing, and every PR check must be
green. These checks prevent an accidental non-linear update or merging a branch
that has fallen behind `main`.

## Merge

Resolve and verify the exact reviewed PR head, then fast-forward `main` to that
commit. A rejected push is a stop condition; do not force-push.

```powershell
$benchmarkPr = "PR_LINK_TO_BE_FILLED"
$benchmarkHead = gh pr view $benchmarkPr --json headRefOid --jq .headRefOid
git fetch origin
git switch main
$mainBefore = git rev-parse origin/main
git merge-base --is-ancestor origin/main $benchmarkHead
git log --merges origin/main..$benchmarkHead
git merge --ff-only $benchmarkHead
git push origin main
```

The push automatically starts **Main and manual builds**, including the full
repository quality gate and target packaging. This change has no service,
database, secret, configuration, restart, or downtime step. It only changes a
scheduled parser, its failure report, and a generated Markdown snapshot.

Watch the exact main-branch run before dispatching the benchmark refresh:

```powershell
$mainRun = gh run list --repo sernst/skills --workflow build.yml --branch main --commit $benchmarkHead --limit 1 --json databaseId --jq '.[0].databaseId'
gh run watch $mainRun --repo sernst/skills --exit-status
```

If the run fails, stop and preserve its logs for diagnosis:

```powershell
gh run view $mainRun --repo sernst/skills --log-failed
```

## Manual benchmark refresh and verification

After the main build succeeds, dispatch the scheduled workflow manually and
watch the run created for `main`:

```powershell
$workflow = "model-benchmarks.yml"
$priorRefreshRunIds = @(gh run list --repo sernst/skills --workflow $workflow --branch main --event workflow_dispatch --limit 20 --json databaseId --jq '.[].databaseId')
if ($LASTEXITCODE -ne 0) { throw "Could not capture prior benchmark workflow runs." }
$dispatchEpoch = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
gh workflow run model-benchmarks.yml --repo sernst/skills --ref main
if ($LASTEXITCODE -ne 0) { throw "Benchmark workflow dispatch failed." }
$refreshRun = $null
$newRefreshRuns = @()
for ($attempt = 1; $attempt -le 30 -and -not $refreshRun; $attempt++) {
    $listedRuns = gh run list --repo sernst/skills --workflow $workflow --branch main --event workflow_dispatch --limit 20 --json databaseId,headSha,createdAt | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "Could not list benchmark workflow runs after dispatch." }
    $newRefreshRuns = @($listedRuns | Where-Object {
        $priorRefreshRunIds -notcontains [string]$_.databaseId -and
        [DateTimeOffset]::Parse($_.createdAt).ToUnixTimeSeconds() -ge $dispatchEpoch
    })
    $refreshRun = @($newRefreshRuns | Where-Object { $_.headSha -eq $benchmarkHead } | Select-Object -First 1)[0]
    if (-not $refreshRun -and $attempt -lt 30) { Start-Sleep -Seconds 2 }
}
if (-not $refreshRun -and $newRefreshRuns.Count) { throw "New benchmark run head did not match reviewed head $benchmarkHead." }
if (-not $refreshRun) { throw "No new benchmark workflow_dispatch run appeared within 60 seconds." }
gh run watch $refreshRun.databaseId --repo sernst/skills --exit-status
if ($LASTEXITCODE -ne 0) { throw "Benchmark workflow run $($refreshRun.databaseId) failed." }
$refreshResult = gh run view $refreshRun.databaseId --repo sernst/skills --json conclusion,headSha,url | ConvertFrom-Json
if ($refreshResult.headSha -ne $benchmarkHead) { throw "Benchmark workflow run used unexpected head $($refreshResult.headSha)." }
$refreshResult | Format-List conclusion,headSha,url
$snapshotPr = gh pr list --repo sernst/skills --head automation/model-benchmark-snapshot --state open --limit 1 --json url --jq '.[0].url // empty'
if ($snapshotPr) { gh pr checks $snapshotPr --watch; gh pr view $snapshotPr --json state,mergedAt,url }
```

The bounded poll must find a run absent from the pre-dispatch ID set, created no
earlier than the dispatch timestamp, and using the exact reviewed commit. A
timeout or head mismatch is a stop condition; do not fall back to an older run.

Expected results:

- DeepSWE validates all published rows before CursorBench is rendered.
- CursorBench retains `Muse Spark 1.3` exactly and validates all published rows.
- If source data changed, the workflow creates or updates its one generated
  snapshot PR and follows the existing bot approval and squash auto-merge path.
- If no semantic data changed, it reports that the snapshot is current and
  creates no PR.
- A malformed row fails the whole refresh, retains the last-known-good snapshot,
  and records the bounded source/row/field reason on the deduplicated failure
  issue. Only a later successful refresh records recovery and closes that issue.

After the workflow succeeds, verify the failure issue state and the current
snapshot provenance:

```powershell
gh issue view 29 --repo sernst/skills --json state,url
git fetch origin
git show origin/main:skills/running-as-maestro/references/benchmark-snapshot.md | Select-String 'Parser version|^Source:|Muse Spark 1\.3'
git log --merges "$mainBefore..$benchmarkHead"
git status --short
```

Issue 29 should be closed after recovery. The snapshot should show parser
version `6`, both source provenance lines, and `Muse Spark 1.3`; the merge log
and working-tree status should remain empty.

## Non-blocking deferrals

- Model labels are untrusted table data. Validation bounds their length and
  character surface and rejects controls, URI-like values, and markup syntax;
  safe rendering keeps accepted text inert. Label syntax does not identify
  instructions or prove the publisher's semantic claim that a label names a
  real model.
- New source adapters, source schema changes, effort labels, harnesses, config
  structures, numeric ranges, or row-limit changes still require review.
- Partial-source refresh state remains deliberately unsupported. Any malformed
  enabled source retains the complete last-known-good snapshot.
