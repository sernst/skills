# Dependency maintenance

Dependabot proposes one grouped Cargo update each day and one grouped GitHub
Actions update each week, including major versions. GitHub's separate security
update service may also open urgent advisory fixes. All proposals require normal
checks and human review; dependency maintenance never approves or merges them.

The weekly **Propose pinned tool updates** workflow covers every `TOOL_VERSIONS`
entry: Rust, just, PSScriptAnalyzer, cargo-llvm-cov, cargo-deny, cargo-zigbuild, Zig,
and zizmor (`workflow-audit`). It also synchronizes the Rust toolchain file and
the canonical `.python-version` pin with development docs. All three Python
workflows read that file directly, eliminating duplicated literals and avoiding
any requirement for the proposal App to have workflow-write permission.
The manifest MSRV is a compatibility promise, not a compiler pin; a proposed
upgrade requiring an MSRV change needs explicit review. Runner images and system
packages remain governed by the existing CI environment.

The standard-library updater reads fixed official metadata endpoints: Rust's
stable channel manifest, crates.io's non-yanked stable versions, PSScriptAnalyzer's
GitHub stable release, Zig's download index, and python.org's published Python
release list. It compares numeric versions, excludes prereleases and Python
install-manager calendar versions, refuses downgrades, and treats malformed or
missing metadata as failure. Each response is limited to 4 MiB and a 30-second
timeout, with at most three attempts for transient errors. It never downloads or
executes upstream code. New sources or duplicate pins require updating the
explicit inventory and tests before the workflow can proceed.

Every source and every expected pin is validated in memory before files are
written. Publication independently rejects any changed file outside the exact
pin-file allowlist. The fixed `codex/dependency-maintenance-updates` branch is
created or refreshed from current main with a force-with-lease push. A stale main
checkout fails visibly. Existing proposal branches containing non-updater commits
or files outside the allowlist require manual resolution. One existing open PR is
updated rather than duplicated. A no-op creates neither a branch nor a PR.

The workflow reuses the installed GitHub App's `BENCHMARK_APP_ID` and
`BENCHMARK_APP_PRIVATE_KEY` secrets. Its short-lived token is restricted to the
current repository with contents and pull-request write permissions, so proposed
PRs trigger normal checks. The token is created only after offline tests, metadata
validation, and diff validation; authentication is passed through an ephemeral
environment variable rather than a remote URL or persisted Git configuration.
The ordinary workflow token only reads contents and maintains one dependency
failure issue. No new account, service installation, or secret pair is required.

Failures open or update **[automation] Dependency maintenance failed**, linking the
latest failed run without adding repeated comments. A successful main run closes
that issue. Inspect the first failed step: source/shape changes need a reviewed
parser update; duplicate pins need synchronization; permission failures need the
App installation/secrets checked; concurrent branch changes need a fresh run or
manual branch resolution. Advisory checks remain strict: no CVE allowlists,
ignored advisories, pinned stale advisory databases, or bypassed quality gates.

Use **workflow_dispatch** on a feature branch to validate offline tests, live
metadata, and proposed edits without creating a token, pushing, opening PRs, or
writing failure issues. Main dispatch also permits proposal publication. Locally:

```powershell
python -m unittest discover -s tools/dependency_maintenance/tests -v
python -m tools.dependency_maintenance check # Metadata and drift checks; no writes.
python -m tools.dependency_maintenance refresh # Review the resulting local diff.
```

These offline tests also run under `just check` and the PR Python OS matrix.
