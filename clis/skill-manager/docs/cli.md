# Command reference

## Common behavior

Running without a command is `status`; `ls` and `list` are aliases. `--json`
emits NDJSON and implies `--no-input`. `--verbose` adds advanced human details
and full import paths without changing JSON. `--color auto` colors only a TTY
and honors `NO_COLOR`; `always` colors even redirected output; `never` is plain.
`--home DIR` overrides the manager home for the whole invocation, ahead of
`SKILL_MANAGER_HOME` and the operating system home; a relative value (and a
relative `SKILL_MANAGER_HOME`) is normalized to an absolute, lexically clean
path against the current directory before any derived path is built. See
[Global and project scopes](#global-and-project-scopes) below.

| Command | Purpose |
| --- | --- |
| `load [SOURCE_OR_SKILL_OR_PATTERN...]` | Discover and deploy skills, replacing existing copies, after one plan confirmation; alias: `install`. |
| `update [SOURCE_OR_SKILL_OR_PATTERN...]` | Update skills already deployed in the selected or inferred scope, after one plan confirmation; alias: `up`. |
| `import SKILL` | Adopt one deployed, possibly agent-modified copy as the new source content. |
| `copy SOURCE DEST` | Copy discovered skills to an arbitrary destination, after one plan confirmation. |
| `remove [SKILL_OR_PATTERN...]` | Remove selected or auto-detected deployments. |
| `status [FILTER...]` | Show source-relative deployment state; aliases: `ls`, `list`. |
| `describe [SELECTOR...]` | Explain matching skills and sources, including bounded source documentation. |
| `describe skill [SELECTOR...]` | Explain only skills. |
| `describe source [SELECTOR...]` | Explain only sources. |
| `resolve [SKILL_OR_PATTERN...]` | Persist a collision preference. |
| `configs [--raw]` | Display the active configuration and available backups. |
| `configs reset [--yes]` | Archive and replace the configuration with an empty v2 configuration. |
| `configs restore [BACKUP_ID] [--yes]` | Restore a selected backup, or the latest backup when omitted. |
| `configs copy FROM TO [--include-cache] [--dry-run] [--yes]` | Seed a destination manager home (configuration plus resolved target directories) from an existing one, merging by path and deleting nothing. |
| `source …` / `target …` | Manage source definitions and deployment targets. |

`load`, `update`, `import`, `remove`, and `status` accept built-in selectors
`--claude`, `--shared`, `--antigravity`/`--ag`, `--all`, and repeatable
`--target NAME`.
Selectors form a deduplicated union; an explicit target can include a disabled
target. The built-ins resolve to `.claude/skills`, `.agents/skills`, and
`.gemini/antigravity/skills` respectively.

## Global and project scopes

Every built-in and custom target has two possible locations. `--global`/`-g`
resolves its root-relative template below the manager home (`--home`, then
`SKILL_MANAGER_HOME`, then the OS home, in that order); `--project`/`-p`
resolves it below the exact current working directory. The manager does not
search for a Git root or an ancestor directory. Project deployments override
global deployments for status and effective-skill selection.

The scope flags are mutually exclusive.

When the physical, normalized CWD is the manager home, project scope is
unavailable: unscoped commands inspect only global deployments, `load` defaults
to global, and `configs` explains the unavailable project. Explicit `--project`
fails before any write and directs the user to `--global` or a project directory.
A directory below home remains a normal project.

| Command | Scope without an explicit flag |
| --- | --- |
| `load` | Infers project-vs-global scope silently: project when an enabled selected target's leading directory exists in CWD (such as `.claude`, `.agents`, or `.gemini`); otherwise global. The inferred scope is shown in the plan and, if the plan is later cancelled, named in the cancel hint. |
| `update` | Infers every existing skill/target deployment, including both scopes when both exist; `-g` or `-p` restricts the eligible deployments. |
| `import` | Scans both scopes for changed deployments. With more than one candidate, source copy is a genuine prompt dimension; `-g` or `-p` narrows the scan and can reduce it to one. |
| `remove` | Removes an unambiguous existing scope. When any selected skill exists in both scopes, its plan presents mutually exclusive project/global/both options, each with its own blast radius, resolved by a single numbered selection (or noninteractively by `--global`/`--project`/`--both`). |
| `status` | Inspects both scopes by default; `-g` or `-p` narrows the report. |

`--json`, recipe input (`--json=…`, `--json-input`, or `--input`), and
`--no-input` are non-interactive. In these modes, `load` infers its scope and
target selection the same way an interactive run would rather than requiring
either to be stated explicitly. Applying the rendered plan is authorized
differently depending on which non-interactive mode is active: `--json` and
every recipe carrier auto-authorize `load`, `update`, and `copy` once the plan
has rendered, so `--yes`/`yes:true` is accepted but not required for those
three commands; plain `--no-input` (with no JSON carrier) still requires an
explicit `--yes` to apply. `remove` and `import` never auto-authorize in any
non-interactive mode—they always require an explicit `--yes`/`yes:true` to
commit, because both are destructive. An ambiguous `remove` still needs an
explicit scope (`--global`, `--project`, or `--both`) in these modes and fails
with an interaction-required error rather than guessing. At home, the only
available scope is global. `--yes` bypasses `remove`'s destructive
confirmation only—it never chooses a scope; `--both` is the only flag that
resolves the scope branch itself, applying to both scopes at once without an
interactive selection.

All discovery commands accept repeatable `--filter PATTERN`, `--refresh`, and
`--dry-run` where applicable. Configured sources are the default. `--cd` adds
CWD for `status`; `--cd-only` uses only CWD; `--no-cd` retains the compatibility
spelling for configured-source-only behavior.

## Change plans for load, update, copy, remove, and import

Interactive `load` and `update` immediately use all enabled targets unless
selectors narrow them; `load` also infers its scope (see above). `load`,
`update`, `copy`, and `remove` all always render a complete plan before
asking anything—no command in this family ever asks a pre-emptive question
before showing what it will do.

`update`'s plan omits unchanged skills. `load`'s plan distinguishes new
installs from overwrites: a deployment that does not yet exist is `load`, one
that exists with different content is `update`, and a destination already
byte-identical to the source is hidden from the table entirely and counted
only in the footer's `already identical` clause. `copy` has no such concept—it
has no prior "existing deployment" to compare against ahead of the diff—so a
byte-identical destination still appears as an ordinary overwrite row with
`no file changes`.

The plan groups each row—one row per changed skill for `update`/`load`, one
row per copied skill for `copy`—with a column per destination target (for
`load`/`update`) or, for `copy`, one destination shown once in the metadata
above the table, since every row shares that same single arbitrary path.
Rows appear in the order the skills were named on the command line (or, for
`copy`, in discovery order, since `copy` takes no positional skill filter), so
review order matches request order, and apply follows the same order the plan
promised. Every render is significance gated: a target column whose every cell
would be the none value is dropped, an inferred scope shared by every planned
deployment is hoisted to a single `Scope` line instead of being repeated per
cell, an explicitly stated scope is not restated at all, and zero counts are
omitted rather than printed. When one skill has exactly one planned
deployment, the table degrades to a single sentence—this is also `copy`'s
usual rendering, since it deploys to exactly one destination. When
deployments have different file/line totals, compact destination-specific
details preserve every delta below the grouped row. Interactive output uses
the symbol vocabulary (`↑`, `↕ both`, `🌐 global`, `📁 project`, `—`, `✓`,
`+`); a redirected stream uses the equivalent words.

The plan ends with a count summary naming its blast radius, such as `4 updates
across 2 targets` or `6 changes across 3 enabled targets: 6 new`. The
qualifier (`enabled` or `selected`) survives only while every target does;
once gating drops one, the phrase degrades to a bare count so it never
overstates what will be written.

`remove`'s plan is the same shape, but with a genuine branch point when any
selected skill exists in both global and project scope and the user did not
say which: the table's cells show pure availability (where a deployment
*exists*, never an action token) and the plan presents three mutually
exclusive, numbered alternatives—project, global, or both—each annotated with
its own blast radius (`− N deployments, N files`). A single-token selection
resolves it (`Select removal scope [1-3, c to cancel]:`); `c` cancels, and
invalid or empty input reprompts and never auto-selects, because every option
is destructive and there is no preselected default. `--both` (remove-only)
resolves the branch noninteractively without an interactive selection. When
scope is already unambiguous—stated explicitly with `--global`/`--project`,
inferred to a single existing scope, or resolved by `--both`—there is no
branch: the plan collapses to a plain action table and its footer reads `{N}
deployment removals across {target label}: − {N} remove; {N} skill(s), {N}
files`, exactly like `update`'s and `load`'s footers.

