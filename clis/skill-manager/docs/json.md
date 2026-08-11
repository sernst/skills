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
required; they are mutually exclusive. `load` infers its scope silently, the
same way an interactive run would, rather than requiring one; an ambiguous
dual-scope `remove` still needs one. A recipe uses canonical `configs`,
`configs.reset`, or `configs.restore` for configuration operations.
`configs.reset` and `configs.restore` need `yes: true` in non-interactive
mode; `configs.restore` optionally accepts the strict `backup` field.

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

## The plan event

A mutating command that renders a plan for review also emits one event per
rendered revision, always before anything is written: `plan` for revision `0`
and `plan.updated` for every narrowed re-render that follows. `update`, `load`,
`copy`, `remove`, and `import` all emit it. `remove` always emits a single
revision `0` — an ambiguous scope is one decision resolved by one prompt, not
a re-rendered sequence, so it never emits `plan.updated`. `import` is the only
command whose plan can carry more than one decision (source copy, then
propagation mode), so it is the only command that can emit `plan.updated` —
one per nonfinal answer, before the corresponding narrowed re-render; the
final answer needs no extra revision because applying begins immediately.
Propagation resolves silently (no flag, no prompt) whenever the resolved
source copy would leave nothing else out of date, so a single-deployment or
already-synchronized import can commit with `yes:true` alone. Otherwise, every
non-interactive carrier (`--json`, `--json-input`, `--input`, plain
`--no-input`) resolves a decision only when the caller actually supplied the
flag or recipe field it needs — an unresolved decision is never guessed. A
`--dry-run` render can therefore still show decisions pending (see
`authorization.pending` in the example below), and a committed `--yes`/
`yes:true` call fails outright while a genuine decision remains open.
`plan.updated` is only ever produced by a live interactive terminal session,
since every non-interactive carrier either commits or fails at revision `0`
without narrowing.

```json
{
  "plan_id": "update:writing-for-agents",
  "revision": 0,
  "command": "update",
  "dry_run": false,
  "authorization": { "kind": "binary", "mode": "prompt", "default": true },
  "selection": {
    "targets": {
      "mode": "inferred",
      "names": ["claude", "shared", "antigravity"]
    },
    "scope": { "mode": "inferred", "value": "global" }
  },
  "destinations": [
    {
      "id": "claude:global",
      "kind": "deployment",
      "label": "claude · global",
      "target": "claude",
      "scope": "global"
    }
  ],
  "entries": [
    {
      "skill": "writing-for-agents",
      "actions": [
        {
          "operation": "update",
          "destination": "claude:global",
          "existed": true,
          "diff": {
            "files_changed": 1,
            "insertions": 1,
            "files": [
              { "path": "SKILL.md", "change": "modified", "insertions": 1 }
            ]
          }
        }
      ]
    }
  ],
  "summary": { "skills": 1, "actions": 2, "update": 2 }
}
```

`plan_id` is stable across the revisions of one invocation and `revision` counts
from `0`, so a progressively narrowed multi-prompt plan is reconstructable.

`authorization.kind` is `binary`, `selection`, or `progressive`; `authorization.mode` is
`prompt`, `yes`, `dry-run`, or `noninteractive`, and `default` is present only
for a `binary` prompt that has one — a `selection` is never preselected, because
every option is destructive. Any plan that carries one or more `decisions`
(`selection` and `progressive` alike) additionally reports `sequence`
(dimension ids in prompt order), `resolved` (dimension id to the chosen option
id), `pending`, and, when a prompt follows this revision, `prompt` naming the
live `dimension`. `selection` carries exactly one dimension resolved by one
mutually exclusive choice — `remove`'s ambiguous global/project/both scope is
the only current example; `progressive` carries more than one, resolved across
successive revisions:

```json
"authorization": {
  "kind": "progressive",
  "mode": "prompt",
  "sequence": ["source_copy", "propagation"],
  "resolved": { "source_copy": "shared:global" },
  "pending": ["propagation"],
  "prompt": { "dimension": "propagation" }
}
```

