# Architecture

The crate has a deliberately narrow shell. `main` parses Clap arguments, constructs real adapters, renders events, and maps an application result to an exit code. `lib` exposes the testable application entry point. Domain modules model sources, skills, targets, collisions, plans, configuration, remote cache, events, prompts, and errors with typed values rather than command-specific maps.

## Boundaries

Application services depend on small ports for time, confirmation, reporting, GitHub transport, configuration storage, and transaction fault injection. Production adapters perform real I/O; tests use temporary filesystems and mocked HTTP at the boundary. The library returns typed `thiserror` errors and never exits the process. Only the executable owns `ExitCode`.

## Determinism

Source order is meaningful: the first eligible source wins a duplicate skill name. Stable vectors and ordered maps preserve that order through discovery, planning, event emission, and status rendering. Skill matching normalizes names and patterns with Unicode NFKC case folding before Python-style `fnmatch` matching.

## Deployment model

Each `(target, skill)` is a small transaction: stage validated content, write a `prepared` journal, move the existing deployment to backup, install the stage, record `committed`, then remove backup and journal. Startup recovery validates every journal path against its target-owned staging, backup, and destination roots before mutating anything; crafted paths cannot move or delete outside content. Valid recovery removes uninstalled stages, restores moved backups when needed, and cleans committed backups. The rename interval can be visible to unrelated processes, but manager processes serialize through canonical-path locks under the cache root.

## Extension points

Add a source transport behind the source-materialization port, a renderer behind the reporter port, or a target policy behind target selection. Keep Clap and terminal concerns at the boundary, preserve event ordering, and add an entry to the parity/deviation ledger for intentionally changed behavior.
