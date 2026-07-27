# Command reference

## Common behavior

Running without a command is `status`; `ls` and `list` are aliases. `--json`
emits NDJSON and implies `--no-input`. Non-TTY human output is ANSI-free plain
text; interactive status tables use compact symbols. `--color auto|always|never`
controls color and `NO_COLOR` disables it.

| Command | Purpose |
| --- | --- |
| `load [SOURCE_OR_PATTERN...]` | Discover and deploy skills, replacing existing copies. |
| `update [SOURCE_OR_PATTERN...]` | Update skills already deployed in the selected or inferred scope. |
| `copy SOURCE DEST` | Copy discovered skills to an arbitrary destination. |
| `remove [SKILL_OR_PATTERN...]` | Remove selected or auto-detected deployments. |
| `status [FILTER...]` | Show source-relative deployment state; aliases: `ls`, `list`. |
| `resolve [SKILL_OR_PATTERN...]` | Persist a collision preference. |
| `configs [--raw]` | Display the active configuration and available backups. |
| `configs reset [--yes]` | Archive and replace the configuration with an empty v2 configuration. |
| `configs restore [BACKUP_ID] [--yes]` | Restore a selected backup, or the latest backup when omitted. |
| `source …` / `target …` | Manage source definitions and deployment targets. |

`load`, `update`, `remove`, and `status` accept built-in selectors `--claude`,
`--shared`, `--antigravity`/`--ag`, `--all`, and repeatable `--target NAME`.
Selectors form a deduplicated union; an explicit target can include a disabled
target. The built-ins resolve to `.claude/skills`, `.agents/skills`, and
`.gemini/antigravity/skills` respectively.

## Global and project scopes

Every built-in and custom target has two possible locations. `--global`/`-g`
resolves its root-relative template below the manager home (including the
`SKILL_MANAGER_HOME` override); `--project`/`-p` resolves it below the exact
current working directory. The manager does not search for a Git root or an
ancestor directory. Project deployments override global deployments for status
and effective-skill selection.

The scope flags are mutually exclusive.

| Command | Scope without an explicit flag |
| --- | --- |
| `load` | Prompts once for a scope. The default is project when an enabled selected target's leading directory exists in CWD (such as `.claude`, `.agents`, or `.gemini`); otherwise global. |
| `update` | Infers scope for every existing skill/target deployment. A project copy wins when both exist; `-g` or `-p` restricts the eligible deployments. |
| `remove` | Removes an unambiguous existing scope. When any selected skill exists in both scopes, it prompts once for project, global, or both; an explicit flag restricts it. |
| `status` | Inspects both scopes by default; `-g` or `-p` narrows the report. |

`--json`, recipe input (`--json=…`, `--json-input`, or `--input`), and
`--no-input` are non-interactive. `load` needs an explicit scope in these modes,
including for `--dry-run`; an ambiguous `remove` also needs one. The command
fails with an interaction-required error rather than guessing. `--yes` bypasses
the destructive removal confirmation only—it never chooses a scope.

All discovery commands accept repeatable `--filter PATTERN`, `--refresh`, and
`--dry-run` where applicable. Configured sources are the default. `--cd` adds
CWD for `status`; `--cd-only` uses only CWD; `--no-cd` retains the compatibility
spelling for configured-source-only behavior.

## Source and target lifecycle

`source add` accepts a local path, GitHub URL, or GitHub shorthand and optional
name, label, source mode, excludes, and cache TTL. Names, labels, and the active
location are updated atomically with `source update`; `--location LOCATION` can
be combined with metadata flags. IDs do not change when a source moves.

`source locate SOURCE LOCATION` is the location-only spelling, with aliases
`relocate`, `move`, and `mv`. `source alternate SOURCE LOCATION` saves or
replaces an inactive location, while `source alternate SOURCE --clear` removes
it. `source swap SOURCE` exchanges active and inactive locations. Local/local,
local/GitHub, and GitHub/GitHub pairs are supported, and local paths need not
exist yet. A swap requires an alternate.

