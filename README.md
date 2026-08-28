# Skills and skill-manager

This repository offers two things:

- [`skills/`](./skills): reusable Markdown instructions that teach AI agents
  specialized workflows.
- [`skill-manager`](./clis/skill-manager): the supported way to discover and
  deploy these skills across Claude Code, Codex/OpenAI agents, and Google
  Antigravity.

Choose **a skill** below to inspect or adapt its instructions. Choose
**skill-manager** to install skills, keep one source of truth, and manage
updates across agent harnesses.

## Skill catalog

### [`drafting-commit-message`](./skills/drafting-commit-message/SKILL.md)

Draft a concise, imperative commit title and motivation-focused change bullets
from staged or unstaged changes.

### [`expecting-pr-outputs`](./skills/expecting-pr-outputs/SKILL.md)

Produce CI-green PR chains as session deliverables — stacked branches under
enforced linear history, deployment runbooks, and explicitly-gated fast-forward
merges with pipeline monitoring between each merge.

### [`grill-me`](./skills/grill-me/SKILL.md)

Explore project context, then interview the user one decision at a time until a
plan or design has no unresolved branches.

### [`managing-skills`](./skills/managing-skills/SKILL.md)

Operate the complete `skill-manager` CLI conversationally, with explicit safety
and confirmation rules.

### [`reviewing-implemented-work-order`](./skills/reviewing-implemented-work-order/SKILL.md)

Review a work-order implementation against its job, research, plan, repository
patterns, security requirements, and test coverage.

### [`reviewing-my-code`](./skills/reviewing-my-code/SKILL.md)

Prepare a focused branch review covering correctness, security, performance, and
test coverage.

### [`running-as-maestro`](./skills/running-as-maestro/SKILL.md)

Run an agent as an accountable orchestrator that delegates work to subagents,
selects appropriate model tiers, and verifies their output.

## Install skill-manager

macOS and Linux:

```console
curl -fsSL https://raw.githubusercontent.com/sernst/skills/main/clis/skill-manager/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/sernst/skills/main/clis/skill-manager/install.ps1 | iex
```

The installers prompt for a destination when interactive. Windows verifies the
release against `SHA256SUMS`; the POSIX installer verifies it when `sha256sum`
or `shasum` is available and prints a prominent warning if neither exists.

### Customize the install directory

`--dir` and `-Dir` accept absolute paths, `~`/`~/...` relative to the active
home, and other relative paths resolved from the invocation directory. The
resolved absolute path is shown before installation.

On macOS and Linux, pass options through the pipe:

```console
curl -fsSL https://raw.githubusercontent.com/sernst/skills/main/clis/skill-manager/install.sh | sh -s -- --dir "./tools/bin" --yes --no-modify-path
```

Because `irm | iex` cannot receive parameters, use environment variables on
Windows:

```powershell
$env:SKILL_MANAGER_INSTALL_DIR = '.\tools\bin'
irm https://raw.githubusercontent.com/sernst/skills/main/clis/skill-manager/install.ps1 | iex
```

Both scripts also support `SKILL_MANAGER_VERSION`,
`SKILL_MANAGER_INSTALL_YES=1`, and `SKILL_MANAGER_NO_MODIFY_PATH=1`. For a
manual installation, use the
[latest release](https://github.com/sernst/skills/releases/latest) and its
`SHA256SUMS`.

Agents must not install the CLI themselves. After a user installs it, follow
the [agent usage guide](./docs/agent-usage.md) to make `managing-skills`
available to agent sessions.

## Try an interactive workflow

Add this repository's skill collection, preview one deployment, then apply it:

```console
skill-manager source add https://github.com/sernst/skills/tree/main/skills --name sernst-skills --label "sernst skills"
skill-manager describe sernst-skills:managing-skills
skill-manager load sernst-skills --filter managing-skills --shared --global --dry-run
skill-manager load sernst-skills --filter managing-skills --shared --global
skill-manager status managing-skills --shared --global
```

`describe` shows trigger text and a bounded README/SKILL.md excerpt; qualify a
skill with its source to inspect an excluded or shadowed copy. The dry run shows
the same plan without deploying the skill. The next command shows the plan again
and asks for authorization. Start a new agent session if your harness scans
installed skills only at startup; you can then ask it to use `$managing-skills`.

## Mental model and safety

```text
sources -> discovered skills -> target + scope deployments
```

- A **source** is a local directory or GitHub repository path containing skill
  directories.
- A **skill** is a directory whose root contains `SKILL.md`.
- A **target** is an agent harness's skill directory; built-ins are `claude`,
  `shared`, and `antigravity`.
- A **scope** is `global` (under the manager home) or `project` (under the
  current project). Project deployments take precedence.

Manager-owned configuration, cache, backups, and locks live beneath
`~/.skill-manager/` by default. Preview `load`, `update`, `copy`, and `remove`
with `--dry-run`. A dry run does not change deployments, though startup storage
migration or a required remote-cache refresh can still update manager-owned
state.

## Go deeper

- [Configuration and filesystem safety](./clis/skill-manager/docs/configuration.md)
- [Human CLI reference](./clis/skill-manager/docs/cli.md)
- [NDJSON and automation contract](./clis/skill-manager/docs/json.md)
- [Goal-oriented cheatsheet](./cheatsheet.skill-manager.md)
- [Using skill-manager through an agent](./docs/agent-usage.md)
- [Architecture and development](./clis/skill-manager/docs/development.md)
- [Contributing](./CONTRIBUTING.md) and [releases](./RELEASES.md)
