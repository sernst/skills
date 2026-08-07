# skill-manager cheatsheet

`skill-manager` discovers `SKILL.md` directories from local or GitHub sources
and deploys them to agent-harness skill directories.

## Prerequisites and invocation

- A `skill-manager` release executable on `PATH`.
- Network access for GitHub sources. `GITHUB_TOKEN` is preferred over
  `GH_TOKEN` for authenticated requests; neither is persisted.
- A skill is a portable directory whose root contains `SKILL.md`.

```text
skill-manager [GLOBAL OPTIONS] [COMMAND]
skill-manager                         # defaults to status
```

Global options can appear before or after subcommands:

| Option | Meaning |
| --- | --- |
| `--json` | Emit NDJSON and disable prompts. |
| `--json=OBJECT` | Apply one strict recipe object and emit NDJSON. The equals sign is required. |
| `--json-input` | Read one recipe object from stdin and emit NDJSON. |
| `--input FILE` | Read one recipe object from a file and emit NDJSON. |
| `--no-input` | Disable prompts; fail when a choice is required. |
| `--color auto\|always\|never` | Human-output color policy; `NO_COLOR` also disables color. |
| `-h`, `--help` | Show help. |
| `-V`, `--version` | Show version. |

The three recipe carriers are mutually exclusive. Machine modes imply
`--no-input`.

## Mental model

```text
configured/local/GitHub sources
             |
             v
 discovered skills -- filters/collision winner --> target templates
                                                   |           |
                                                   v           v
                                                global      project
```

Built-in targets:

| Selector | Target | Template |
| --- | --- | --- |
| `--claude` | Claude Code | `.claude/skills` |
| `--shared` | Shared/OpenAI-compatible agents | `.agents/skills` |
| `--antigravity`, `--ag` | Google Antigravity | `.gemini/antigravity/skills` |

Use `--all` for all enabled configured targets or repeat `--target NAME`.
Target selectors form a deduplicated union. `--all` never opts into a disabled
target; only an explicit `--target NAME` may do so.

Global scope (`--global`, `-g`) resolves below the manager home. Project scope
(`--project`, `-p`) resolves below the exact CWD; there is no Git-root search.
The flags conflict. A project deployment takes precedence over global when both
exist.

## Common workflows

| Goal | Command |
| --- | --- |
| Add this repository's skills | `skill-manager source add sernst/skills/skills sernst-skills --label "sernst skills"` |
| List sources | `skill-manager source list` |
| Preview one global shared skill | `skill-manager load sernst-skills --filter managing-skills --shared --global --dry-run --no-input` |
| Deploy it | `skill-manager load sernst-skills --filter managing-skills --shared --global --no-input` |
| Inspect it in both scopes | `skill-manager status managing-skills --shared --no-input` |
| Preview updates to existing copies | `skill-manager update sernst-skills --filter managing-skills --all --dry-run --no-input` |
| Adopt an agent-modified copy back into its source | `skill-manager import managing-skills --claude --global --dry-run --no-input` |
| Preview project removal | `skill-manager remove managing-skills --shared --project --dry-run --no-input` |
| Remove after review | `skill-manager remove managing-skills --shared --project --yes --no-input` |
| Refresh remote source state | `skill-manager status --refresh --no-input` |
| Use only skills in CWD | `skill-manager status --cd-only --no-input` |
| Copy selected skills elsewhere | `skill-manager copy sernst-skills ./vendor/skills --filter 'managing-*' --dry-run --no-input` |

## Discovery, deployment, and status commands

### `load` (`install`) and `update`

```text
skill-manager load [SOURCE_OR_PATTERN ...] [OPTIONS]
skill-manager install [SOURCE_OR_PATTERN ...] [OPTIONS]
skill-manager update [SOURCE_OR_PATTERN ...] [OPTIONS] [--yes]
```

`load` creates or replaces deployments; `install` is a visible alias for it and
accepts the identical options. `update` changes only skills already deployed in
eligible targets/scopes; it never creates a new deployment.

