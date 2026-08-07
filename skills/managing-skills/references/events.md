# NDJSON event reference

Parse every stdout line from JSON mode as one independent JSON object:

```json
{"version":1,"event":"skill.loaded","level":"info","data":{}}
```

## Contents

- [Envelope and streams](#envelope-and-streams)
- [Event inventory](#event-inventory)
- [Payload families](#payload-families)
- [Completion and failure](#completion-and-failure)

## Envelope and streams

- `version` is integer `1`.
- `event` is one of the names below.
- `level` is `"info"`, `"warning"`, or `"error"`.
- `data` is an event-specific object.
- Semantic JSON output is stdout, including failures. Human diagnostics use
  stderr only outside JSON mode.
- Events preserve plan/execution order. A command-level `summary` is normally
  last. A partial transaction can emit committed action events followed by
  `command.failed`.

Exit status `0` means normal completion, no work, or user cancellation. `1`
means an operational, validation, or interaction-required failure. `2` is a
Clap usage error, which may occur before an NDJSON reporter exists.

## Event inventory

The comments in this section are machine-checked against production emit sites.

<!-- event: collision.detected -->
- `collision.detected`: competing sources and selected winner.
<!-- event: collision.resolved -->
- `collision.resolved`: skill and persisted preferred source.
<!-- event: command.cancelled -->
- `command.cancelled`: action cancelled without mutation.
<!-- event: command.failed -->
- `command.failed`: terminal error message; earlier actions may be committed.
<!-- event: config.migrated -->
- `config.migrated`: startup storage component move and paths.
<!-- event: config.reset -->
- `config.reset`: reset path and created backup metadata.
<!-- event: config.restored -->
- `config.restored`: restored backup and displaced-state backup metadata.
<!-- event: config.shown -->
- `config.shown`: active config, storage roots, persistence, and backups.
<!-- event: diagnostic -->
- `diagnostic`: warning message, sometimes with an unmatched `pattern`.
<!-- event: skill.copied -->
- `skill.copied`: copy plan or committed copy.
<!-- event: skill.import-planned -->
- `skill.import-planned`: selected deployment and its source-overwrite plan.
<!-- event: skill.import-skipped -->
- `skill.import-skipped`: no selected deployment differs from the source.
<!-- event: skill.imported -->
- `skill.imported`: import plan or committed source overwrite.
<!-- event: skill.loaded -->
- `skill.loaded`: load plan or committed deployment.
<!-- event: skill.removed -->
- `skill.removed`: removal plan or committed removal.
<!-- event: skill.skipped -->
- `skill.skipped`: already equal deployment.
<!-- event: skill.updated -->
- `skill.updated`: update plan or committed deployment.
<!-- event: source.added -->
- `source.added`: stored source definition.
<!-- event: source.alternate-cleared -->
- `source.alternate-cleared`: inactive location removed or already absent.
<!-- event: source.alternate-set -->
- `source.alternate-set`: inactive location set or already equal.
<!-- event: source.listed -->
- `source.listed`: one stored source.
<!-- event: source.location-set -->
- `source.location-set`: active source location changed or unchanged.
<!-- event: source.locations-swapped -->
- `source.locations-swapped`: active and inactive locations exchanged.
<!-- event: source.removed -->
- `source.removed`: removed stored source.
<!-- event: source.updated -->
- `source.updated`: source metadata/location update.
<!-- event: status.row -->
- `status.row`: one discovered or deployed skill and its target states.
<!-- event: summary -->
- `summary`: command-specific final counts.
<!-- event: target.added -->
- `target.added`: new custom target.
<!-- event: target.disabled -->
- `target.disabled`: disabled target.
<!-- event: target.enabled -->
- `target.enabled`: enabled target.
<!-- event: target.listed -->
- `target.listed`: one resolved built-in/custom target.
<!-- event: target.path-set -->
- `target.path-set`: changed target template.
<!-- event: target.removed -->
- `target.removed`: custom/override removed or built-in disabled.

## Payload families

<!-- payload: source fields: alternate,mode,source,source_id,source_label,source_name,source_type -->
<!-- payload: source-location fields: source,source_type -->
<!-- payload: source-previous fields: alternate,source,source_type -->
<!-- payload: source-change fields: alternate,changed,mode,previous,source,source_id,source_label,source_name,source_type -->
`source.added`, `source.removed`, and `source.listed` contain one flattened
source object (source identity is not nested):

```json
{
  "source": "owner/repo:main/skills",
  "source_id": "src_...",
  "source_name": "team",
  "source_label": "Team skills",
  "source_type": "github",
  "mode": "collection",
  "alternate": {
    "source": "/work/team-skills",
    "source_type": "local"
  }
}
```

`alternate` is either `null` or exactly `{source,source_type}`.
`source.updated`, `source.location-set`, `source.alternate-set`,
`source.alternate-cleared`, and `source.locations-swapped` add `changed`
(`bool`) and `previous`, where `previous` contains exactly `source`,
`source_type`, and `alternate`. These event payloads do not expose exclusions
or cache TTL.

<!-- payload: target fields: builtin,enabled,label,legacy_override,name,path -->
<!-- payload: target-removed fields: name -->
`target.listed`, `target.added`, `target.enabled`, `target.disabled`, and
`target.path-set` contain exactly:

```json
{
  "name": "shared",
  "label": "Shared agents",
  "path": "/home/me/.agents/skills",
  "enabled": true,
  "builtin": true,
  "legacy_override": false
}
```

Here `path` is the resolved target root reported by that command.
`target.removed` is a separate shape containing only `{"name":"..."}`.

<!-- payload: skill-action fields: action,alternate,destination,dry_run,mode,path,scope,skill,source,source_id,source_label,source_name,source_type,target,target_path -->
`skill.loaded`, `skill.updated`, `skill.skipped`, and `skill.copied` contain the
flattened source fields above plus:

```json
{
  "skill": "name",
  "source": "owner/repo:main/skills",
  "source_id": "src_...",
  "source_name": "team",
  "source_label": "Team skills",
  "source_type": "github",
  "mode": "collection",
  "alternate": null,
  "path": "/materialized/source/name",
  "target": "shared",
  "scope": "global",
  "target_path": "/home/me/.agents/skills",
  "destination": "/home/me/.agents/skills/name",
  "action": "loaded|overwritten|updated|copied|skipped",
  "dry_run": false
}
```

For these action events, `path` is the discovered/materialized source skill
directory, `target_path` is the destination target root, and `destination` is
the destination skill directory. `copy` has no `scope`; load/update/skip do.
`action` for these events is `loaded`, `overwritten`, `updated`, `copied`, or
`skipped` as applicable.

<!-- payload: skill-import fields: action,alternate,deletions,deployment,destination,dry_run,files_changed,insertions,mode,path,scope,skill,source,source_id,source_label,source_name,source_type,target,target_path -->
<!-- payload: skill-import-skipped fields: action,alternate,dry_run,mode,path,skill,source,source_id,source_label,source_name,source_type -->
`skill.import-planned` and `skill.imported` reverse the action direction and
contain the flattened source fields plus:

```json
{
  "skill": "name",
  "path": "/materialized/source/name",
  "target": "claude",
  "scope": "global",
  "target_path": "/home/me/.claude/skills",
  "deployment": "/home/me/.claude/skills/name",
  "destination": "/work/skills/name",
  "files_changed": 3,
  "insertions": 12,
  "deletions": 9,
  "action": "planned|imported",
  "dry_run": false
}
```

For import events, `deployment` is the deployed copy that supplies the content
and `destination` is the local source skill directory that is replaced in full.
The counts describe that same replacement: `files_changed` counts added,
modified, and deleted files, while `insertions` and `deletions` count text
lines and stay `0` for binary content. `skill.import-skipped` reports only
`skill`, `path`, `action` (`skipped`), `dry_run`, and the flattened source
fields: no deployment was selected, so no destination was resolved.

<!-- payload: skill-removed fields: action,dry_run,path,scope,skill,target,target_path -->
`skill.removed` deliberately has a different shape and no source provenance or
`destination` field:

```json
{
  "skill": "name",
  "target": "shared",
  "scope": "global",
  "target_path": "/home/me/.agents/skills",
  "path": "/home/me/.agents/skills/name",
  "action": "removed",
  "dry_run": false
}
```

For removal only, `path` is the destination skill directory being removed.

<!-- payload: status-row fields: deployments,location,mixed,shadowed_global_divergent,skill,source,targets -->
<!-- payload: status-deployment fields: effective,installed,path,scope,state,target -->
`status.row` contains:

```json
{
  "skill": "name",
  "source": {
    "source": "owner/repo:main/skills",
    "source_id": "src_...",
    "source_name": "team",
    "source_label": "Team skills",
    "source_type": "github",
    "mode": "collection",
    "alternate": null
  },
  "targets": {"shared":"up-to-date"},
  "location": "global|project|both|none",
  "mixed": false,
  "shadowed_global_divergent": false,
  "deployments": [{
    "target": "shared",
    "scope": "project",
    "path": "/work/app/.agents/skills/name",
    "installed": true,
    "state": "up-to-date|needs-update|not-loaded|no-connection",
    "effective": true
  }]
}
```

Deployments are deterministic. A project copy is effective when present;
otherwise the global copy is effective. Each deployment `path` is the resolved
deployed skill directory for that target/scope; append only `SKILL.md` to read
the installed skill. The row-level `source` is the exact flattened source
object described above, or `null` for a deployed-only skill.

`collision.detected` is `{skill,winner,candidates}`; `winner` is one source
object and `candidates` is an array of source objects in the exact flattened
source shape above. `collision.resolved` is `{skill,preferred_source}`, where
`preferred_source` is one such source object. `diagnostic` contains `message`
and may also contain `pattern`.

<!-- payload: collision-detected fields: candidates,skill,winner -->
<!-- payload: collision-resolved fields: preferred_source,skill -->
<!-- payload: diagnostic-message fields: message -->
<!-- payload: diagnostic-pattern fields: message,pattern -->

<!-- payload: config-shown fields: backups,config,home,path,persisted,project_root,storage_root,targets -->
<!-- payload: config-target fields: builtin,enabled,global_path,label,legacy_override,name,project_path,template -->
<!-- payload: config-backup fields: created_at,id,original_path,present,raw_path,reason,schema_version,valid -->
<!-- payload: config-migrated fields: component,from,to -->
<!-- payload: config-reset fields: backup_id,backup_path,path -->
<!-- payload: config-restored fields: backup_id,backup_path,displaced_backup_id,displaced_backup_path,path,present -->
`config.shown` contains exactly `path`, `storage_root`, global/project roots
(`home` and `project_root`), `persisted`, parsed `config`, resolved `targets`,
and `backups`. Each resolved target contains exactly `name`, `label`,
`template`, `enabled`, `builtin`, `legacy_override`, `global_path`, and
`project_path`. Each backup contains exactly `id`, `created_at`, `reason`,
`original_path`, `present`, `schema_version`, `valid`, and `raw_path`.
`config.migrated` is exactly `{component,from,to}`. `config.reset` is
`{path,backup_id,backup_path}`. `config.restored` is
`{path,backup_id,backup_path,displaced_backup_id,displaced_backup_path,present}`.
These events never include backup bytes.

In these configuration events, `path` is the active configuration path,
`home` is the manager's global-scope root, and `project_root` is the exact
current working directory. A `backup_path` or `displaced_backup_path` is the
raw archived-byte path; its corresponding ID is the stable selector for
restore operations.

`command.cancelled` is one `{"action":"..."}` object naming the declined
command, such as `remove`, `update`, `import`, `configs.reset`, or
`configs.restore`. `command.failed` is `{"message":"..."}`.

<!-- payload: command-cancelled fields: action -->
<!-- payload: command-failed fields: message -->

<!-- payload: summary-source-list fields: sources -->
<!-- payload: summary-load-update fields: action,changed,dry_run,skipped -->
<!-- payload: summary-import fields: action,dry_run,imported,skipped -->
<!-- payload: summary-copy fields: action,copied,dry_run -->
<!-- payload: summary-remove fields: action,dry_run,removed -->
<!-- payload: summary-status fields: action,skills -->
<!-- payload: summary-resolve fields: action,resolved -->
`summary.data` has one of these exact shapes:

- `source.list`: `{sources}`;
- `load`/`update`: `{action,changed,skipped,dry_run}`;
- `import`: `{action,imported,skipped,dry_run}`;
- `copy`: `{action,copied,dry_run}`;
- `remove`: `{action,removed,dry_run}`;
- `status`: `{action,skills}`;
- `resolve`: `{action,resolved}`.

Other source, target, and configuration lifecycle commands finish with their
specific event and do not emit `summary`. Do not require `summary` as a generic
success condition.

## Completion and failure

Do not decide success from the presence of a `summary` alone. Check both:

1. every parsed event, preserving any committed action events; and
2. the process exit status.

On exit `1`, find `command.failed.data.message`, report it, and separately list
action events that occurred earlier. On exit `2`, report the usage failure and
do not assume any NDJSON was emitted. On `command.cancelled` with exit `0`,
report cancellation as a non-error with no requested mutation completed.

One bootstrap exception is expected absence, not success. A filtered `status`
may emit `command.failed` and exit `1` because its requested skill pattern
matched nothing or produced no actionable skill before any relevant source is
configured. Treat that exact parsed message as evidence that the skill is
absent only after `source.list` confirms the missing source context. It may
justify continuing with the source-add/install workflow requested by the user;
it does not make the failed status successful. Any different exit-1 message,
including configuration, cache, permission, network, recipe, or interaction
errors, remains blocking.
