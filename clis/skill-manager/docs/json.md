# JSON and recipe contract

## Modes and precedence

`--json` alone selects NDJSON output. `--json=OBJECT`, `--json-input`, and
`--input FILE` supply exactly one recipe object, are mutually exclusive, and
also select NDJSON/non-interactive mode. Recipe command names are canonical
(`load`, `configs.reset`, `target.set-path`, and so on); an argv command and a
recipe command must agree.

Values resolve in this order: command defaults, recipe fields, then explicit
command-line flags. Thus explicit JSON `false` is preserved. Repeatable fields
accept either one string or an array of strings. Unknown fields, invalid types,
null for non-nullable fields, invalid command fields, and invocation arrays are
errors. Relative source locations in source recipes are relative to the recipe
file; inline and stdin recipes use CWD. Target template paths are never rebased.
The `global` and `project` fields are validated as one mutually exclusive scope
choice. An explicit argv scope overrides that recipe choice, while all supplied
recipe values are still type-checked.

`--json`, recipes, and `--no-input` cannot answer prompts. Scope-dependent
commands must therefore specify `global: true` or `project: true` where
required; they are mutually exclusive. `load` always needs one in
non-interactive mode, while an ambiguous dual-scope `remove` needs one. A
recipe uses canonical `configs`, `configs.reset`, or `configs.restore` for
configuration operations. `configs.reset` and `configs.restore` need
`yes: true` in non-interactive mode; `configs.restore` optionally accepts the
strict `backup` field.

## Event stream

Every semantic stdout line is a JSON object with this envelope:

```json
{"version":1,"event":"skill.loaded","level":"info","data":{}}
```

`version` is currently `1`; `level` is `info`, `warning`, or `error`. Events
cover planned and committed skill actions, status rows, sources, targets,
collisions, diagnostics, configuration lifecycle, summaries, cancellation, and
`command.failed`. Action data includes provenance, target path, dry-run state,
and `scope` (`global` or `project`). Events follow plan order and a summary is
last. A partial transaction emits committed actions before `command.failed`.

`status.row` retains its effective `targets` state map and adds:

```json
{
  "location": "global|project|both|none",
  "mixed": false,
  "shadowed_global_divergent": false,
  "deployments": [
    {
      "target": "claude",
      "scope": "project",
      "path": "/work/app/.claude/skills/example",
      "installed": true,
      "state": "up-to-date",
      "effective": true
    }
  ]
}
```

Deployments are deterministically ordered. A project deployment is `effective`
when it exists; otherwise its global counterpart is effective. `config.shown`
contains the active config path, storage root, persistence state, parsed config,
and backup metadata. `config.reset` and `config.restored` identify backup IDs
and paths but never include backup bytes. Layout migration emits
`config.migrated`; collision cleanup guidance remains a diagnostic warning.

## Exit status and streams

Normal completion, no work, and user cancellation return `0`. Operational,
validation, and interaction-required failures return `1`; Clap usage errors
return `2`. Human data is stdout and diagnostics stderr. In JSON mode, semantic
errors are NDJSON on stdout, so consumers can parse every output line.