| Option | Meaning |
| --- | --- |
| `--filter PATTERN` | Include skill-name pattern; repeatable and ORed. |
| `--cd` | Add CWD to configured sources. |
| `--cd-only` | Use only CWD. |
| `--no-cd` | Configured-sources-only compatibility spelling. |
| `--claude`, `--shared`, `--antigravity`/`--ag` | Select built-in target. |
| `--all` | Select all enabled configured targets. |
| `--target NAME` | Select a configured target; repeatable. |
| `--global`/`-g`, `--project`/`-p` | Select installation scope. |
| `--dry-run` | Plan without deploying skills. |
| `--refresh` | Force GitHub cache refresh. |
| `--yes`/`-y` (`update` only) | Skip the plan confirmation; the plan still prints. |

Interactive `update` prints a change plan first—one `update`, `load`, or `skip`
line per skill/target/scope with its file and line deltas—and then asks once
before deploying. Declining cancels with exit `0` and no deployment. `--yes`
and `--dry-run` print the same plan without prompting, and machine mode keeps
its event-only contract.

In interactive `load`, scope defaults to project when a selected target's
leading directory already exists in CWD; otherwise global. Non-interactive
`load`, including dry-run, requires an explicit scope. Unscoped `update` infers
all existing deployments, preferring a project copy when both exist. A
committed non-interactive `load` or `update` must explicitly select targets with
one or more built-in selectors, `--all`, or `--target NAME`; dry-run may
implicitly preview enabled targets.

### `import`

```text
skill-manager import SKILL [OPTIONS]
```

The reverse of `load`: it adopts a deployed—possibly agent-modified—copy of one
skill as the new canonical source content.

| Option | Meaning |
| --- | --- |
| `--claude`, `--shared`, `--antigravity`/`--ag` | Narrow detection to a built-in target. |
| `--all` | Consider all enabled configured targets. |
| `--target NAME` | Narrow detection to a configured target; repeatable. |
| `--global`/`-g`, `--project`/`-p` | Narrow detection to one scope. |
| `--dry-run` | Show the plan without writing to the source. |
| `--yes`/`-y` | Skip the destructive source-overwrite confirmation. |

Exactly one skill name is accepted; patterns are not. The skill is resolved
through the same first-source-wins discovery as `load`. Candidates are deployed
copies that differ from the source; identical copies are never candidates, and
having none is a clean success. One candidate is preselected, and several
require an interactive choice or narrower target/scope flags.

Before writing, `import` prints a `from` deployment, a `to` source path, a
git-style per-file summary (`added`/`modified`/`deleted` with `+N/-N`, or a byte
delta for binary content), and a totals line. The confirmation defaults to no.
Applying it makes the source directory byte-identical to the chosen deployed
copy, including deleting source files the deployment no longer has, through the
same staged, journaled transaction used for deployments.

Import writes to local source checkouts only. A GitHub-backed source needs a
local alternate location and an interactive confirmation naming that path;
without a resolvable local destination the command fails rather than guessing.
Both only apply once a candidate exists, so an up-to-date source never prompts.

### `copy`

```text
skill-manager copy SOURCE DESTINATION [--filter PATTERN ...] [--dry-run] [--refresh]
```

Copies selected discovered skills to an arbitrary destination. `--filter` is
repeatable and ORed. Copy does not use deployment target or scope selectors.

### `remove`

```text
skill-manager remove [SKILL_OR_PATTERN ...] [OPTIONS]
```

Accepts the same `--filter`, source, target, scope, `--dry-run`, and `--refresh`
options as deployment commands. `--yes`/`-y` skips the removal confirmation but
never chooses a scope. With no skill operands, discovery selection determines
the candidate set.

Without an explicit scope, an unambiguous existing copy is removed. If a
selected skill exists in both scopes, interactive mode asks for global, project,
or both; non-interactive mode fails. For unattended removal from both, run one
explicit global command and one explicit project command.

### `status` (`ls`, `list`)

```text
skill-manager status [FILTER ...] [OPTIONS]
```