A plan that carries dimensions also carries a `decisions` array describing all
of them — resolved and pending alike — so the payload always describes the whole
plan the user reviewed rather than only the question currently on screen. The
alternatives are never gated out once answered, because automation must be able
to tell which alternatives were declined:

```json
"decisions": [
  {
    "id": "source_copy",
    "prompt": "Select source copy",
    "state": "resolved",
    "resolved": "shared:global",
    "options": [
      {
        "id": "shared:global",
        "token": "2",
        "label": "shared · global",
        "consequence": {
          "operation": "import",
          "path": "C:\\Users\\swern\\.agents\\skills\\importing-meeting-notes",
          "actions": [
            { "operation": "import", "destination": "personal:source", "existed": true, "diff": { "files_changed": 2, "insertions": 9, "deletions": 7, "files": [] } },
            { "operation": "update", "destination": "claude:global", "existed": true, "diff": { "files_changed": 2, "insertions": 9, "deletions": 7, "files": [] } },
            { "operation": "skip", "destination": "shared:global", "existed": true }
          ],
          "totals": { "deployments": 5 }
        }
      }
    ]
  }
]
```

An option reports `id`, `token`, `label`, `recommended` when it is guidance, its
rendered `effect` clause, and a typed `consequence`. The consequence holds the
`operation` the option performs, the `path` it identifies, the per-destination
`actions` it would write with their own diffs, and named aggregate `totals` for
a blast radius too wide to enumerate per destination — which is how `remove`
states each scope option's cost across every skill at once.

Each `selection` dimension reports whether it was `explicit` or `inferred`.
Unlike the rendered plan, `selection` is never significance gated:
`targets.names` lists every selected target even when one of them turned out to
have no work, so a target that was selected and idle stays distinguishable from
one that was never selected. A `destination` is a target/scope deployment
(`kind: "deployment"`), an arbitrary directory (`kind: "path"`, with `path`), or
a canonical source (`kind: "source"`, with `source`); every destination some
entry or decision alternative references is listed.

An entry lists the `actions` it will perform and, for a plan that exposes where
an item exists without yet deciding what to do about it, an `available` array of
destination ids. Availability is evidence, not an operation, and never counts as
an action — `remove`'s unresolved global/project branch is the only current
source of it: while the scope is undecided, an entry's `actions` array is
empty and its `available` array lists every deployment id the skill occupies;
once the branch resolves (explicit scope, `--both`, or a made selection),
those same destinations become concrete `"operation": "remove"` actions.

A `deployment` destination carries `path` when the command decides target
roots itself rather than accepting an arbitrary one — `load` populates it;
`update` and `remove` do not, to keep their established payloads unchanged. An
entry carries `source` naming the resolved source label an item came from
whenever the command tracks that provenance (`load` does; `update` and
`remove` do not, for the same reason). Fields that a command has never emitted
are simply absent rather than `null`, so consumers can treat presence itself
as meaningful.

Significance gating never hides a semantic action from `entries`, even when
rendering hides the same row from the human table because every action on it
is a no-op: a fully identical `load` row still appears as a complete entry
whose actions are all `"operation": "skip"`. `summary` reflects the same
completeness. Its per-operation counts use the vocabulary each command's plan
distinguishes: `update`'s and `remove`'s `summary` bucket by the literal
action word (`"update"`, `"remove"`); `load` and `copy` instead bucket by
whether a deployment already `existed` (`"new"` for one that did not,
`"overwrite"` for one that did), with `"skip"` always counted separately for
identical, no-op actions regardless of that distinction. Automation should key
off these command-specific categories rather than assuming one fixed set
across every command.

Significance gating is a property of human rendering and never reaches this
stream. The payload omits only what is genuinely absent: a zero metric, an empty
diff, an inapplicable field. `summary` always carries `skills` and `actions`,
adds `available` when the plan carries availability, and adds one nonzero count
per operation.

## Exit status and streams

Normal completion, no work, and user cancellation return `0`. Operational,
validation, and interaction-required failures return `1`; Clap usage errors
return `2`. Human data is stdout and diagnostics stderr. In JSON mode, semantic
errors are NDJSON on stdout, so consumers can parse every output line.