One confirmation follows, defaulting to yes for `load` and `copy` (both are
constructive/regenerable) as well as for `update`, but to **no** for a
resolved `remove` plan (destructive, `[y/N]`); an unresolved `remove` branch
is a numbered selection instead of a yes/no question, so it has no default at
all. Declining cancels with exit `0`, emits `command.cancelled`, deploys
nothing, and prints a hint naming only the decisions the CLI inferred—target
and/or scope for `load`, targets and deployed scopes for `update` and
`remove`—so the next invocation is obvious. Cancelling `remove`'s branch
selection prints no hint: the user just made an explicit, real-time choice,
so there is nothing left to teach. `copy` never infers anything (source,
destination, and filters are always stated explicitly), so declining its plan
has no hint to print. A no-op prints one precise result without a table or
confirmation: for `update`, that the skill is up to date or not deployed to
any target in the searched scope; for `load`, that every requested skill is
already identical across the selected or enabled targets; for `copy`, that no
skills matched the filter or were found in the source; for `remove`, that the
named skill is not deployed to any selected target in either scope.

`--yes`/`-y` (added to `load` and `copy` alongside `update`'s and `remove`'s
existing flag) renders the same plan and then applies it. `--dry-run` renders
the plan and stops with a single `Dry run — no changes were made.` conclusion
rather than echoing every item—for `remove`'s unresolved branch, `--dry-run`
enumerates all three alternatives and their blast radius but never offers to
cancel, since there is nothing to apply or decline. Applying prints one
progress line per deployment or copied skill—with the scope only when the
plan has more than one, though `load`'s progress lines never carry a scope
suffix, since `load` decides its scope once for the whole run—followed by a
summary footer such as `completed: 4 deployments updated` or `completed: 6
deployments changed (6 loaded)`, whose zero categories are omitted entirely.
Under plain `--no-input` (no JSON carrier), inferred defaults still apply but
`--yes` is required to authorize the write; `--json` and every recipe carrier
auto-authorize `load`, `update`, and `copy` instead, as described above—
`remove` never auto-authorizes in any non-interactive mode, and an ambiguous
scope still requires an explicit `--global`, `--project`, or `--both` rather
than accepting `--yes` alone to guess one. Machine mode keeps its existing
event-only contract while additionally emitting the structured `plan` event,
at revision `0`, described in [json.md](json.md), for all of `load`,
`update`, `copy`, `remove`, and `import`.

