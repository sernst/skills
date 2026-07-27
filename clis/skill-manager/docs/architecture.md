# Architecture

The crate has a deliberately narrow shell. `main` parses Clap arguments,
constructs real adapters, renders events, and maps application results to exit
codes. `lib` exposes the testable application entry point. Domain modules model
sources, skills, scoped targets, collisions, plans, configuration, remote cache,
events, prompts, and errors with typed values rather than command-specific maps.

## Boundaries and determinism

Application services depend on small ports for time, confirmation, reporting,
GitHub transport, configuration storage, and transaction fault injection.
Production adapters perform real I/O; tests use temporary filesystems and
mocked HTTP. The library returns typed `thiserror` errors and never exits the
process. Only the executable owns `ExitCode`.

Source order is meaningful: the first eligible source wins a duplicate skill
name. Ordered collections preserve that order through discovery, planning,
event emission, and status rendering. Skill names and Python-style `fnmatch`
patterns use NFKC case folding.

## Scoped targets and effective deployments

Targets persist one normalized root-relative template. The resolver pairs that
template with an explicit `Global` or `Project` scope, then roots it at the
manager home or exact process CWD. This keeps persistence independent of a
machine-specific absolute path and avoids a separate record per scope.

Status reads both scoped deployments and computes an effective view: project
wins over global per target, while aggregate location and mixed/shadowed state
retain the full picture. Update planning uses the same resolver to infer each
existing deployment independently; load chooses a single scope for its full
plan.

## Configuration lifecycle

The `storage_migration` module is intentionally isolated from configuration
schema migration. It moves the historical flat configuration, cache, and v0
backup layout into `~/.skill-manager/` before normal work, with destination-win
collision handling and resumable component-level migration. It can be removed
after the adoption window without affecting configuration parsing or backup
behavior.

Configuration storage owns schema migration, raw-byte backups, reset/restore,
retention, locking, and atomic replacement. Configuration display is dispatched
before normal parsed-config execution, allowing `configs --raw` to recover
malformed bytes safely. Mutations snapshot the displaced state before replacing
it.

## Deployment model

Each `(target, scope, skill)` is a small transaction: stage validated content,
write a `prepared` journal, move the existing deployment to backup, install the
stage, record `committed`, then remove backup and journal. Startup recovery
validates every journal path against target-owned staging, backup, and
destination roots before mutating anything; crafted paths cannot move or delete
outside managed content. Valid recovery removes uninstalled stages, restores
moved backups when needed, and cleans committed backups. The rename interval can
be visible to unrelated processes, but manager processes serialize through
canonical-path locks under the consolidated storage root.

## Extension points

Add a source transport behind the source-materialization port, a renderer
behind the reporter port, or a target policy behind target selection. Keep Clap
and terminal concerns at the boundary, preserve event ordering, and record
intentional public semantic changes in the deviation ledger.
