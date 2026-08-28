# Model benchmark refresh rollout

## Merge order

| position | pull request | version | contents |
| ---: | --- | --- | --- |
| 1 | `feature/maestro-model-benchmarks` (PR pending) | no release | Maestro guidance, generated snapshot, updater, tests, and automation |

## Pre-merge gates — do not merge if any fail

1. Create and install a repository-only GitHub App for `sernst/skills`. Grant
   repository `Contents: read and write` and `Pull requests: read and write`;
   grant no organization or account permissions. Generate a private key.
2. Store the App ID and complete PEM private key as Actions secrets:

   ```powershell
   gh secret set BENCHMARK_APP_ID --repo sernst/skills
   gh secret set BENCHMARK_APP_PRIVATE_KEY --repo sernst/skills < path\to\app.private-key.pem
   ```

3. Enable squash auto-merge and permit Actions reviews:

   ```powershell
   gh api --method PATCH repos/sernst/skills -F allow_auto_merge=true
   gh api repos/sernst/skills/actions/permissions/workflow
   ```

   In **Settings → Actions → General → Workflow permissions**, retain read-only
   default workflow permissions and enable **Allow GitHub Actions to create and
   approve pull requests**. Re-run the second command and confirm
   `can_approve_pull_request_reviews` is `true`.
4. In the `main` branch protection rule, require the named status check
   **PR required gate**, retain linear history and one approving review, and
   confirm squash merging remains enabled. Do not grant the App a branch-rule
   bypass.
5. On the feature branch, run `just check` and `zizmor --pedantic
   .github/workflows`; both must exit 0. Confirm the generated snapshot is below
   24 KiB and the human guidance below 1,200 words.

The updater is a standard-library-only Python package and is shell-neutral. It
requires Python 3.13 or newer; no environment setup or package installation is
needed. For a read-only live local probe, run:

```text
python -m tools.model_benchmarks check
```

Exit 0 means current, 2 means an update is available, and 1 means fetch or
validation failed. `python -m tools.model_benchmarks refresh` performs the same
validation and atomically replaces the snapshot only when semantic content
changes. `python -m unittest discover -s tools/model_benchmarks/tests -v` runs
the deterministic offline suite on any supported shell and operating system.

## Merge and automated behavior

Squash-merge the feature PR after its required check and review pass. There is
no deployment, restart, or expected user disruption. On merge, the daily
schedule and manual dispatch become available.

Each refresh validates both allowlisted sources before writing. The package
also owns the deduplicated GitHub issue failure/recovery lifecycle so the
workflow does not depend on shell-specific parsing. A semantic
change updates only the generated snapshot, creates or updates one App-authored
PR, receives an Actions-bot approval, and squash auto-merges after **PR required
gate** succeeds. An unchanged refresh creates no branch or PR. Any fetch,
schema, sanitization, limit, diff-allowlist, PR, approval, or merge failure keeps
the last-known-good snapshot and opens or updates one issue mentioning
`@sernst`. A later successful run comments with provenance and closes it.

## Verification

1. Run **Refresh model benchmarks** manually from the Actions UI.
2. If sources are unchanged, expect a green run containing `Unchanged` and no
   automation PR.
3. For an end-to-end update test, wait for a real semantic source change or use
   a reviewed temporary fixture change on a non-default branch. Expect exactly
   one PR whose diff contains only
   `skills/running-as-maestro/references/benchmark-snapshot.md`; verify its body
   shows before/after source versions and timestamps, CI passes, approval is by
   `github-actions[bot]`, and it squash-merges.
4. Confirm `main` contains the updated snapshot and the automation branch was
   deleted.

## GitHub App token HTTP 422 recovery

If **Create narrowly scoped updater token** fails with HTTP 422, the installed
GitHub App cannot satisfy the workflow's requested repository permissions. The
App must have **Contents: read and write** and **Pull requests: read and
write** for `sernst/skills`; do not weaken the workflow scopes or bypass App
authentication. After changing the App's permissions, the installation owner
must accept the updated permissions prompt, or uninstall and reinstall the App
for the repository. Then re-run **Refresh model benchmarks** manually from the
Actions UI.

The failure handler runs independently of the App-token step and uses the
job's `issues: write` permission with the standard `GITHUB_TOKEN`, so it can
open or update the tagged failure issue even when App-token creation fails.

## Non-blocking punchlist

- Add or remove benchmark sources only through reviewed registry, adapter,
  fixture, and test changes; scheduled automation must never discover sources.
- Reassess the 24 KiB/200-row ceilings when a reviewed source is added or
  removed, without truncating published rows.
