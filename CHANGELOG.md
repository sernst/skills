# Changelog

All notable changes to the downloadable CLIs are documented here.

## 0.1.1 - 2026-08-07

- Added `import`, which adopts an agent-modified deployed skill copy as the new canonical source content after showing a git-style change plan.
- Added `install` as a visible alias for `load` on the command line and in JSON recipes.
- Added a pre-confirmation change plan to `update`, listing every skill/target/scope action with its file and line deltas before anything is deployed.
- Added terminal installation instructions to the repository README.

## 0.1.0 - 2026-07-25

- Initial release of `skill-manager`, the native Rust skill manager CLI.
- Added stable source relocation with `source update --location`, `source locate` (plus `relocate`, `move`, and `mv`), paired alternate locations, and `source swap`.
- Added strict schema-v1 alternate locations, deterministic relocated-ID fallback, mutation-boundary collision checks, structured before/after events, aligned inactive-location display, strict recipe support, and remote cache identity validation.
