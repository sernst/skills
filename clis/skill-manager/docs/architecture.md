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

GitHub branch selection is manager-local configuration. The application asks
the transport to validate the requested branch before it emits a plan or saves
configuration, while the transport reuses the same bounded retry and response
handling as archive materialization. Per-location typed baselines distinguish
an explicit saved branch from following the repository default. A source-wide
generation joins remote identity in cache metadata, so any branch change makes
old content ineligible even after a switch away and back; replacement remains
transactional and recoverable.

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

## Filesystem reliability

`fs_retry` retries individual filesystem primitives, never an entire import,
transaction, archive download, or partially completed stream. The first attempt
is immediate; eligible failures wait 25, 50, 100, 200, and 400ms (775ms total).
Interrupted/would-block errors qualify everywhere; Windows sharing, locking,
busy, and access-denied errors also qualify. Directory-not-empty qualifies only
for deletion. Missing paths, existing-path conflicts, validation, and parsing
failures do not trigger retries. Manager advisory-lock contention retains its
separate ten-second timeout. Stream reads/writes advance normally, preserving
bytes already transferred; atomic persist retries retain the same staged file.
Exhausted interrupted stream errors cannot restart the retry budget through
standard-library or archive helpers. Filesystem APIs restore the original error.

Deployment and cache journals record staging ownership before populating the
scratch directory. New deployment records retain `stage` and `staging_root`
through commit; legacy records remain readable. Recovery validates all paths
and rejects links/reparse points before mutating any backup or stage. A
committed operation returns its result with a warning if cleanup is still
pending, retaining its journal for recovery under the same lock. Transaction
success requires a persisted `Committed` record. Failure to persist that record
after placing the replacement is an interrupted operation, not pending cleanup;
the error identifies the installed data and retained prior-state journal.
Recovery of that prior state may restore prior content. Transaction
recovery runs when a later operation enters the transaction API; commands that
stop at discovery or a no-op do not reach it. Cache recovery runs during a
later non-dry-run source materialization. There is no separate recovery command
or automatic recovery before discovery. After a process interruption in
`OldMoved`, a missing import source can prevent discovery from reaching internal
recovery; the backup and journal remain available for diagnosis.
Configuration and migration backup staging use exact-path cleanup receipts
under their respective locks. No filename-prefix sweep authorizes deletion:
unknown leftovers require inspection and must be moved aside manually.

Source relocation uses one command-specific batch journal across every selected
physical skill and configuration. It holds the existing config lock, then the
destination target lock, and retains exact before/after config images. The config
write session installs atomically without post-install backup pruning. Source and
destination evidence is rechecked after authorization; all selected trees are
staged and validated before any placement, and all backups remain until durable
commit. Any precommit failure rolls back configuration and all placed copies.

Relocation metadata uses an exact destination-hashed journal name found only at
that destination's ancestor paths; multiple claims fail. Staging and backups stay
on the destination filesystem. Workspace and ancestor directory ownership is
recorded only after successful exclusive creation and before population or further
mutation. A crash before that ownership record preserves ambiguous paths and
requires inspection. Recovery validates every recorded mapping, restores only
matching before/after configuration images, and never overwrites divergent edits.
Committed recovery only cleans owned scratch data. The application renders and
authorizes a pending recovery phase before writes, then builds a fresh copy plan.
Dry-run, cancelled, configuration-only, and location no-op operations never run
recovery; no recovery CLI or filename-prefix sweep is provided.
