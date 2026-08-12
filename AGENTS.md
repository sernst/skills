# AGENTS.md

## What this repository is

Two things live here:

1. [`skills/`](./skills) — reusable Markdown instructions (`SKILL.md` files)
   that teach AI agents specialized workflows. These are content, not code.
2. [`clis/`](./clis) — native CLI tools, currently just
   [`skill-manager`](./clis/skill-manager), a Rust binary that discovers
   skills from local or GitHub sources and deploys them into the skill
   directories used by Claude Code, Codex/OpenAI agents, and Google
   Antigravity.

Additional CLIs register in [`clis/registry.just`](./clis/registry.just); the
root `Justfile` fans every recipe out to each registered CLI via
`tools/run-registered.ps1`.

## Build, test, and lint

Run from the repository root (fans out to every registered CLI):

```powershell
just build          # cargo build --locked
just test           # cargo test --locked --all-features
just lint           # cargo clippy --all-targets --all-features -- -D warnings
just format-check   # cargo fmt --check
just check          # format-check + lint + build + test + docs + deny + coverage
```

Or from `clis/skill-manager` directly with the same recipe names (`just
build`, `just test`, `just check`, ...). `just check` is what CI runs for a
pull request; run it before opening one.

## Working on `skill-manager`

**Any change to `skill-manager` command output, prompts, or plans — new
commands, new flags, new tables, new confirmations — MUST follow
[`clis/skill-manager/docs/ux-guidelines.md`](clis/skill-manager/docs/ux-guidelines.md)
before writing code.** That document derives the CLI's output conventions
(plan-before-prompt authorization, significance gating, symbol/color
vocabulary, confirmation defaults, NDJSON contract) from one governing
principle and is the authoritative reference — read it first, not the
existing per-command mocks or a similar-looking command, since some commands
have not yet migrated to the described model.

Other docs in `clis/skill-manager/docs/`:

- [`cli.md`](clis/skill-manager/docs/cli.md) — command reference and current
  behavior contracts.
- [`json.md`](clis/skill-manager/docs/json.md) — NDJSON event envelope and
  recipe (`--json-input`/`--input`) contract.
- [`architecture.md`](clis/skill-manager/docs/architecture.md) — module
  boundaries, ports/adapters, deployment transaction model.
- [`configuration.md`](clis/skill-manager/docs/configuration.md) —
  configuration schema, storage layout, and migration.
- [`development.md`](clis/skill-manager/docs/development.md) — build/test/CI
  recipes in more detail, release process.
- [`deviations.md`](clis/skill-manager/docs/deviations.md) — deliberate
  behavior differences from the legacy Python manager.
- [`parity-ledger.md`](clis/skill-manager/docs/parity-ledger.md) — test
  traceability between the Python and Rust implementations.

**Never run a built `skill-manager` binary against your real user home while
validating, smoke testing, or otherwise manually exercising it.** Always pass
`--home <scratch-dir>` (it outranks `SKILL_MANAGER_HOME` and the OS home).
A prior session skipped this and corrupted live configuration that had to be
manually undone — treat it as a hard rule, not a suggestion.

A bare scratch `--home` starts empty, which does not resemble a real
configuration and is a poor smoke-testing environment. To seed a scratch
directory from an existing one — including the real home, read-only — pair
`configs copy` with `--home` pointed at the *same* scratch directory you are
seeding, then keep using that `--home` for every command that follows.

Pass `FROM` as an already-expanded absolute path, not a literal `~`. The CLI
expands a leading `~` against the *active* `--home`, so a literal `~` here
would resolve to the scratch home — the same path as `TO` — and the command
would refuse the identical operands. Let the shell expand the real home
instead (`$HOME` in PowerShell, `~` in POSIX shells before it reaches the CLI).

A relative `--home` is supported — it is normalized to an absolute path
(against the current directory at invocation time) before anything derives a
path from it, so relative and absolute values resolve the same store. Prefer
an absolute scratch path anyway when a smoke test spans several commands: a
relative value re-resolves against whatever the CWD happens to be for each
invocation, so an absolute path keeps every command pinned to the identical
home regardless of where it runs:

```powershell
PS> $smoke = Join-Path $env:TEMP 'skill-manager-smoke'
PS> skill-manager --home $smoke configs copy $HOME $smoke --yes
PS> skill-manager --home $smoke status
```

The destination naturally sits under your home (a `TEMP` or repo directory
usually does), which is fully supported: `configs copy` seeds only `FROM`'s
`.skill-manager` directory and its resolved target roots, so a destination
anywhere under `FROM` — just not inside one of those copied roots — is fine.
`configs copy` reads its `FROM` argument (here the real home) directly from
disk and never opens it — or the active `--home` — through the configuration
repository, so it never migrates, backs up, locks, or otherwise writes to
either one. That holds even when `FROM` *is* the active home, and even under
`--dry-run`, which changes nothing anywhere. Passing `--home $smoke` on the
`configs copy` invocation itself, not only on the commands that follow, still
matters: that flag governs which configuration the active-home fallback
consults when `FROM` has no configuration of its own, and
it is the home every following command uses, by the same `--home` >
`SKILL_MANAGER_HOME` > OS home precedence as any other command.
