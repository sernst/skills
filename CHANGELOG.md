# Changelog

All notable changes to the downloadable CLIs are documented here.

## 0.1.3 - 2026-08-10

- Adds `install.sh` and `install.ps1` release installers so the README documents one install-or-upgrade command per platform instead of manual download steps; both verify the download against `SHA256SUMS`, report the installed and incoming versions, replace the binary only after the download is proven to run, and can add the install directory to `PATH`.
- Adds a global `--home DIR` flag that overrides the manager home for the whole invocation, taking precedence over `SKILL_MANAGER_HOME` and the operating system home, so validation and smoke testing can never touch real user state; a relative value is resolved to an absolute, lexically clean path before any derived path is built.
- Adds `configs copy FROM TO`, seeding a destination manager home (configuration plus resolved target directories) from an existing one by merging paths and deleting nothing, so a scratch `--home` can be made to resemble a real one before smoke testing; cache, backup, and lock directories are excluded by default (`--include-cache` opts in) even when a resolved target points inside them via a `..`-obfuscated path, source-configuration target paths are lexically normalized and rejected when they escape `FROM`, every destination path is preflighted for symlink and reparse-point (including Windows junction) escapes and for file/directory conflicts in both directions before the plan is rendered so an unsafe copy fails cleanly without a partial seed, any configured source root that is a symlink or junction — the `.skill-manager` configuration root (checked before its `config.json` is read), the copied configuration directory, and every resolved target root — is never descended and is instead reported as an explicit skipped item so the seed is never silently incomplete, the link/ancestor check is re-run immediately before each write so an ancestor swapped for a link after preflight is still caught, neither `FROM` nor the active `--home` is ever migrated, locked, or written — including when `FROM` is the active home and under `--dry-run`, which changes nothing anywhere — and it is fully recipe-drivable (`--json-input`/`--input`/`--json=OBJECT` with `command: "configs.copy"`) like its sibling commands, always ending with the shared terminal `summary` event.
- Makes contract tests platform-independent by no longer assuming Windows path separators.
- Makes the `copy` contract tests independent of path canonicalization by expecting the canonical destination spelling the command actually renders, so a symlinked temporary directory (macOS `/var` resolving to `/private/var`) or a Windows 8.3 short-name temporary directory no longer fails the copy plan assertions.
- Fixes `load`/`install` and `update`/`up` treating a bare skill name as a current-directory path; a literal operand now resolves case-insensitively against discovered skill names when it does not name a configured source, path, or GitHub reference.
- Adds a hard error, `no configured source, directory, or skill named "NAME"`, for a bare operand that resolves to nothing, with a hint to run `skill-manager ls`.
- Warns and prefers the skill when a bare operand matches both a discovered skill name and a same-named directory in the current directory, pointing at `./NAME` to force the directory interpretation.
- Fixes a relative `SKILL_MANAGER_HOME` value not being normalized to an absolute path, which leaked a `./`, `..`, or foreign-separator segment into cache staging paths and made every command that touches the cache fail the journal's own path-safety validation.

## 0.1.2 - 2026-08-08

- Treats the manager home as global-only across every scoped command, preventing duplicate project deployments and rejecting explicit `--project` use there with clear guidance.
- Redesigns import output around concise source and target labels, with full paths available through the new global `--verbose` option, and offers an explicitly reviewed update of other outdated installed copies after interactive imports.
- Adds `up` as an alias for `update` and replaces noisy per-deployment plans with changed-only, one-row-per-skill target matrices and concise no-op results.
- Reworks `configs` into beginner-friendly configuration, source, target, and backup sections while retaining exact `--raw` output and exposing advanced details through `--verbose`.
- Makes `--color always` work when output is redirected while preserving TTY-only `auto` behavior and ANSI-free `never` output.

## 0.1.1 - 2026-08-07

- Added `import`, which adopts an agent-modified deployed skill copy as the new canonical source content after showing a git-style change plan.
- Added `install` as a visible alias for `load` on the command line and in JSON recipes.
- Added a pre-confirmation change plan to `update`, listing every skill/target/scope action with its file and line deltas before anything is deployed.
- Added terminal installation instructions to the repository README.

## 0.1.0 - 2026-07-25

- Initial release of `skill-manager`, the native Rust skill manager CLI.
- Added stable source relocation with `source update --location`, `source locate` (plus `relocate`, `move`, and `mv`), paired alternate locations, and `source swap`.
- Added strict schema-v1 alternate locations, deterministic relocated-ID fallback, mutation-boundary collision checks, structured before/after events, aligned inactive-location display, strict recipe support, and remote cache identity validation.