Source selectors are a stable ID, name, unique label, or active location.
Inactive locations are deliberately not selectors. A newly set active or
alternate location cannot collide with any slot of another source; existing
cross-source collisions remain loadable for compatibility.

`source list` and the status source preamble use the same display-width-aware
aligned renderer. An inactive location appears immediately below its source as
`alternate (inactive)`.

`target add` creates custom targets and `target set-path` updates a custom
template. New target paths must be root-relative templates (not absolute
destinations); see [configuration](configuration.md). Built-ins are `claude`,
`shared`, and `antigravity`. `target remove` deletes custom targets and instead
disables an unoverridden built-in. Legacy built-in overrides remain explicit
legacy overrides until updated or removed.

## Skill selection and patterns

Python `fnmatch` patterns are case-folded consistently. Positional operands
containing `*`, `?`, or `[` are skill-name patterns for `load`, `update`,
`remove`, and `resolve`; literal operands preserve their command-specific
source, skill-directory, or collection-directory behavior. `copy` continues to
use only `--filter`. `status` positionals still match a skill name, source name,
or unique source label.

Positional patterns are ORed together and then ANDed with the OR-combined
repeatable `--filter` patterns. Their candidate universes are discovered winners
after exclusions for `load`; winners with an eligible existing deployment for
`update`; deployed names in the selected targets and scopes for `remove`; and
collided names for `resolve`. `status` continues to match its existing skill
name, source name, and unique source-label fields. Each unmatched positional
pattern emits a warning; matching patterns still run. If supplied positional
patterns leave no actionable skill, the command fails.

## Status values and locations

Each effective source/target pair is `up-to-date`, `needs-update`,
`not-loaded`, or `no-connection`; equality compares relative regular-file names
and SHA-256 content only. Empty directories, timestamps, and ownership do not
affect equality.

The status table has a compact aggregate **location** column:

| Interactive marker | Redirected text | Meaning |
| --- | --- | --- |
| `🌐` | `global` | Installed globally only. |
| `📁` | `project` | Installed in the current project only. |
| `↕` | `both` | Installed in both scopes; project is effective. |
| `—` | `none` | Not installed. |

`⚠` is appended when installed targets use differing non-empty scope sets.
The summary counts every location, mixed placement, and global copies whose
contents differ from the effective project copy. Per-target state still reports
the effective project deployment whenever it exists.

Human status first maps compact source names to labels and local or GitHub
locations, then uses those names in an aligned skill-by-target table. Interactive
terminals show one colored symbol per target state (`✓`, `↑`, `✗`, or `~`) and a
matching nonzero-only summary legend; custom target names widen only their own
headers. Redirected output remains ANSI-free and uses textual state/location
names. Deployed-only skills display `unknown` as their human source while the
NDJSON provenance remains unchanged. Ad hoc CWD sources use `cwd`; other
unconfigured sources use a deterministic disambiguated short name.

## Configuration commands

`skill-manager configs` gives an interactive-readable view of the storage,
global-home, and project roots; schema/persistence state; sources; target
templates and both resolved locations; exclusions; cache settings; preserved
unknown fields; and backup metadata. In JSON mode it emits `config.shown`.

`configs --raw` is CLI-only and writes the exact active configuration bytes,
even if they are malformed. It conflicts with JSON and recipe input carriers.
When there is no active configuration, it first creates the canonical empty
schema-v2 document. Human and JSON display instead report an unpersisted
default when the active configuration is absent and reject malformed
configuration.

`configs reset` and `configs restore` are destructive. Interactive use accepts
only the exact lowercase confirmation `yes`; any other response cancels without
changing state. Non-interactive use requires `--yes`. Reset first archives the
exact current bytes and then writes an empty v2 document. Restore uses the
provided backup ID or the newest backup, snapshots the displaced configuration,
and can deliberately restore a malformed backup.

See [configuration and migration](configuration.md) for storage and backup
details, and [the JSON contract](json.md) for automation.