Options: repeatable `--filter PATTERN`; all source, target, and scope selectors;
and `--refresh`. Positional and option filters case-insensitively match skill
names, source names, or unique labels. Status inspects both scopes by default.

States are `up-to-date`, `needs-update`, `not-loaded`, and `no-connection`.
Locations are `global`, `project`, `both`, and `none`; project is effective for
`both`. Equality compares relative regular-file names and SHA-256 content, not
timestamps or empty directories.

### `resolve`

```text
skill-manager resolve [SKILL_OR_PATTERN ...] [--prefer-source NAME_OR_ID]
                      [--cd|--cd-only|--no-cd] [--refresh]
```

Chooses one source for each collision and persists exclusions on losing
configured sources. Omit skill operands to resolve all collisions. In
non-interactive use, pass `--prefer-source`.

## Source commands

Sources accept local paths, GitHub tree URLs, or
`owner/repo[:ref][/path]`. A source selector can be stable ID, name, unique
label, or active location. Inactive alternate locations are not selectors.

| Command | Options and behavior |
| --- | --- |
| `source add [SOURCE] [NAME]` | `--name NAME` (conflicts with positional name), `--label TEXT`, repeatable `--exclude PATTERN`, `--mode collection\|single`, `--cache-ttl-hours HOURS`. |
| `source remove [SOURCE]` | Remove by active path/reference, name, or ID. |
| `source list` | List stored sources and inactive alternates. |
| `source update SOURCE` | `--name NAME`, `--location LOCATION`, `--label TEXT`, repeatable `--exclude PATTERN`, `--clear-exclude`, `--cache-ttl-hours HOURS`. |
| `source locate SOURCE LOCATION` | Change active location only. Aliases: `relocate`, `move`, `mv`. |
| `source alternate SOURCE [LOCATION]` | Set/replace inactive location; use `--clear` instead of a location to remove it. |
| `source swap SOURCE` | Exchange active and inactive locations; requires an alternate. |

`collection` means immediate child directories are skills; `single` means the
source root is one skill. IDs survive relocation. Active and alternate
identities must differ and cannot collide with any location slot on another
source. Machine/non-interactive `source add` requires an explicit nonblank
positional `NAME` or `--name NAME`; it never invents one.

## Target commands

```text
skill-manager target add NAME PATH
skill-manager target list
skill-manager target enable NAME
skill-manager target disable NAME
skill-manager target remove NAME
skill-manager target set-path NAME PATH
```

New target paths are root-relative templates. Empty paths, absolute paths,
`~user`, and traversal outside the selected root are rejected. `target add`
creates an enabled custom target. `set-path` applies to custom targets and
legacy built-in overrides. Removing a normal built-in disables it; removing a
custom target or legacy override deletes that definition.

## Configuration commands

```text
skill-manager configs
skill-manager configs --raw
skill-manager configs reset [--yes]
skill-manager configs restore [BACKUP_ID] [--yes]
```

`configs` shows active storage, global/project roots, schema and persistence,
sources, targets and resolved locations, exclusions, unknown preserved fields,
and backups. In JSON mode it emits `config.shown`.

`configs --raw` writes exact active configuration bytes and is incompatible
with all JSON recipe/output carriers. It is a recovery tool that can read
malformed configuration.

Reset archives exact current bytes before writing an empty schema-v2 config.
Restore selects the provided backup or newest backup, snapshots displaced state,
then restores the selected bytes. Interactive confirmation accepts exact
lowercase `yes`; non-interactive execution needs `--yes`.

## Hidden generation commands

```text
skill-manager generate-completions --shell bash|zsh|fish|powershell
skill-manager generate-man --output FILE
```

Generation commands are argv-only and do not accept recipes. Do not combine
them with `--json`.

## Skill selection and patterns

Patterns use case-folded Python `fnmatch` behavior. A positional operand
containing `*`, `?`, or `[` is a skill-name pattern for `load`, `update`,
`remove`, and `resolve`; `import` takes exactly one literal skill name. Other positional operands retain their command-specific
source, skill-directory, or collection-directory meaning.

