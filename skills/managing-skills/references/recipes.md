# Recipe reference

Use this file to construct strict, single-invocation JSON objects for
`skill-manager --json-input`, `--input FILE`, or, only when safely quoted,
`--json=OBJECT`.

## Contents

- [Carrier and type rules](#carrier-and-type-rules)
- [Shared fields](#shared-fields)
- [Discovery and deployment recipes](#discovery-and-deployment-recipes)
- [Source recipes](#source-recipes)
- [Target recipes](#target-recipes)
- [Configuration recipes](#configuration-recipes)
- [CLI-only commands](#cli-only-commands)

## Carrier and type rules

- Supply exactly one JSON object. Invocation arrays and `null` are invalid.
- Unknown fields, wrong types, and missing required values fail.
- `--json=OBJECT`, `--json-input`, and `--input FILE` are mutually exclusive.
  All imply NDJSON output and non-interactive behavior.
- Precedence is defaults, then recipe fields, then explicit argv values. An
  argv command and recipe `command` must agree.
- Repeatable fields accept one string or an array of strings.
- For `--input FILE`, relative source locations and copy destinations are
  rebased to the recipe file directory. Inline/stdin recipes use CWD. Selectors
  remain verbatim. Target `path` is always a root-relative template and is never
  rebased.
- `global` and `project` are mutually exclusive. Non-interactive `load` requires
  one. Ambiguous removal requires one. No `"both"` value exists.
- `no_input` is a boolean accepted by every recipe. Recipe carriers are already
  non-interactive.

Field notation below uses `string`, `bool`, `integer`, and `string|string[]`.
Alternatives separated by `/` are aliases; prefer the first listed canonical
plural field.

## Shared fields

- Source selection: `cd`, `cd_only`, and `no_cd` are mutually exclusive
  booleans. `cd` adds CWD, `cd_only` uses only CWD, and `no_cd` is the
  configured-sources-only compatibility spelling.
- Target selection: `claude`, `shared`, `antigravity`, and `all_targets` are
  booleans; `targets` is `string|string[]`. Aliases are `all` and `target`.
  Selectors form a deduplicated union. `all_targets:true` selects enabled
  configured targets only. A disabled target requires explicit selection by
  `targets`/`target`; its built-in boolean selector is rejected while disabled.
- Scope selection: `global` and `project` are mutually exclusive booleans.
- Filters: `filters` is `string|string[]`; alias `filter`. Multiple patterns
  are ORed. Positional-style skill patterns and filters are combined as
  described in the CLI reference.

## Discovery and deployment recipes

<!-- recipe-command: load fields: all,all_targets,antigravity,cd,cd_only,claude,command,dry_run,filter,filters,global,no_cd,no_input,project,refresh,shared,source,sources,target,targets -->
### `load` (`install`)

Deploy discovered skills, replacing existing deployments. The recipe alias
`install` canonicalizes to `load`; emit `"command":"load"`.

Fields: `command:"load"`; `sources`/`source` (`string|string[]`);
`filters`/`filter`; source, target, and scope selection; `dry_run` and `refresh`
(`bool`); `no_input`. A non-interactive call must choose `global:true` or
`project:true`. A committed non-interactive call must also explicitly select at
least one target using a built-in target boolean, `all_targets:true`, or
`targets`; a dry run may implicitly preview enabled targets.

```json
{"command":"load","sources":["sernst-skills"],"filters":["managing-skills"],"shared":true,"global":true,"dry_run":true}
```

<!-- recipe-command: update fields: all,all_targets,antigravity,cd,cd_only,claude,command,dry_run,filter,filters,global,no_cd,no_input,project,refresh,shared,source,sources,target,targets,yes -->
### `update`

Refresh only existing deployments. Fields match `load` plus `yes` (`bool`). A
committed non-interactive call must explicitly select at least one target; a
dry run may implicitly preview enabled targets. Without an explicit scope it
infers each existing deployment; specify a scope to restrict it. The human
pre-confirmation plan and its `yes` bypass are interactive-only; recipe
carriers are already non-interactive and never prompt.

```json
{"command":"update","sources":["managing-*"],"all_targets":true,"dry_run":true}
```

<!-- recipe-command: import fields: all,all_targets,antigravity,claude,command,dry_run,global,no_input,project,shared,skill,target,targets,yes -->
### `import`

Adopt a deployed, possibly agent-modified copy of one skill as the new source
content. Required: `skill` (`string`), exactly one skill name and never a
pattern. Optional: target selection, scope selection, `dry_run`, `yes`, and
`no_input`.

Import is the reverse of `load`, so it fully mirrors the chosen deployment over
the configured local source directory, including deleting source files the
deployment no longer has. It writes to local source checkouts only. A
GitHub-backed source requires a local alternate location and an interactive
confirmation, so a machine call against such a source fails instead of
guessing. A committed non-interactive call requires `yes:true`, and it must
narrow target/scope selection whenever more than one deployment differs from
the source.

```json
{"command":"import","skill":"managing-skills","claude":true,"global":true,"dry_run":true}
```

<!-- recipe-command: copy fields: command,destination,dry_run,filter,filters,no_input,refresh,source -->
### `copy`

Copy discovered skills to an arbitrary destination. Required: `source`
(`string`) and `destination` (`string`). Optional: `filters`/`filter`,
`dry_run`, `refresh`, `no_input`.

```json
{"command":"copy","source":"sernst-skills","destination":"./vendor/skills","filters":["managing-*"],"dry_run":true}
```

<!-- recipe-command: remove fields: all,all_targets,antigravity,cd,cd_only,claude,command,dry_run,filter,filters,global,no_cd,no_input,project,refresh,shared,skill,skills,target,targets,yes -->
### `remove`

Remove deployments. `skills`/`skill` is `string|string[]`. Also accepts filters,
source, target, and scope selection; `dry_run`, `refresh`, `yes`, and
`no_input`. `yes:true` skips only the destructive confirmation. It does not
select a scope.

```json
{"command":"remove","skills":["obsolete-skill"],"shared":true,"project":true,"dry_run":true}
```

<!-- recipe-command: status fields: all,all_targets,antigravity,cd,cd_only,claude,command,filter,filters,global,no_cd,no_input,project,refresh,shared,target,targets -->
### `status` (`ls`, `list`)

Inspect both scopes by default or narrow to one. `filters`/`filter` matches
skill names, source names, and unique labels. Also accepts source and target
selection, scope, `refresh`, and `no_input`. Recipe aliases `ls` and `list`
canonicalize to `status`; emit `"command":"status"`.

```json
{"command":"status","filters":["managing-skills"],"shared":true}
```

<!-- recipe-command: resolve fields: cd,cd_only,command,no_cd,no_input,prefer_source,refresh,skill,skills -->
### `resolve`

Persist exclusions so one source wins a collision. `skills`/`skill` is
`string|string[]`; `prefer_source` is a source name, ID, or reference string.
Also accepts source selection, `refresh`, and `no_input`. Omit skills to address
all collisions, but never omit `prefer_source` in agent-driven use unless the
user explicitly wants the command to prompt—which recipes cannot do.

```json
{"command":"resolve","skills":["shared-name"],"prefer_source":"team","no_input":true}
```

## Source recipes

<!-- recipe-command: source.add fields: cache_ttl_hours,command,directory,exclude,label,mode,name,no_input,source,source_name -->
### `source.add`

Add a local path, GitHub tree URL, or `owner/repo[:ref][/path]`. Fields:
`source`/`directory` (`string`), `name`/`source_name` (`string`), `label`
(`string`), `exclude` (`string|string[]`), `mode` (`"collection"|"single"`),
`cache_ttl_hours` (`integer`), and `no_input`. Machine/non-interactive use
requires an explicit nonblank `name` or `source_name`; do not rely on a prompt.
Omitted `source` means CWD, but agents should normally make it explicit.

```json
{"command":"source.add","source":"sernst/skills/skills","name":"sernst-skills","label":"sernst skills"}
```

<!-- recipe-command: source.remove fields: command,directory,no_input,source -->
### `source.remove`

Remove a stored source selected by path, name, ID, or active GitHub reference.
Fields: `source`/`directory` (`string`) and `no_input`.

```json
{"command":"source.remove","source":"old-source"}
```

<!-- recipe-command: source.list fields: command,no_input -->
### `source.list`

List stored sources. Only `command` and `no_input` are accepted.

```json
{"command":"source.list"}
```

<!-- recipe-command: source.update fields: cache_ttl_hours,clear_exclude,command,directory,exclude,label,location,name,no_input,source -->
### `source.update`

Required: `source` selector. Optional: replacement `name`, active `location`,
`label`, added `exclude` (`string|string[]`), `clear_exclude` (`bool`),
`cache_ttl_hours` (`integer`), and `no_input`. `directory` aliases the selector.

```json
{"command":"source.update","source":"team","label":"Team skills","clear_exclude":true}
```

<!-- recipe-command: source.locate fields: command,location,no_input,source -->
### `source.locate`

Change only the active location. Required strings: `source`, `location`.
`relocate`, `move`, and `mv` are argv aliases, not recipe command names.

```json
{"command":"source.locate","source":"team","location":"../team-skills"}
```

<!-- recipe-command: source.alternate fields: clear,command,location,no_input,source -->
### `source.alternate`

Set or clear the inactive location. Required: `source`; then exactly one of
`location` (`string`) or `clear:true`.

```json
{"command":"source.alternate","source":"team","location":"org/team-skills/skills"}
```

<!-- recipe-command: source.swap fields: command,no_input,source -->
### `source.swap`

Exchange active and inactive locations. Required: `source`.

```json
{"command":"source.swap","source":"team"}
```

## Target recipes

Target paths are non-empty root-relative templates. Absolute paths, traversal,
and `~user` are rejected. Built-ins are `claude`, `shared`, and `antigravity`.

<!-- recipe-command: target.add fields: command,name,no_input,path -->
### `target.add`

Add and enable a custom target. Required strings: `name`, `path`.

```json
{"command":"target.add","name":"my-agent","path":".my-agent/skills"}
```

<!-- recipe-command: target.list fields: command,no_input -->
### `target.list`

List built-in and custom targets. Only `command` and `no_input` are accepted.

```json
{"command":"target.list"}
```

<!-- recipe-command: target.enable fields: command,name,no_input -->
### `target.enable`

Enable a target. Required: `name`.

```json
{"command":"target.enable","name":"shared"}
```

<!-- recipe-command: target.disable fields: command,name,no_input -->
### `target.disable`

Disable a target. Required: `name`.

```json
{"command":"target.disable","name":"antigravity"}
```

<!-- recipe-command: target.remove fields: command,name,no_input -->
### `target.remove`

Remove a custom target or legacy built-in override. For an unoverridden
built-in, this disables it. Required: `name`.

```json
{"command":"target.remove","name":"my-agent"}
```

<!-- recipe-command: target.set-path fields: command,name,no_input,path -->
### `target.set-path`

Change a custom target or legacy override template. Required strings: `name`,
`path`.

```json
{"command":"target.set-path","name":"my-agent","path":".agent/skills"}
```

## Configuration recipes

<!-- recipe-command: configs fields: command,no_input -->
### `configs`

Show resolved storage, roots, schema state, sources, targets, exclusions,
unknown preserved fields, and backup metadata. Only `command` and `no_input`
are accepted.

```json
{"command":"configs"}
```

<!-- recipe-command: configs.reset fields: command,no_input,yes -->
### `configs.reset`

Archive the exact active bytes and replace them with the canonical empty schema
v2 document. Fields: `yes` (`bool`) and `no_input`. Non-interactive execution
requires `yes:true`.

```json
{"command":"configs.reset","yes":true}
```

<!-- recipe-command: configs.restore fields: backup,command,no_input,yes -->
### `configs.restore`

Restore `backup` (`string`) or the newest backup when omitted, while first
snapshotting the displaced state. Non-interactive execution requires
`yes:true`.

```json
{"command":"configs.restore","backup":"2026-01-01T000000Z-reset","yes":true}
```

## CLI-only commands

These commands deliberately do not accept recipes:

```console
skill-manager configs --raw
skill-manager generate-completions --shell bash
skill-manager generate-completions --shell zsh
skill-manager generate-completions --shell fish
skill-manager generate-completions --shell powershell
skill-manager generate-man --output skill-manager.1
skill-manager --help
skill-manager --version
```

`configs --raw` conflicts with all JSON carriers. Generation output is not
NDJSON; do not add `--json`.
