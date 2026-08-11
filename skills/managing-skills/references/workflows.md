# Conversational workflows

Use these patterns after reading `recipes.md` and `events.md`. Each JSON object
is a separate `skill-manager --json-input` invocation.

## Contents

- [Discover and inspect](#discover-and-inspect)
- [Install or update skills](#install-or-update-skills)
- [Import a modified deployment](#import-a-modified-deployment)
- [Remove skills](#remove-skills)
- [Manage sources](#manage-sources)
- [Manage targets](#manage-targets)
- [Resolve collisions](#resolve-collisions)
- [Recover configuration](#recover-configuration)
- [Copy and generate artifacts](#copy-and-generate-artifacts)

## Discover and inspect

On a fresh or unknown configuration, establish stored context before applying a
narrow filter:

```json
{"command":"source.list"}
{"command":"target.list"}
{"command":"status"}
```

After confirming that a relevant source exists, narrow the report:

```json
{"command":"status","filters":["requested-*"]}
```

Use `refresh:true` only when the user asks for current remote content or stale
cache is material to the task. Use `cd:true` to add CWD temporarily or
`cd_only:true` to ignore configured sources. Report source collisions before
mutating.

If a narrow status was attempted before source context was established, parse
its `command.failed` event. Continue with an explicitly requested source-add or
install only when the message specifically reports that the requested pattern
matched nothing/no actionable skill and `source.list` confirms the relevant
source is absent. Record that result as expected absence, not successful status.
Treat every other exit-1 message as blocking.

## Install or update skills

For a clear global shared installation:

```json
{"command":"status","filters":["example"],"shared":true,"global":true}
{"command":"load","sources":["team"],"filters":["example"],"shared":true,"global":true,"dry_run":true}
{"command":"load","sources":["team"],"filters":["example"],"shared":true,"global":true}
{"command":"status","filters":["example"],"shared":true,"global":true}
```

Execute the non-dry-run load after a clean plan without a second user turn.
If a name could identify either a source or skill and preflight does not resolve
it, ask which source/skill the user means.

Use `update` when the user wants only already-deployed skills refreshed. An
unscoped update can infer every existing placement:

```json
{"command":"update","sources":["team"],"filters":["example"],"all_targets":true,"dry_run":true}
{"command":"update","sources":["team"],"filters":["example"],"all_targets":true}
{"command":"status","filters":["example"],"all_targets":true}
```

Never turn an update request into a load of a new deployment.

## Import a modified deployment

Use `import` when the user edited a deployed skill in place and wants that copy
to become the canonical source content. It overwrites the source in full, so
dry-run first, show the reported file and line deltas, and obtain an explicit
second confirmation before the committed call. `import` has two decisions:
which deployed copy to adopt (only ambiguous when more than one still differs
from the source after target/scope selection) and whether to propagate the
imported content to every other deployment afterward — set `update:true`
(import + update, recommended) or `no_update:true`/`update:false`; neither is
implied by `yes:true`. Propagation resolves silently, with no flag needed,
whenever the copy adopted would leave nothing else out of date:

```json
{"command":"status","filters":["example"]}
{"command":"import","skill":"example","claude":true,"global":true,"dry_run":true}
{"command":"import","skill":"example","claude":true,"global":true,"update":true,"yes":true}
```

Read `files_changed`, `insertions`, `deletions`, `deployment`, and `destination`
from the `plan` event's per-option `consequence` when reporting the plan, and
from `skill.imported` after a committed apply. `skill.import-skipped` means
every selected deployment already matches the source, which is a clean
success. An `InteractionRequired` failure means either several deployments
still differ and target/scope fields did not narrow them to one, or the
source is GitHub-backed with no configured local alternate location — add one
with `source.alternate` first; once configured, import proceeds
non-interactively like any other source.

## Remove skills

Preflight, dry-run, show exact target/scope paths, then obtain explicit
confirmation:

```json
{"command":"remove","skills":["example"],"shared":true,"project":true,"dry_run":true}
{"command":"remove","skills":["example"],"shared":true,"project":true,"yes":true}
{"command":"status","filters":["example"],"shared":true,"project":true}
```

For both scopes, never omit scope and hope the CLI chooses. Use two calls:

```json
{"command":"remove","skills":["example"],"shared":true,"global":true,"dry_run":true}
{"command":"remove","skills":["example"],"shared":true,"project":true,"dry_run":true}
```

After one confirmation covering the combined plans:

```json
{"command":"remove","skills":["example"],"shared":true,"global":true,"yes":true}
{"command":"remove","skills":["example"],"shared":true,"project":true,"yes":true}
{"command":"status","filters":["example"],"shared":true}
```

If the first execution succeeds and the second fails, report the global removal
as committed and the project failure; do not claim atomicity across calls.

## Manage sources

Before adding, list sources and compare normalized active identities. Reuse a
source whose active location is already the requested remote, even if named
differently. If the requested name belongs to another location, stop for user
direction.

```json
{"command":"source.list"}
{"command":"source.add","source":"owner/repo/skills","name":"team","label":"Team skills"}
```

For metadata/location changes, preflight and then use the narrowest command:

```json
{"command":"source.update","source":"team","label":"Platform skills"}
{"command":"source.locate","source":"team","location":"../skills"}
{"command":"source.alternate","source":"team","location":"owner/repo/skills"}
{"command":"source.swap","source":"team"}
```

Ask before `source.remove` after showing the exact stable source and noting that
deployed files are not automatically removed.

## Manage targets

List targets and resolved paths first. Target `path` is a root-relative
template, not an arbitrary absolute destination:

```json
{"command":"target.list"}
{"command":"target.add","name":"custom","path":".custom-agent/skills"}
{"command":"target.enable","name":"custom"}
```

Require confirmation before disable, remove, or set-path. Explain that removing
an unoverridden built-in disables it and that changing a template can leave
deployments in the old resolved directory.

## Resolve collisions

Use status/discovery to show candidates and provenance. Ask the user to choose
unless their request names the desired winner exactly:

```json
{"command":"resolve","skills":["duplicate-name"],"prefer_source":"team"}
{"command":"status","filters":["duplicate-name"]}
```

Resolution persists exclusions on losing configured sources. It cannot persist
an exclusion for a temporary source.

## Recover configuration

Use structured `configs` to show active state and backup IDs:

```json
{"command":"configs"}
```

Use direct `skill-manager configs --raw` only when exact active bytes are
needed. It can return malformed non-UTF-8-compatible content and cannot be
combined with JSON.

For reset or restore, show the selected backup/effect and obtain explicit
confirmation before:

```json
{"command":"configs.reset","yes":true}
{"command":"configs.restore","backup":"BACKUP_ID","yes":true}
```

Restore without `backup` selects the latest. Never guess that the latest is the
one the user intended when multiple backups are plausible.

## Copy and generate artifacts

Copy uses an arbitrary destination but should still be previewed:

```json
{"command":"copy","source":"team","destination":"./vendor/skills","filters":["example"],"dry_run":true}
{"command":"copy","source":"team","destination":"./vendor/skills","filters":["example"]}
```

Completion and man generation are CLI-only and write to user-authorized
destinations:

```console
skill-manager generate-completions --shell powershell
skill-manager generate-man --output ./skill-manager.1
```
