//! Guards the audited mapping from the Python implementation to the Rust port.

#![allow(
    clippy::expect_used,
    reason = "A malformed checked-in ledger is an unrecoverable repository integrity failure."
)]

use std::collections::{BTreeMap, BTreeSet};

const INVENTORY: &str = include_str!("fixtures/python-test-inventory.txt");
const LEDGER: &str = include_str!("../docs/parity-ledger.md");
const RUST_TEST_SOURCES: &str = concat!(
    include_str!("behavior_contract.rs"),
    include_str!("cache_contract.rs"),
    include_str!("cli_contract.rs"),
    include_str!("operations_contract.rs"),
    include_str!("parity_ledger.rs"),
    include_str!("recipe_contract.rs"),
    include_str!("transaction_security_contract.rs"),
    include_str!("../src/config.rs"),
    include_str!("../src/skills.rs"),
    include_str!("../src/transaction.rs"),
);

const REQUIRED_MAPPINGS: &[(&str, &str)] = &[
    (
        "TestCliImprovements.test_status_json_includes_name_label_and_id",
        "operations_contract::status_json_preserves_stable_and_human_source_provenance",
    ),
    (
        "TestCliImprovements.test_source_add_rejects_positional_and_flag_name",
        "cli_contract::source_add_name_forms_are_positional_or_flag_but_not_both",
    ),
    (
        "TestDirectoryAdd.test_prompts_for_name_then_label_when_not_provided",
        "operations_contract::human_prompts_cover_text_confirmation_cancellation_and_invalid_answers",
    ),
    (
        "TestStatus.test_shows_source_preamble",
        "operations_contract::human_status_renders_sources_header_rows_summary_and_empty_state",
    ),
    (
        "TestStatus.test_no_skills_prints_message",
        "operations_contract::human_status_renders_sources_header_rows_summary_and_empty_state",
    ),
    (
        "TestStatus.test_table_header_contains_target_keys",
        "operations_contract::human_status_renders_sources_header_rows_summary_and_empty_state",
    ),
    (
        "TestStatus.test_summary_line_printed",
        "operations_contract::human_status_renders_sources_header_rows_summary_and_empty_state",
    ),
    (
        "TestJsonOutputLoad.test_overwritten_action",
        "operations_contract::skill_action_events_preserve_loaded_overwritten_updated_copied_and_removed_provenance",
    ),
    (
        "TestJsonOutputUpdate.test_updated_and_skipped_records",
        "operations_contract::skill_action_events_preserve_loaded_overwritten_updated_copied_and_removed_provenance",
    ),
];

const REQUIRED_CONTRACT_TESTS: &[&str] = &[
    "operations_contract::explicit_named_and_builtin_target_selectors_form_a_deduplicated_union",
    "operations_contract::human_prompts_cover_text_confirmation_cancellation_and_invalid_answers",
    "operations_contract::human_status_renders_sources_header_rows_summary_and_empty_state",
    "operations_contract::nested_v0_type_errors_fail_without_rewriting_or_creating_a_backup",
    "operations_contract::status_json_preserves_stable_and_human_source_provenance",
    "operations_contract::skill_action_events_preserve_loaded_overwritten_updated_copied_and_removed_provenance",
    "transaction_security_contract::recovery_rejects_outside_stage_backup_and_destination_before_mutation",
];

fn inventory_names() -> Vec<&'static str> {
    INVENTORY
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

fn ledger_rows() -> Vec<(&'static str, &'static str, &'static str)> {
    LEDGER
        .lines()
        .filter(|line| line.starts_with("| ") && line.contains(".test_"))
        .map(|line| {
            let columns: Vec<_> = line.split('|').map(str::trim).collect();
            assert_eq!(
                columns.len(),
                5,
                "ledger row must have exactly three columns"
            );
            (
                columns[1].trim_matches(char::from(96)),
                columns[2],
                columns[3].trim_matches(char::from(96)),
            )
        })
        .collect()
}

/// Inventory and ledger must be exact, classified, and point to real tests.
#[test]
fn records_every_legacy_python_test_once_with_real_rust_symbols() {
    let inventory = inventory_names();
    assert_eq!(inventory.len(), 196, "committed inventory size changed");
    let unique_inventory: BTreeSet<_> = inventory.iter().copied().collect();
    assert_eq!(unique_inventory.len(), 196, "inventory contains duplicates");

    let rows = ledger_rows();
    assert_eq!(
        rows.len(),
        196,
        "every Python test must have one ledger row"
    );
    let ledger_names: Vec<_> = rows.iter().map(|(name, _, _)| *name).collect();
    assert_eq!(
        ledger_names, inventory,
        "ledger names/order must exactly match the generated inventory"
    );
    let coverage_by_name: BTreeMap<_, _> = rows
        .iter()
        .map(|(name, _, coverage)| (*name, *coverage))
        .collect();
    for (python_test, rust_test) in REQUIRED_MAPPINGS {
        let coverage = coverage_by_name
            .get(python_test)
            .expect("required Python contract is absent");
        assert!(
            coverage
                .split(" + ")
                .any(|reference| reference == *rust_test),
            "{python_test} must map to its focused Rust assertion {rust_test}"
        );
    }
    for reference in REQUIRED_CONTRACT_TESTS {
        let (_, symbol) = reference
            .rsplit_once("::")
            .expect("required contract must be module::test_symbol");
        assert!(
            RUST_TEST_SOURCES.contains(&format!("fn {symbol}(")),
            "required hardening contract is missing: {reference}"
        );
    }

    let mut reference_counts = BTreeMap::<&str, usize>::new();
    for (name, disposition, coverage) in rows {
        assert!(name.contains(".test_"), "invalid Python test name: {name}");
        if disposition == "Merged" {
            assert!(
                !coverage.starts_with('D'),
                "merged row must name Rust tests"
            );
            for reference in coverage.split(" + ") {
                let (module, symbol) = reference
                    .rsplit_once("::")
                    .expect("Rust coverage must be module::test_symbol");
                assert!(
                    matches!(
                        module,
                        "behavior_contract"
                            | "cache_contract"
                            | "cli_contract"
                            | "operations_contract"
                            | "recipe_contract"
                            | "transaction_security_contract"
                            | "config::tests"
                            | "skills::tests"
                            | "transaction::tests"
                    ),
                    "unknown Rust test module: {module}"
                );
                assert!(
                    RUST_TEST_SOURCES.contains(&format!("fn {symbol}(")),
                    "ledger references missing Rust test symbol: {reference}"
                );
                *reference_counts.entry(reference).or_default() += 1;
            }
        } else {
            let deviation = disposition
                .strip_prefix("Replaced (")
                .and_then(|value| value.strip_suffix(')'))
                .expect("disposition must be Merged or Replaced (D-NNN)");
            assert_eq!(coverage, deviation, "replacement must name its deviation");
            assert!(
                deviation.len() == 5
                    && deviation.starts_with("D-")
                    && deviation[2..]
                        .chars()
                        .all(|character| character.is_ascii_digit()),
                "invalid deviation identifier: {deviation}"
            );
            assert!(
                LEDGER.contains(&format!("| {deviation} |")),
                "deviation has no definition: {deviation}"
            );
        }
    }

    assert!(
        reference_counts.values().all(|count| *count <= 25),
        "one broad Rust test is claiming more than 25 distinct Python contracts: {reference_counts:?}"
    );
}