`import SKILL` reverses `load`. It resolves exactly one skill—patterns are
rejected—through the same first-source-wins discovery, then scans the selected
or enabled targets in both scopes for deployed copies whose content differs from
the source. Identical copies are never candidates, and finding none succeeds
with `skill.import-skipped`.

Import has two decision dimensions, and the complete plan for both always
renders before any prompt. The first dimension is which deployed copy to
adopt: with more than one candidate, the plan numbers each one with its path,
its diff against the current source, and a nested per-copy preview of what
propagating that copy would do to every other deployment, under a deferred
`Propagation modes (chosen after the source copy)` heading that is visible
but not yet asked; a non-prompting render of the same list (`--dry-run`, an
ambiguous `--yes`) carries the equivalent deferred heading `Source copies
(chosen first)` instead of vanishing unlabeled. `Select source copy [1-N, c
to cancel]:` resolves it with one token; `c` cancels, and invalid or empty
input reprompts without ever auto-selecting. With exactly one candidate, or
when every candidate is byte-identical to the others, the dimension resolves
silently in configured order — adopting any of them would produce the same
source and the same propagation, so there is no genuine choice to force.
Answering a nonfinal prompt re-renders the plan narrowed by that answer: the
resolved dimension, every non-selected copy, and their diffs and previews
disappear, and the chosen copy demotes to ordinary `From`/`Path` metadata.

