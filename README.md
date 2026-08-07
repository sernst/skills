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

Download the archive for your operating system and CPU from the
[latest release](https://github.com/sernst/skills/releases/latest), verify it
against `SHA256SUMS`, extract `skill-manager` (or `skill-manager.exe`), and put
it on `PATH`.

For a complete, paste-into-an-agent installation and upgrade procedure on
Windows, macOS, and Linux, use
[`install.skill-manager.md`](./install.skill-manager.md). The release archive
also includes shell completions and a man page.

### Install from the terminal

Each snippet resolves the latest release, downloads the archive for your CPU,
extracts just the `skill-manager` binary into the current directory, and
removes the download. They skip `SHA256SUMS` verification for brevity; use
`install.skill-manager.md` above for the fully verified procedure.

Windows (PowerShell):

```powershell
$ErrorActionPreference = 'Stop'
$repo  = 'sernst/skills'
$tag   = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name
$arch  = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
$asset = "skill-manager-$tag-$arch-pc-windows-msvc.zip"
$tmp   = Join-Path $env:TEMP "skill-manager-$tag"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
Invoke-WebRequest "https://github.com/$repo/releases/download/$tag/$asset" `
  -OutFile (Join-Path $tmp $asset)
Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $tmp -Force
Get-ChildItem -Path $tmp -Recurse -Filter 'skill-manager.exe' |
  Select-Object -First 1 | Move-Item -Destination . -Force
Remove-Item -Recurse -Force $tmp
```

macOS (bash):

```bash
set -euo pipefail
repo="sernst/skills"
tag=$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" \
  | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')
arch=$(uname -m)
if [ "$arch" = "arm64" ]; then arch=aarch64; else arch=x86_64; fi
asset="skill-manager-$tag-$arch-apple-darwin.tar.gz"
tmp=$(mktemp -d)
curl -fsSL -o "$tmp/$asset" "https://github.com/$repo/releases/download/$tag/$asset"
tar -xzf "$tmp/$asset" -C "$tmp"
mv "$tmp"/*/skill-manager .
rm -rf "$tmp"
```

Linux (bash, static musl build):

```bash
set -euo pipefail
repo="sernst/skills"
tag=$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" \
  | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')
arch=$(uname -m)
if [ "$arch" = "aarch64" ]; then arch=aarch64; else arch=x86_64; fi
asset="skill-manager-$tag-$arch-unknown-linux-musl.tar.gz"
tmp=$(mktemp -d)
curl -fsSL -o "$tmp/$asset" "https://github.com/$repo/releases/download/$tag/$asset"
tar -xzf "$tmp/$asset" -C "$tmp"
mv "$tmp"/*/skill-manager .
rm -rf "$tmp"
```

Run `./skill-manager --version` (or `.\skill-manager.exe --version` on
Windows) to confirm it landed, then move the binary onto `PATH`.

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
- A **skill** is a directory whose root contains `SKILL.md`. Patterns and
  filters select which discovered skills an operation uses.
- A **target** is a root-relative deployment template. Built-ins are `claude`
  (`.claude/skills`), `shared` (`.agents/skills`), and `antigravity`
  (`.gemini/antigravity/skills`).
- A **scope** resolves a target beneath the manager home (`global`) or the exact
  current working directory (`project`). A project deployment takes precedence
  over a global deployment.

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
