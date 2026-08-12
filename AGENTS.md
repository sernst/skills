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
