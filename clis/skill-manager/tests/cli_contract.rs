//! Black-box command-line contracts. These deliberately avoid internal APIs.
#![allow(
    clippy::expect_used,
    reason = "A missing Cargo test binary is a harness failure and has no recoverable assertion path."
)]

use assert_cmd::Command;
use predicates::prelude::*;

/// Help advertises the canonical command surface and status aliases.
#[test]
fn help_lists_all_top_level_commands() {
    let mut command = Command::cargo_bin("skill-manager").expect("test binary");
    command
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("load"))
        .stdout(predicate::str::contains("update"))
        .stdout(predicate::str::contains("copy"))
        .stdout(predicate::str::contains("remove"))
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("resolve"))
        .stdout(predicate::str::contains("source"))
        .stdout(predicate::str::contains("target"));
}

/// Version is the release version and remains available without configuration.
#[test]
fn version_is_available_without_configuration() {
    let mut command = Command::cargo_bin("skill-manager").expect("test binary");
    command
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("0.1.0"));
}

/// Parser misuse follows Clap's conventional usage exit code and stderr stream.
#[test]
fn unknown_command_is_a_usage_error() {
    let mut command = Command::cargo_bin("skill-manager").expect("test binary");
    command
        .arg("does-not-exist")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("error").or(predicate::str::contains("unrecognized")));
}

/// The status aliases must parse successfully and retain the status command contract.
#[test]
fn status_aliases_are_accepted() {
    for alias in ["status", "ls", "list"] {
        let mut command = Command::cargo_bin("skill-manager").expect("test binary");
        command
            .args([alias, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--target"));
    }
}

/// Positional operand help distinguishes literal sources/skills from patterns.
#[test]
fn skill_operand_help_mentions_patterns() {
    for command in ["load", "update", "remove", "resolve"] {
        let mut invocation = Command::cargo_bin("skill-manager").expect("test binary");
        invocation
            .args([command, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("PATTERN"));
    }
}

/// Machine output requests must not send structured records to stderr.
#[test]
fn json_errors_are_machine_readable_on_stdout() {
    let mut command = Command::cargo_bin("skill-manager").expect("test binary");
    command
        .args(["--json={not-json}", "status"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"event\"").or(predicate::str::contains("\"level\"")));
}

/// Source names support either spelling while rejecting ambiguous duplication.
#[test]
fn source_add_name_forms_are_positional_or_flag_but_not_both() {
    let home = tempfile::tempdir().expect("isolated home");
    let mut named = Command::cargo_bin("skill-manager").expect("test binary");
    named
        .current_dir(home.path())
        .env("SKILL_MANAGER_HOME", home.path())
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .args(["--json", "source", "add", ".", "--name", "flag-name"])
        .assert()
        .success();

    let mut conflicting = Command::cargo_bin("skill-manager").expect("test binary");
    conflicting
        .args([
            "source",
            "add",
            ".",
            "positional-name",
            "--name",
            "flag-name",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
}
