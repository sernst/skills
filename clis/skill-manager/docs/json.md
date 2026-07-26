# JSON and recipe contract

## Modes and precedence

`--json` alone selects NDJSON output. `--json=OBJECT`, `--json-input`, and `--input FILE` supply one complete JSON invocation, are mutually exclusive, and also select NDJSON/noninteractive mode. The accepted command names are canonical (`load`, `source.add`, `target.set-path`, and so on); an argv command and a recipe command must agree.

Values resolve in this order: command defaults, recipe fields, then explicitly provided command-line flags. Thus an explicit JSON `false` is not discarded. Repeatable fields accept either one string or an array of strings. Other fields have strict types. Unknown fields, null for non-nullable fields, invalid command fields, and an array of invocations are errors. Relative location values in `source.add`, `source.update`, `source.locate`, and `source.alternate` recipes are relative to the recipe file; inline and stdin recipes use CWD. Source selector fields are never rebased.

Canonical recipe commands include `source.locate`, `source.alternate`, and `source.swap`; CLI aliases are rejected as recipe names. `source.update` accepts `location`. After recipe/argv merging, `source.alternate` requires exactly one of `location` or `clear:true`. An explicit argv location suppresses recipe `clear`, and explicit `--clear` suppresses recipe `location`.

## Event stream

Every semantic stdout line is a JSON object with this envelope:

```json
{"version":1,"event":"skill.loaded","level":"info","data":{}}
```

`version` is currently `1`; `level` is `info`, `warning`, or `error`. Events cover planned and committed skill actions, status rows, sources, targets, collisions, diagnostics, summaries, cancellation, and `command.failed`. Action data includes provenance, target path, outcome, and dry-run state where relevant. Events follow plan order and a summary is last. A partial transaction emits committed actions before `command.failed`.

Source payloads add nullable post-state `alternate`, represented as `{ "source": "...", "source_type": "local|github" }`. Location mutations emit `source.updated`, `source.location-set`, `source.alternate-set`, `source.alternate-cleared`, or `source.locations-swapped`. Each includes post-state top-level `source`, `source_type`, and `alternate`, plus `changed` and `previous: { source, source_type, alternate }`. No-op events use identical snapshots, report `changed:false`, and do not rewrite configuration.

## Exit status and streams

Normal completion, no work, and user cancellation return `0`. Operational and validation failures return `1`. Clap usage errors return `2`. Human data is stdout and diagnostics stderr; in JSON mode semantic errors are NDJSON on stdout, allowing a consumer to parse every emitted line.
