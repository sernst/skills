# Changelog

All notable changes to the downloadable CLIs are documented here.

## 0.1.3 - 2026-08-10

- Adds a global `--home DIR` flag that overrides the manager home for the whole invocation, taking precedence over `SKILL_MANAGER_HOME` and the operating system home, so validation and smoke testing can never touch real user state.
- Makes contract tests platform-independent by no longer assuming Windows path separators.
- Fixes `load`/`install` and `update`/`up` treating a bare skill name as a current-directory path; a literal operand now resolves case-insensitively against discovered skill names when it does not name a configured source, path, or GitHub reference.
- Adds a hard error, `no configured source, directory, or skill named "NAME"`, for a bare operand that resolves to nothing, with a hint to run `skill-manager ls`.
- Warns and prefers the skill when a bare operand matches both a discovered skill name and a same-named directory in the current directory, pointing at `./NAME` to force the directory interpretation.

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
