# skill-manager

`skill-manager` discovers `SKILL.md` directories from local or GitHub sources and deploys them to AI-tool skill directories. It is a standalone Rust 2024 executable with a testable library and recoverable filesystem deployment.

Start with the repository [README](../../README.md) for the two-part toolkit and
five-minute setup, or keep the
[skill-manager cheatsheet](../../cheatsheet.skill-manager.md) nearby for
goal-oriented examples. Agents can use the
[`managing-skills` skill](../../skills/managing-skills/SKILL.md) for complete
conversational operation; the
[pasteable installer](../../install.skill-manager.md) installs both.

## Install

Download the archive matching your operating system and CPU from the GitHub release, unpack it, and add the executable to `PATH`. Archives include shell completions and a man page.

## Quick start

```console
$ skill-manager source add ./my-skills --name team --label "Team skills"
$ skill-manager load --all --global --no-input
$ skill-manager status
$ skill-manager update --target claude --no-input
```

Use `--dry-run` before a deployment mutation to emit its plan without changing
skill deployments. Startup storage migration still runs, and discovery may
refresh manager-owned remote cache when required, even during a dry run.

## Commands

`status` is the default command and has `ls` and `list` aliases. `load` creates or replaces deployments, while `update` only changes skills already present in a target. `copy` copies one source to an arbitrary destination. `remove` removes deployments and needs `--yes` for unattended use. `resolve` records a collision preference by excluding the losing source's duplicate.

`source add|remove|list|update|locate|alternate|swap` manages sources. For example, pair a development checkout with its normal remote and switch without retyping either location:

```console
$ skill-manager source alternate personal sernst/skills
$ skill-manager source swap personal
$ skill-manager source swap personal
```

`source locate` also has `relocate`, `move`, and `mv` aliases, and `source update --location` combines relocation with metadata changes. `target add|list|enable|disable|remove|set-path` manages deployment targets. See [the command reference](docs/cli.md) for the full contract.

## Configuration and safety

Manager-owned state is consolidated beneath `~/.skill-manager/`: `config.json`,
`cache/`, `backups/`, and `locks/`. On startup, the manager safely and
idempotently migrates recognized legacy flat configuration, cache, and backup
locations. Schema migrations archive exact source bytes before conversion.

Deployments are staged and journaled per skill. A later failed skill does not undo earlier committed skills; the next invocation recovers incomplete work. The manager refuses unsafe tree entries such as links and special files. Details are in [configuration and migration](docs/configuration.md) and [the canonical-deviation ledger](docs/deviations.md).

## Automation

`--json` writes NDJSON events. `--json='{...}'`, `--json-input`, and `--input
FILE` also provide a strict single-invocation recipe and imply noninteractive
JSON mode. See [the JSON contract](docs/json.md) for envelope, validation,
precedence, and exit codes, [the command reference](docs/cli.md) for human CLI
behavior, and [configuration and migration](docs/configuration.md) for storage,
backups, and target templates.

## Development

From the repository root, run `just skill-manager-check`. `just skill-manager-format` is the only recipe that rewrites formatting. The [architecture note](docs/architecture.md) and [contributor guide](docs/development.md) explain the design and release checks.
