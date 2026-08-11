# Command reference

## Common behavior

Running without a command is `status`; `ls` and `list` are aliases. `--json`
emits NDJSON and implies `--no-input`. `--verbose` adds advanced human details
and full import paths without changing JSON. `--color auto` colors only a TTY
and honors `NO_COLOR`; `always` colors even redirected output; `never` is plain.

| Command | Purpose |
| --- | --- |
| `load [SOURCE_OR_SKILL_OR_PATTERN...]` | Discover and deploy skills, replacing existing copies; alias: `install`. |
| `update [SOURCE_OR_SKILL_OR_PATTERN...]` | Update skills already deployed in the selected or inferred scope, after one plan confirmation; alias: `up`. |
| `import SKILL` | Adopt one deployed, possibly agent-modified copy as the new source content. |
| `copy SOURCE DEST` | Copy discovered skills to an arbitrary destination. |
| `remove [SKILL_OR_PATTERN...]` | Remove selected or auto-detected deployments. |
| `status [FILTER...]` | Show source-relative deployment state; aliases: `ls`, `list`. |
| `resolve [SKILL_OR_PATTERN...]` | Persist a collision preference. |
| `configs [--raw]` | Display the active configuration and available backups. |
| `configs reset [--yes]` | Archive and replace the configuration with an empty v2 configuration. |
| `configs restore [BACKUP_ID] [--yes]` | Restore a selected backup, or the latest backup when omitted. |
| `source …` / `target …` | Manage source definitions and deployment targets. |

`load`, `update`, `import`, `remove`, and `status` accept built-in selectors
`--claude`, `--shared`, `--antigravity`/`--ag`, `--all`, and repeatable
`--target NAME`.
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

When the physical, normalized CWD is the manager home, project scope is
unavailable: unscoped commands inspect only global deployments, `load` defaults
to global, and `configs` explains the unavailable project. Explicit `--project`
fails before any write and directs the user to `--global` or a project directory.
A directory below home remains a normal project.

| Command | Scope without an explicit flag |
| --- | --- |
| `load` | Prompts once for a scope. The default is project when an enabled selected target's leading directory exists in CWD (such as `.claude`, `.agents`, or `.gemini`); otherwise global. |
| `update` | Infers every existing skill/target deployment, including both scopes when both exist; `-g` or `-p` restricts the eligible deployments. |
| `import` | Scans both scopes for changed deployments. When more than one differs from the source, it prompts for the copy to import; `-g` or `-p` restricts the scan. |
| `remove` | Removes an unambiguous existing scope. When any selected skill exists in both scopes, it prompts once for project, global, or both; an explicit flag restricts it. |
| `status` | Inspects both scopes by default; `-g` or `-p` narrows the report. |

`--json`, recipe input (`--json=…`, `--json-input`, or `--input`), and
`--no-input` are non-interactive. Outside the manager home, `load` needs an
explicit scope in these modes, including for `--dry-run`; an ambiguous `remove`
also needs one. At home, the only available scope is global. The command
fails with an interaction-required error rather than guessing. `--yes` bypasses
the destructive removal confirmation only—it never chooses a scope.

All discovery commands accept repeatable `--filter PATTERN`, `--refresh`, and
`--dry-run` where applicable. Configured sources are the default. `--cd` adds
CWD for `status`; `--cd-only` uses only CWD; `--no-cd` retains the compatibility
spelling for configured-source-only behavior.

## Change plans for update and import

Interactive `update` immediately uses all enabled targets unless selectors
narrow them. Its plan omits unchanged skills and groups each changed skill into
one row with a column per selected target (all enabled targets when selectors
are omitted); cells identify `global`, `project`, `both`, or no action. When
deployments have different file/line totals, compact target-specific details
preserve every delta below the grouped row. A no-op prints one concise result
without a table or confirmation. Changed plans end with a count summary and one
confirmation defaulting to yes. Declining cancels with exit `0`, emits
`command.cancelled`, and deploys nothing. `--yes`/`-y` and `--dry-run` print the
same plan without prompting. `load` never adds this confirmation, and machine
mode keeps its existing event-only contract.

`import SKILL` reverses `load`. It resolves exactly one skill—patterns are
rejected—through the same first-source-wins discovery, then scans the selected
or enabled targets in both scopes for deployed copies whose content differs from
the source. Identical copies are never candidates, and finding none succeeds
with `skill.import-skipped`. A single candidate is preselected; several require
an interactive choice or narrower target and scope flags.

The import plan names the `from` deployment and the `to` source directory, then
lists each added, modified, or deleted file with `+N/-N` line counts and a byte
delta for binary content, plus a totals line. Its confirmation defaults to no
because the write is destructive to the source. Applying it makes the source
directory byte-identical to the chosen deployed copy, including deleting source
files the deployment no longer has, using the same staged, journaled, locked
transaction as a deployment. `--dry-run` renders the plan and writes nothing.

After a real interactive import, if another enabled, installed deployment is
now outdated—including another scope of the same target—the CLI defaults to
offering a review. Accepting shows the standard changed-only update plan and a
second default-yes confirmation before synchronizing. The invitation is absent
when nothing remains and is skipped for JSON, `--no-input`, and `--dry-run`.
Declining that optional second confirmation leaves the successful source import
in place and explicitly reports that the other deployments were not updated.
Import `--yes` approves only the source replacement and never silently fans out.
Normal output uses source and `target · scope` names; `--verbose` adds full paths.

Import writes to local source checkouts only. When the resolved source is
GitHub-backed, it requires a configured local alternate location and an
interactive confirmation, and it imports into that alternate.
Without a resolvable local destination—including in non-interactive mode—it
fails with an actionable error instead of guessing. That destination is resolved
only after a candidate is found, so a source that is already up to date never
prompts for, or fails over, a destination it would not write.

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
`remove`, and `resolve`. `copy` continues to use only `--filter`. `status`
positionals still match a skill name, source name, or unique source label.

For `load`/`install` and `update`/`up` (which share the same operand
handling), each non-pattern positional operand is resolved in order:

1. A configured source (by id, name, active location, or unique label).
2. A path-shaped or GitHub-ref-shaped reference — absolute, `~`-prefixed,
   `./`/`../` (or `.\`/`..\`) prefixed, or containing a path separator (so
   `owner/repo` GitHub references resolve here).
3. Case-insensitively, a discovered skill name — the fix for the reported
   bug where a bare skill name run from any working directory used to be
   misread as a directory path.
4. An existing directory under the current working directory, preserving
   the historical bare-relative-directory behavior.

If a bare word matches **both** a discovered skill name and a same-named
directory under the current working directory, the skill wins and the
command emits a warning naming the ambiguity and pointing at `./name` to
force the directory interpretation instead. A bare word that matches none of
the above is a hard error: `no configured source, directory, or skill named
"NAME"`, with a hint to run `skill-manager ls`; this is deliberately
stricter than an unmatched glob pattern, which only warns.

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

`skill-manager configs` gives a beginner-oriented summary followed by aligned
Sources, Targets, and Backups sections. It explains storage and scope roots,
including an unavailable project at home, while hiding internal IDs and schema
mechanics. `--verbose` adds IDs, target templates, alternate locations,
overrides, exclusions, and extension fields. In JSON mode it emits the unchanged
`config.shown` contract.

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
