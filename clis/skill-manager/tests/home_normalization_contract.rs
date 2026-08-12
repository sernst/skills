//! Proves a RELATIVE `--home` (or `SKILL_MANAGER_HOME`) value is normalized to
//! an absolute, lexically clean path before it is threaded into any derived
//! path. The regression this guards: a raw relative override leaked a `./`
//! (or `..`, or a foreign separator) segment into the cache journal's staging
//! path and tripped its own path-safety validation, hard-failing every command
//! that touches the cache — i.e. any configuration with a GitHub source.
//!
//! Both override sources funnel through the same resolution point
//! (`manager_home` in `config.rs`), so a relative value must resolve
//! identically whether it arrives via the flag or the environment variable.
//!
//! A GitHub source forces the cache journal staging path to be built and
//! path-safety-validated. That validation (and the local source lock it takes)
//! happens BEFORE any network call, so these tests assert on the precise
//! regression — the journal error is gone and the cache path was built under
//! the correctly resolved home — rather than on overall success, which for a
//! fresh source would additionally depend on a real network fetch.

#![allow(
    clippy::expect_used,
    reason = "Test fixture construction and missing test binaries are unrecoverable harness failures."
)]

use std::fs;
use std::path::Path;
use std::process::Output;

use assert_cmd::Command;
use serde_json::json;
use tempfile::TempDir;

/// The pre-fix failure text. Its absence from stderr is the regression signal:
/// once the home is normalized, no command can produce it.
const JOURNAL_PATH_ERROR: &str = "cache journal";

/// Seed `<home>/.skill-manager/config.json` with a single GitHub source, whose
/// presence forces every command to build a cache journal staging path — the
/// exact code path that rejected an un-normalized relative home.
fn seed_github_config(home: &Path) {
    let storage_root = home.join(".skill-manager");
    fs::create_dir_all(&storage_root).expect("create scratch storage root");
    let config = json!({
        "schema_version": 2,
        "sources": [{
            "id": "src_relative_home_probe",
            "type": "github",
            "mode": "collection",
            "name": "probe",
            "label": "Probe",
            "exclude": [],
            "owner": "octocat",
            "repo": "hello-world",
            "ref": "main",
            "repo_path": "skills"
        }],
        "targets": {},
        "legacy_target_overrides": {},
        "builtins": {},
        "exclude": []
    });
    let bytes = serde_json::to_vec_pretty(&config).expect("serialize probe config");
    fs::write(storage_root.join("config.json"), bytes).expect("write probe config");
}

/// A fresh invocation rooted at `cwd`, with the OS-home seams unset so a bug
/// that skipped the override could not silently borrow the real home, and no
/// GitHub token so nothing performs an authenticated fetch.
fn command_at(cwd: &Path) -> Command {
    let mut command = Command::cargo_bin("skill-manager").expect("test binary");
    command
        .current_dir(cwd)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env("NO_COLOR", "1");
    command
}

/// The cache lock a GitHub source takes lives under the resolved home; its
/// existence proves the home resolved to `home` AND that the cache journal
/// path was built and passed path-safety validation (both happen before the
/// lock is acquired).
fn cache_locks_dir(home: &Path) -> std::path::PathBuf {
    home.join(".skill-manager").join("cache").join(".locks")
}

/// Assert the precise regression is fixed for an invocation whose GitHub
/// source exercised the cache: no journal path-safety error, and the cache
/// path was built under the expected (correctly normalized) home.
fn assert_cache_reached_without_journal_error(output: &Output, expected_home: &Path) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains(JOURNAL_PATH_ERROR),
        "cache journal path-safety error resurfaced: {stderr}"
    );
    assert!(
        cache_locks_dir(expected_home).exists(),
        "cache path was not built under the normalized home {}",
        expected_home.display()
    );
}

#[test]
fn relative_home_flag_resolves_against_the_cwd_and_exercises_the_cache_path() {
    let scratch = TempDir::new().expect("scratch temp dir");
    let expected_home = scratch.path().join("h");
    seed_github_config(&expected_home);

    // The bug's exact shape: a `./h` relative home plus a GitHub source, which
    // together tripped the cache journal path-safety check before this fix.
    let output = command_at(scratch.path())
        .args(["--home", "./h", "--no-input", "status"])
        .output()
        .expect("run status");

    assert_cache_reached_without_journal_error(&output, &expected_home);
}

#[test]
fn relative_home_flag_resolves_to_the_same_store_as_the_absolute_form() {
    let relative_scratch = TempDir::new().expect("relative scratch dir");
    let relative_home = relative_scratch.path().join("h");
    seed_github_config(&relative_home);
    let relative_output = command_at(relative_scratch.path())
        .args(["--home", "./h", "--no-input", "status"])
        .output()
        .expect("run relative status");

    let absolute_scratch = TempDir::new().expect("absolute scratch dir");
    let absolute_home = absolute_scratch.path().join("h");
    seed_github_config(&absolute_home);
    let absolute_output = command_at(absolute_scratch.path())
        .args(["--home"])
        .arg(&absolute_home)
        .args(["--no-input", "status"])
        .output()
        .expect("run absolute status");

    // Both forms must build the store at their cwd-joined `h`, i.e. the
    // relative value is treated exactly like the absolute one.
    assert_cache_reached_without_journal_error(&relative_output, &relative_home);
    assert_cache_reached_without_journal_error(&absolute_output, &absolute_home);
}

#[test]
fn relative_home_flag_works_for_a_second_command() {
    let scratch = TempDir::new().expect("scratch temp dir");
    seed_github_config(&scratch.path().join("h"));

    // `configs` is a second, non-fetching command; a relative home must let it
    // run cleanly to completion.
    command_at(scratch.path())
        .args(["--home", "./h", "--no-input", "configs"])
        .assert()
        .success();
}

#[test]
fn dot_parent_and_trailing_separator_home_forms_all_resolve() {
    let scratch = TempDir::new().expect("scratch temp dir");
    let expected_home = scratch.path().join("h");
    seed_github_config(&expected_home);

    // `./nested/../h/` collapses to `<cwd>/h`; the trailing separator and the
    // `.`/`..` segments must all normalize away.
    let output = command_at(scratch.path())
        .args(["--home", "./nested/../h/", "--no-input", "status"])
        .output()
        .expect("run status");

    assert_cache_reached_without_journal_error(&output, &expected_home);
}

#[cfg(windows)]
#[test]
fn mixed_separator_home_form_resolves_on_windows() {
    let scratch = TempDir::new().expect("scratch temp dir");
    let expected_home = scratch.path().join("h");
    seed_github_config(&expected_home);

    let output = command_at(scratch.path())
        .args(["--home", r".\h/", "--no-input", "status"])
        .output()
        .expect("run status");

    assert_cache_reached_without_journal_error(&output, &expected_home);
}

#[test]
fn blank_home_flag_is_rejected_with_exit_code_two() {
    let scratch = TempDir::new().expect("scratch temp dir");
    command_at(scratch.path())
        .args(["--home", "   ", "--no-input", "status"])
        .assert()
        .code(2);
}

#[test]
fn skill_manager_home_env_relative_value_normalizes_like_the_flag() {
    let scratch = TempDir::new().expect("scratch temp dir");
    let expected_home = scratch.path().join("h");
    seed_github_config(&expected_home);

    let output = command_at(scratch.path())
        .env("SKILL_MANAGER_HOME", "./h")
        .args(["--no-input", "status"])
        .output()
        .expect("run status");

    assert_cache_reached_without_journal_error(&output, &expected_home);
}
