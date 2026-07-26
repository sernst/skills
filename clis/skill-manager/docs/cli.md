# Command reference

## Common behavior

Running without a command is `status`. `--color auto|always|never` controls human color; `NO_COLOR` disables color. Non-TTY human output is aligned plain text with no ANSI escapes or emoji. On an interactive terminal, color is limited to semantic status cells rather than whole lines; status symbols remain available when color is disabled. `--json` emits NDJSON and implies `--no-input`.

| Command | Purpose |
| --- | --- |
| `load [SOURCE...]` | Discover and deploy skills, replacing existing copies. |
| `update [SOURCE...]` | Update only skills that already exist in a target. |
| `copy SOURCE DEST` | Copy discovered skills to `DEST`. |
| `remove [SKILL...]` | Remove selected or auto-detected deployed skills. |
| `status [FILTER...]` | Show source-relative target states; aliases: `ls`, `list`. |
| `resolve [SKILL...]` | Persist collision preferences. |
| `source …` | Add, remove, list, or update local/GitHub source definitions. |
| `target …` | Add, list, enable, disable, remove, or update target paths. |

`load`, `update`, `remove`, and `status` accept built-in selectors `--claude`, `--shared`, `--antigravity`/`--ag`, `--all`, and repeatable `--target NAME`. Selectors form a deduplicated union; an explicit `--target` can include a disabled target alongside enabled built-ins. `load` and `update` prompt when there is no explicit target; noninteractive calls must select a target. `remove` prompts before destructive work unless `--yes`/`-y` is supplied.

All discovery commands accept repeatable `--filter PATTERN`, `--refresh`, and `--dry-run` where applicable. Configured sources are default. `--cd` adds CWD for `status`; `--cd-only` uses only CWD; `--no-cd` keeps the compatibility spelling for configured-source-only behavior.

## Source and target lifecycle

`source add` accepts a local path, GitHub URL, or GitHub shorthand and optional name, label, source mode, excludes, and cache TTL. Names and labels are updated with `source update`; IDs do not change. `target add` only creates custom targets; `target set-path` changes a custom path. Built-ins are `claude`, `shared`, and `antigravity`. `target remove` deletes custom targets and instead disables an unoverridden built-in. Legacy built-in overrides remain available as explicit legacy overrides and warn until updated or removed.

## Status values

Each source/target pair is `up-to-date`, `needs-update`, `not-loaded`, or `no-connection`. Equality compares relative regular-file names and SHA-256 content only; timestamps, ownership, and empty directories do not affect it.

Human `status` output first maps compact source names to labels and local or GitHub locations without a redundant legend header, then uses those names in an aligned skill-by-target table. The table has lowercase headers, a dashed separator, and two-space column gaps. Interactive terminals show one colored symbol per target state (`✓`, `↑`, `✗`, or `~`) and a matching, nonzero-only summary legend; custom target names therefore widen only their headers. Redirected output remains ANSI-free and uses textual state names. Deployed-only skills display `unknown` as their human source while the NDJSON event schema and source provenance remain unchanged. Ad hoc current-directory sources use `cwd`; other unconfigured sources use a deterministic, disambiguated short name.
