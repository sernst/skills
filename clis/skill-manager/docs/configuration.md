# Configuration, backups, and migration

## Storage layout

All manager-owned state is consolidated beneath the manager home (normally the
user home; overridden by `--home DIR`, or by `SKILL_MANAGER_HOME` when
`--home` is not given):

```text
~/.skill-manager/
  config.json
  cache/
  backups/
  locks/
```

`config.json` is the active configuration. Remote-source cache content lives
under `cache/`; immutable config snapshots live under `backups/<ID>/`; process
and migration locks live under `locks/`. This layout avoids adding more
top-level dotfiles as the manager gains stateful features.

On every startup, an isolated layout migration runs before ordinary command
processing, even for `--dry-run`. It prioritizes an existing new
`config.json`, then the Rust flat `~/.skill-manager.config.json`, then the
older Python `~/.skills-syncer.config.json`. The selected legacy config is
staged and durably installed before its source is removed. If both flat configs
exist, the Rust file wins and the Python file remains with cleanup guidance.

Legacy cache entries and recognized `.v0.bak` files migrate independently into
the new `cache/` and `backups/` trees. Stale cache locks are ignored. A new
destination always wins a collision, leaving the conflicting legacy source
untouched and emitting a warning. Any non-collision migration I/O failure
aborts without removing source data. The operation is serialized and
idempotent, so interrupted migrations safely resume. Successful moves emit
`config.migrated` in JSON mode.

## Schema v2 and target templates

Schema v2 retains `targets.*.path` and `legacy_target_overrides.*.path`, but
their values are target **templates**, not absolute destinations. A template is
resolved beneath either the global manager home or the exact process CWD for
project scope. Built-ins use `.claude/skills`, `.agents/skills`, and
`.gemini/antigravity/skills`.

The normalized physical manager home is global-only. If it is also CWD, the
project root is unavailable rather than resolved to the same directories;
explicit project operations fail before writes. A child directory of home is
still a distinct project root.

New templates (including recipe target paths) are normalized by removing a
leading `~/`, normalizing separators and `.` components, and requiring a
non-empty path under the selected root. `~user`, absolute paths, and traversal
outside the root are rejected. Recipe paths are never rebased against the
recipe file.

Schema v0 and v1 migrate to v2 while preserving flattened and unknown fields.
Before schema conversion, the manager archives the exact raw input. A v1
absolute target path becomes the suffix starting at its final dot-prefixed
component; when no such component exists, the final directory component is
retained. Future, malformed, and nested type-invalid configuration is never
silently rewritten.

The canonical empty configuration is:

```json
{
  "schema_version": 2,
  "sources": [],
  "targets": {},
  "legacy_target_overrides": {},
  "builtins": {},
  "exclude": []
}
```

New source IDs are SHA-256 hashes of Python-compatible compact, sorted,
ASCII-escaped identity JSON, including null identity fields and normalized
paths. Existing and legacy local IDs remain authoritative. If a relocated
source retains the canonical ID required by a newly added source, deterministic
salted identity hashes are tried in decimal order until an unused `src_` ID is
found. Writes are locked, pretty-printed with a final newline, and atomically
replaced in the configuration directory.

Schema v2 keeps the active source location flattened on each source. An
optional inactive location is a closed typed object:

```json
{
  "alternate": {
    "type": "github",
    "owner": "sernst",
    "repo": "skills",
    "ref": "main",
    "repo_path": "skills"
  }
}
```

Local alternates contain exactly `type: "local"` and an absolute `path`.
GitHub alternates contain exactly `type: "github"`, `owner`, `repo`, and
optional `ref`/`repo_path`. Cross-type and unknown nested fields are errors,
while source-wide extension fields remain preserved. Active and alternate
identities must differ. Local identities are absolute and platform-aware
(case-insensitive on Windows); GitHub owner/repo are case-insensitive while ref
and repository path remain case-sensitive.

## Backups, reset, and restore

Every immutable snapshot contains `backups/<ID>/metadata.json` and, when the
original configuration was present, `backups/<ID>/config.raw`. IDs are
path-safe UTC timestamps with a reason and collision suffix. Metadata records
the ID, creation time, reason, original path, whether a configuration was
present, and best-effort schema/validity information.

`configs reset` snapshots the exact active bytes—including malformed or future
schema content—before writing the canonical empty document. `configs restore`
selects a named or latest snapshot, stages it, snapshots the state it would
displace, and then restores the selected bytes. A backup with `present: false`
restores an absent configuration rather than creating a file. The manager only
prunes after a successful config mutation: snapshots older than 30 days may be
removed, but it always keeps the newest snapshot and a selected in-flight one.

The raw display form is intentionally a recovery tool: `configs --raw` returns
the active bytes unparsed. Normal human and JSON display parse and validate the
configuration; absent config is shown as an unpersisted default, while malformed
content is an error.

## Source cache and deployment safety

GitHub source content is keyed by stable source ID below
`cache/<source-id>/content`; `metadata.json` records fetch time, resolved ref,
and normalized complete remote identity (`owner`, `repo`, `ref`, and
`repo_path`). Reuse requires fresh metadata, content, and an exact identity
match. Missing legacy identity metadata or a mismatch refreshes, including
during dry runs; refresh failure never falls back to mismatched content. Cache
directories remain keyed by stable source ID, and switching to local leaves
remote content available for a later matching swap. `GITHUB_TOKEN` is preferred
over `GH_TOKEN`; neither is logged or persisted. Refreshes stage and journal
cache replacement; a failed refresh preserves the old cache and fails rather
than deploying stale data.

Deployments are staged and journaled per skill. A later failure does not undo
earlier committed skills; the next invocation recovers incomplete work.
Archives are streamed under compressed, expanded, entry-count, file-size, and
path-length limits. Link entries and unsafe paths are rejected. Local skill
trees likewise reject symlinks, junctions/reparse points, special files, and
external hard links on Unix. Stable Rust does not expose a Windows link count,
so ordinary Windows hard links cannot currently be distinguished; copied
deployments still receive fresh files. Skill names must be portable safe UTF-8
components and cannot be reserved Windows device names.
