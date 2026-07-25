# skill-manager

`skill-manager` discovers `SKILL.md` directories from local or GitHub sources and deploys them to AI-tool skill directories. It is a standalone Rust 2024 executable with a testable library and recoverable filesystem deployment.

## Install

Download the archive matching your operating system and CPU from the GitHub release, unpack it, and add the executable to `PATH`. Archives include shell completions and a man page.

## Quick start

```console
$ skill-manager source add ./my-skills --name team --label "Team skills"
$ skill-manager load --all --no-input
$ skill-manager status
$ skill-manager update --target claude --no-input
```

Use `--dry-run` before a mutating command to emit its plan without changing configuration, cache, targets, backups, or lock state.

## Commands

`status` is the default command and has `ls` and `list` aliases. `load` creates or replaces deployments, while `update` only changes skills already present in a target. `copy` copies one source to an arbitrary destination. `remove` removes deployments and needs `--yes` for unattended use. `resolve` records a collision preference by excluding the losing source's duplicate.

`source add|remove|list|update` manages sources. `target add|list|enable|disable|remove|set-path` manages deployment targets. See [the command reference](docs/cli.md) for the full contract.

## Configuration and safety

The active configuration file is `~/.skill-manager.config.json`; the older `~/.skills-syncer.config.json` is migrated once. The v0-to-v1 migration makes a non-overwriting `.v0.bak` backup. Remote cache content is under `~/.skill-manager-cache`.

Deployments are staged and journaled per skill. A later failed skill does not undo earlier committed skills; the next invocation recovers incomplete work. The manager refuses unsafe tree entries such as links and special files. Details are in [configuration and migration](docs/configuration.md) and [the canonical-deviation ledger](docs/deviations.md).

## Automation

`--json` writes NDJSON events. `--json='{...}'`, `--json-input`, and `--input FILE` also provide a strict single-invocation recipe and imply noninteractive JSON mode. See [the JSON contract](docs/json.md) for envelope, validation, precedence, and exit codes.

## Development

From the repository root, run `just skill-manager-check`. `just skill-manager-format` is the only recipe that rewrites formatting. The [architecture note](docs/architecture.md) and [contributor guide](docs/development.md) explain the design and release checks.