The second dimension is propagation mode: `1 Import + update (recommended)`
imports the chosen copy and then updates every other deployment to match it;
`2 Import only` imports without touching the rest. `--update`/`--no-update`
resolve it noninteractively; neither is implied by `--yes`, because import is
destructive and propagation is a second, independent commitment. Propagation
is genuine only when the resolved source copy would actually leave at least
one other deployment out of date; when it would not — a single deployment,
or every deployment already matching that copy — the dimension resolves
silently with no prompt, no flag, and no rendered decision, so a plan that
never had a real second question to ask never asks one. This is always the
last prompt when it is genuinely asked, so its answer applies immediately
with no trailing `[y/N]`. Applying makes the source directory byte-identical
to the chosen deployed copy, including deleting source files the deployment
no longer has, using the same staged, journaled, locked transaction as a
deployment, then performs the resolved propagation. `--dry-run` renders the
complete plan for both dimensions and stops with a single conclusion; no
per-item echoes.

`--yes` renders the plan and then applies it, but only once every dimension is
resolved by flags—`import`, like `remove`, never auto-authorizes in any
non-interactive mode, so `--json`, every recipe carrier, and plain
`--no-input` all require an explicit `--yes`/`yes:true`, and a genuine
multi-copy ambiguity that flags do not resolve fails with an
interaction-required error rather than guessing. Normal output uses source and
`target · scope` names; `--verbose` adds full paths.

Import writes to local source checkouts only. When the resolved source is
GitHub-backed, it requires a configured local alternate location; without one
it fails with an actionable error naming the missing configuration instead of
guessing or prompting, and that check only runs once a changed candidate is
found, so a source that is already up to date never fails over a destination
it would not write. Once an alternate is configured, import proceeds exactly
like any other source, with the alternate as the ordinary `Into` destination.

## Source and target lifecycle

