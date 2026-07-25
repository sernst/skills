# Configuration, migration, and cache

The active file is `~/.skill-manager.config.json`. When only the old `~/.skills-syncer.config.json` exists, the manager retries its rename three times. If that fails, it uses the old location for the invocation and emits a warning. When both exist, the current file wins.

Schema v0 (including a missing schema) migrates one way to schema v1. Before a write the manager creates `<active-config>.v0.bak` with create-new semantics; an existing backup must exactly equal the pre-migration bytes or the command fails. Migration is in-memory for dry-run. Malformed, nested type-invalid, or future schema files are never overwritten. Invalid v0 names, exclusions, target paths, and enabled/disabled values are not silently discarded or coerced. Unknown root/source/target fields are retained where their shape permits it; legacy `skills_directories` is consumed but not backfilled.

New source IDs are SHA-256 hashes of Python-compatible compact, sorted, ASCII-escaped identity JSON (including null identity fields and normalized paths). Existing and legacy local IDs remain authoritative. Writes are locked, pretty-printed with a final newline, and atomically replaced in the config directory.

GitHub source content lives under `~/.skill-manager-cache/<source-id>/content` with `metadata.json` containing `fetched_at` and `resolved_ref`. An absent, invalid, expired, or zero-TTL cache refreshes. `GITHUB_TOKEN` is preferred over `GH_TOKEN` and neither is logged or persisted. Refreshes stage and journal cache replacement; a failed refresh preserves the old cache and fails rather than deploying stale data.

Archives are streamed under compressed, expanded, entry-count, file-size, and path-length limits. Link entries and unsafe paths are rejected. Local skill trees likewise reject symlinks, junctions/reparse points, and special files, and reject external hard links on Unix. Stable Rust does not expose a Windows link count, so ordinary Windows hard links cannot currently be detected; copied deployments still receive fresh files. Skill names must be portable safe UTF-8 components and cannot be reserved Windows device names.
