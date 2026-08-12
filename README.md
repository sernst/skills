# Skills and skill-manager

This repository is a two-part toolkit:

1. [`skills/`](./skills) contains reusable instructions that teach AI agents
   specialized workflows.
2. [`skill-manager`](./clis/skill-manager) is a native CLI that discovers those
   skills from local or GitHub sources and deploys them to the skill directories
   used by agent harnesses.

Use the CLI when you want one source of truth for skills across Claude Code,
Codex/OpenAI agents, and Google Antigravity. Use the skills directly when you
want to inspect or adapt the instructions.

## Install

Install or upgrade `skill-manager` with the script for your platform. Each one
resolves the latest release, verifies the download against `SHA256SUMS`, and
prompts for an install location.

macOS and Linux:

```console
$ curl -fsSL https://raw.githubusercontent.com/sernst/skills/main/clis/skill-manager/install.sh | sh
```

Windows (PowerShell):

```powershell
powershell -c "irm https://raw.githubusercontent.com/sernst/skills/main/clis/skill-manager/install.ps1 | iex"
```

### Customize an installation

On macOS and Linux, pass `--version`, `--dir`, `--yes`, `--force`, or
`--no-modify-path` through the pipe:

```console
$ curl -fsSL https://raw.githubusercontent.com/sernst/skills/main/clis/skill-manager/install.sh | sh -s -- --version 0.1.3 --dir "$HOME/.local/bin" --yes --no-modify-path
```

The scripts also read `SKILL_MANAGER_VERSION`, `SKILL_MANAGER_INSTALL_DIR`,
`SKILL_MANAGER_INSTALL_YES=1`, and `SKILL_MANAGER_NO_MODIFY_PATH=1`. Because
`irm | iex` cannot receive parameters, use those environment variables on
Windows:

```powershell
$env:SKILL_MANAGER_INSTALL_DIR = 'C:\tools\bin'
powershell -c "irm https://raw.githubusercontent.com/sernst/skills/main/clis/skill-manager/install.ps1 | iex"
```

For a manual, independently verified installation, use the
[latest release](https://github.com/sernst/skills/releases/latest) and its
`SHA256SUMS`. For agent-assisted installation, use
[`install.skill-manager.md`](./install.skill-manager.md). Release archives also
include shell completions and a man page.

## Five-minute quick start

The repository hosts its own skills below `skills/`, so it is also a useful
self-hosting example:

```console
$ skill-manager --json-input
{"command":"source.add","source":"sernst/skills/skills","name":"sernst-skills","label":"sernst skills"}
{"version":1,"event":"source.added","level":"info","data":{...}}

$ skill-manager --json-input
{"command":"load","sources":["sernst-skills"],"filters":["managing-skills"],"shared":true,"global":true,"dry_run":true}
{"version":1,"event":"skill.loaded","level":"info","data":{"skill":"managing-skills",...,"dry_run":true}}
{"version":1,"event":"summary","level":"info","data":{"action":"load",...}}

$ skill-manager --json-input
{"command":"load","sources":["sernst-skills"],"filters":["managing-skills"],"shared":true,"global":true}
{"version":1,"event":"skill.loaded","level":"info","data":{"skill":"managing-skills",...}}
{"version":1,"event":"summary","level":"info","data":{"action":"load",...}}

$ skill-manager --json-input
{"command":"status","filters":["managing-skills"],"shared":true,"global":true}
{"version":1,"event":"status.row","level":"info","data":{"skill":"managing-skills",...}}
{"version":1,"event":"summary","level":"info","data":{"action":"status","skills":1}}
```

Start a new agent session if the harness scans installed skills only at
startup. You can then ask the agent to use `$managing-skills` for conversational
skill discovery, deployment, update, removal, and configuration.

For an interactive shell, the equivalent shorter flow is:

```console
skill-manager source add sernst/skills/skills sernst-skills --label "sernst skills"
skill-manager load sernst-skills --filter managing-skills --shared --global --dry-run --no-input
skill-manager load sernst-skills --filter managing-skills --shared --global --no-input
skill-manager status managing-skills --shared --global --no-input
```

## Mental model

```text
sources -> discovered skills -> targets
                              -> global scope
                              -> project scope
```

- A **source** is a local directory or GitHub repository path containing one
  skill or a collection of skill directories.
- A **skill** is a directory whose root contains `SKILL.md`. Patterns, filters,
  and exact skill names (case-insensitive) all select which discovered skills
  an operation uses; a bare name that matches both a discovered skill and a
  same-named directory in the current directory selects the skill and warns
  (use `./name` to force the directory), and a name matching none of those is
  a hard error.
- A **target** is a root-relative deployment template. Built-ins are `claude`
  (`.claude/skills`), `shared` (`.agents/skills`), and `antigravity`
  (`.gemini/antigravity/skills`).
- A **scope** resolves a target beneath the manager home (`global`) or the exact
  current working directory (`project`). A project deployment takes precedence
  over a global deployment. When CWD is the manager home, only global scope is
  available; explicit `--project` is rejected instead of aliasing global files.

Manager-owned configuration, cache, backups, and locks live beneath
`~/.skill-manager/` by default. Set `SKILL_MANAGER_HOME` to isolate that state.

## Learn the CLI

- [`cheatsheet.skill-manager.md`](./cheatsheet.skill-manager.md): goal-oriented
  command, flag, JSON, NDJSON, configuration, and safety reference.
- [`clis/skill-manager/docs/cli.md`](./clis/skill-manager/docs/cli.md):
  canonical human CLI behavior.
- [`clis/skill-manager/docs/json.md`](./clis/skill-manager/docs/json.md):
  strict recipe and NDJSON contract for automation.
- [`clis/skill-manager/docs/configuration.md`](./clis/skill-manager/docs/configuration.md):
  storage, target templates, backups, migration, and filesystem safety.
- [`clis/skill-manager/README.md`](./clis/skill-manager/README.md): CLI package
  overview and contributor entry point.

Preview `load`, `update`, `copy`, and `remove` with `--dry-run`. A dry run does
not deploy or remove skills, but startup storage migration and a required
remote-cache refresh can still update manager-owned state. Use explicit targets
and scopes in unattended calls, parse every NDJSON line, and check the process
exit code.

## Skill catalog

### [`drafting-commit-message`](./skills/drafting-commit-message/SKILL.md)

Draft a concise, imperative commit title and motivation-focused change bullets
from staged or unstaged changes.

### [`grill-me`](./skills/grill-me/SKILL.md)

Explore the available project context, then interview the user one decision at
a time until a plan or design has no unresolved branches.

### [`managing-skills`](./skills/managing-skills/SKILL.md)

Operate the complete `skill-manager` CLI conversationally through strict JSON
recipes and parsed NDJSON, with explicit safety and confirmation rules.

### [`reviewing-implemented-work-order`](./skills/reviewing-implemented-work-order/SKILL.md)

Review a work-order implementation against its job, research, plan, repository
patterns, security requirements, and test coverage.

### [`reviewing-my-code`](./skills/reviewing-my-code/SKILL.md)

Prepare a focused review of branch changes by identifying key themes and
surfacing correctness, security, performance, and coverage issues.

### [`running-as-maestro`](./skills/running-as-maestro/SKILL.md)

Run an agent as an accountable orchestrator that delegates work to subagents,
selects appropriate model tiers, and verifies their output.

## Contributing and releases

See [`CONTRIBUTING.md`](./CONTRIBUTING.md) for the Just-based quality workflow
and [`RELEASES.md`](./RELEASES.md) for tagged release procedures.