`source add` accepts a local path, GitHub URL, or GitHub shorthand and optional
name, label, source mode, excludes, and cache TTL. `target add` likewise takes
a root-relative target path and a name. With both location/path and name given
as two positionals, either order is accepted. For `source add`, an explicit
GitHub URL or valid `owner/repo` shorthand is the location. Explicit local
spellings (`./`, `../`, `.\`, `..\`, rooted/absolute paths, and supported
`~` forms) are always local, never shorthand. Otherwise the command checks
both operands as folders: exactly one existing folder is the location. If both
or neither are folders, the roles are ambiguous and interactive use renders a
two-mapping plan (plus cancel) with no default; it never falls back to legacy
ordering. Identical operands are allowed, using one as each role. In
`--yes`, `--no-input`, or any JSON mode ambiguity fails before mutation; use
the unambiguous `LOCATION --name NAME` form. Recipes always use explicit
`source`/`path` and `name` fields and never infer or prompt. Names, labels,
and the active location are updated atomically with `source update`;
`--location LOCATION` can be combined with metadata flags. IDs do not change
when a source moves.

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

## Describe skills and sources

`describe` is a read-only, resolver-aware inspection command. With no
positional selector, no selector flag, and no type-only flag, it shows help.
`describe skill` and `describe source` are equivalent to `describe --skills`
and `describe --sources`, respectively, and expose only flags that apply to
their type.

Positional selectors are case-folded fnmatch patterns. An unqualified pattern
matches the effective resolver-visible skill set; `describe teach` therefore
shows the winning `teach`. Prefix a selector with `SOURCE:` to inspect a
specific physical source copy, including a copy that is excluded or shadowed:
`describe personal:slack-to-todoist` and `describe personal:*`. Qualified
results label their resolver status and, when applicable, the exclusion reason
or winning source. If a positional selector matches no skill, it is tried as a
source selector, so `describe personal` describes source `personal`.

The following selectors add or narrow candidates:

| Selector | Behavior |
| --- | --- |
| `--all` | All effective skills and all configured sources. |
| `--all-skills` / `--all-sources` | All effective skills / all configured sources. |
| `--skills` / `--sources` | Final type restriction; with no other selectors, all effective skills / all sources. They are mutually exclusive. |
| `--source SOURCE` | Restricts positional skill selectors to one or more sources. With no positional selector it means `SOURCE:*`; multiple values widen that source scope only. |
| `--installed`, `--loaded` | Skills deployed to at least one configured target. |
| `--outdated` | Skills with at least one deployed copy needing an update. |
| `--not-installed`, `--available` | Skills deployed to no configured target. |

State flags OR together, then intersect the selected skills; an outdated skill
can therefore match both `--installed` and `--outdated`. Used without another
skill selector, state flags begin with all effective skills. `--source` is a
narrowing scope, not an additional union: `describe grill-me --source personal
--installed` selects installed `personal:grill-me`, while `describe --source
personal --installed` selects every installed skill in `personal`. Sources do
not have deployment state, so source-only invocations do not expose state or
source-scope flags.

Selector families that produce at least one result succeed and warn for each
unmatched selector. If no final result remains — including after state/type
filters — the command fails. Remote sources may be materialized through the
normal cache to inspect them, but `describe` never refreshes a materialized
source or changes configuration, deployments, exclusions, or target contents.

Human output separates every result after the first, then gives a colored
title and structured content. A skill result shows trigger text from its
`SKILL.md` frontmatter plus up to 100 lines of `README.md`, or the first 20
raw `SKILL.md` lines when no README exists. A source result shows line-by-line
configuration (IDs, labels, names, and locations), the same optional README
excerpt, and each available skill with trigger text. Excerpts preserve source
text and show a truncation notice when needed.

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

An empty pattern set never means "match every candidate": a literal skill
name given to `remove` or `resolve` resolves to only that name, plus any
genuine fnmatch pattern operands given alongside it. For `load`, `update`,
and `remove`, positional operands (literal names and patterns) union
together first, and the repeatable `--filter` patterns then intersect that
union — a name that unions in but matches no `--filter` pattern is dropped.
`resolve` has no `--filter` flag. Each command's "everything" fallback is
keyed on the absence of skill-name/pattern selectors, not on an empty operand
list: `load` and `update` also accept source references and paths as
operands, and supplying only those (for example `load some-source`) still
triggers the fallback, because a source operand narrows the discovery
universe rather than suppressing the fallback — the command then selects
every winner discovered from that narrowed set of sources. `remove` and
`resolve` operands are always skill selectors (a name, pattern, or
skill/collection path for `remove`; a name or pattern for `resolve`), so for
those two the fallback fires only when the operand list is fully empty: bare
`remove` falls back to discovered source winners (still narrowed by
`--filter`) rather than to every existing deployment, so a deployment whose
skill is no longer present in any configured source is left untouched by a
bare `remove`; bare `resolve` falls back to every unresolved collision. Each
command opts into its own fallback explicitly rather than treating an empty
selector set as "select everything".

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

### `configs copy`

`configs copy FROM TO` seeds a destination manager home from an existing one —
typically to make a scratch `--home` directory (see
[Global and project scopes](#global-and-project-scopes)) resemble a real one
for smoke testing. It copies the manager configuration
(`.skill-manager/config.json` and any other files or folders under
`.skill-manager/`) plus every resolved target skill directory that actually
exists under `FROM`. Directories that do not exist under `FROM` are skipped
rather than created empty.

Which target directories are resolved is decided by, in order: `FROM`'s own
`.skill-manager/config.json` when it is present and already at the current
schema; otherwise the active `--home` configuration (persisted or defaulted);
otherwise the built-in defaults (`.claude/skills`, `.agents/skills`, and
`.gemini/antigravity/skills`). Both `FROM` and the active `--home` are read
directly from disk for this check and are never opened through the
configuration repository, so neither is migrated, backed up, locked, or
otherwise written — not even when `FROM` *is* the active home (the canonical
`configs copy ~ ./tmp/scratch` shape), and not even under `--dry-run`, which
changes nothing anywhere. A `FROM` configuration that is present but
unreadable, not valid JSON, or on an unsupported schema is a hard error that
names the offending file, rather than a silent fall-through that would copy
those bytes while quietly resolving custom targets from somewhere else. Its
custom target paths are lexically normalized the same way the repository
normalizes them on load; a target path that is absolute or escapes `FROM` via
`..` is a hard error naming the offending target, so a source configuration can
never make the copy read outside `FROM` or — through the destination join —
write outside `TO`.

The regenerable `cache`, `backups`, and `locks` directories under
`.skill-manager/` are excluded by default, since they can be large and are
rebuilt automatically; pass `--include-cache` to copy them too. This exclusion
is global and is applied to the *normalized* target path: a resolved target
directory that itself points inside one of those reserved locations is dropped
as well, so a target configured at, say, `.skill-manager/cache` — or an
obfuscated spelling such as `.skill-manager/x/../cache` — cannot smuggle
excluded bytes past the filter. A configured source ROOT that is a symlink or
reparse point (including a Windows junction) is never descended, because
following it could pull content from outside `FROM` into `TO`. This is checked
with `symlink_metadata` (never a link-following `is_dir()`) on every source
root the copy would open — the `.skill-manager` configuration root (before its
`config.json` is ever read, which must itself be a regular file, not a link),
the `.skill-manager` directory copied as an item, and every resolved target
root — and is re-checked immediately before each write at apply time so a link
planted after preflight is still caught. A linked source root is not silently
dropped: it is reported as an explicit `skipped (linked source)` row in the
plan and result output so the seed is never quietly incomplete. (A linked path
found *inside* an already-descendable tree is skipped like any other special
entry.) Destination-side links remain a hard error rather than a skip, because
a copy must never write through a link out of `TO`. Empty directories are
copied like any other: a folder that exists under `FROM` but is missing at
`TO` counts as work and is recreated, honoring the "copies folders, merges
paths, deletes nothing" contract even when the folder holds no files.

Copying is a merge, never a mirror: existing files at `TO` are overwritten
only where `FROM` has a same-path file, and nothing already present at `TO`
that is not part of the copy is ever deleted. `TO` is created if it does not
yet exist. `FROM` and `TO` support the same `~` expansion as other path
arguments (resolved against the active `--home`) and may be relative to the
current directory.

`FROM` must exist and be a directory; a `TO` that exists as a file is
rejected. Because the copy touches only `FROM`'s `.skill-manager` directory
plus each resolved target root — not all of `FROM` — a `TO` that merely lives
somewhere under `FROM` is fully supported; this is the canonical `configs copy
~ ./temp/smoke-testing/` shape, where the scratch destination naturally sits
inside the home. Only a genuine recursion or self-overwrite hazard is
rejected, naming the specific colliding source directory: `TO` being the same
directory as `FROM`, `TO` lying inside a source root that is actually copied
(the configuration root or a resolved target root), or such a source root
lying inside `TO`. It is also an error for `FROM` to have neither a
configuration nor any existing resolved target directory to copy.

Every destination path — and every ancestor of it within `TO` — is checked
before the plan is rendered or anything is written. A symlink or other reparse
point met there (including a Windows directory junction) is rejected, so a
planted link cannot redirect a write to a file outside `TO`. Every incoming
entry is classified by kind, so a conflict in either direction — an incoming
directory over an existing destination file, or an incoming file over an
existing destination directory — is caught here too. Because that preflight
runs before the plan, the plan can never promise a seed that would then only
partially apply: an unsafe destination fails cleanly with an actionable error
and nothing on disk changes. The same link/ancestor rejection is re-run
immediately before each item is written at apply time, so an ancestor swapped
for a link after preflight (for example while a confirmation prompt is waiting)
is still caught; this shrinks the window to "checked immediately before the
write" and is not a per-handle TOCTOU guarantee.

Like every other command, `configs copy` narrates what it discovered, then
renders one consolidated plan of every directory it would create or merge
before its one confirmation; `--yes` skips the prompt without skipping the
plan, `--dry-run` renders the plan and applies nothing, and `--no-input`
either auto-approves under `--json` or otherwise fails closed. Like its
sibling commands, `configs copy` accepts `--json-input`/`--input`/
`--json=OBJECT` recipes, with fields `from`, `to`, `include_cache`,
`dry_run`, and `yes` mirroring the CLI flags; a relative `from`/`to` in a
recipe file is rebased against that file's directory the same way `copy`'s
`source`/`destination` are, while a bare `~` or an absolute path passes
through unchanged so it still expands against the active `--home`. See
[the JSON contract](json.md#configs-copy-events) for its `plan` payload shape
and its `configs.copy.item`/`summary` events.

See [configuration and migration](configuration.md) for storage and backup
details, and [the JSON contract](json.md) for automation.
