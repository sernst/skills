# Canonical Rust behavior and Python deviations

The Rust executable preserves the useful command capability of the Python
manager, but it is the canonical implementation. This ledger records deliberate
semantic hardening and interface normalization. A listed Python test is either
covered by the stronger Rust contract or replaced when its expected behavior is
unsafe.

| ID | Canonical Rust behavior | Python relationship and affected coverage |
| --- | --- | --- |
| D-001 | Reject unknown JSON recipe fields. | Replaces TestApplyJsonInput.test_unknown_keys_ignored. |
| D-002 | Reject malformed/type-invalid config without rewriting it. | Replaces TestLoadConfig.test_returns_empty_dict_on_invalid_json. |
| D-003 | Treat JSON output, inline JSON, stdin JSON, and file recipes as one strict invocation model with an envelope on every semantic event. | Extends TestApplyJsonInput and TestJsonOutput classes; no former behavior is relaxed. |
| D-004 | Apply NFKC Unicode case folding consistently to identities, excludes, and filters. | Strengthens case-insensitive exclusions and filter tests. |
| D-005 | Explicit target selection can select disabled targets; built-in names are reserved for new custom targets; target removal has an explicit disable/delete lifecycle. | Strengthens TestCliImprovements.test_target_override_selects_disabled_target. |
| D-006 | Non-TTY human output is plain, color obeys NO_COLOR, human diagnostics use stderr, and JSON semantic errors use stdout only. | Strengthens TestOutHelper and TestJsonOutput classes. |
| D-007 | Validate zero-or-positive TTL, use GITHUB_TOKEN before GH_TOKEN, never persist credentials, and fail refreshes without silently using stale content. | Extends GitHub materialization coverage; no direct old test asserted these safeguards. |
| D-008 | Reject symbolic links, reparse points, special files, unsafe archive entries, reserved Windows names, and nonportable skill tree paths. Unix also rejects external hard links. Stable Rust does not expose the Windows link count, so ordinary Windows hard links cannot currently be distinguished; deployment still copies them into fresh files. | New security coverage; no Python test asserted this behavior. |
| D-009 | Dry runs do not mutate config, cache, locks, targets, journals, or backups. | Strengthens remove/update dry-run tests. |
| D-010 | Use per-skill staged, locked, journaled transactions with recovery and partial-commit reporting. | New crash-safety coverage; Python copied directly. |
| D-011 | Ask one aggregate target confirmation instead of rendering Python's source-by-source “from” preamble and CWD special cases. | Replaces four presentation-only TestConfirmAllTargets expectations while preserving confirmation, cancellation, noninteractive failure, and dry-run behavior. |
| D-012 | Represent empty-source, unmatched-filter, and missing-remove outcomes with typed zero-count summaries instead of command-specific warning/info prose records. | Replaces the Python no-work message assertions while preserving successful exit status and deterministic machine output. |
| D-013 | Render emoji status cells only on interactive terminals; redirected human output uses plain textual states. | Replaces three Python status-rendering assertions that always expected emoji and preserves deterministic non-TTY output. |

These are additions to—not exceptions from—the documented public command
contract. These five actual parity replacements remain the only entries marked
Replaced in the traceability ledger.