Positional patterns are ORed, then ANDed with the OR-combined `--filter`
patterns. Each unmatched positional pattern emits a warning; matching patterns
still run. Supplying only patterns that produce no actionable skill is an error.
`copy` uses only `--filter`.

When multiple sources contain the same skill name, discovery chooses the first
source and emits collision details. Use `resolve --prefer-source` to persist a
different winner.

## JSON recipes

For agents and automation, prefer stdin so shell quoting cannot corrupt JSON:

```console
skill-manager --json-input
{"command":"load","sources":["sernst-skills"],"filters":["managing-skills"],"shared":true,"global":true,"dry_run":true}
```

Or save one object in a file:

```console
skill-manager --input recipe.json
```

The recipe command names are:

```text
load  update  import  copy  remove  status  resolve
source.add  source.remove  source.list  source.update
source.locate  source.alternate  source.swap
target.add  target.list  target.enable  target.disable
target.remove  target.set-path
configs  configs.reset  configs.restore
```

Values are strict: unknown fields, wrong types, `null`, missing required
fields, and invocation arrays fail. Repeatable fields accept one string or an
array of strings. Resolution precedence is defaults, recipe, explicit argv.
Relative locations/destinations in an input file are relative to that file;
stdin/inline recipes use CWD. Target templates are never rebased.

See the complete per-command field and type reference in
[`skills/managing-skills/references/recipes.md`](./skills/managing-skills/references/recipes.md).

## NDJSON and exit codes

Every semantic stdout line in machine mode has:

```json
{"version":1,"event":"status.row","level":"info","data":{}}
```

Parse all lines in order and also inspect the exit status:

| Exit | Meaning |
| --- | --- |
| `0` | Completed, no work, or user cancelled. |
| `1` | Operational, validation, or interaction-required failure. |
| `2` | Command-line usage error. |

Human data is stdout and human diagnostics are stderr. In JSON mode, semantic
errors are NDJSON on stdout. A partial transaction emits committed action events
before `command.failed`; do not discard them.

Important events include `skill.loaded`, `skill.updated`, `skill.copied`,
`skill.removed`, `skill.skipped`, `skill.import-planned`, `skill.imported`,
`skill.import-skipped`, `status.row`, `collision.detected`,
`collision.resolved`, source/target/config lifecycle events, `diagnostic`,
`summary`, `command.cancelled`, and `command.failed`. See
[`skills/managing-skills/references/events.md`](./skills/managing-skills/references/events.md)
for the checked inventory and payload shapes.

## Configuration and state

All manager-owned state is consolidated beneath the manager home (normally the
user home, or `SKILL_MANAGER_HOME`):

```text
~/.skill-manager/
  config.json
  cache/
  backups/
  locks/
```

On startup—including `--dry-run`—the manager may migrate recognized legacy flat
configuration, cache, and backup paths into this layout. Existing new
destinations win collisions. GitHub cache refresh can also occur during dry-run
when required for discovery; failed refresh does not fall back to mismatched
content.

Deployments are staged and journaled per skill. A later failed skill does not
undo earlier committed skills, and the next invocation recovers incomplete
work. Remote archives and local source trees reject links, special files,
unsafe paths, and other non-portable entries.

Use [`clis/skill-manager/docs/configuration.md`](./clis/skill-manager/docs/configuration.md)
for migration, backup retention, schema, template normalization, cache identity,
and filesystem-safety details.

## Safety checklist

- Preflight with `status`, `source list`, `target list`, or `configs`.
- Use explicit targets and scopes in unattended operations.
- Dry-run `load`, `update`, `import`, `copy`, and `remove`.
- Review exact removal, import, and configuration effects before `--yes`;
  `import` overwrites source content, including deleting source files.
- Remember that dry-run does not suppress startup migration or required cache
  refresh.
- Parse the full NDJSON stream and exit code.
- Use `SKILL_MANAGER_HOME` plus a temporary CWD for isolated tests.
- Do not edit `config.json` directly; use lifecycle commands and backups.
