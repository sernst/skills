//! End-to-end filesystem and machine-contract coverage.

#![allow(
    clippy::expect_used,
    reason = "Test fixture construction and missing test binaries are unrecoverable harness failures."
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use skill_manager::config::{acquire_lock, canonical_config_bytes};
use tempfile::TempDir;

mod support;

use support::portable_canonicalize;

/// Windows verbatim path spelling that user-facing output must never contain.
const VERBATIM_PREFIX: &str = r"\\?\";

fn sandbox() -> TempDir {
    tempfile::tempdir().expect("create isolated home")
}

fn cli(home: &Path) -> Command {
    let mut command = Command::cargo_bin("skill-manager").expect("test binary");
    command
        .current_dir(home)
        .env("SKILL_MANAGER_HOME", home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env("NO_COLOR", "1");
    command
}

fn create_skill(collection: &Path, name: &str, body: &str) -> PathBuf {
    let root = collection.join(name);
    fs::create_dir_all(&root).expect("create skill directory");
    fs::write(root.join("SKILL.md"), body).expect("write SKILL.md");
    root
}

fn update_fixture_named_alpha() -> TempDir {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha\n");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "alpha",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--claude", "--global"])
        .assert()
        .success();
    home
}

#[cfg(unix)]
fn try_directory_symlink(source: &Path, alias: &Path) -> bool {
    std::os::unix::fs::symlink(source, alias).is_ok()
}

#[cfg(windows)]
fn try_directory_symlink(source: &Path, alias: &Path) -> bool {
    std::os::windows::fs::symlink_dir(source, alias).is_ok()
}

fn read_config(home: &Path) -> Value {
    let bytes = fs::read(home.join(".skill-manager").join("config.json"))
        .expect("read generated config file");
    serde_json::from_slice(&bytes).expect("parse generated config")
}

fn json_events(output: std::process::Output) -> Vec<Value> {
    assert!(output.status.success(), "command failed: {output:?}");
    String::from_utf8(output.stdout)
        .expect("utf8 events")
        .lines()
        .map(|line| serde_json::from_str(line).expect("NDJSON event"))
        .collect()
}

fn v0_skills_directories(path: &Path, metadata: Value) -> Value {
    let mut directories = serde_json::Map::new();
    directories.insert(path.to_string_lossy().into_owned(), metadata);
    serde_json::json!({ "skills_directories": directories })
}

#[test]
fn no_command_defaults_to_status_and_every_json_line_has_the_envelope() {
    let home = sandbox();
    let output = cli(home.path())
        .arg("--json")
        .output()
        .expect("run default command");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("utf8 output");
    let lines: Vec<_> = stdout.lines().collect();
    assert!(!lines.is_empty());
    for line in lines {
        let event: Value = serde_json::from_str(line).expect("NDJSON event");
        assert_eq!(event["version"], 1);
        assert!(event["event"].is_string());
        assert!(matches!(
            event["level"].as_str(),
            Some("info" | "warning" | "error")
        ));
        assert!(event["data"].is_object());
    }
    assert!(stdout.contains("\"action\":\"status\""));
}

#[test]
fn source_lifecycle_persists_updates_and_removal() {
    let home = sandbox();
    let source = home.path().join("skills");
    create_skill(&source, "alpha", "# Alpha");

    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "team",
            "--label",
            "Team Skills",
            "--exclude",
            "draft-*",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"source.added\""));

    let added = read_config(home.path());
    let source_id = added["sources"][0]["id"]
        .as_str()
        .expect("generated source ID")
        .to_owned();
    assert_eq!(added["schema_version"], 2);
    assert_eq!(added["sources"][0]["name"], "team");
    assert_eq!(added["sources"][0]["label"], "Team Skills");
    assert_eq!(added["sources"][0]["exclude"][0], "draft-*");

    cli(home.path())
        .args([
            "--json",
            "source",
            "update",
            "team",
            "--label",
            "Renamed",
            "--clear-exclude",
            "--exclude",
            "private-*",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"source.updated\""));

    cli(home.path())
        .args(["--json", "source", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"source.listed\""))
        .stdout(predicate::str::contains("Renamed"));

    cli(home.path())
        .args([
            "--json",
            "source",
            "update",
            "Renamed",
            "--label",
            "Final Label",
        ])
        .assert()
        .success();
    assert_eq!(read_config(home.path())["sources"][0]["id"], source_id);

    cli(home.path())
        .args(["--json", "source", "remove", "Final Label"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"source.removed\""));

    assert_eq!(
        read_config(home.path())["sources"]
            .as_array()
            .expect("sources array")
            .len(),
        0
    );
}

#[test]
fn source_add_and_remove_without_a_reference_use_the_current_directory() {
    let home = sandbox();
    create_skill(home.path(), "alpha", "# Alpha");

    cli(home.path())
        .args(["source", "add"])
        .write_stdin("cwd-source\n")
        .assert()
        .success()
        .stderr(predicate::str::contains("Source name"));
    let config = read_config(home.path());
    assert_eq!(config["sources"][0]["name"], "cwd-source");
    assert_eq!(config["sources"][0]["label"], "Cwd Source");
    assert_eq!(
        portable_canonicalize(PathBuf::from(
            config["sources"][0]["path"]
                .as_str()
                .expect("stored local source path")
        ))
        .expect("canonical stored source"),
        portable_canonicalize(home.path()).expect("canonical sandbox")
    );

    cli(home.path())
        .args(["--json", "source", "remove"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"source.removed\""));
    assert!(
        read_config(home.path())["sources"]
            .as_array()
            .expect("sources array")
            .is_empty()
    );
}

#[test]
fn source_list_machine_and_empty_cases() {
    let home = sandbox();
    cli(home.path())
        .args(["--json", "source", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"sources\":0"))
        .stdout(predicate::str::contains("\"event\":\"summary\""));

    let missing = home.path().join("missing-source");
    let config = serde_json::json!({
        "schema_version": 1,
        "sources": [{
            "id": "src_missing",
            "type": "local",
            "mode": "collection",
            "name": "missing",
            "label": "Missing Source",
            "path": missing
        }]
    });
    fs::write(
        home.path().join(".skill-manager").join("config.json"),
        config.to_string(),
    )
    .expect("write configured missing source");
    cli(home.path())
        .args(["--json", "source", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"source.listed\""))
        .stdout(predicate::str::contains("src_missing"))
        .stdout(predicate::str::contains("Missing Source"));
}

#[test]
fn target_lifecycle_enforces_builtin_and_custom_semantics() {
    let home = sandbox();
    cli(home.path())
        .args(["--json", "target", "add", "custom", "first-target"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"target.added\""));

    cli(home.path())
        .args(["--json", "target", "disable", "custom"])
        .assert()
        .success();
    assert_eq!(
        read_config(home.path())["targets"]["custom"]["enabled"],
        false
    );

    cli(home.path())
        .args(["--json", "target", "enable", "custom"])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "set-path", "custom", "second-target"])
        .assert()
        .success();
    assert_eq!(
        read_config(home.path())["targets"]["custom"]["path"],
        "second-target"
    );

    cli(home.path())
        .args(["--json", "target", "remove", "custom"])
        .assert()
        .success();
    assert!(read_config(home.path())["targets"]["custom"].is_null());

    cli(home.path())
        .args(["--json", "target", "add", "claude", "somewhere"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"event\":\"command.failed\""));

    cli(home.path())
        .args(["--json", "target", "remove", "claude"])
        .assert()
        .success();
    assert_eq!(
        read_config(home.path())["builtins"]["claude"]["enabled"],
        false
    );
}

#[test]
fn disabled_builtin_flags_fail_but_explicit_target_name_opts_in() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha");
    cli(home.path())
        .args(["--json", "target", "disable", "claude"])
        .assert()
        .success();

    cli(home.path())
        .args([
            "--json",
            "load",
            source.to_str().expect("utf8 path"),
            "--claude",
            "--no-input",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("disabled"));
    assert!(!home.path().join(".claude/skills/alpha").exists());

    cli(home.path())
        .args([
            "--json",
            "load",
            source.to_str().expect("utf8 path"),
            "--target",
            "claude",
            "--global",
        ])
        .assert()
        .success();
    assert!(home.path().join(".claude/skills/alpha/SKILL.md").is_file());
}

#[test]
fn explicit_named_and_builtin_target_selectors_form_a_deduplicated_union() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha");
    cli(home.path())
        .args(["--json", "target", "add", "custom", "custom"])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "disable", "custom"])
        .assert()
        .success();

    let output = cli(home.path())
        .args([
            "--json",
            "load",
            source.to_str().expect("utf8 path"),
            "--target",
            "custom",
            "--target",
            "custom",
            "--claude",
            "--shared",
            "--ag",
            "--global",
            "--dry-run",
        ])
        .output()
        .expect("run selector union");
    assert!(output.status.success());
    let targets: BTreeSet<String> = String::from_utf8(output.stdout)
        .expect("utf8 events")
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["event"] == "skill.loaded")
        .filter_map(|event| event["data"]["target"].as_str().map(ToOwned::to_owned))
        .collect();
    assert_eq!(
        targets,
        ["antigravity", "claude", "custom", "shared"]
            .into_iter()
            .map(ToOwned::to_owned)
            .collect()
    );

    let all_output = cli(home.path())
        .args([
            "--json",
            "load",
            source.to_str().expect("utf8 path"),
            "--target",
            "custom",
            "--all",
            "--global",
            "--dry-run",
        ])
        .output()
        .expect("run all union");
    assert!(all_output.status.success());
    let all_targets: BTreeSet<String> = String::from_utf8(all_output.stdout)
        .expect("utf8 events")
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["event"] == "skill.loaded")
        .filter_map(|event| event["data"]["target"].as_str().map(ToOwned::to_owned))
        .collect();
    assert_eq!(all_targets, targets);
}

#[test]
fn copy_load_update_status_and_remove_mutate_expected_trees() {
    let home = sandbox();
    let source = home.path().join("source");
    let copy_target = home.path().join("copy");
    let managed_target = home.path().join("managed");
    create_skill(&source, "alpha", "# version one");

    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "add", "test-target", "managed"])
        .assert()
        .success();

    cli(home.path())
        .args([
            "--json",
            "copy",
            source.to_str().expect("utf8 path"),
            copy_target.to_str().expect("utf8 path"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"skill.copied\""));
    assert_eq!(
        fs::read_to_string(copy_target.join("alpha").join("SKILL.md")).expect("read copied skill"),
        "# version one"
    );

    cli(home.path())
        .args([
            "--json",
            "load",
            "--target",
            "test-target",
            "--global",
            "--no-input",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"skill.loaded\""));
    assert!(managed_target.join("alpha").join("SKILL.md").is_file());

    fs::write(source.join("alpha").join("SKILL.md"), "# version two").expect("modify source skill");
    cli(home.path())
        .args(["--json", "update", "--target", "test-target", "--no-input"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"skill.updated\""));
    assert_eq!(
        fs::read_to_string(managed_target.join("alpha").join("SKILL.md"))
            .expect("read updated skill"),
        "# version two"
    );

    cli(home.path())
        .args(["--json", "status", "--target", "test-target"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"status.row\""))
        .stdout(predicate::str::contains("up-to-date"));

    cli(home.path())
        .args([
            "--json",
            "remove",
            "alpha",
            "--target",
            "test-target",
            "--global",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"skill.removed\""));
    assert!(!managed_target.join("alpha").exists());
}

#[test]
fn skill_action_events_preserve_loaded_overwritten_updated_copied_and_removed_provenance() {
    let home = sandbox();
    let source = home.path().join("source");
    let target = home.path().join("target");
    let copy_target = home.path().join("copy");
    create_skill(&source, "alpha", "# One");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "add", "custom", "target"])
        .assert()
        .success();

    let loaded = json_events(
        cli(home.path())
            .args(["--json", "load", "--target", "custom", "--global"])
            .output()
            .expect("load"),
    );
    assert!(
        loaded.iter().any(|event| {
            event["event"] == "skill.loaded" && event["data"]["action"] == "loaded"
        })
    );

    fs::write(source.join("alpha/SKILL.md"), "# Two").expect("change source");
    let overwritten = json_events(
        cli(home.path())
            .args(["--json", "load", "--target", "custom", "--global"])
            .output()
            .expect("overwrite"),
    );
    assert!(overwritten.iter().any(|event| {
        event["event"] == "skill.loaded" && event["data"]["action"] == "overwritten"
    }));

    fs::write(source.join("alpha/SKILL.md"), "# Three").expect("change source again");
    create_skill(&source, "beta", "# Not deployed");
    let updated = json_events(
        cli(home.path())
            .args(["--json", "update", "--target", "custom"])
            .output()
            .expect("update"),
    );
    assert!(updated.iter().any(|event| {
        event["event"] == "skill.updated" && event["data"]["action"] == "updated"
    }));
    assert!(!target.join("beta").exists());

    let copied = json_events(
        cli(home.path())
            .args([
                "--json",
                "copy",
                source.to_str().expect("utf8 path"),
                copy_target.to_str().expect("utf8 path"),
            ])
            .output()
            .expect("copy"),
    );
    assert!(
        copied.iter().any(|event| {
            event["event"] == "skill.copied" && event["data"]["action"] == "copied"
        })
    );

    let removed = json_events(
        cli(home.path())
            .args([
                "--json", "remove", "alpha", "--target", "custom", "--global", "--yes",
            ])
            .output()
            .expect("remove"),
    );
    assert!(removed.iter().any(|event| {
        event["event"] == "skill.removed" && event["data"]["action"] == "removed"
    }));
}

#[test]
fn status_filter_sorting_and_target_scoping_are_deterministic() {
    let home = sandbox();
    let source = home.path().join("source");
    let target_one = home.path().join("target-one");
    let target_two = home.path().join("target-two");
    create_skill(&source, "zeta", "# Zeta");
    create_skill(&source, "alpha", "# Alpha");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
            "--label",
            "Primary Label",
        ])
        .assert()
        .success();
    for (name, path) in [("one", &target_one), ("two", &target_two)] {
        cli(home.path())
            .args([
                "--json",
                "target",
                "add",
                name,
                path.file_name()
                    .and_then(|value| value.to_str())
                    .expect("utf8 path"),
            ])
            .assert()
            .success();
    }
    cli(home.path())
        .args(["--json", "load", "--target", "one", "--global"])
        .assert()
        .success();
    create_skill(&target_one, "orphan", "# Orphan");

    let output = cli(home.path())
        .args(["--json", "status", "--target", "one"])
        .output()
        .expect("run status");
    assert!(output.status.success());
    let events: Vec<Value> = String::from_utf8(output.stdout)
        .expect("utf8 output")
        .lines()
        .map(|line| serde_json::from_str(line).expect("NDJSON event"))
        .filter(|event: &Value| event["event"] == "status.row")
        .collect();
    let names: Vec<_> = events
        .iter()
        .filter_map(|event| event["data"]["skill"].as_str())
        .collect();
    assert_eq!(names, ["alpha", "orphan", "zeta"]);
    assert!(events.iter().all(|event| {
        event["data"]["targets"].get("one").is_some()
            && event["data"]["targets"].get("two").is_none()
    }));
    assert!(events.iter().any(|event| {
        event["data"]["skill"] == "orphan" && event["data"]["targets"]["one"] == "no-connection"
    }));

    cli(home.path())
        .args(["status", "--target", "one", "--global"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "orphan  unknown  global    no-connection",
        ))
        .stdout(predicate::str::contains(
            "up-to-date: 2, no-connection: 1, global: 3",
        ))
        .stdout(predicate::str::contains("~").not())
        .stdout(predicate::str::contains("\u{1b}[").not());

    cli(home.path())
        .args(["--json", "status", "--target", "one", "--filter", "a*"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"skill\":\"alpha\""))
        .stdout(predicate::str::contains("\"skill\":\"zeta\"").not());
}

#[test]
fn status_json_preserves_stable_and_human_source_provenance() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "source-name",
            "--label",
            "Source Label",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "add", "custom", "target"])
        .assert()
        .success();
    let source_id = read_config(home.path())["sources"][0]["id"]
        .as_str()
        .expect("source ID")
        .to_owned();

    let output = cli(home.path())
        .args([
            "--json",
            "status",
            "--target",
            "custom",
            "--filter",
            "source label",
        ])
        .output()
        .expect("run status");
    assert!(output.status.success());
    let row: Value = String::from_utf8(output.stdout)
        .expect("utf8 events")
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .find(|event: &Value| event["event"] == "status.row")
        .expect("status row");
    assert_eq!(row["data"]["source"]["source_id"], source_id);
    assert_eq!(row["data"]["source"]["source_name"], "source-name");
    assert_eq!(row["data"]["source"]["source_label"], "Source Label");
}

#[test]
fn human_status_renders_compact_source_legend_table_and_plain_summary() {
    let home = sandbox();
    let source = home.path().join("source with spaces");
    let second_source = home.path().join("second source");
    create_skill(&source, "alpha", "# Alpha");
    create_skill(&source, "skill-with-long-name", "# Long");
    create_skill(&second_source, "zeta", "# Zeta");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
            "--label",
            "Primary Label",
        ])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            second_source.to_str().expect("utf8 path"),
            "very-long-source-alias",
            "--label",
            "Other Label",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "add", "custom", "target"])
        .assert()
        .success();

    cli(home.path())
        .args(["status", "--target", "custom"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sources:"))
        .stdout(predicate::str::contains("primary"))
        .stdout(predicate::str::contains("(Primary Label)"))
        .stdout(predicate::str::contains("NAME").not())
        .stdout(predicate::str::contains("LABEL").not())
        .stdout(predicate::str::contains("LOCATION").not())
        .stdout(predicate::str::contains("skill"))
        .stdout(predicate::str::contains("source"))
        .stdout(predicate::str::contains("custom"))
        .stdout(predicate::str::contains("--------------------"))
        .stdout(predicate::str::contains(
            "alpha                 primary                 none      not-loaded",
        ))
        .stdout(predicate::str::contains(
            "skill-with-long-name  primary                 none      not-loaded",
        ))
        .stdout(predicate::str::contains(
            "zeta                  very-long-source-alias  none      not-loaded",
        ))
        .stdout(predicate::str::contains("source with spaces"))
        .stdout(predicate::str::contains("\t").not())
        .stdout(predicate::str::contains("not-loaded: 3"))
        .stdout(predicate::str::contains("Summary:").not())
        .stdout(predicate::str::contains("\u{1b}[").not());

    let empty_home = sandbox();
    let empty_source = empty_home.path().join("empty");
    fs::create_dir(&empty_source).expect("create empty source");
    cli(empty_home.path())
        .args([
            "--json",
            "source",
            "add",
            empty_source.to_str().expect("utf8 path"),
            "empty",
        ])
        .assert()
        .success();
    cli(empty_home.path())
        .args(["status", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sources:"))
        .stdout(predicate::str::contains("No skills found"));
}

#[test]
fn human_status_names_an_implicit_current_directory_source() {
    let home = sandbox();
    let working_directory = home.path().join("working collection");
    create_skill(&working_directory, "from-cwd", "# CWD");

    let mut command = cli(home.path());
    command.current_dir(&working_directory);
    command
        .args(["status", "--cd-only"])
        .assert()
        .success()
        .stdout(predicate::str::contains("cwd  (Current directory)  "))
        .stdout(predicate::str::contains("from-cwd  cwd"))
        .stdout(predicate::str::contains("\t").not())
        .stdout(predicate::str::contains("\u{1b}[").not());
}

#[test]
fn dry_run_never_writes_deployments_or_configuration() {
    let home = sandbox();
    let source = home.path().join("source");
    let target = home.path().join("target");
    create_skill(&source, "alpha", "# Alpha");

    cli(home.path())
        .args(["--json", "target", "add", "custom", "target"])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
        ])
        .assert()
        .success();
    let config_path = home.path().join(".skill-manager/config.json");
    let config_before = fs::read(&config_path).expect("read config");

    cli(home.path())
        .args([
            "--json",
            "load",
            "--target",
            "custom",
            "--global",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"dry_run\":true"));

    assert!(!target.exists());
    assert_eq!(
        fs::read(config_path).expect("read config again"),
        config_before
    );
}

#[test]
fn cwd_source_selectors_change_discovery_without_reordering_configured_sources() {
    let home = sandbox();
    let configured = home.path().join("configured");
    let target = home.path().join("target");
    create_skill(&configured, "alpha", "# Alpha");
    create_skill(home.path(), "beta", "# Beta");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            configured.to_str().expect("utf8 path"),
            "configured",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "add", "custom", "target"])
        .assert()
        .success();

    cli(home.path())
        .args([
            "--json",
            "load",
            "--target",
            "custom",
            "--global",
            "--cd-only",
            "--filter",
            "beta",
        ])
        .assert()
        .success();
    assert!(target.join("beta/SKILL.md").is_file());
    assert!(!target.join("alpha").exists());

    cli(home.path())
        .args([
            "--json", "load", "--target", "custom", "--global", "--no-cd", "--filter", "alpha",
        ])
        .assert()
        .success();
    assert!(target.join("alpha/SKILL.md").is_file());

    fs::write(home.path().join("beta/SKILL.md"), "# Beta v2").expect("update cwd skill");
    cli(home.path())
        .args([
            "--json", "update", "--target", "custom", "--cd", "--filter", "beta",
        ])
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(target.join("beta/SKILL.md")).expect("read updated cwd skill"),
        "# Beta v2"
    );
}

#[test]
fn filters_update_only_and_remove_confirmation_preserve_unselected_content() {
    let home = sandbox();
    let source = home.path().join("source");
    let copy_target = home.path().join("filtered-copy");
    let managed_target = home.path().join("managed");
    create_skill(&source, "alpha", "# Alpha");
    create_skill(&source, "beta", "# Beta");

    cli(home.path())
        .args([
            "--json",
            "copy",
            source.to_str().expect("utf8 path"),
            copy_target.to_str().expect("utf8 path"),
            "--filter",
            "alpha",
        ])
        .assert()
        .success();
    assert!(copy_target.join("alpha").exists());
    assert!(!copy_target.join("beta").exists());

    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "add", "custom", "managed"])
        .assert()
        .success();

    cli(home.path())
        .args(["--json", "update", "--target", "custom"])
        .assert()
        .success();
    assert!(
        !managed_target.exists(),
        "update-only must not create a target"
    );

    cli(home.path())
        .args(["--json", "load", "--target", "custom", "--global"])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json",
            "remove",
            "alpha",
            "--target",
            "custom",
            "--global",
            "--yes",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"dry_run\":true"));
    assert!(managed_target.join("alpha/SKILL.md").is_file());
    cli(home.path())
        .args([
            "remove",
            "alpha",
            "--target",
            "custom",
            "--global",
            "--no-input",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--yes"));
    assert!(managed_target.join("alpha").exists());
    assert!(managed_target.join("beta").exists());
}

#[test]
fn legacy_config_migrates_with_backup_and_dry_run_stays_in_memory() {
    let home = sandbox();
    let source = home.path().join("legacy-source");
    create_skill(&source, "alpha", "# Alpha");
    let legacy = home.path().join(".skills-syncer.config.json");
    let payload = format!(
        "{{\"skills_directories\":{{{}:{{\"name\":\"legacy\",\"label\":\"Legacy\"}}}}}}",
        serde_json::to_string(source.to_str().expect("utf8 path")).expect("quote path")
    );
    fs::write(&legacy, &payload).expect("write legacy config");

    cli(home.path())
        .args(["--json", "source", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"source.listed\""));
    assert!(!legacy.exists());
    assert!(home.path().join(".skill-manager/config.json").exists());
    assert!(home.path().join(".skill-manager/backups").is_dir());

    let dry_home = sandbox();
    let dry_legacy = dry_home.path().join(".skills-syncer.config.json");
    fs::write(&dry_legacy, "{}").expect("write dry-run legacy config");
    cli(dry_home.path())
        .args(["--json", "load", "--all", "--global", "--dry-run"])
        .assert()
        .success();
    assert!(!dry_legacy.exists());
    assert!(dry_home.path().join(".skill-manager/config.json").exists());
    assert!(dry_home.path().join(".skill-manager/backups").is_dir());
}

#[test]
fn strict_recipe_modes_resolve_paths_and_cli_values_win() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha");

    let inline_target = home.path().join("inline");
    let inline = serde_json::json!({
        "command": "copy",
        "source": source,
        "destination": inline_target,
        "dry_run": false
    });
    cli(home.path())
        .arg(format!("--json={inline}"))
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"skill.copied\""));
    assert!(inline_target.join("alpha").join("SKILL.md").is_file());

    let stdin_target = home.path().join("stdin");
    let stdin_recipe = serde_json::json!({
        "command": "copy",
        "source": source,
        "destination": stdin_target
    });
    cli(home.path())
        .arg("--json-input")
        .write_stdin(stdin_recipe.to_string())
        .assert()
        .success();
    assert!(stdin_target.join("alpha").join("SKILL.md").is_file());

    let recipe_dir = home.path().join("recipes");
    fs::create_dir_all(&recipe_dir).expect("create recipe directory");
    let recipe_source = recipe_dir.join("relative-source");
    create_skill(&recipe_source, "from-recipe", "# rebased");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            recipe_source.to_str().expect("utf8 recipe source"),
            "relative-source",
        ])
        .assert()
        .success();
    fs::write(
        recipe_dir.join("copy.json"),
        serde_json::json!({
            "command": "copy",
            "source": "relative-source",
            "destination": "../from-file"
        })
        .to_string(),
    )
    .expect("write recipe");
    cli(home.path())
        .args(["--input", "recipes/copy.json"])
        .assert()
        .success();
    assert!(
        home.path()
            .join("from-file")
            .join("from-recipe")
            .join("SKILL.md")
            .is_file()
    );

    let override_target = home.path().join("override");
    let override_recipe = serde_json::json!({
        "command": "copy",
        "dry_run": false
    });
    cli(home.path())
        .arg(format!("--json={override_recipe}"))
        .args([
            "copy",
            source.to_str().expect("utf8 path"),
            override_target.to_str().expect("utf8 path"),
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"dry_run\":true"));
    assert!(!override_target.exists());
}

#[test]
fn recipe_validation_and_config_failures_use_documented_streams() {
    let home = sandbox();

    cli(home.path())
        .arg(r#"--json={"command":"status","mispelled":true}"#)
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"event\":\"command.failed\""))
        .stdout(predicate::str::contains("unknown JSON invocation field"))
        .stderr(predicate::str::is_empty());

    cli(home.path())
        .arg(r#"--json={"command":"copy","source":17,"destination":"out"}"#)
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"event\":\"command.failed\""))
        .stderr(predicate::str::is_empty());

    fs::write(home.path().join(".skill-manager.config.json"), "{broken")
        .expect("write malformed config");
    cli(home.path())
        .arg("status")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error: invalid configuration"))
        .stdout(predicate::str::is_empty());
    assert_eq!(
        fs::read_to_string(home.path().join(".skill-manager/config.json"))
            .expect("read malformed config"),
        "{broken"
    );

    let future_home = sandbox();
    fs::write(
        future_home.path().join(".skill-manager.config.json"),
        r#"{"schema_version":999}"#,
    )
    .expect("write future config");
    cli(future_home.path())
        .arg("status")
        .assert()
        .failure()
        .stderr(predicate::str::contains("newer than supported"));
}

#[test]
fn nested_v0_type_errors_fail_without_rewriting_or_creating_a_backup() {
    let fixture_root = sandbox();
    let source = fixture_root.path().join("source");
    let target = fixture_root.path().join("target");
    let cases = [
        v0_skills_directories(&source, serde_json::json!("not-an-object")),
        v0_skills_directories(&source, serde_json::json!({ "name": 7 })),
        v0_skills_directories(&source, serde_json::json!({ "exclude": "draft-*" })),
        v0_skills_directories(&source, serde_json::json!({ "exclude": ["ok", 7] })),
        serde_json::json!({
            "targets": { "custom": { "path": target, "disabled": "false" } }
        }),
        serde_json::json!({
            "targets": { "custom": { "path": 7 } }
        }),
    ];

    for value in cases {
        let home = sandbox();
        let path = home.path().join(".skill-manager.config.json");
        let raw = serde_json::to_vec(&value).expect("serialize malformed fixture");
        fs::write(&path, &raw).expect("write malformed v0 fixture");
        cli(home.path())
            .args(["--json", "status", "--all"])
            .assert()
            .failure()
            .stdout(predicate::str::contains("\"event\":\"command.failed\""))
            .stderr(predicate::str::is_empty());
        assert!(!path.exists());
        assert_eq!(
            fs::read(home.path().join(".skill-manager/config.json"))
                .expect("read migrated unchanged config"),
            raw
        );
        assert!(!home.path().join(".skill-manager/backups").exists());
    }
}

#[test]
fn completion_and_man_generation_hooks_produce_installable_assets() {
    let home = sandbox();
    for shell in ["bash", "zsh", "fish", "powershell"] {
        let output = cli(home.path())
            .args(["generate-completions", "--shell", shell])
            .output()
            .expect("generate completion script");
        assert!(output.status.success());
        let script = String::from_utf8(output.stdout).expect("utf8 completion script");
        assert!(script.contains("skill-manager"));
        assert!(
            script.contains("update up")
                || script.contains("'up'")
                || script.contains("\"up\"")
                || script.contains("(up)"),
            "{shell} completion must expose the update alias"
        );
    }

    let man_page = home.path().join("share/man/man1/skill-manager.1");
    cli(home.path())
        .args([
            "generate-man",
            "--output",
            man_page.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();
    let rendered = fs::read_to_string(man_page).expect("read generated man page");
    assert!(rendered.contains(".TH"));
    assert!(rendered.contains("skill-manager"));
    assert!(
        rendered
            .lines()
            .any(|line| line == "Refresh only skills already deployed. Alias: up"),
        "generated man page must document the standalone update alias phrase"
    );

    let blocker = home.path().join("not-a-directory");
    fs::write(&blocker, "blocking file").expect("create parent blocker");
    let impossible = blocker.join("skill-manager.1");
    cli(home.path())
        .args([
            "generate-man",
            "--output",
            impossible.to_str().expect("utf8 path"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Error: filesystem operation failed",
        ));
    cli(home.path())
        .args([
            "--json",
            "generate-man",
            "--output",
            impossible.to_str().expect("utf8 path"),
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"event\":\"command.failed\""))
        .stderr(predicate::str::is_empty());
}

#[test]
fn human_output_honors_color_policy_and_diagnostic_streams() {
    let home = sandbox();
    cli(home.path())
        .args(["--json", "target", "add", "custom", "target"])
        .assert()
        .success();

    let mut redirected = cli(home.path());
    redirected.env_remove("NO_COLOR");
    redirected
        .args(["--color", "always", "target", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[36m").not())
        .stderr(predicate::str::is_empty());

    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--target", "custom", "--global"])
        .assert()
        .success();
    fs::write(source.join("alpha/SKILL.md"), "# Alpha changed")
        .expect("make the deployment outdated");
    cli(home.path())
        .args(["--color", "always", "status", "--target", "custom"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[33m"));

    let mut no_color = cli(home.path());
    no_color
        .env("NO_COLOR", "1")
        .args(["--color", "always", "status", "--target", "custom"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[33m"));

    let mut automatic = cli(home.path());
    automatic.env_remove("NO_COLOR");
    automatic
        .args(["--color", "auto", "status", "--target", "custom"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[").not());
    let mut never = cli(home.path());
    never.env_remove("NO_COLOR");
    never
        .args(["--color", "never", "status", "--target", "custom"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[").not());

    fs::write(home.path().join(".skill-manager/config.json"), "{broken")
        .expect("write malformed config");
    let mut diagnostic = cli(home.path());
    diagnostic.env_remove("NO_COLOR");
    diagnostic
        .args(["--color", "always", "status"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("Error:"))
        .stderr(predicate::str::contains("\u{1b}[31m"));
}

#[test]
fn later_target_failure_preserves_prior_commits_and_orders_failure_last() {
    let home = sandbox();
    let source = home.path().join("source");
    let good = home.path().join("good-target");
    let bad = home.path().join("bad-target");
    create_skill(&source, "alpha", "# Alpha");
    fs::write(&bad, "not a directory").expect("create invalid target root");

    for (name, path) in [("good", &good), ("bad", &bad)] {
        cli(home.path())
            .args([
                "--json",
                "target",
                "add",
                name,
                path.file_name()
                    .and_then(|value| value.to_str())
                    .expect("utf8 path"),
            ])
            .assert()
            .success();
    }
    let output = cli(home.path())
        .args([
            "--json",
            "load",
            source.to_str().expect("utf8 path"),
            "--target",
            "good",
            "--target",
            "bad",
            "--global",
        ])
        .output()
        .expect("run partial load");
    assert!(!output.status.success());
    assert!(good.join("alpha/SKILL.md").is_file());
    let stdout = String::from_utf8(output.stdout).expect("utf8 events");
    let events: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("NDJSON event"))
        .collect();
    assert_eq!(
        events.last().and_then(|event| event["event"].as_str()),
        Some("command.failed")
    );
    assert!(
        events
            .iter()
            .any(|event| { event["event"] == "skill.loaded" && event["data"]["target"] == "good" })
    );
}

#[test]
fn cross_process_configuration_lock_times_out_cleanly() {
    let home = sandbox();
    let lock_path = home.path().join(".skill-manager/locks/config.lock");
    let _held = acquire_lock(&lock_path, "test-holder", Duration::from_secs(1))
        .expect("hold config lock in test process");
    cli(home.path())
        .args(["--json", "target", "disable", "claude"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"event\":\"command.failed\""))
        .stdout(predicate::str::contains(
            "timed out waiting for configuration lock",
        ));
    assert!(!home.path().join(".skill-manager/config.json").exists());
}

#[test]
fn v0_migration_preserves_disabled_builtin_target_state() {
    let home = sandbox();
    let claude = home.path().join(".claude/skills");
    let legacy = serde_json::json!({
        "targets": {
            "Claude": {
                "path": claude,
                "label": "Claude Code",
                "disabled": true
            }
        }
    });
    fs::write(
        home.path().join(".skill-manager.config.json"),
        legacy.to_string(),
    )
    .expect("write v0 config");
    cli(home.path())
        .args(["--json", "target", "list"])
        .assert()
        .success();
    let config = read_config(home.path());
    assert!(config["builtins"]["claude"].is_null());
    assert_eq!(
        config["legacy_target_overrides"]["claude"]["enabled"].as_bool(),
        Some(false)
    );
    assert!(config["targets"]["claude"].is_null());

    cli(home.path())
        .args(["--json", "target", "set-path", "claude", "legacy-claude"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"target.path-set\""));
    assert_eq!(
        read_config(home.path())["legacy_target_overrides"]["claude"]["path"],
        "legacy-claude"
    );

    cli(home.path())
        .args(["--json", "target", "remove", "claude"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"level\":\"warning\""));
    let revealed = read_config(home.path());
    assert!(revealed["legacy_target_overrides"]["claude"].is_null());
    cli(home.path())
        .args(["--json", "target", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\":\"claude\""))
        .stdout(predicate::str::contains("\"legacy_override\":false"));
}

#[test]
fn v0_migration_canonicalizes_mixed_case_builtin_target_for_lifecycle() {
    let home = sandbox();
    let legacy_path = home.path().join("mixed-case-claude");
    fs::write(
        home.path().join(".skill-manager.config.json"),
        serde_json::json!({
            "targets": {
                "Claude": {
                    "path": legacy_path,
                    "disabled": true
                }
            }
        })
        .to_string(),
    )
    .expect("write mixed-case v0 config");

    cli(home.path())
        .args(["--json", "target", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\":\"claude\""))
        .stdout(predicate::str::contains("\"enabled\":false"))
        .stdout(predicate::str::contains("\"legacy_override\":true"));

    let migrated = read_config(home.path());
    assert!(migrated["legacy_target_overrides"]["Claude"].is_null());
    let migrated_template = PathBuf::from(
        migrated["legacy_target_overrides"]["claude"]["path"]
            .as_str()
            .expect("migrated target template"),
    );
    assert!(migrated_template.is_relative());
    assert!(migrated_template.ends_with("mixed-case-claude"));

    cli(home.path())
        .args([
            "--json",
            "target",
            "set-path",
            "CLAUDE",
            "changed-mixed-case-claude",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"target.path-set\""));
    assert_eq!(
        read_config(home.path())["legacy_target_overrides"]["claude"]["path"],
        "changed-mixed-case-claude"
    );
}

#[test]
fn collisions_are_first_source_wins_and_resolve_persists_exclude() {
    let home = sandbox();
    let first = home.path().join("first");
    let second = home.path().join("second");
    create_skill(&first, "common", "# First");
    create_skill(&second, "common", "# Second");

    for (path, name) in [(&first, "first"), (&second, "second")] {
        cli(home.path())
            .args([
                "--json",
                "source",
                "add",
                path.to_str().expect("utf8 path"),
                name,
            ])
            .assert()
            .success();
    }

    cli(home.path())
        .args([
            "--json",
            "resolve",
            "com*",
            "--prefer-source",
            "second",
            "--no-input",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"collision.resolved\""));

    let config = read_config(home.path());
    let sources = config["sources"].as_array().expect("sources array");
    let first_source = sources
        .iter()
        .find(|source| source["name"] == "first")
        .expect("first source");
    assert!(
        first_source["exclude"]
            .as_array()
            .expect("exclude array")
            .iter()
            .any(|value| value == "common")
    );
}

/// Regression test for the same dangerous "empty patterns select everything"
/// contract that caused the `remove` data-loss bug: `resolve <literal-name>`
/// (a bare name, no fnmatch metacharacters) must resolve only the named
/// collision. It must never widen to every other collision just because no
/// fnmatch pattern operand was supplied. Two distinct skill names collide
/// between two sources; resolving one by its literal name must leave the
/// other collision untouched (no exclude persisted, still ambiguous).
#[test]
fn resolve_literal_skill_name_resolves_only_that_collision_leaving_the_other_unresolved() {
    let home = sandbox();
    let first = home.path().join("first");
    let second = home.path().join("second");
    create_skill(&first, "alpha", "# First Alpha");
    create_skill(&first, "beta", "# First Beta");
    create_skill(&second, "alpha", "# Second Alpha");
    create_skill(&second, "beta", "# Second Beta");

    for (path, name) in [(&first, "first"), (&second, "second")] {
        cli(home.path())
            .args([
                "--json",
                "source",
                "add",
                path.to_str().expect("utf8 path"),
                name,
            ])
            .assert()
            .success();
    }

    cli(home.path())
        .args([
            "--json",
            "resolve",
            "alpha",
            "--prefer-source",
            "second",
            "--no-input",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"skill\":\"alpha\""))
        .stdout(predicate::str::contains("\"resolved\":1"))
        .stdout(predicate::str::contains("\"skill\":\"beta\"").not());

    let config = read_config(home.path());
    let sources = config["sources"].as_array().expect("sources array");
    let first_source = sources
        .iter()
        .find(|source| source["name"] == "first")
        .expect("first source");
    let excluded = first_source["exclude"].as_array().expect("exclude array");
    assert!(
        excluded.iter().any(|value| value == "alpha"),
        "alpha collision must be resolved: {excluded:?}"
    );
    assert!(
        !excluded.iter().any(|value| value == "beta"),
        "beta collision must remain unresolved: {excluded:?}"
    );
}

/// Companion coverage for the documented "omitted means every collision"
/// contract (`ResolveArgs::skills`): a bare `resolve` with no operands at
/// all -- no literal names and no patterns -- must still resolve every
/// pending collision. This is the one legitimate "select everything" path,
/// and it must keep working even though `resolve <literal-name>` no longer
/// implicitly expands to everything.
#[test]
fn resolve_with_no_operands_resolves_every_collision() {
    let home = sandbox();
    let first = home.path().join("first");
    let second = home.path().join("second");
    create_skill(&first, "alpha", "# First Alpha");
    create_skill(&first, "beta", "# First Beta");
    create_skill(&second, "alpha", "# Second Alpha");
    create_skill(&second, "beta", "# Second Beta");

    for (path, name) in [(&first, "first"), (&second, "second")] {
        cli(home.path())
            .args([
                "--json",
                "source",
                "add",
                path.to_str().expect("utf8 path"),
                name,
            ])
            .assert()
            .success();
    }

    cli(home.path())
        .args([
            "--json",
            "resolve",
            "--prefer-source",
            "second",
            "--no-input",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"skill\":\"alpha\""))
        .stdout(predicate::str::contains("\"skill\":\"beta\""))
        .stdout(predicate::str::contains("\"resolved\":2"));

    let config = read_config(home.path());
    let sources = config["sources"].as_array().expect("sources array");
    let first_source = sources
        .iter()
        .find(|source| source["name"] == "first")
        .expect("first source");
    let excluded = first_source["exclude"].as_array().expect("exclude array");
    assert!(excluded.iter().any(|value| value == "alpha"));
    assert!(excluded.iter().any(|value| value == "beta"));
}

#[test]
fn human_prompts_cover_text_confirmation_cancellation_and_invalid_answers() {
    let home = sandbox();
    let source = home.path().join("prompt-source");
    create_skill(&source, "alpha", "# Alpha");

    cli(home.path())
        .args(["source", "add", source.to_str().expect("utf8 path")])
        .write_stdin("interactive-name\nInteractive Label\n")
        .assert()
        .success()
        .stderr(predicate::str::contains("Source name"))
        .stderr(predicate::str::contains("Label [Interactive Name]"));
    assert_eq!(
        read_config(home.path())["sources"][0]["name"],
        "interactive-name"
    );
    assert_eq!(
        read_config(home.path())["sources"][0]["label"],
        "Interactive Label"
    );

    cli(home.path())
        .args(["load", "--filter", "alpha"])
        .write_stdin("n\n")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Apply this load plan to 3 enabled targets? [Y/n]",
        ))
        .stdout(predicate::str::contains("Cancelled."));
    assert!(!home.path().join(".claude/skills/alpha").exists());
    assert!(!home.path().join(".agents/skills/alpha").exists());
    assert!(
        !home
            .path()
            .join(".gemini/antigravity/skills/alpha")
            .exists()
    );
    cli(home.path())
        .args(["load", "--filter", "alpha", "--no-input"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "applying this plan noninteractively requires --yes.",
        ));
    cli(home.path())
        .args(["load", "--filter", "alpha", "--no-input", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Dry run — no changes were made."));

    cli(home.path())
        .args(["load", "--filter", "alpha"])
        .write_stdin("perhaps\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("expected 'yes' or 'no'"));

    let target = home.path().join("prompt-target");
    cli(home.path())
        .args(["--json", "target", "add", "prompt-target", "prompt-target"])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--target", "prompt-target", "--global"])
        .assert()
        .success();

    cli(home.path())
        .args(["remove", "alpha", "--target", "prompt-target", "--global"])
        .write_stdin("n\n")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Remove this deployment from 1 selected target?",
        ));
    assert!(target.join("alpha/SKILL.md").is_file());

    cli(home.path())
        .args(["remove", "alpha", "--target", "prompt-target", "--global"])
        .write_stdin("y\n")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Remove this deployment from 1 selected target?",
        ));
    assert!(!target.join("alpha").exists());
}

/// Regression test for a data-loss bug: `remove <literal-name>` (a bare skill
/// name, with no fnmatch metacharacters) must resolve to only that one name.
/// It must never widen to every deployed skill just because no fnmatch
/// pattern operand was supplied. Deploys three distinct skills to two
/// targets, removes one skill by its literal name, and asserts the other two
/// skills' deployments survive untouched in both targets.
#[test]
fn remove_literal_skill_name_does_not_touch_other_deployed_skills() {
    let home = sandbox();
    let source = home.path().join("source");
    let target_a = home.path().join("target-a");
    let target_b = home.path().join("target-b");
    create_skill(&source, "alpha", "# Alpha");
    create_skill(&source, "beta", "# Beta");
    create_skill(&source, "gamma", "# Gamma");

    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "add", "target-a", "target-a"])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "add", "target-b", "target-b"])
        .assert()
        .success();

    cli(home.path())
        .args([
            "--json", "load", "--target", "target-a", "--target", "target-b", "--global",
        ])
        .assert()
        .success();

    for target in [&target_a, &target_b] {
        for skill in ["alpha", "beta", "gamma"] {
            assert!(
                target.join(skill).join("SKILL.md").is_file(),
                "expected {skill} deployed to {target:?} before remove"
            );
        }
    }

    cli(home.path())
        .args(["remove", "alpha", "--global", "--yes"])
        .assert()
        .success();

    for target in [&target_a, &target_b] {
        assert!(
            !target.join("alpha").exists(),
            "alpha must be removed from {target:?}"
        );
        assert!(
            target.join("beta").join("SKILL.md").is_file(),
            "beta must survive removing alpha from {target:?}"
        );
        assert!(
            target.join("gamma").join("SKILL.md").is_file(),
            "gamma must survive removing alpha from {target:?}"
        );
    }
}

#[test]
fn interactive_collision_choice_selects_the_requested_winner() {
    let home = sandbox();
    let first = home.path().join("first");
    let second = home.path().join("second");
    create_skill(&first, "common", "# First");
    create_skill(&second, "common", "# Second");
    for (path, name) in [(&first, "first"), (&second, "second")] {
        cli(home.path())
            .args([
                "--json",
                "source",
                "add",
                path.to_str().expect("utf8 path"),
                name,
            ])
            .assert()
            .success();
    }

    cli(home.path())
        .args(["--json", "resolve", "common"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "resolve requires --prefer-source in noninteractive mode",
        ));

    cli(home.path())
        .args(["resolve", "common"])
        .write_stdin("2\n")
        .assert()
        .success()
        .stderr(predicate::str::contains("Choose source for common"))
        .stderr(predicate::str::contains("Choice:"));

    let config = read_config(home.path());
    let sources = config["sources"].as_array().expect("sources array");
    let first_source = sources
        .iter()
        .find(|source| source["name"] == "first")
        .expect("first source");
    assert!(
        first_source["exclude"]
            .as_array()
            .expect("exclude array")
            .iter()
            .any(|value| value == "common")
    );
}

#[test]
fn no_work_paths_succeed_without_creating_destination_content() {
    let home = sandbox();
    let source = home.path().join("source");
    let destination = home.path().join("destination");
    let managed = home.path().join("managed");
    create_skill(&source, "alpha", "# Alpha");

    cli(home.path())
        .args([
            "--json",
            "copy",
            source.to_str().expect("utf8 path"),
            destination.to_str().expect("utf8 path"),
            "--filter",
            "missing-*",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"copied\":0"));
    assert!(!destination.exists());

    cli(home.path())
        .args(["--json", "target", "add", "managed", "managed"])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json", "remove", "missing", "--target", "managed", "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"removed\":0"));
    assert!(!managed.exists());
}

#[test]
fn source_validation_rejects_duplicates_unknowns_and_invalid_values() {
    let home = sandbox();
    let source = home.path().join("source");
    let second_source = home.path().join("second-source");
    create_skill(&source, "alpha", "# Alpha");
    create_skill(&second_source, "beta", "# Beta");

    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            second_source.to_str().expect("utf8 path"),
            "secondary",
        ])
        .assert()
        .success();
    for duplicate in [
        vec![
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "other",
        ],
        vec![
            "--json",
            "source",
            "add",
            home.path().to_str().expect("utf8 path"),
            "PRIMARY",
        ],
    ] {
        cli(home.path())
            .args(duplicate)
            .assert()
            .failure()
            .stdout(predicate::str::contains("\"event\":\"command.failed\""));
    }

    for arguments in [
        vec!["--json", "source", "remove", "unknown"],
        vec!["--json", "source", "update", "unknown", "--label", "Nope"],
        vec!["--json", "source", "update", "primary", "--name", ""],
        vec![
            "--json",
            "source",
            "update",
            "secondary",
            "--name",
            "PRIMARY",
        ],
        vec![
            "--json",
            "source",
            "update",
            "primary",
            "--cache-ttl-hours=-1",
        ],
        vec![
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "ttl",
            "--cache-ttl-hours=-1",
        ],
    ] {
        cli(home.path())
            .args(arguments)
            .assert()
            .failure()
            .stdout(predicate::str::contains("\"event\":\"command.failed\""));
    }
}

#[test]
fn target_validation_rejects_duplicates_and_unknown_lifecycle_references() {
    let home = sandbox();
    cli(home.path())
        .args(["--json", "target", "add", "custom", "target"])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "add", "custom", "duplicate"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("already exists"));

    for arguments in [
        vec!["--json", "target", "set-path", "unknown", "elsewhere"],
        vec!["--json", "target", "enable", "unknown"],
        vec!["--json", "target", "remove", "unknown"],
    ] {
        cli(home.path())
            .args(arguments)
            .assert()
            .failure()
            .stdout(predicate::str::contains("\"event\":\"command.failed\""));
    }
}

#[test]
fn machine_input_carriers_are_exclusive_and_noninteractive() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha");

    cli(home.path())
        .args([r#"--json={"command":"status"}"#, "--json-input", "status"])
        .write_stdin(r#"{"command":"status"}"#)
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"event\":\"command.failed\""))
        .stdout(predicate::str::contains("mutually exclusive"));

    let payload = serde_json::json!({
        "command": "source.add",
        "source": source,
        "name": false
    });
    cli(home.path())
        .arg(format!("--json={payload}"))
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"event\":\"command.failed\""))
        .stdout(predicate::str::contains("JSON field must be a string"));

    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "source name is required in noninteractive mode",
        ));
}

#[test]
// This end-to-end contract intentionally exercises the complete cross-scope lifecycle.
#[allow(clippy::too_many_lines)]
fn project_scope_overrides_global_and_update_remove_infer_existing_scope() {
    let home = sandbox();
    let project = home.path().join("project");
    let source = home.path().join("source");
    fs::create_dir_all(&project).expect("create project");
    create_skill(&source, "alpha", "# Global");

    cli(home.path())
        .args(["--json", "target", "add", "custom", ".custom/skills"])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json",
            "load",
            source.to_str().expect("utf8 source"),
            "--target",
            "custom",
            "--global",
        ])
        .assert()
        .success();
    let global_skill = home.path().join(".custom/skills/alpha/SKILL.md");
    assert_eq!(
        fs::read_to_string(&global_skill).expect("global skill"),
        "# Global"
    );

    fs::write(source.join("alpha/SKILL.md"), "# Project").expect("change source");
    let mut project_load = cli(home.path());
    project_load.current_dir(&project);
    project_load
        .args([
            "--json",
            "load",
            source.to_str().expect("utf8 source"),
            "--target",
            "custom",
            "--project",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"scope\":\"project\""));
    let project_skill = project.join(".custom/skills/alpha/SKILL.md");
    assert_eq!(
        fs::read_to_string(&project_skill).expect("project skill"),
        "# Project"
    );

    fs::write(source.join("alpha/SKILL.md"), "# Updated Project").expect("change source again");
    let mut inferred_update = cli(home.path());
    inferred_update.current_dir(&project);
    inferred_update
        .args([
            "--json",
            "update",
            source.to_str().expect("utf8 source"),
            "--target",
            "custom",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"scope\":\"project\""));
    assert_eq!(
        fs::read_to_string(&project_skill).expect("updated project skill"),
        "# Updated Project"
    );
    assert_eq!(
        fs::read_to_string(&global_skill).expect("updated global skill"),
        "# Updated Project"
    );
    fs::write(&global_skill, "# Divergent global").expect("diverge the shadowed global copy");

    let mut status = cli(home.path());
    status.current_dir(&project);
    let events = json_events(
        status
            .args(["--json", "status", "--target", "custom"])
            .output()
            .expect("scoped status"),
    );
    let row = events
        .iter()
        .find(|event| event["event"] == "status.row")
        .expect("status row");
    assert_eq!(row["data"]["location"], "both");
    assert_eq!(row["data"]["shadowed_global_divergent"], true);
    let deployments = row["data"]["deployments"]
        .as_array()
        .expect("deployment details");
    let expected_global_deployment = home.path().join(".custom").join("skills").join("alpha");
    let expected_project_deployment = project.join(".custom").join("skills").join("alpha");
    assert_eq!(deployments.len(), 2);
    assert_eq!(deployments[0]["scope"], "global");
    assert_eq!(deployments[0]["effective"], false);
    assert_eq!(
        portable_canonicalize(PathBuf::from(
            deployments[0]["path"]
                .as_str()
                .expect("global deployment path"),
        ))
        .expect("canonical global deployment path"),
        portable_canonicalize(&expected_global_deployment)
            .expect("canonical expected global deployment path")
    );
    assert_eq!(deployments[1]["scope"], "project");
    assert_eq!(deployments[1]["effective"], true);
    assert_eq!(
        portable_canonicalize(PathBuf::from(
            deployments[1]["path"]
                .as_str()
                .expect("project deployment path"),
        ))
        .expect("canonical project deployment path"),
        portable_canonicalize(&expected_project_deployment)
            .expect("canonical expected project deployment path")
    );

    let mut ambiguous_remove = cli(home.path());
    ambiguous_remove.current_dir(&project);
    ambiguous_remove
        .args(["--json", "remove", "alpha", "--target", "custom", "--yes"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "choose --project, --global, or --both",
        ));

    let mut project_remove = cli(home.path());
    project_remove.current_dir(&project);
    project_remove
        .args([
            "--json",
            "remove",
            "alpha",
            "--target",
            "custom",
            "--project",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"scope\":\"project\""));
    assert!(!project_skill.exists());
    assert!(global_skill.exists());
}

#[test]
fn load_scope_inference_uses_exact_cwd_vendor_directory_as_its_default() {
    let home = sandbox();
    let source = home.path().join("source");
    let project = home.path().join("project");
    let other_project = home.path().join("other-project");
    create_skill(&source, "alpha", "# Alpha");
    fs::create_dir_all(project.join(".agents")).expect("create project vendor directory");
    fs::create_dir_all(&other_project).expect("create other project");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();

    // load's scope is now inferred silently (never asked as its own
    // question); the single plan confirmation is all that remains
    // interactive, so a bare "\n" answer accepts its `[Y/n]` default.
    let mut project_load = cli(home.path());
    project_load.current_dir(&project);
    project_load
        .args(["load", "--shared", "--filter", "alpha"])
        .write_stdin("\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("shared: new deployment"));
    assert!(project.join(".agents/skills/alpha/SKILL.md").is_file());

    let mut global_load = cli(home.path());
    global_load.current_dir(&other_project);
    global_load
        .args(["load", "--shared", "--filter", "alpha"])
        .write_stdin("\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("shared: new deployment"));
    assert!(home.path().join(".agents/skills/alpha/SKILL.md").is_file());
    assert!(!other_project.join(".agents/skills/alpha").exists());
}

#[test]
// This end-to-end contract keeps pattern operations and byte-safe recovery in one workflow.
#[allow(clippy::too_many_lines)]
fn positional_fnmatch_and_config_recovery_contracts_are_end_to_end() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "grill-one", "# One");
    create_skill(&source, "grill-two", "# Two");
    create_skill(&source, "other", "# Other");
    cli(home.path())
        .args(["--json", "target", "add", "custom", ".custom/skills"])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json",
            "load",
            source.to_str().expect("utf8 source"),
            "grill-*",
            "--filter",
            "*two",
            "--target",
            "custom",
            "--global",
        ])
        .assert()
        .success();
    assert!(!home.path().join(".custom/skills/grill-one").exists());
    assert!(
        home.path()
            .join(".custom/skills/grill-two/SKILL.md")
            .is_file()
    );
    assert!(!home.path().join(".custom/skills/other").exists());

    fs::write(source.join("grill-two/SKILL.md"), "# Two Updated").expect("update patterned source");
    cli(home.path())
        .args([
            "--json",
            "update",
            source.to_str().expect("utf8 source"),
            "grill-*",
            "--target",
            "custom",
            "--global",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"skill.updated\""));
    assert_eq!(
        fs::read_to_string(home.path().join(".custom/skills/grill-two/SKILL.md"))
            .expect("updated patterned deployment"),
        "# Two Updated"
    );
    cli(home.path())
        .args([
            "--json", "status", "grill-*", "--target", "custom", "--global",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"skill\":\"grill-two\""))
        .stdout(predicate::str::contains("\"skill\":\"other\"").not());
    cli(home.path())
        .args([
            "--json", "remove", "grill-*", "--target", "custom", "--global", "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"skill.removed\""));
    assert!(!home.path().join(".custom/skills/grill-two").exists());

    cli(home.path())
        .args([
            "--json",
            "load",
            source.to_str().expect("utf8 source"),
            "missing-*",
            "--target",
            "custom",
            "--global",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("skill pattern matched nothing"));

    let raw_home = sandbox();
    let expected = canonical_config_bytes().expect("canonical config");
    let output = cli(raw_home.path())
        .args(["configs", "--raw"])
        .output()
        .expect("raw config");
    assert!(output.status.success());
    assert_eq!(output.stdout, expected);
    assert_eq!(
        fs::read(raw_home.path().join(".skill-manager/config.json")).expect("stored config"),
        expected
    );

    let malformed = b"{ definitely malformed\n";
    fs::write(
        raw_home.path().join(".skill-manager/config.json"),
        malformed,
    )
    .expect("write malformed config");
    let reset_events = json_events(
        cli(raw_home.path())
            .args(["--json", "configs", "reset", "--yes"])
            .output()
            .expect("reset malformed config"),
    );
    let backup_id = reset_events
        .iter()
        .find(|event| event["event"] == "config.reset")
        .and_then(|event| event["data"]["backup_id"].as_str())
        .expect("reset backup ID")
        .to_owned();
    assert_eq!(
        fs::read(
            raw_home
                .path()
                .join(".skill-manager/backups")
                .join(&backup_id)
                .join("config.raw")
        )
        .expect("backup raw bytes"),
        malformed
    );
    cli(raw_home.path())
        .args(["--json", "configs", "restore", &backup_id, "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"config.restored\""));
    assert_eq!(
        fs::read(raw_home.path().join(".skill-manager/config.json")).expect("restored config"),
        malformed
    );
    for rejected in [" yes \n", "yes \n", "YES\n", "Yes\n", "y\n"] {
        cli(raw_home.path())
            .args(["configs", "reset"])
            .write_stdin(rejected)
            .assert()
            .success()
            .stdout(predicate::str::contains("Cancelled."));
        assert_eq!(
            fs::read(raw_home.path().join(".skill-manager/config.json"))
                .expect("cancelled reset preserves config"),
            malformed
        );
    }
    cli(raw_home.path())
        .args(["configs", "reset"])
        .write_stdin("yes\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Reset configuration."));
    assert_eq!(
        fs::read(raw_home.path().join(".skill-manager/config.json"))
            .expect("exact confirmation resets config"),
        expected
    );
}

fn events_of<'a>(events: &'a [Value], name: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event["event"] == name)
        .collect()
}

fn seed_github_cache(home: &Path, skill: &str, body: &str) {
    let config = read_config(home);
    let id = config["sources"][0]["id"]
        .as_str()
        .expect("generated source ID")
        .to_owned();
    let cache = home.join(".skill-manager").join("cache").join(id);
    let content = cache.join("content").join(skill);
    fs::create_dir_all(&content).expect("create cached skill directory");
    fs::write(content.join("SKILL.md"), body).expect("write cached skill");
    fs::write(
        cache.join("metadata.json"),
        serde_json::json!({
            "fetched_at": chrono::Utc::now().to_rfc3339(),
            "resolved_ref": "main",
            "owner": "acme",
            "repo": "skills",
            "ref": null,
            "repo_path": null
        })
        .to_string(),
    )
    .expect("write cache metadata");
}

/// `claude` loaded at both scopes; the project copy and the global copy are
/// each edited to different content, so both differ from the source and both
/// become genuine `source_copy` alternatives. `Available source copies`
/// orders project before global (scope-major, project first).
fn import_ambiguous_fixture() -> (TempDir, PathBuf) {
    let home = sandbox();
    let project = home.path().join("project");
    fs::create_dir_all(&project).expect("create project directory");
    let source = home.path().join("source");
    create_skill(&source, "docwriter", "# Doc\n");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "docwriter", "--claude", "--global"])
        .assert()
        .success();
    let mut project_load = cli(home.path());
    project_load
        .current_dir(&project)
        .args(["--json", "load", "docwriter", "--claude", "--project"])
        .assert()
        .success();
    fs::write(
        home.path().join(".claude/skills/docwriter/SKILL.md"),
        "# Doc\nglobal edit\n",
    )
    .expect("edit global deployment");
    fs::write(
        project.join(".claude/skills/docwriter/SKILL.md"),
        "# Doc\nproject edit\n",
    )
    .expect("edit project deployment");
    (home, project)
}

/// Same layout as [`import_ambiguous_fixture`], but the project deployment is
/// resynchronized to the source's own content, leaving only one genuine
/// `source_copy` candidate (`claude · global`). Only the propagation
/// dimension is pending, so exactly one prompt is asked.
fn import_single_copy_fixture() -> (TempDir, PathBuf) {
    let (home, project) = import_ambiguous_fixture();
    fs::write(project.join(".claude/skills/docwriter/SKILL.md"), "# Doc\n")
        .expect("resync project deployment to the source");
    (home, project)
}

/// Two genuine candidates once more (any difference from the *current*
/// source -- including merely gaining an extra file -- qualifies a
/// deployment for `source_copy`, so `claude · project` becomes a second
/// alternative here purely by carrying `notes.md`), but the test resolves
/// `claude · global` (source copy 2, whose own diff from source is a single
/// `SKILL.md` line). Propagating *that* choice to `claude · project` must
/// both rewrite `SKILL.md` and remove the extra file the new source lacks,
/// so its advertised total (`2 files changed`) only matches what apply
/// actually writes if both files are enumerated from the same source of
/// truth used to apply. This is the E8 regression fixture: Stage 3's remove
/// defect computed a representative count instead of the resolved apply
/// list, promising fewer files than it deleted.
fn import_drifted_fixture() -> (TempDir, PathBuf) {
    let (home, project) = import_single_copy_fixture();
    fs::write(
        project.join(".claude/skills/docwriter/notes.md"),
        "extra notes\n",
    )
    .expect("give the project deployment a file the candidate lacks");
    (home, project)
}

/// The interactive rendering of the multi-copy plan: `Available source
/// copies`, both alternatives with their own diff and nested propagation
/// preview, the `c Cancel` option, and the deferred `Propagation modes`
/// heading -- everything but the pending-decision footer, which callers
/// append themselves since it differs between an unresolved render and a
/// `--yes` failure that never prints it.
const IMPORT_TWO_COPY_INTERACTIVE_BODY: &str = "\
Import plan

Into  Primary (source)

Available source copies

  1  claude \u{b7} project
     Path    PROJECT_DEPLOYMENT
     Source  \u{2190} 1 file changed, +1/-0
       ~  SKILL.md  +1/-0
     Propagation with import + update  2 deployments
       claude \u{b7} global   \u{2191} 1 file changed, +1/-1
       claude \u{b7} project  \u{2713} source copy; synchronized, no file changes

  2  claude \u{b7} global
     Path    GLOBAL_DEPLOYMENT
     Source  \u{2190} 1 file changed, +1/-0
       ~  SKILL.md  +1/-0
     Propagation with import + update  2 deployments
       claude \u{b7} global   \u{2713} source copy; synchronized, no file changes
       claude \u{b7} project  \u{2191} 1 file changed, +1/-1

  c  Cancel

Propagation modes (chosen after the source copy)

  1  Import + update  (recommended)
     Replace the source, then synchronize every deployment shown for that copy.

  2  Import only
     Replace the source; write no deployments and leave the other 1 out of date.

";

/// The same plan with no prompt pending: `--dry-run` (and a `--yes` run that
/// still finds both dimensions ambiguous) never offers a numbered `Cancel`,
/// so the `c Cancel` line disappears -- `render_decision` only appends
/// `Cancel` while genuinely prompting. The heading itself does not disappear;
/// it switches to the deferred wording (`Source copies (chosen first)`) so a
/// reader of a non-prompting render can still tell the two numbered lists
/// apart (see item G).
const IMPORT_TWO_COPY_DRY_BODY: &str = "\
Import plan

Into  Primary (source)

Source copies (chosen first)

  1  claude \u{b7} project
     Path    PROJECT_DEPLOYMENT
     Source  \u{2190} 1 file changed, +1/-0
       ~  SKILL.md  +1/-0
     Propagation with import + update  2 deployments
       claude \u{b7} global   \u{2191} 1 file changed, +1/-1
       claude \u{b7} project  \u{2713} source copy; synchronized, no file changes

  2  claude \u{b7} global
     Path    GLOBAL_DEPLOYMENT
     Source  \u{2190} 1 file changed, +1/-0
       ~  SKILL.md  +1/-0
     Propagation with import + update  2 deployments
       claude \u{b7} global   \u{2713} source copy; synchronized, no file changes
       claude \u{b7} project  \u{2191} 1 file changed, +1/-1

Propagation modes (chosen after the source copy)

  1  Import + update  (recommended)
     Replace the source, then synchronize every deployment shown for that copy.

  2  Import only
     Replace the source; write no deployments and leave the other 1 out of date.

";

/// The footer for the unresolved two-copy plan: neither dimension is
/// resolved yet, so it names both remaining questions and their order.
const IMPORT_TWO_COPY_UNRESOLVED_FOOTER: &str =
    "2 source copies; propagation decision follows source selection\n";

/// The narrowed re-render after answering `2` (choose `claude \u{b7} global`):
/// the whole `Available source copies` section is gone, option `1`
/// (`claude \u{b7} project`) and its own diff and nested preview are gone, and
/// the chosen copy demotes to ordinary `From`/`Path` metadata. Only the
/// still-pending propagation dimension remains, now active (`c Cancel` is
/// offered and the heading is the un-deferred `Propagation preview` block).
const IMPORT_NARROWED_TO_GLOBAL_BODY: &str = "\n\
Import plan \u{2014} source copy 2 selected

From  claude \u{b7} global
Path  GLOBAL_DEPLOYMENT
Into  Primary (source)

Source replacement
  \u{2190} 1 file changed, +1/-0
  ~  SKILL.md  +1/-0

Propagation preview
  claude \u{b7} global   \u{2713} source copy; synchronized, no file changes
  claude \u{b7} project  \u{2191} 1 file changed, +1/-1

  1  Import + update  (recommended)
     Replace the source and synchronize 2 deployments (1 source copy, 1 updated).

  2  Import only
     Replace the source; write no deployments and leave 1 out of date.

  c  Cancel

1 source copy selected; 2 propagation modes
";

fn import_two_copy_interactive_body(home: &Path, project: &Path) -> String {
    let project_deployment =
        portable_canonicalize(project.join(".claude/skills/docwriter")).expect("project path");
    let global_deployment =
        portable_canonicalize(home.join(".claude/skills/docwriter")).expect("global path");
    IMPORT_TWO_COPY_INTERACTIVE_BODY
        .replace(
            "PROJECT_DEPLOYMENT",
            &project_deployment.display().to_string(),
        )
        .replace(
            "GLOBAL_DEPLOYMENT",
            &global_deployment.display().to_string(),
        )
}

fn import_two_copy_dry_body(home: &Path, project: &Path) -> String {
    let project_deployment =
        portable_canonicalize(project.join(".claude/skills/docwriter")).expect("project path");
    let global_deployment =
        portable_canonicalize(home.join(".claude/skills/docwriter")).expect("global path");
    IMPORT_TWO_COPY_DRY_BODY
        .replace(
            "PROJECT_DEPLOYMENT",
            &project_deployment.display().to_string(),
        )
        .replace(
            "GLOBAL_DEPLOYMENT",
            &global_deployment.display().to_string(),
        )
}

fn import_narrowed_to_global_body(home: &Path) -> String {
    let global_deployment =
        portable_canonicalize(home.join(".claude/skills/docwriter")).expect("global path");
    IMPORT_NARROWED_TO_GLOBAL_BODY.replace(
        "GLOBAL_DEPLOYMENT",
        &global_deployment.display().to_string(),
    )
}

/// E1: the complete multi-copy plan -- every source-copy alternative's own
/// diff, its nested per-copy propagation preview, the deferred propagation
/// heading, and both propagation modes -- renders in full before any prompt
/// is asked. E9: cancelling exits 0 with no writes and no extra hint.
#[test]
fn import_renders_the_complete_multi_copy_plan_before_the_first_prompt() {
    let (home, project) = import_ambiguous_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude"])
        .write_stdin("c\n")
        .output()
        .expect("run ambiguous import then cancel");
    assert!(output.status.success(), "cancelling is not a failure");
    let body = import_two_copy_interactive_body(home.path(), &project);
    assert_eq!(
        stdout_of(output.clone()),
        format!("{body}{IMPORT_TWO_COPY_UNRESOLVED_FOOTER}Cancelled.\n")
    );
    assert_eq!(
        stderr_of(&output),
        "Select source copy [1-2, c to cancel]: "
    );
    assert_eq!(
        fs::read_to_string(home.path().join("source/docwriter/SKILL.md"))
            .expect("source untouched"),
        "# Doc\n"
    );
    assert_eq!(
        fs::read_to_string(home.path().join(".claude/skills/docwriter/SKILL.md"))
            .expect("global deployment untouched"),
        "# Doc\nglobal edit\n"
    );
    assert_eq!(
        fs::read_to_string(project.join(".claude/skills/docwriter/SKILL.md"))
            .expect("project deployment untouched"),
        "# Doc\nproject edit\n"
    );
}

/// E2/E9: an empty answer and an invalid token both reprompt with the same
/// instruction; the selection never auto-picks an option, and the plan is
/// printed exactly once regardless of how many reprompts follow.
#[test]
fn import_source_selection_reprompts_on_invalid_and_empty_input_without_auto_selecting() {
    let (home, project) = import_ambiguous_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude"])
        .write_stdin("\nbogus\nc\n")
        .output()
        .expect("run reprompted source selection");
    assert!(output.status.success());
    assert_eq!(
        stderr_of(&output),
        "Select source copy [1-2, c to cancel]: Enter 1, 2, or c.\n\
Select source copy [1-2, c to cancel]: Enter 1, 2, or c.\n\
Select source copy [1-2, c to cancel]: "
    );
    let body = import_two_copy_interactive_body(home.path(), &project);
    assert_eq!(
        stdout_of(output),
        format!("{body}{IMPORT_TWO_COPY_UNRESOLVED_FOOTER}Cancelled.\n")
    );
}

/// E3: after answering `2`, the plan re-renders narrowed to that one
/// candidate. Option `1` (`claude \u{b7} project`), its own diff, and its own
/// nested preview -- along with the whole `Available source copies` heading
/// -- vanish; the chosen copy demotes to ordinary `From`/`Path` metadata;
/// only the unresolved propagation dimension remains, now active.
#[test]
fn import_narrowed_re_render_gates_out_the_resolved_source_dimension() {
    let (home, project) = import_ambiguous_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude"])
        .write_stdin("2\nc\n")
        .output()
        .expect("run narrowed re-render then cancel");
    assert!(output.status.success());
    let body = import_two_copy_interactive_body(home.path(), &project);
    let narrowed = import_narrowed_to_global_body(home.path());
    assert_eq!(
        stdout_of(output.clone()),
        format!("{body}{IMPORT_TWO_COPY_UNRESOLVED_FOOTER}{narrowed}Cancelled.\n")
    );
    assert_eq!(
        stderr_of(&output),
        "Select source copy [1-2, c to cancel]: Select propagation [1-2, c to cancel]: "
    );
    assert_eq!(
        fs::read_to_string(home.path().join("source/docwriter/SKILL.md")).expect("untouched"),
        "# Doc\n"
    );
}

/// E9: cancelling the second (propagation) prompt after the source copy was
/// already chosen still exits 0 and writes nothing at all -- not even the
/// source replacement -- because the final decision was never authorized.
#[test]
fn import_cancel_at_the_propagation_prompt_writes_nothing() {
    let (home, project) = import_ambiguous_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude"])
        .write_stdin("2\nc\n")
        .output()
        .expect("run cancelled propagation selection");
    assert!(output.status.success(), "cancelling is not a failure");
    assert_eq!(
        fs::read_to_string(home.path().join("source/docwriter/SKILL.md"))
            .expect("source untouched"),
        "# Doc\n"
    );
    assert_eq!(
        fs::read_to_string(project.join(".claude/skills/docwriter/SKILL.md"))
            .expect("project deployment untouched"),
        "# Doc\nproject edit\n"
    );
}

/// E2/E9: the same reprompt discipline applies at the second prompt.
#[test]
fn import_propagation_reprompts_on_invalid_and_empty_input_without_auto_selecting() {
    let (home, project) = import_ambiguous_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude"])
        .write_stdin("2\n\nbogus\nc\n")
        .output()
        .expect("run reprompted propagation selection");
    assert!(output.status.success());
    assert_eq!(
        stderr_of(&output),
        "Select source copy [1-2, c to cancel]: Select propagation [1-2, c to cancel]: Enter 1, 2, or c.\n\
Select propagation [1-2, c to cancel]: Enter 1, 2, or c.\n\
Select propagation [1-2, c to cancel]: "
    );
    let body = import_two_copy_interactive_body(home.path(), &project);
    let narrowed = import_narrowed_to_global_body(home.path());
    assert_eq!(
        stdout_of(output),
        format!("{body}{IMPORT_TWO_COPY_UNRESOLVED_FOOTER}{narrowed}Cancelled.\n")
    );
}

/// E4: the final propagation answer applies immediately -- no trailing
/// `[y/N]` -- replacing the source and synchronizing the other deployment,
/// including a `Synchronized ... (source copy)` line for the resolved
/// candidate's own now-redundant deployment.
#[test]
fn import_final_propagation_answer_applies_immediately() {
    let (home, project) = import_ambiguous_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude"])
        .write_stdin("2\n1\n")
        .output()
        .expect("run full accepted import");
    assert!(output.status.success());
    let body = import_two_copy_interactive_body(home.path(), &project);
    let narrowed = import_narrowed_to_global_body(home.path());
    assert_eq!(
        stdout_of(output.clone()),
        format!(
            "{body}{IMPORT_TWO_COPY_UNRESOLVED_FOOTER}{narrowed}\n\
Imported docwriter from claude \u{b7} global into Primary (source).\n\
Synchronized docwriter -> claude (global) (source copy)\n\
Updated docwriter -> claude (project)\n\
\n\
completed: 1 source replaced (1 file, +1/-0), 2 deployments synchronized (1 source copy, 1 updated)\n"
        )
    );
    assert_eq!(
        stderr_of(&output),
        "Select source copy [1-2, c to cancel]: Select propagation [1-2, c to cancel]: "
    );
    assert_eq!(
        fs::read_to_string(home.path().join("source/docwriter/SKILL.md")).expect("imported"),
        "# Doc\nglobal edit\n"
    );
    assert_eq!(
        fs::read_to_string(project.join(".claude/skills/docwriter/SKILL.md")).expect("synced"),
        "# Doc\nglobal edit\n"
    );
}

/// Both propagation outcomes are real code paths: "Import only" replaces the
/// source but leaves the other deployment untouched (and out of date).
#[test]
fn import_only_propagation_leaves_the_other_deployment_out_of_date() {
    let (home, project) = import_ambiguous_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude"])
        .write_stdin("2\n2\n")
        .output()
        .expect("run import-only propagation");
    assert!(output.status.success());
    let body = import_two_copy_interactive_body(home.path(), &project);
    let narrowed = import_narrowed_to_global_body(home.path());
    assert_eq!(
        stdout_of(output),
        format!(
            "{body}{IMPORT_TWO_COPY_UNRESOLVED_FOOTER}{narrowed}\n\
Imported docwriter from claude \u{b7} global into Primary (source).\n\
\n\
completed: 1 source replaced (1 file, +1/-0)\n"
        )
    );
    assert_eq!(
        fs::read_to_string(home.path().join("source/docwriter/SKILL.md")).expect("imported"),
        "# Doc\nglobal edit\n"
    );
    assert_eq!(
        fs::read_to_string(project.join(".claude/skills/docwriter/SKILL.md"))
            .expect("left untouched and out of date"),
        "# Doc\nproject edit\n"
    );
}

/// E5: when only one deployment genuinely differs from the source, only the
/// propagation dimension is real, so the whole session is exactly one
/// prompt: `From`/`Path` metadata from the very first render, no `Available
/// source copies` section, no source-copy prompt at all.
#[test]
fn import_single_copy_case_uses_exactly_one_prompt() {
    let (home, project) = import_single_copy_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude"])
        .write_stdin("1\n")
        .output()
        .expect("run single-copy import");
    assert!(output.status.success());
    let global_deployment =
        portable_canonicalize(home.path().join(".claude/skills/docwriter")).expect("global path");
    assert_eq!(
        stdout_of(output.clone()),
        format!(
            "Import plan\n\
\n\
From  claude \u{b7} global\n\
Path  {global}\n\
Into  Primary (source)\n\
\n\
Source replacement\n\
\u{20}\u{20}\u{2190} 1 file changed, +1/-0\n\
\u{20}\u{20}~  SKILL.md  +1/-0\n\
\n\
Propagation preview\n\
\u{20}\u{20}claude \u{b7} global   \u{2713} source copy; synchronized, no file changes\n\
\u{20}\u{20}claude \u{b7} project  \u{2191} 1 file changed, +1/-0\n\
\n\
\u{20}\u{20}1  Import + update  (recommended)\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Replace the source and synchronize 2 deployments (1 source copy, 1 updated).\n\
\n\
\u{20}\u{20}2  Import only\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Replace the source; write no deployments and leave 1 out of date.\n\
\n\
\u{20}\u{20}c  Cancel\n\
\n\
1 source copy; 2 propagation modes\n\
\n\
Imported docwriter from claude \u{b7} global into Primary (source).\n\
Synchronized docwriter -> claude (global) (source copy)\n\
Updated docwriter -> claude (project)\n\
\n\
completed: 1 source replaced (1 file, +1/-0), 2 deployments synchronized (1 source copy, 1 updated)\n",
            global = global_deployment.display()
        )
    );
    assert_eq!(
        stderr_of(&output),
        "Select propagation [1-2, c to cancel]: "
    );
    assert_eq!(
        fs::read_to_string(project.join(".claude/skills/docwriter/SKILL.md")).expect("synced"),
        "# Doc\nglobal edit\n"
    );
}

/// `--update` resolves the propagation dimension without a prompt, but the
/// plan still applies through the ordinary `[y/N]` confirmation -- an
/// explicit flag is not `--yes`.
#[test]
fn import_update_flag_resolves_propagation_without_a_prompt() {
    let (home, project) = import_single_copy_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude", "--update"])
        .write_stdin("y\n")
        .output()
        .expect("run --update import");
    assert!(output.status.success());
    let stdout = stdout_of(output.clone());
    assert!(
        stdout.contains("Mode  import + update (recommended, explicitly selected)"),
        "unexpected stdout:\n{stdout}"
    );
    assert!(
        stdout.contains(
            "1 source replacement; 2 deployments synchronized (1 source copy, 1 updated)"
        )
    );
    assert_eq!(
        stderr_of(&output),
        "Apply this import plan from claude \u{b7} global? [y/N] "
    );
    assert_eq!(
        fs::read_to_string(project.join(".claude/skills/docwriter/SKILL.md")).expect("synced"),
        "# Doc\nglobal edit\n"
    );
}

/// `--no-update` resolves propagation to "Import only" without a prompt;
/// declining the `[y/N]` confirmation writes nothing.
#[test]
fn import_no_update_flag_resolves_propagation_without_a_prompt_and_can_be_declined() {
    let (home, project) = import_single_copy_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude", "--no-update"])
        .write_stdin("n\n")
        .output()
        .expect("run --no-update import decline");
    assert!(output.status.success(), "declining is not a failure");
    let stdout = stdout_of(output.clone());
    assert!(stdout.contains("Mode  import only (explicitly selected)"));
    assert!(
        stdout.contains(
            "1 source replacement from claude \u{b7} global; 1 deployment left out of date"
        )
    );
    assert!(stdout.ends_with("Cancelled.\n"));
    assert_eq!(
        fs::read_to_string(home.path().join("source/docwriter/SKILL.md")).expect("untouched"),
        "# Doc\n"
    );
}

/// M: when propagation is explicitly resolved to import-only and a genuine
/// bystander would be left out of date, the render must not promise a write
/// that will not happen. `Propagation preview` (with `\u{2191}` marking a
/// pending write) is reframed as `Left out of date` staleness, and the
/// resolved copy's own now-none-value "synchronized" entry is dropped
/// entirely rather than reused under a framing it does not fit.
#[test]
fn import_no_update_dry_run_reframes_the_unchosen_propagation_as_staleness() {
    let (home, project) = import_single_copy_fixture();
    let global_deployment =
        portable_canonicalize(home.path().join(".claude/skills/docwriter")).expect("global path");
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args([
            "import",
            "docwriter",
            "--claude",
            "--no-update",
            "--dry-run",
        ])
        .output()
        .expect("run --no-update --dry-run import");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output),
        format!(
            "Import plan\n\
\n\
From  claude \u{b7} global\n\
Path  {global}\n\
Into  Primary (source)\n\
Mode  import only (explicitly selected)\n\
\n\
Source replacement\n\
\u{20}\u{20}\u{2190} 1 file changed, +1/-0\n\
\u{20}\u{20}~  SKILL.md  +1/-0\n\
\n\
Left out of date\n\
\u{20}\u{20}claude \u{b7} project  1 file behind, +1/-0\n\
\n\
1 source replacement from claude \u{b7} global; 1 deployment left out of date\n\
\n\
Dry run \u{2014} no changes were made.\n",
            global = global_deployment.display()
        )
    );
}

/// E6: `--yes` never implies a propagation mode. On an otherwise-unambiguous
/// single copy, `--yes` alone still leaves propagation pending and refuses.
#[test]
fn import_yes_alone_does_not_resolve_propagation_and_is_refused() {
    let (home, project) = import_single_copy_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude", "--yes"])
        .output()
        .expect("run --yes alone on a single copy");
    assert!(!output.status.success());
    assert_eq!(
        stderr_of(&output),
        "Error: propagation choice is required before --yes; pass --update or --no-update.\n"
    );
    assert_eq!(
        fs::read_to_string(home.path().join("source/docwriter/SKILL.md")).expect("untouched"),
        "# Doc\n"
    );
}

/// E6/E9: on a genuinely ambiguous (multi-copy) import, `--yes` refuses
/// until both the source copy and the propagation mode are resolved; the
/// full plan still renders (without the interactive `Cancel` framing) so
/// the failure is self-explanatory.
#[test]
fn import_yes_refuses_when_both_dimensions_are_ambiguous() {
    let (home, project) = import_ambiguous_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude", "--yes"])
        .output()
        .expect("run --yes on an ambiguous import");
    assert!(!output.status.success());
    let body = import_two_copy_dry_body(home.path(), &project);
    assert_eq!(stdout_of(output.clone()), body);
    assert_eq!(
        stderr_of(&output),
        "Error: import requires a source copy and propagation mode before --yes; \
choose exactly one target and scope, then pass --update or --no-update.\n"
    );
    assert_eq!(
        fs::read_to_string(home.path().join("source/docwriter/SKILL.md")).expect("untouched"),
        "# Doc\n"
    );
}

/// E6: with both dimensions resolved explicitly (`--claude --global` narrows
/// the copy, `--update` resolves propagation), `--yes` renders the plan --
/// including the `Mode` metadata line recording that propagation was
/// explicitly selected -- and applies immediately with no prompt at all.
#[test]
fn import_yes_applies_immediately_with_explicit_dimensions() {
    let (home, project) = import_ambiguous_fixture();
    fs::write(project.join(".claude/skills/docwriter/SKILL.md"), "# Doc\n")
        .expect("resync project deployment");
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args([
            "import",
            "docwriter",
            "--claude",
            "--global",
            "--update",
            "--yes",
        ])
        .output()
        .expect("run --yes with both dimensions explicit");
    assert!(output.status.success());
    let stdout = stdout_of(output.clone());
    assert!(stdout.contains("Mode  import + update (recommended, explicitly selected)"));
    assert!(
        stdout.contains(
            "1 source replacement; 2 deployments synchronized (1 source copy, 1 updated)"
        )
    );
    assert!(stdout.contains("Imported docwriter from claude \u{b7} global into Primary (source)."));
    assert!(stdout.contains("Synchronized docwriter -> claude (global) (source copy)"));
    assert!(stdout.contains("Updated docwriter -> claude (project)"));
    assert!(stdout
        .contains("completed: 1 source replaced (1 file, +1/-0), 2 deployments synchronized (1 source copy, 1 updated)"));
    assert_eq!(stderr_of(&output), "", "no prompt at all under --yes");
    assert_eq!(
        fs::read_to_string(project.join(".claude/skills/docwriter/SKILL.md")).expect("synced"),
        "# Doc\nglobal edit\n"
    );
}

/// Applying noninteractively without `--yes` refuses even when both
/// dimensions are already resolved by flags -- flags answer the questions,
/// they do not authorize the write.
#[test]
fn import_no_input_without_yes_refuses_to_apply() {
    let (home, project) = import_single_copy_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude", "--update", "--no-input"])
        .output()
        .expect("run --no-input without --yes");
    assert!(!output.status.success());
    assert_eq!(
        stderr_of(&output),
        "Error: applying this plan noninteractively requires --yes.\n"
    );
    assert_eq!(
        fs::read_to_string(home.path().join("source/docwriter/SKILL.md")).expect("untouched"),
        "# Doc\n"
    );
}

/// `--dry-run` on the single-copy case renders the whole plan (still with
/// its own `c Cancel` option and pending-decision footer, since a dry run is
/// simply the same plan minus the offer to answer it -- no, dry run drops
/// `Cancel` specifically) and exits 0 with a single conclusion, no per-item
/// echoes, and no writes.
#[test]
fn import_dry_run_renders_the_single_copy_plan_and_exits_zero() {
    let (home, project) = import_single_copy_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude", "--dry-run"])
        .output()
        .expect("run single-copy dry run");
    assert!(output.status.success());
    let global_deployment =
        portable_canonicalize(home.path().join(".claude/skills/docwriter")).expect("global path");
    assert_eq!(
        stdout_of(output.clone()),
        format!(
            "Import plan\n\
\n\
From  claude \u{b7} global\n\
Path  {global}\n\
Into  Primary (source)\n\
\n\
Source replacement\n\
\u{20}\u{20}\u{2190} 1 file changed, +1/-0\n\
\u{20}\u{20}~  SKILL.md  +1/-0\n\
\n\
Propagation preview\n\
\u{20}\u{20}claude \u{b7} global   \u{2713} source copy; synchronized, no file changes\n\
\u{20}\u{20}claude \u{b7} project  \u{2191} 1 file changed, +1/-0\n\
\n\
Propagation modes (chosen after the source copy)\n\
\n\
\u{20}\u{20}1  Import + update  (recommended)\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Replace the source and synchronize 2 deployments (1 source copy, 1 updated).\n\
\n\
\u{20}\u{20}2  Import only\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Replace the source; write no deployments and leave 1 out of date.\n\
\n\
1 source copy; 2 propagation modes\n\
\n\
Dry run \u{2014} 2 alternatives shown; no option selected and no changes were made.\n",
            global = global_deployment.display()
        )
    );
    assert!(stderr_of(&output).is_empty(), "a dry run never prompts");
    assert_eq!(
        fs::read_to_string(home.path().join("source/docwriter/SKILL.md")).expect("untouched"),
        "# Doc\n"
    );
}

/// `--dry-run` on the multi-copy case renders the same plan `--yes` would
/// (no `Available source copies` heading, no `c Cancel`) plus the two-copy
/// pending footer, and exits 0 with a single conclusion.
#[test]
fn import_dry_run_renders_the_multi_copy_plan_and_exits_zero() {
    let (home, project) = import_ambiguous_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude", "--dry-run"])
        .output()
        .expect("run multi-copy dry run");
    assert!(output.status.success());
    let body = import_two_copy_dry_body(home.path(), &project);
    assert_eq!(
        stdout_of(output.clone()),
        format!(
            "{body}{IMPORT_TWO_COPY_UNRESOLVED_FOOTER}\n\
Dry run \u{2014} 2 alternatives shown; no option selected and no changes were made.\n"
        )
    );
    assert!(stderr_of(&output).is_empty(), "a dry run never prompts");
}

/// A syntactically valid skill name that is not deployed anywhere fails
/// with `NotFound`, never exiting 0.
#[test]
fn import_reports_missing_skill() {
    let home = sandbox();
    cli(home.path())
        .args(["import", "missing", "--claude"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Error: source skill not found: missing",
        ));
}

/// `import` selects exactly one skill; fnmatch-style patterns are rejected
/// even when they would otherwise resolve to a real skill.
#[test]
fn import_rejects_patterns() {
    let (home, project) = import_single_copy_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    import
        .args(["import", "doc*", "--claude"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Error: import selects exactly one skill and does not accept patterns: doc*",
        ));
}

/// When every deployment already matches the source, nothing is ambiguous
/// and nothing is written; the message never mentions a destination it
/// would not write to, and it exits 0 (a literal that simply is not present
/// among changed deployments, not a pattern matching nothing).
#[test]
fn import_reports_nothing_to_import_when_every_deployment_matches_the_source() {
    let (home, project) = import_single_copy_fixture();
    fs::write(
        home.path().join(".claude/skills/docwriter/SKILL.md"),
        "# Doc\n",
    )
    .expect("resync global deployment too");
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude"])
        .output()
        .expect("run idle import");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output.clone()),
        "docwriter has no changed deployment to import from the enabled targets in global or project scope.\n"
    );
    assert!(stderr_of(&output).is_empty());
}

/// GitHub-backed sources import only into a confirmed local alternate
/// location; without one, the failure names the missing configuration
/// before any plan or write is attempted.
#[test]
fn import_into_a_github_source_requires_a_local_alternate() {
    let home = sandbox();
    cli(home.path())
        .args(["--json", "source", "add", "acme/skills", "remote"])
        .assert()
        .success();
    seed_github_cache(home.path(), "teach", "# teach remote\n");
    cli(home.path())
        .args(["--json", "load", "--claude", "--global", "--no-input"])
        .assert()
        .success();

    let idle = cli(home.path())
        .args(["import", "teach", "--claude", "--global"])
        .output()
        .expect("run idle import against a GitHub source");
    assert!(idle.status.success());
    assert_eq!(
        stdout_of(idle),
        "teach has no changed deployment to import from the enabled targets in global scope.\n"
    );

    fs::write(
        home.path().join(".claude/skills/teach/SKILL.md"),
        "# teach remote\nagent addition\n",
    )
    .expect("agent edits the deployed remote skill");
    let failed = cli(home.path())
        .args(["import", "teach", "--claude", "--global"])
        .output()
        .expect("run import against a GitHub source with no local alternate");
    assert!(!failed.status.success());
    assert!(
        stdout_of(failed.clone()).is_empty(),
        "no plan without a destination to plan against"
    );
    assert_eq!(
        stderr_of(&failed),
        "Error: import writes to local source checkouts only; 'remote' is GitHub-backed \
(acme/skills) and has no local alternate location. Add one with: skill-manager source alternate remote <local-path>\n"
    );
}

/// Once a local alternate is configured, the ordinary single-prompt plan
/// applies against it like any other source: `Into` names the local
/// alternate, and `--no-update --yes` applies without prompting.
#[test]
fn import_writes_to_a_configured_github_local_alternate_after_review() {
    let home = sandbox();
    cli(home.path())
        .args(["--json", "source", "add", "acme/skills", "remote"])
        .assert()
        .success();
    seed_github_cache(home.path(), "teach", "# teach remote\n");
    cli(home.path())
        .args(["--json", "load", "--claude", "--global", "--no-input"])
        .assert()
        .success();
    fs::write(
        home.path().join(".claude/skills/teach/SKILL.md"),
        "# teach remote\nagent addition\n",
    )
    .expect("agent edits the deployed remote skill");

    let checkout = home.path().join("checkout");
    fs::create_dir_all(&checkout).expect("create local alternate checkout");
    cli(home.path())
        .args([
            "--json",
            "source",
            "alternate",
            "remote",
            checkout.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();

    let applied = cli(home.path())
        .args([
            "import",
            "teach",
            "--claude",
            "--global",
            "--no-update",
            "--yes",
        ])
        .output()
        .expect("run import against the configured local alternate");
    assert!(applied.status.success());
    let applied_stdout = stdout_of(applied);
    assert!(
        applied_stdout.contains("local alternate"),
        "Into must name the local alternate:\n{applied_stdout}"
    );
    assert!(
        applied_stdout.contains("Imported teach from claude \u{b7} global into Remote (source).")
    );
    assert_eq!(
        fs::read_to_string(checkout.join("teach/SKILL.md")).expect("alternate receives import"),
        "# teach remote\nagent addition\n"
    );
}

/// Displayed source and deployment paths never leak Windows verbatim
/// prefixes, in either human or machine output.
#[test]
fn import_paths_are_reported_without_verbatim_prefixes() {
    let (home, project) = import_single_copy_fixture();
    let expected_deployment = portable_canonicalize(home.path().join(".claude/skills/docwriter"))
        .expect("canonical deployment");
    let mut import = cli(home.path());
    import.current_dir(&project);
    let human = import
        .args(["import", "docwriter", "--claude", "--update", "--yes"])
        .output()
        .expect("run human import");
    assert!(human.status.success());
    let stdout = stdout_of(human);
    assert!(stdout.contains("Imported docwriter"));
    assert!(
        !stdout.contains(VERBATIM_PREFIX),
        "human paths must not use verbatim spellings: {stdout}"
    );
    assert!(stdout.contains(&expected_deployment.display().to_string()));

    fs::write(
        home.path().join(".claude/skills/docwriter/SKILL.md"),
        "# Doc\nedited again\n",
    )
    .expect("edit again");
    let mut machine = cli(home.path());
    machine.current_dir(&project);
    let events = json_events(
        machine
            .args(["--json", "import", "docwriter", "--claude", "--dry-run"])
            .output()
            .expect("run machine dry run"),
    );
    let plan = events
        .iter()
        .find(|event| event["event"] == "plan")
        .expect("plan event");
    let plan_text = plan.to_string();
    assert!(
        !plan_text.contains(VERBATIM_PREFIX),
        "machine paths must not use verbatim spellings: {plan_text}"
    );
    let consequence_path = plan["data"]["decisions"][0]["options"][0]["consequence"]["path"]
        .as_str()
        .expect("the resolved candidate's option carries its own path");
    assert!(
        !consequence_path.contains(VERBATIM_PREFIX),
        "the resolved candidate's own path (the only path serialized for a single-copy \
plan) must be clean: {consequence_path}"
    );
}

/// Apply order equals `review_sequence()`, the same order the plan showed:
/// the resolved candidate's own now-redundant deployment is reported before
/// the deployment that genuinely changes, exactly as the propagation
/// preview listed them (`claude \u{b7} global` before `claude \u{b7} project`).
#[test]
fn import_plan_order_equals_apply_order() {
    let (home, project) = import_ambiguous_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude"])
        .write_stdin("2\n1\n")
        .output()
        .expect("run full accepted import");
    assert!(output.status.success());
    let stdout = stdout_of(output);
    let synchronized_at = stdout
        .find("Synchronized docwriter -> claude (global) (source copy)")
        .expect("synchronized line present");
    let updated_at = stdout
        .find("Updated docwriter -> claude (project)")
        .expect("updated line present");
    assert!(
        synchronized_at < updated_at,
        "apply order must match the propagation preview's destination order:\n{stdout}"
    );
    let preview_global_at = stdout
        .find("claude \u{b7} global   \u{2713} source copy; synchronized, no file changes")
        .expect("preview line for the resolved copy");
    let preview_project_at = stdout
        .rfind("claude \u{b7} project  \u{2191} 1 file changed, +1/-1")
        .expect("preview line for the updated copy");
    assert!(preview_global_at < preview_project_at);
}

/// E8 regression test: propagating a candidate whose own deployment matches
/// the source, into a deployment that has genuinely drifted apart (missing
/// a file the candidate lacks -- gaining one, and needing its `SKILL.md`
/// rewritten), advertises `2 files changed, +1/-1` in the plan preview. The
/// actual apply must write exactly those two files with exactly that
/// aggregate diff, proving both numbers are derived from the very
/// enumeration that apply uses, not a representative copy's count (Stage
/// 3's remove defect).
/// E8 regression test: propagating a resolved candidate whose own diff from
/// source is a single line, into a deployment that has genuinely drifted
/// apart (gaining a file the candidate lacks, needing both an add/remove and
/// its `SKILL.md` rewritten), advertises `2 files changed, +1/-1` in the
/// plan preview -- both under the losing candidate's own nested preview and
/// under the resolved candidate's. The actual apply must write exactly
/// those two files with exactly that aggregate diff, proving both numbers
/// are derived from the very enumeration that apply uses, not a
/// representative copy's count (Stage 3's remove defect).
#[test]
fn import_plan_event_reports_true_per_option_totals_when_deployments_have_drifted_apart() {
    let (home, project) = import_drifted_fixture();
    let mut dry_run = cli(home.path());
    dry_run.current_dir(&project);
    let dry_output = dry_run
        .args(["import", "docwriter", "--claude", "--dry-run"])
        .output()
        .expect("run drifted dry run");
    assert!(dry_output.status.success());
    let dry_stdout = stdout_of(dry_output);
    assert_eq!(
        dry_stdout
            .matches("claude \u{b7} project  \u{2191} 2 files changed, +1/-1")
            .count(),
        1,
        "only source copy 2's nested preview shows project's true drifted diff:\n{dry_stdout}"
    );
    assert_eq!(
        dry_stdout
            .matches("claude \u{b7} global   \u{2191} 2 files changed, +1/-1")
            .count(),
        1,
        "source copy 1's nested preview shows global's symmetric drifted diff:\n{dry_stdout}"
    );

    let mut apply = cli(home.path());
    apply.current_dir(&project);
    let output = apply
        .args(["import", "docwriter", "--claude"])
        .write_stdin("2\n1\n")
        .output()
        .expect("apply the drifted propagation, choosing source copy 2 (claude · global)");
    assert!(output.status.success());
    assert_eq!(
        fs::read_to_string(home.path().join("source/docwriter/SKILL.md")).expect("new source"),
        "# Doc\nglobal edit\n"
    );
    assert_eq!(
        fs::read_to_string(project.join(".claude/skills/docwriter/SKILL.md")).expect("rewritten"),
        "# Doc\nglobal edit\n"
    );
    assert!(
        !project.join(".claude/skills/docwriter/notes.md").exists(),
        "the extra file the new source lacks must be removed, matching the advertised diff"
    );
}

/// The single-copy dry-run `plan` event (revision 0) already carries a
/// resolved `source_copy` decision -- there was only ever one candidate --
/// while `propagation` stays pending; every option carries its typed
/// consequence, including the nested per-destination actions a source-copy
/// option lists.
#[test]
fn import_emits_a_structured_plan_event_with_the_resolved_source_copy() {
    let (home, project) = import_single_copy_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let events = json_events(
        import
            .args(["--json", "import", "docwriter", "--claude", "--dry-run"])
            .output()
            .expect("run machine dry run"),
    );
    let plans = events_of(&events, "plan");
    assert_eq!(plans.len(), 1, "exactly one plan event: {events:?}");
    let data = &plans[0]["data"];
    assert_eq!(data["plan_id"], "import:docwriter");
    assert_eq!(data["revision"], 0);
    assert_eq!(data["dry_run"], true);
    assert_eq!(data["authorization"]["kind"], "progressive");
    assert_eq!(data["authorization"]["mode"], "dry-run");
    assert_eq!(
        data["authorization"]["sequence"],
        json!(["source_copy", "propagation"])
    );
    assert_eq!(
        data["authorization"]["resolved"],
        json!({ "source_copy": "claude:global" })
    );
    assert_eq!(data["authorization"]["pending"], json!(["propagation"]));

    let decisions = data["decisions"].as_array().expect("decisions array");
    assert_eq!(decisions.len(), 2);
    let source_decision = &decisions[0];
    assert_eq!(source_decision["id"], "source_copy");
    assert_eq!(source_decision["state"], "resolved");
    assert_eq!(source_decision["resolved"], "claude:global");
    let source_options = source_decision["options"].as_array().expect("options");
    assert_eq!(source_options.len(), 1);
    assert_eq!(source_options[0]["id"], "claude:global");
    assert_eq!(source_options[0]["token"], "1");
    let source_actions = source_options[0]["consequence"]["actions"]
        .as_array()
        .expect("nested actions");
    assert_eq!(
        source_actions.len(),
        3,
        "import + skip-self + update: {source_actions:?}"
    );
    assert_eq!(source_options[0]["consequence"]["totals"]["deployments"], 2);

    let propagation_decision = &decisions[1];
    assert_eq!(propagation_decision["id"], "propagation");
    assert_eq!(propagation_decision["state"], "pending");
    let propagation_options = propagation_decision["options"].as_array().expect("options");
    assert_eq!(propagation_options[0]["id"], "import-update");
    assert_eq!(propagation_options[0]["recommended"], true);
    assert_eq!(
        propagation_options[0]["consequence"]["totals"]["updated"],
        1
    );
    assert_eq!(
        propagation_options[0]["consequence"]["totals"]["skipped"],
        1
    );
    assert_eq!(propagation_options[1]["id"], "import-only");
    assert_eq!(propagation_options[1]["consequence"]["totals"]["stale"], 1);
}

/// The multi-copy `plan` event (revision 0) leaves both decisions pending
/// and serializes every source-copy option's own nested propagation
/// preview, proving the NDJSON stream never depends on human-only gating.
#[test]
fn import_revision_zero_serializes_every_source_option_and_its_propagation() {
    let (home, project) = import_ambiguous_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let events = json_events(
        import
            .args(["--json", "import", "docwriter", "--claude", "--dry-run"])
            .output()
            .expect("run machine dry run"),
    );
    let plans = events_of(&events, "plan");
    assert_eq!(plans.len(), 1);
    let data = &plans[0]["data"];
    assert_eq!(data["authorization"]["resolved"], json!({}));
    assert_eq!(
        data["authorization"]["pending"],
        json!(["source_copy", "propagation"])
    );
    let decisions = data["decisions"].as_array().expect("decisions");
    let source_options = decisions[0]["options"].as_array().expect("source options");
    assert_eq!(source_options.len(), 2);
    for option in source_options {
        let actions = option["consequence"]["actions"]
            .as_array()
            .expect("actions");
        assert_eq!(actions.len(), 3);
        assert_eq!(option["consequence"]["totals"]["deployments"], 2);
    }
    assert_eq!(source_options[0]["id"], "claude:project");
    assert_eq!(source_options[1]["id"], "claude:global");
}

/// Interactive symbol+color rendering for a terminal user: the multi-copy
/// branch plan under `--color always` with `SKILL_MANAGER_FORCE_INTERACTIVE`
/// colors the section headings and delta markers and leaves unchanged
/// entries (`\u{2713} source copy; synchronized, no file changes`) uncolored.
#[test]
fn import_renders_the_interactive_symbol_and_color_branch_plan_for_a_terminal_user() {
    let (home, project) = import_ambiguous_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let stdout = stdout_of(
        import
            .env_remove("NO_COLOR")
            .env("SKILL_MANAGER_FORCE_INTERACTIVE", "1")
            .args([
                "import",
                "docwriter",
                "--claude",
                "--color",
                "always",
                "--dry-run",
            ])
            .output()
            .expect("run interactive dry-run import"),
    );
    let project_deployment =
        portable_canonicalize(project.join(".claude/skills/docwriter")).expect("project path");
    let global_deployment =
        portable_canonicalize(home.path().join(".claude/skills/docwriter")).expect("global path");
    assert_eq!(
        stdout,
        format!(
            "\u{1b}[1;36mImport plan\u{1b}[0m\n\
\n\
Into  Primary (source)\n\
\n\
\u{1b}[1;36mSource copies (chosen first)\u{1b}[0m\n\
\n\
\u{20}\u{20}1  claude \u{b7} project\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Path    {project}\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Source  \u{1b}[33m\u{2190} 1 file changed, +1/-0\u{1b}[0m\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{1b}[33m~\u{1b}[0m  SKILL.md  +1/-0\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{1b}[1;36mPropagation with import + update\u{1b}[0m  2 deployments\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}claude \u{b7} global   \u{1b}[33m\u{2191} 1 file changed, +1/-1\u{1b}[0m\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}claude \u{b7} project  \u{2713} source copy; synchronized, no file changes\n\
\n\
\u{20}\u{20}2  claude \u{b7} global\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Path    {global}\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Source  \u{1b}[33m\u{2190} 1 file changed, +1/-0\u{1b}[0m\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{1b}[33m~\u{1b}[0m  SKILL.md  +1/-0\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{1b}[1;36mPropagation with import + update\u{1b}[0m  2 deployments\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}claude \u{b7} global   \u{2713} source copy; synchronized, no file changes\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}claude \u{b7} project  \u{1b}[33m\u{2191} 1 file changed, +1/-1\u{1b}[0m\n\
\n\
\u{1b}[1;36mPropagation modes (chosen after the source copy)\u{1b}[0m\n\
\n\
\u{20}\u{20}1  Import + update  (recommended)\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Replace the source, then synchronize every deployment shown for that copy.\n\
\n\
\u{20}\u{20}2  Import only\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Replace the source; write no deployments and leave the other 1 out of date.\n\
\n\
2 source copies; propagation decision follows source selection\n\
\n\
Dry run \u{2014} 2 alternatives shown; no option selected and no changes were made.\n",
            project = project_deployment.display(),
            global = global_deployment.display()
        )
    );
}

/// The same terminal user reviewing the single-copy plan sees the
/// collapsed rendering: `From`/`Path` metadata from the start, a colored
/// `Source replacement` and `Propagation preview` heading, and the same
/// uncolored `\u{2713}` for the resolved copy's own now-redundant entry.
#[test]
fn import_renders_the_interactive_symbol_and_color_collapsed_plan_for_a_terminal_user() {
    let (home, project) = import_single_copy_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let stdout = stdout_of(
        import
            .env_remove("NO_COLOR")
            .env("SKILL_MANAGER_FORCE_INTERACTIVE", "1")
            .args([
                "import",
                "docwriter",
                "--claude",
                "--color",
                "always",
                "--dry-run",
            ])
            .output()
            .expect("run interactive collapsed dry-run import"),
    );
    let global_deployment =
        portable_canonicalize(home.path().join(".claude/skills/docwriter")).expect("global path");
    assert_eq!(
        stdout,
        format!(
            "\u{1b}[1;36mImport plan\u{1b}[0m\n\
\n\
From  claude \u{b7} global\n\
Path  {global}\n\
Into  Primary (source)\n\
\n\
\u{1b}[1;36mSource replacement\u{1b}[0m\n\
\u{20}\u{20}\u{1b}[33m\u{2190} 1 file changed, +1/-0\u{1b}[0m\n\
\u{20}\u{20}\u{1b}[33m~\u{1b}[0m  SKILL.md  +1/-0\n\
\n\
\u{1b}[1;36mPropagation preview\u{1b}[0m\n\
\u{20}\u{20}claude \u{b7} global   \u{2713} source copy; synchronized, no file changes\n\
\u{20}\u{20}claude \u{b7} project  \u{1b}[33m\u{2191} 1 file changed, +1/-0\u{1b}[0m\n\
\n\
\u{1b}[1;36mPropagation modes (chosen after the source copy)\u{1b}[0m\n\
\n\
\u{20}\u{20}1  Import + update  (recommended)\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Replace the source and synchronize 2 deployments (1 source copy, 1 updated).\n\
\n\
\u{20}\u{20}2  Import only\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Replace the source; write no deployments and leave 1 out of date.\n\
\n\
1 source copy; 2 propagation modes\n\
\n\
Dry run \u{2014} 2 alternatives shown; no option selected and no changes were made.\n",
            global = global_deployment.display()
        )
    );
}

/// E1/E9/L: exactly one deployment total (not merely one *candidate* among
/// several deployments -- see the item-B/C fixture below for that case).
/// Propagating a resolved source copy to "every other deployment" is
/// vacuous when there is no other deployment, so propagation is degenerate
/// for the only candidate there is and the whole command collapses to a
/// plain plan with no decisions at all: no `Propagation modes` section, no
/// zero-valued counts anywhere, and a plain `[y/N]`-shaped `--yes` recipe
/// contract identical to `update`/`load` (item A). This is the exact
/// harness the adversarial review used to find item A.
fn import_degenerate_fixture() -> TempDir {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--claude", "--global", "--no-input"])
        .assert()
        .success();
    fs::write(
        home.path().join(".claude/skills/alpha/SKILL.md"),
        "# Alpha\nedit",
    )
    .expect("edit the only deployment");
    home
}

/// Two candidates whose content is byte-identical to each other (both
/// deployments were edited to the same text), so adopting either produces
/// the same resulting source and the same (empty) propagation -- not a
/// genuine branch by the same rule item A applies to propagation (item I).
/// With only these two deployments in existence, propagation is *also*
/// degenerate once resolved, so both dimensions collapse silently and there
/// is no prompt at all.
fn import_identical_candidates_fixture() -> TempDir {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--claude", "--global", "--no-input"])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--shared", "--global", "--no-input"])
        .assert()
        .success();
    fs::write(
        home.path().join(".claude/skills/alpha/SKILL.md"),
        "# Alpha\nedit",
    )
    .expect("edit claude deployment");
    fs::write(
        home.path().join(".agents/skills/alpha/SKILL.md"),
        "# Alpha\nedit",
    )
    .expect("edit shared deployment identically");
    home
}

/// Same two identical candidates as [`import_identical_candidates_fixture`],
/// plus a third, untouched deployment that still matches the *original*
/// source and so is not itself a `source_copy` candidate -- but it is a
/// genuine propagation target once one of the identical candidates is
/// adopted, since the new source content differs from the untouched
/// deployment. `source_copy` still collapses silently (item I): the only
/// two candidates are identical to each other. Propagation, however, stays
/// genuinely pending, so the whole session is exactly one prompt --
/// propagation -- reached via silent collapse rather than single candidacy.
fn import_identical_candidates_with_bystander_fixture() -> TempDir {
    let home = import_identical_candidates_fixture();
    cli(home.path())
        .args(["--json", "load", "--antigravity", "--global", "--no-input"])
        .assert()
        .success();
    // Left untouched: still equals the original source, so it is a genuine
    // propagation target for either identical candidate but never itself a
    // `source_copy` candidate.
    home
}

/// Three deployments: `claude` and `shared` are edited to the same content
/// (so one is a genuine `source_copy` candidate and the other merely
/// happens to be identical to it, never itself the resolved identity), and
/// `antigravity` is edited to *different* content (so it stays genuinely
/// out of date after either is chosen, keeping propagation genuine and the
/// `Propagation preview` block from being gated away). Proves item B: only
/// the resolved copy is labelled "source copy"; a merely-identical
/// deployment gets its own honest "synchronized, no file changes" label,
/// and the apply-time footer breakdown accounts for it separately from
/// "updated" so the three counts always sum to the deployment count (item
/// C's `skill.skipped` event and item F's "already identical" clause).
fn import_identical_bystander_fixture() -> TempDir {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
        ])
        .assert()
        .success();
    for target in ["--claude", "--shared", "--antigravity"] {
        cli(home.path())
            .args(["--json", "load", target, "--global", "--no-input"])
            .assert()
            .success();
    }
    fs::write(
        home.path().join(".claude/skills/alpha/SKILL.md"),
        "# Alpha\nedit",
    )
    .expect("edit claude deployment");
    fs::write(
        home.path().join(".agents/skills/alpha/SKILL.md"),
        "# Alpha\nedit",
    )
    .expect("edit shared deployment identically to claude");
    fs::write(
        home.path()
            .join(".gemini/antigravity/skills/alpha/SKILL.md"),
        "# Alpha\nother edit",
    )
    .expect("edit antigravity deployment to different content");
    home
}

/// E1/E9/E10/A/L: with exactly one deployment total, propagation cannot
/// possibly be genuine (there is nothing else to propagate to), so it
/// resolves silently: no `Propagation modes` section, no `Mode` metadata
/// line, and no zero-valued counts anywhere in the plan. The whole command
/// degenerates to a plain-plan-plus-confirmation shape identical to
/// `update`/`load`.
#[test]
fn import_degenerate_propagation_renders_a_plain_plan_with_no_decisions() {
    let home = import_degenerate_fixture();
    let global = portable_canonicalize(home.path().join(".claude/skills/alpha")).expect("path");
    let output = cli(home.path())
        .args(["import", "alpha", "--claude", "--global", "--dry-run"])
        .output()
        .expect("run degenerate dry-run import");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output),
        format!(
            "Import plan\n\
\n\
From  claude \u{b7} global\n\
Path  {global}\n\
Into  Primary (source)\n\
\n\
Source replacement\n\
\u{20}\u{20}\u{2190} 1 file changed, +1/-0\n\
\u{20}\u{20}~  SKILL.md  +1/-0\n\
\n\
1 source replacement from claude \u{b7} global\n\
\n\
Dry run \u{2014} no changes were made.\n",
            global = global.display()
        )
    );
}

/// A/E: the exact adversarial-review repro. The original recipe
/// (`yes:true` with no `update`/`no_update`) previously broke because
/// propagation was forced as a genuine dimension even when both answers are
/// provably identical; after the fix it succeeds again, since propagation
/// resolves silently rather than refusing `--yes`/`yes:true`. Also restores
/// the binary-content round-trip coverage that
/// `import_renders_a_plain_plan_and_runs_from_a_recipe` provided before
/// this migration (item J): a binary file present only in the deployment
/// must be written into the source and be byte-identical on the far side.
#[test]
fn import_yes_alone_commits_a_degenerate_recipe_and_round_trips_binary_content() {
    let home = import_degenerate_fixture();
    let deployed = home.path().join(".claude/skills/alpha");
    fs::write(deployed.join("logo.bin"), [0_u8, 1, 2, 3]).expect("agent adds binary content");

    let events = json_events(
        cli(home.path())
            .arg("--json-input")
            .write_stdin(
                serde_json::json!({
                    "command": "import",
                    "skill": "alpha",
                    "claude": true,
                    "global": true,
                    "yes": true
                })
                .to_string(),
            )
            .output()
            .expect("run the degenerate recipe with only yes:true"),
    );
    assert!(events_of(&events, "command.failed").is_empty());
    let plan = events_of(&events, "plan")[0];
    assert_eq!(
        plan["data"]["authorization"]["resolved"]["propagation"],
        "import-only"
    );
    assert!(!events_of(&events, "skill.imported").is_empty());
    assert!(
        events_of(&events, "skill.skipped").is_empty(),
        "import-only never loops the deployment list"
    );

    assert_eq!(
        fs::read_to_string(home.path().join("source/alpha/SKILL.md")).expect("imported"),
        "# Alpha\nedit"
    );
    assert_eq!(
        fs::read(home.path().join("source/alpha/logo.bin")).expect("binary round trip"),
        [0_u8, 1, 2, 3]
    );
}

/// N: an explicit `--update` must stay honest in the machine stream even on
/// a degenerate plan -- `resolved` records `import-update`, the mode the
/// caller actually asked for, never the silent default that would apply had
/// no flag been given. Because propagation genuinely does nothing here
/// (`updated: 0`), the applied stream still completes with a `skill.skipped`
/// for the sole deployment (honest completeness: import-update was recorded,
/// so it runs and reports what it found), while the human footer keeps
/// gating the resulting zero count and reads exactly as the plain,
/// no-decision form -- never `(1 source copy, 0 updated)`.
#[test]
fn import_explicit_update_on_a_degenerate_plan_records_import_update_and_hides_the_zero_count() {
    let home = import_degenerate_fixture();

    let events = json_events(
        cli(home.path())
            .arg("--json-input")
            .write_stdin(
                serde_json::json!({
                    "command": "import",
                    "skill": "alpha",
                    "claude": true,
                    "global": true,
                    "update": true,
                    "yes": true
                })
                .to_string(),
            )
            .output()
            .expect("run the degenerate recipe with an explicit update:true"),
    );
    assert!(events_of(&events, "command.failed").is_empty());
    let plan = events_of(&events, "plan")[0];
    assert_eq!(
        plan["data"]["authorization"]["resolved"]["propagation"], "import-update",
        "an explicit --update must be recorded honestly even when degenerate"
    );
    let propagation_option = plan["data"]["decisions"][1]["options"][0].clone();
    assert_eq!(propagation_option["id"], "import-update");
    assert_eq!(propagation_option["consequence"]["totals"]["updated"], 0);
    assert!(!events_of(&events, "skill.imported").is_empty());
    assert_eq!(
        events_of(&events, "skill.skipped").len(),
        1,
        "import-update was recorded, so it runs and reports the one no-op deployment"
    );

    let human_home = import_degenerate_fixture();
    let output = cli(human_home.path())
        .args([
            "import", "alpha", "--claude", "--global", "--update", "--yes",
        ])
        .output()
        .expect("run the human-facing form of the same scenario");
    assert!(output.status.success());
    let human_global =
        portable_canonicalize(human_home.path().join(".claude/skills/alpha")).expect("path");
    assert_eq!(
        stdout_of(output),
        format!(
            "Import plan\n\
\n\
From  claude \u{b7} global\n\
Path  {global}\n\
Into  Primary (source)\n\
\n\
Source replacement\n\
\u{20}\u{20}\u{2190} 1 file changed, +1/-0\n\
\u{20}\u{20}~  SKILL.md  +1/-0\n\
\n\
1 source replacement from claude \u{b7} global\n\
\n\
Imported alpha from claude \u{b7} global into Primary (source).\n\
Synchronized alpha -> claude (global) (source copy)\n\
\n\
completed: 1 source replaced (1 file, +1/-0)\n",
            global = human_global.display()
        )
    );
    let dry_run_home = import_degenerate_fixture();
    assert!(
        !stdout_of(
            cli(dry_run_home.path())
                .args([
                    "import",
                    "alpha",
                    "--claude",
                    "--global",
                    "--update",
                    "--dry-run"
                ])
                .output()
                .expect("dry-run the same scenario")
        )
        .contains("0 updated"),
        "the human footer must never print a forbidden zero count"
    );
}

/// I: when every candidate is byte-identical to every other, forcing a
/// choice between them would ask a question with no observable answer, so
/// `source_copy` resolves silently to the first in configured order --
/// exactly like the single-real-candidate case. With only these two
/// deployments, propagation is degenerate too (item A), so the whole
/// session has zero prompts.
#[test]
fn import_byte_identical_candidates_resolve_source_copy_silently_with_no_prompt() {
    let home = import_identical_candidates_fixture();
    let claude = portable_canonicalize(home.path().join(".claude/skills/alpha")).expect("path");
    let output = cli(home.path())
        .args(["import", "alpha", "--dry-run"])
        .output()
        .expect("run identical-candidates dry-run import");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output),
        format!(
            "Import plan\n\
\n\
From  claude \u{b7} global\n\
Path  {claude}\n\
Into  Primary (source)\n\
\n\
Source replacement\n\
\u{20}\u{20}\u{2190} 1 file changed, +1/-0\n\
\u{20}\u{20}~  SKILL.md  +1/-0\n\
\n\
1 source replacement from claude \u{b7} global\n\
\n\
Dry run \u{2014} no changes were made.\n",
            claude = claude.display()
        )
    );
    let applied = cli(home.path())
        .args(["import", "alpha", "--yes"])
        .output()
        .expect("run identical-candidates apply");
    assert!(applied.status.success());
    assert_eq!(
        fs::read_to_string(home.path().join("source/alpha/SKILL.md")).expect("imported"),
        "# Alpha\nedit"
    );
}

/// I (bystander variant): `source_copy` still collapses silently because
/// the two candidates are byte-identical, but a third, untouched deployment
/// keeps propagation genuinely pending -- reached via silent collapse
/// rather than single candidacy (E5's other route). Exactly one prompt.
/// Also proves the pending footer never claims a choice existed for a
/// dimension that was never rendered as one: "1 source copy" (not "2 source
/// copies"), since only one alternative was ever genuinely on offer.
#[test]
fn import_identical_candidates_with_a_genuine_bystander_ask_only_about_propagation() {
    let home = import_identical_candidates_with_bystander_fixture();
    let claude = portable_canonicalize(home.path().join(".claude/skills/alpha")).expect("path");
    let output = cli(home.path())
        .args(["import", "alpha", "--dry-run"])
        .output()
        .expect("run identical-candidates-with-bystander dry-run import");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output),
        format!(
            "Import plan\n\
\n\
From  claude \u{b7} global\n\
Path  {claude}\n\
Into  Primary (source)\n\
\n\
Source replacement\n\
\u{20}\u{20}\u{2190} 1 file changed, +1/-0\n\
\u{20}\u{20}~  SKILL.md  +1/-0\n\
\n\
Propagation preview\n\
\u{20}\u{20}claude \u{b7} global       \u{2713} source copy; synchronized, no file changes\n\
\u{20}\u{20}shared \u{b7} global       \u{2713} synchronized, no file changes\n\
\u{20}\u{20}antigravity \u{b7} global  \u{2191} 1 file changed, +1/-0\n\
\n\
Propagation modes (chosen after the source copy)\n\
\n\
\u{20}\u{20}1  Import + update  (recommended)\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Replace the source and synchronize 3 deployments (1 source copy, 1 updated).\n\
\n\
\u{20}\u{20}2  Import only\n\
\u{20}\u{20}\u{20}\u{20}\u{20}Replace the source; write no deployments and leave 1 out of date.\n\
\n\
1 source copy; 2 propagation modes\n\
\n\
Dry run \u{2014} 2 alternatives shown; no option selected and no changes were made.\n",
            claude = claude.display()
        )
    );
    let output = cli(home.path())
        .args(["import", "alpha"])
        .write_stdin("1\n")
        .output()
        .expect("run identical-candidates-with-bystander apply");
    assert!(output.status.success());
    assert_eq!(
        stderr_of(&output),
        "Select propagation [1-2, c to cancel]: "
    );
    assert!(
        stdout_of(output).contains(
            "completed: 1 source replaced (1 file, +1/-0), 3 deployments synchronized (1 source copy, 1 updated, 1 already identical)"
        )
    );
}

/// B/C/F: only the deployment actually resolved as the source copy is
/// labelled "source copy"; a merely byte-identical bystander gets its own
/// honest "synchronized, no file changes" label in every rendering
/// (options, propagation preview, and the applied human lines), and the
/// applied `skill.skipped` machine event fires for it (item C). The footer
/// breakdown (`1 source copy, 1 updated, 1 already identical`) sums to the
/// full deployment count.
#[test]
fn import_labels_only_the_resolved_copy_as_source_copy_and_emits_skipped_for_a_bystander() {
    let home = import_identical_bystander_fixture();
    let claude = portable_canonicalize(home.path().join(".claude/skills/alpha")).expect("path");
    let output = cli(home.path())
        .args([
            "import",
            "alpha",
            "--claude",
            "--global",
            "--update",
            "--dry-run",
        ])
        .output()
        .expect("run bystander dry-run import");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output),
        format!(
            "Import plan\n\
\n\
From  claude \u{b7} global\n\
Path  {claude}\n\
Into  Primary (source)\n\
Mode  import + update (recommended, explicitly selected)\n\
\n\
Source replacement\n\
\u{20}\u{20}\u{2190} 1 file changed, +1/-0\n\
\u{20}\u{20}~  SKILL.md  +1/-0\n\
\n\
Propagation preview\n\
\u{20}\u{20}claude \u{b7} global       \u{2713} source copy; synchronized, no file changes\n\
\u{20}\u{20}shared \u{b7} global       \u{2713} synchronized, no file changes\n\
\u{20}\u{20}antigravity \u{b7} global  \u{2191} 1 file changed, +1/-1\n\
\n\
1 source replacement; 3 deployments synchronized (1 source copy, 1 updated, 1 already identical)\n\
\n\
Dry run \u{2014} no changes were made.\n",
            claude = claude.display()
        )
    );

    let events = json_events(
        cli(home.path())
            .args([
                "--json", "import", "alpha", "--claude", "--global", "--update", "--yes",
            ])
            .output()
            .expect("run bystander apply"),
    );
    assert!(events_of(&events, "command.failed").is_empty());
    let skipped = events_of(&events, "skill.skipped");
    assert_eq!(
        skipped.len(),
        2,
        "the resolved source copy and the byte-identical bystander both skip"
    );
    let skipped_targets: BTreeSet<&str> = skipped
        .iter()
        .map(|event| event["data"]["target"].as_str().expect("target"))
        .collect();
    assert_eq!(
        skipped_targets,
        BTreeSet::from(["claude", "shared"]),
        "claude is the resolved source copy; shared merely happens to be identical"
    );
    let updated = events_of(&events, "skill.updated");
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0]["data"]["target"], "antigravity");

    assert_eq!(
        fs::read_to_string(home.path().join(".agents/skills/alpha/SKILL.md")).expect("bystander"),
        "# Alpha\nedit"
    );
    assert_eq!(
        fs::read_to_string(
            home.path()
                .join(".gemini/antigravity/skills/alpha/SKILL.md")
        )
        .expect("synced"),
        "# Alpha\nedit"
    );
}

/// J: several changed deployments require selection, which recipes cannot
/// supply -- but once a genuine ambiguity is resolved interactively, the
/// opposite propagation direction (project source copy synchronized down
/// to the global deployment) must actually apply and write, not merely be
/// previewed. Existing coverage only applied the global-to-project
/// direction.
#[test]
fn import_project_to_global_propagation_actually_writes_in_the_reverse_direction() {
    let (home, project) = import_ambiguous_fixture();
    let mut import = cli(home.path());
    import.current_dir(&project);
    let output = import
        .args(["import", "docwriter", "--claude"])
        .write_stdin("1\n1\n")
        .output()
        .expect("choose the project copy, then import + update");
    assert!(output.status.success());
    assert!(
        stdout_of(output)
            .contains("Imported docwriter from claude \u{b7} project into Primary (source).")
    );
    assert_eq!(
        fs::read_to_string(home.path().join("source/docwriter/SKILL.md")).expect("imported"),
        "# Doc\nproject edit\n"
    );
    assert_eq!(
        fs::read_to_string(home.path().join(".claude/skills/docwriter/SKILL.md"))
            .expect("global deployment synced from the project copy"),
        "# Doc\nproject edit\n"
    );
    assert_eq!(
        fs::read_to_string(project.join(".claude/skills/docwriter/SKILL.md"))
            .expect("project deployment is the resolved source copy, left as-is"),
        "# Doc\nproject edit\n"
    );
}

/// Update displays its plan first and only then asks for one confirmation.
#[test]
fn update_confirms_a_rendered_plan_before_deploying() {
    let home = sandbox();
    let source = home.path().join("source");
    let alpha = create_skill(&source, "alpha", "# Alpha\n");
    create_skill(&source, "beta", "# Beta\n");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 path"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--claude", "--global", "--no-input"])
        .assert()
        .success();
    let deployed = home.path().join(".claude/skills/alpha/SKILL.md");
    fs::write(alpha.join("SKILL.md"), "# Alpha\nsecond line\n").expect("edit source skill");

    cli(home.path())
        .args(["update", "--claude", "--global"])
        .write_stdin("n\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Update plan"))
        .stdout(predicate::str::contains(
            "update alpha -> claude: 1 file changed, +1/-0",
        ))
        .stdout(predicate::str::contains("beta").not())
        .stdout(predicate::str::contains(
            "1 update across 1 selected target",
        ))
        .stdout(predicate::str::contains("Cancelled."))
        .stderr(predicate::str::contains(
            "Apply this update plan to 1 selected target?",
        ));
    assert_eq!(
        fs::read_to_string(&deployed).expect("declined update deploys nothing"),
        "# Alpha\n"
    );

    let accepted = cli(home.path())
        .args(["update", "--claude", "--global"])
        .write_stdin("y\n")
        .output()
        .expect("run accepted update");
    assert!(accepted.status.success());
    let stdout = String::from_utf8(accepted.stdout).expect("utf8 update output");
    assert!(stdout.contains("Update plan"));
    assert!(stdout.contains("Updated alpha -> claude"));
    assert!(
        !stdout.contains("Updated alpha -> claude (global)"),
        "an explicit uniform scope is stated once, not on every progress line"
    );
    assert!(stdout.contains("completed: 1 deployment updated"));
    assert!(!stdout.contains('\u{1b}'), "plain output must be ANSI-free");
    assert_eq!(
        fs::read_to_string(&deployed).expect("accepted update deploys"),
        "# Alpha\nsecond line\n"
    );

    fs::write(alpha.join("SKILL.md"), "# Alpha\nthird line\n").expect("edit source skill again");
    let confirmed = cli(home.path())
        .args(["update", "--claude", "--global", "--yes"])
        .output()
        .expect("run preconfirmed update");
    assert!(confirmed.status.success());
    assert!(
        String::from_utf8(confirmed.stdout)
            .expect("utf8 update output")
            .contains("Update plan")
    );
    assert!(
        !String::from_utf8(confirmed.stderr)
            .expect("utf8 update diagnostics")
            .contains("Apply this update plan to"),
        "--yes must skip the confirmation while still printing the plan"
    );
    assert_eq!(
        fs::read_to_string(&deployed).expect("preconfirmed update deploys"),
        "# Alpha\nthird line\n"
    );

    fs::write(alpha.join("SKILL.md"), "# Alpha\nfourth line\n").expect("edit source once more");
    let machine = cli(home.path())
        .args(["--json", "update", "--claude", "--global"])
        .output()
        .expect("run machine update");
    let events = json_events(machine);
    assert_eq!(events_of(&events, "skill.updated").len(), 1);
    assert_eq!(events_of(&events, "skill.skipped").len(), 1);
    let summary = events_of(&events, "summary")[0]["data"].clone();
    assert_eq!(summary["action"], "update");
    assert_eq!(summary["changed"], 1);
    assert_eq!(summary["skipped"], 1);
    assert_eq!(
        fs::read_to_string(&deployed).expect("machine update deploys without prompting"),
        "# Alpha\nfourth line\n"
    );
}

#[test]
fn home_directory_is_global_only_across_scoped_commands_and_configs() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha\n");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--claude", "--global"])
        .assert()
        .success();
    let deployment = home.path().join(".claude/skills/alpha/SKILL.md");
    fs::write(&deployment, "# Agent edit\n").expect("edit global deployment");

    let import = cli(home.path())
        .args(["import", "alpha", "--claude", "--dry-run"])
        .output()
        .expect("inspect import at home");
    assert!(import.status.success());
    let import_stdout = String::from_utf8(import.stdout).expect("utf8 import output");
    assert!(import_stdout.contains("From  claude · global"));
    assert!(!import_stdout.contains("project"));

    let status = json_events(
        cli(home.path())
            .args(["--json", "status", "alpha", "--claude"])
            .output()
            .expect("status at home"),
    );
    let deployments = events_of(&status, "status.row")[0]["data"]["deployments"]
        .as_array()
        .expect("deployment list");
    assert_eq!(deployments.len(), 1);
    assert_eq!(deployments[0]["scope"], "global");

    let remove = json_events(
        cli(home.path())
            .args(["--json", "remove", "alpha", "--claude", "--dry-run"])
            .output()
            .expect("remove dry-run at home"),
    );
    assert_eq!(events_of(&remove, "skill.removed").len(), 1);
    assert_eq!(
        events_of(&remove, "skill.removed")[0]["data"]["scope"],
        "global"
    );

    let update = json_events(
        cli(home.path())
            .args(["--json", "update", "--filter", "alpha", "--claude"])
            .output()
            .expect("update at home"),
    );
    assert_eq!(events_of(&update, "skill.updated").len(), 1);
    assert_eq!(
        events_of(&update, "skill.updated")[0]["data"]["scope"],
        "global"
    );

    let configs = cli(home.path())
        .arg("configs")
        .output()
        .expect("show configs at home");
    assert!(configs.status.success());
    let configs_stdout = String::from_utf8(configs.stdout).expect("utf8 configs output");
    assert!(
        configs_stdout.contains("Project       unavailable — current directory is the global home")
    );
    assert!(!configs_stdout.contains("project directory"));

    for args in [
        vec!["load", "--claude", "--project", "--dry-run"],
        vec!["update", "alpha", "--claude", "--project", "--yes"],
        vec!["import", "alpha", "--claude", "--project", "--dry-run"],
        vec!["remove", "alpha", "--claude", "--project", "--dry-run"],
        vec!["status", "alpha", "--claude", "--project"],
    ] {
        cli(home.path())
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "project scope is unavailable because the current directory is your global home",
            ))
            .stderr(predicate::str::contains("use --global"));
    }
    assert!(
        deployment.is_file(),
        "rejected project commands must not remove global data"
    );
}

#[test]
fn symlinked_home_spelling_is_still_global_only_when_supported() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha\n");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--claude", "--global"])
        .assert()
        .success();

    let alias = home.path().join("home-alias");
    if !try_directory_symlink(home.path(), &alias) {
        return;
    }

    let mut status = cli(home.path());
    status
        .env("SKILL_MANAGER_HOME", &alias)
        .env("HOME", &alias)
        .env("USERPROFILE", &alias)
        .args(["--json", "status", "alpha", "--claude"]);
    let events = json_events(status.output().expect("status through aliased home"));
    let deployments = events_of(&events, "status.row")[0]["data"]["deployments"]
        .as_array()
        .expect("deployment list");
    assert_eq!(deployments.len(), 1);
    assert_eq!(deployments[0]["scope"], "global");

    fs::write(
        home.path().join(".claude/skills/alpha/SKILL.md"),
        "# Edited\n",
    )
    .expect("edit deployment");
    let mut import = cli(home.path());
    import
        .env("SKILL_MANAGER_HOME", &alias)
        .env("HOME", &alias)
        .env("USERPROFILE", &alias)
        .args(["import", "alpha", "--claude", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("claude · global"))
        .stdout(predicate::str::contains("project").not());

    let mut rejected = cli(home.path());
    rejected
        .env("SKILL_MANAGER_HOME", &alias)
        .env("HOME", &alias)
        .env("USERPROFILE", &alias)
        .args(["status", "alpha", "--claude", "--project"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "project scope is unavailable because the current directory is your global home",
        ));
}

#[test]
fn grouped_update_plan_uses_target_columns_both_scopes_and_up_alias() {
    let home = sandbox();
    let project = home.path().join("project");
    let source = home.path().join("source");
    fs::create_dir_all(&project).expect("create project");
    let source_skill = create_skill(&source, "alpha", "# Original\n");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--claude", "--shared", "--global"])
        .assert()
        .success();
    let mut project_load = cli(home.path());
    project_load.current_dir(&project);
    project_load
        .args(["--json", "load", "--claude", "--shared", "--project"])
        .assert()
        .success();
    fs::write(source_skill.join("SKILL.md"), "# Changed\n").expect("change source");

    let mut update = cli(home.path());
    update.current_dir(&project);
    let output = update
        .args(["update", "--yes"])
        .output()
        .expect("grouped update");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 update output");
    let stderr = String::from_utf8(output.stderr).expect("utf8 update diagnostics");
    assert!(stdout.contains("skill  change"));
    assert!(stdout.contains("claude"));
    assert!(stdout.contains("shared"));
    assert!(
        !stdout.contains("antigravity"),
        "a target with no deployment contributes no information: {stdout}"
    );
    assert_eq!(stdout.matches("update both").count(), 2);
    assert!(stdout.contains("4 updates across 2 targets"));
    assert!(!stderr.contains("Use all"));

    fs::write(source_skill.join("SKILL.md"), "# Alias\n").expect("change source again");
    let mut alias = cli(home.path());
    alias.current_dir(&project);
    alias
        .args(["up", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("4 updates across 2 targets"));

    let mut no_op = cli(home.path());
    no_op.current_dir(&project);
    no_op
        .arg("up")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "alpha is up to date across 3 enabled targets.",
        ))
        .stdout(predicate::str::contains("Update plan").not())
        .stderr(predicate::str::contains("Apply this update plan").not());
}

#[test]
fn filtered_exact_and_pattern_updates_report_successful_no_work() {
    let home = update_fixture_named_alpha();
    cli(home.path())
        .args(["update", "alpha", "--filter", "beta"])
        .assert()
        .success()
        .stdout(predicate::eq("No installed skills matched this update.\n"))
        .stderr(predicate::str::is_empty());
    cli(home.path())
        .args(["update", "alpha*", "--filter", "beta"])
        .assert()
        .success()
        .stdout(predicate::eq("No installed skills matched this update.\n"))
        .stderr(predicate::str::is_empty());
    let filtered = json_events(
        cli(home.path())
            .args(["--json", "update", "alpha*", "--filter", "beta"])
            .output()
            .expect("machine filtered update"),
    );
    assert!(events_of(&filtered, "command.failed").is_empty());
    let filtered_summary = events_of(&filtered, "summary")[0];
    assert_eq!(filtered_summary["data"]["changed"], 0);
    assert_eq!(filtered_summary["data"]["skipped"], 0);

    let recipe_filtered = json_events(
        cli(home.path())
            .arg("--json-input")
            .write_stdin(
                serde_json::json!({
                    "command": "update",
                    "source": "alpha",
                    "filter": ["beta"]
                })
                .to_string(),
            )
            .output()
            .expect("recipe filtered update"),
    );
    assert!(events_of(&recipe_filtered, "command.failed").is_empty());
    let recipe_summary = events_of(&recipe_filtered, "summary")[0];
    assert_eq!(recipe_summary["data"]["changed"], 0);
    assert_eq!(recipe_summary["data"]["skipped"], 0);
}

#[test]
fn exact_source_update_without_enabled_targets_is_successful_no_work() {
    let home = update_fixture_named_alpha();
    for target in ["claude", "shared", "antigravity"] {
        cli(home.path())
            .args(["--json", "target", "disable", target])
            .assert()
            .success();
    }
    cli(home.path())
        .args(["update", "alpha"])
        .assert()
        .success()
        .stdout(predicate::eq(
            "No enabled targets are available for update.\n",
        ))
        .stderr(predicate::str::is_empty());
    let no_targets = json_events(
        cli(home.path())
            .args(["--json", "update", "alpha"])
            .output()
            .expect("machine update without enabled targets"),
    );
    assert!(events_of(&no_targets, "command.failed").is_empty());
    let no_target_summary = events_of(&no_targets, "summary")[0];
    assert_eq!(no_target_summary["data"]["changed"], 0);
    assert_eq!(no_target_summary["data"]["skipped"], 0);
    let recipe_no_targets = json_events(
        cli(home.path())
            .arg("--json-input")
            .write_stdin(serde_json::json!({ "command": "update", "source": "alpha" }).to_string())
            .output()
            .expect("recipe update without enabled targets"),
    );
    assert!(events_of(&recipe_no_targets, "command.failed").is_empty());
    let recipe_no_target_summary = events_of(&recipe_no_targets, "summary")[0];
    assert_eq!(recipe_no_target_summary["data"]["changed"], 0);
    assert_eq!(recipe_no_target_summary["data"]["skipped"], 0);
}

#[test]
fn genuinely_unmatched_update_positionals_remain_not_found() {
    let home = update_fixture_named_alpha();
    cli(home.path())
        .args(["update", "missing-*", "--yes"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("No installed skills matched").not())
        .stderr(predicate::str::contains(
            "Error: skill matching positional pattern not found: missing-*",
        ));
    cli(home.path())
        .args(["--json", "update", "missing-*"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"event\":\"diagnostic\""))
        .stdout(predicate::str::contains("\"pattern\":\"missing-*\""))
        .stdout(predicate::str::contains("\"event\":\"command.failed\""))
        .stdout(predicate::str::contains("\"event\":\"summary\"").not())
        .stderr(predicate::str::is_empty());
    cli(home.path())
        .arg("--json-input")
        .write_stdin(serde_json::json!({ "command": "update", "source": "missing-*" }).to_string())
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"event\":\"diagnostic\""))
        .stdout(predicate::str::contains("\"event\":\"command.failed\""))
        .stdout(predicate::str::contains("\"event\":\"summary\"").not())
        .stderr(predicate::str::is_empty());
}

#[test]
fn grouped_update_plan_preserves_each_divergent_target_delta() {
    let home = sandbox();
    let source = home.path().join("source");
    let source_skill = create_skill(&source, "alpha", "# Old\n");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--claude", "--shared", "--global"])
        .assert()
        .success();
    fs::write(source_skill.join("SKILL.md"), "# New\nline\n").expect("change source");
    fs::write(home.path().join(".agents/skills/alpha/stale.md"), "stale\n")
        .expect("make shared delta distinct");

    cli(home.path())
        .args(["update", "--claude", "--shared", "--global", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 destination-specific changes"))
        .stdout(predicate::str::contains("claude  1 file changed, +2/-1"))
        .stdout(predicate::str::contains("shared  2 files changed, +2/-2"))
        .stdout(predicate::str::contains("Dry run — no changes were made."));
}

#[test]
fn explicit_disabled_update_uses_selected_target_wording() {
    let home = sandbox();
    let source = home.path().join("source");
    let source_skill = create_skill(&source, "alpha", "# Old\n");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "add", "offline", ".offline/skills"])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--target", "offline", "--global"])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "disable", "offline"])
        .assert()
        .success();
    fs::write(source_skill.join("SKILL.md"), "# New\n").expect("change source");

    cli(home.path())
        .args(["update", "--target", "offline", "--global", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "1 update across 1 selected target",
        ))
        .stdout(predicate::str::contains("enabled target").not());
}

#[test]
fn update_sections_have_one_blank_line_before_results_in_every_confirmation_mode() {
    let home = sandbox();
    let source = home.path().join("source");
    let source_skill = create_skill(&source, "alpha", "# Zero\n");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--claude", "--global"])
        .assert()
        .success();

    for (version, extra, input) in [
        ("One", Vec::<&str>::new(), "y\n"),
        ("Two", vec!["--yes"], ""),
        ("Three", vec!["--dry-run"], ""),
    ] {
        fs::write(source_skill.join("SKILL.md"), format!("# {version}\n")).expect("change source");
        let dry_run = extra.contains(&"--dry-run");
        let mut args = vec!["update", "--claude", "--global"];
        args.extend(extra);
        let output = cli(home.path())
            .args(args)
            .write_stdin(input)
            .output()
            .expect("run update spacing case");
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout)
            .expect("utf8 update output")
            .replace("\r\n", "\n");
        assert!(
            stdout.starts_with("Update plan\n\n"),
            "direct update must begin with its plan heading:\n{stdout}"
        );
        let tail = if dry_run {
            "1 update across 1 selected target\n\nDry run — no changes were made."
        } else {
            "1 update across 1 selected target\n\nUpdated alpha"
        };
        assert!(
            stdout.contains(tail),
            "unexpected update section spacing:\n{stdout}"
        );
        assert!(
            !stdout.contains("\n\n\n"),
            "duplicate blank line:\n{stdout}"
        );
        assert!(
            !stdout.contains("(dry-run)"),
            "a dry run concludes once instead of echoing every item:\n{stdout}"
        );
        if dry_run {
            cli(home.path())
                .args(["--json", "update", "--claude", "--global"])
                .assert()
                .success();
        }
    }
}

/// Two skills deployed to two of three enabled targets in global scope.
///
/// Antigravity stays enabled but empty, so every plan built from this fixture
/// exercises significance gating rather than target selection.
fn update_review_fixture() -> (TempDir, PathBuf) {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "writing-for-agents", "# Writing\n");
    create_skill(&source, "drafting-commit-message", "# Drafting\n");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--claude", "--shared", "--global"])
        .assert()
        .success();
    (home, source)
}

fn change_skill(source: &Path, name: &str, body: &str) {
    fs::write(source.join(name).join("SKILL.md"), body).expect("change source skill");
}

fn stdout_of(output: std::process::Output) -> String {
    String::from_utf8(output.stdout)
        .expect("utf8 stdout")
        .replace("\r\n", "\n")
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stderr.clone())
        .expect("utf8 stderr")
        .replace("\r\n", "\n")
}

/// The approved invocation, whose positional order the plan must preserve.
const UPDATE_REVIEW_ARGS: [&str; 3] = ["update", "writing-for-agents", "drafting-commit-message"];

const UPDATE_REVIEW_PLAN: &str = "\
Update plan

Scope  global (inferred)

skill                    change                 claude  shared
-----------------------  ---------------------  ------  ------
writing-for-agents       1 file changed, +1/-0  update  update
drafting-commit-message  1 file changed, +1/-0  update  update

4 updates across 2 targets
";

/// The whole plan is rendered before the single confirmation, and declining it
/// says exactly which decisions were inferred.
#[test]
fn update_renders_its_whole_plan_before_one_confirmation_and_cancels_specifically() {
    let (home, source) = update_review_fixture();
    change_skill(&source, "writing-for-agents", "# Writing\nmore\n");
    change_skill(&source, "drafting-commit-message", "# Drafting\nmore\n");

    let declined = cli(home.path())
        .args(UPDATE_REVIEW_ARGS)
        .write_stdin("n\n")
        .output()
        .expect("run declined update");
    assert!(declined.status.success(), "cancelling is not a failure");
    assert_eq!(
        stdout_of(declined.clone()),
        format!(
            "{UPDATE_REVIEW_PLAN}Cancelled.\n\
Hint: targets and deployed scopes were inferred. Re-run with --claude, --shared, --antigravity, --all, or --target NAME, and --global or --project, to narrow this plan.\n"
        )
    );
    assert_eq!(
        stderr_of(&declined),
        "Apply this update plan to 2 targets? [Y/n] "
    );
    assert_eq!(
        fs::read_to_string(
            home.path()
                .join(".claude/skills/writing-for-agents/SKILL.md")
        )
        .expect("declined update writes nothing"),
        "# Writing\n"
    );
}

/// A dry run reviews the plan and concludes once instead of echoing every item.
#[test]
fn update_dry_run_renders_the_plan_and_concludes_once() {
    let (home, source) = update_review_fixture();
    change_skill(&source, "writing-for-agents", "# Writing\nmore\n");
    change_skill(&source, "drafting-commit-message", "# Drafting\nmore\n");

    let output = cli(home.path())
        .args([UPDATE_REVIEW_ARGS.as_slice(), ["--dry-run"].as_slice()].concat())
        .output()
        .expect("run dry-run update");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output),
        format!("{UPDATE_REVIEW_PLAN}\nDry run — no changes were made.\n")
    );
    assert_eq!(
        fs::read_to_string(
            home.path()
                .join(".claude/skills/writing-for-agents/SKILL.md")
        )
        .expect("dry run writes nothing"),
        "# Writing\n"
    );
}

/// `--yes` still renders the plan, then applies it with a styled summary footer.
#[test]
fn update_yes_renders_the_plan_then_applies_with_a_summary_footer() {
    let (home, source) = update_review_fixture();
    change_skill(&source, "writing-for-agents", "# Writing\nmore\n");
    change_skill(&source, "drafting-commit-message", "# Drafting\nmore\n");

    let output = cli(home.path())
        .args([UPDATE_REVIEW_ARGS.as_slice(), ["--yes"].as_slice()].concat())
        .output()
        .expect("run preconfirmed update");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output.clone()),
        format!(
            "{UPDATE_REVIEW_PLAN}\n\
Updated writing-for-agents -> claude\n\
Updated writing-for-agents -> shared\n\
Updated drafting-commit-message -> claude\n\
Updated drafting-commit-message -> shared\n\
\n\
completed: 4 deployments updated\n"
        )
    );
    assert!(
        !stderr_of(&output).contains("Apply this update plan"),
        "--yes authorizes the plan without asking again"
    );
}

/// A target with nothing to do is dropped from the plan, not printed empty.
#[test]
fn update_drops_a_target_column_whose_every_cell_is_the_none_value() {
    let (home, source) = update_review_fixture();
    change_skill(&source, "writing-for-agents", "# Writing\nmore\n");
    change_skill(&source, "drafting-commit-message", "# Drafting\nmore\n");
    let targets = cli(home.path())
        .args(["--json", "target", "list"])
        .output()
        .expect("list targets");
    let listed = json_events(targets);
    let antigravity = events_of(&listed, "target.listed")
        .into_iter()
        .find(|event| event["data"]["name"] == "antigravity")
        .expect("antigravity is a configured target");
    assert_eq!(
        antigravity["data"]["enabled"], true,
        "the dropped column must be an enabled target, not an unselected one"
    );

    let stdout = stdout_of(
        cli(home.path())
            .args(["update", "--dry-run"])
            .output()
            .expect("run gated update"),
    );
    assert!(
        !stdout.contains("antigravity"),
        "an all-none column carries no information:\n{stdout}"
    );
    assert!(
        stdout.contains("4 updates across 2 targets"),
        "a degraded destination phrase must not claim every enabled target:\n{stdout}"
    );
}

/// A zero result category is omitted entirely rather than reported as zero.
#[test]
fn update_result_footer_omits_zero_categories() {
    let (home, source) = update_review_fixture();
    change_skill(&source, "writing-for-agents", "# Writing\nmore\n");

    let partial = stdout_of(
        cli(home.path())
            .args(["update", "--yes"])
            .output()
            .expect("run partial update"),
    );
    assert!(
        partial.ends_with("completed: 2 deployments updated, unchanged: 2\n"),
        "both nonzero categories must survive:\n{partial}"
    );

    change_skill(&source, "writing-for-agents", "# Writing\nmore\nstill\n");
    change_skill(&source, "drafting-commit-message", "# Drafting\nmore\n");
    let complete = stdout_of(
        cli(home.path())
            .args(["update", "--yes"])
            .output()
            .expect("run complete update"),
    );
    assert!(
        complete.ends_with("completed: 4 deployments updated\n"),
        "an empty unchanged category must vanish, not print as zero:\n{complete}"
    );
}

/// An explicitly stated scope is never repeated back as an inferred default.
#[test]
fn update_omits_the_scope_line_when_the_scope_was_stated() {
    let (home, source) = update_review_fixture();
    change_skill(&source, "writing-for-agents", "# Writing\nmore\n");

    let stdout = stdout_of(
        cli(home.path())
            .args(["update", "--global", "--dry-run"])
            .output()
            .expect("run explicitly scoped update"),
    );
    assert!(
        !stdout.contains("Scope"),
        "a stated scope adds nothing when repeated:\n{stdout}"
    );
    assert_eq!(
        stdout,
        "\
Update plan

skill               change                 claude  shared
------------------  ---------------------  ------  ------
writing-for-agents  1 file changed, +1/-0  update  update

2 updates across 2 targets

Dry run — no changes were made.
"
    );
}

/// The plan is reviewed and applied in the order the user named the skills.
#[test]
fn update_reviews_and_applies_skills_in_the_order_they_were_requested() {
    let (home, source) = update_review_fixture();
    change_skill(&source, "writing-for-agents", "# Writing\nmore\n");
    change_skill(&source, "drafting-commit-message", "# Drafting\nmore\n");

    let reversed = stdout_of(
        cli(home.path())
            .args([
                "update",
                "drafting-commit-message",
                "writing-for-agents",
                "--yes",
            ])
            .output()
            .expect("run reversed update"),
    );
    let rows = reversed
        .lines()
        .filter(|line| line.contains("1 file changed"))
        .map(|line| line.split_whitespace().next().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        ["drafting-commit-message", "writing-for-agents"],
        "reviewing in request order is not alphabetical order:\n{reversed}"
    );
    let applied = reversed
        .lines()
        .filter(|line| line.starts_with("Updated "))
        .collect::<Vec<_>>();
    assert_eq!(
        applied,
        [
            "Updated drafting-commit-message -> claude",
            "Updated drafting-commit-message -> shared",
            "Updated writing-for-agents -> claude",
            "Updated writing-for-agents -> shared",
        ],
        "apply must honour the order the plan promised:\n{reversed}"
    );
}

/// With no positional names the sequence comes from discovery, not the
/// alphabet, and apply must still follow exactly what the plan rendered.
#[test]
fn update_reviews_and_applies_in_discovery_order_when_nothing_was_named() {
    let home = sandbox();
    let later = home.path().join("later");
    create_skill(&later, "zebra-skill", "# Zebra\n");
    let earlier = home.path().join("earlier");
    create_skill(&earlier, "alpha-skill", "# Alpha\n");
    for (path, name) in [(&later, "later"), (&earlier, "earlier")] {
        cli(home.path())
            .args([
                "--json",
                "source",
                "add",
                path.to_str().expect("utf8 source"),
                name,
            ])
            .assert()
            .success();
    }
    cli(home.path())
        .args(["--json", "load", "--claude", "--global"])
        .assert()
        .success();
    change_skill(&later, "zebra-skill", "# Zebra\nmore\n");
    change_skill(&earlier, "alpha-skill", "# Alpha\nmore\n");

    let stdout = stdout_of(
        cli(home.path())
            .args(["update", "--yes"])
            .output()
            .expect("run unnamed update"),
    );
    let rows = stdout
        .lines()
        .filter(|line| line.contains("1 file changed"))
        .map(|line| line.split_whitespace().next().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        ["zebra-skill", "alpha-skill"],
        "the fallback sequence is discovery order, not alphabetical:\n{stdout}"
    );
    let applied = stdout
        .lines()
        .filter(|line| line.starts_with("Updated "))
        .collect::<Vec<_>>();
    assert_eq!(
        applied,
        [
            "Updated zebra-skill -> claude",
            "Updated alpha-skill -> claude",
        ],
        "apply must honour the order the plan promised:\n{stdout}"
    );
}

/// A terminal user reviews the same plan in the symbol vocabulary, in color.
#[test]
fn update_renders_the_interactive_symbol_and_color_plan_for_a_terminal_user() {
    let (home, source) = update_review_fixture();
    change_skill(&source, "writing-for-agents", "# Writing\nmore\n");
    change_skill(&source, "drafting-commit-message", "# Drafting\nmore\n");

    let stdout = stdout_of(
        cli(home.path())
            .env_remove("NO_COLOR")
            .env("SKILL_MANAGER_FORCE_INTERACTIVE", "1")
            .args(
                [
                    UPDATE_REVIEW_ARGS.as_slice(),
                    ["--color", "always", "--dry-run"].as_slice(),
                ]
                .concat(),
            )
            .output()
            .expect("run interactive dry-run update"),
    );
    assert_eq!(
        stdout,
        "\u{1b}[1;36mUpdate plan\u{1b}[0m\n\
\n\
Scope  🌐 global (inferred)\n\
\n\
skill                    change                 claude  shared\n\
-----------------------  ---------------------  ------  ------\n\
writing-for-agents       1 file changed, +1/-0  \u{1b}[33m↑\u{1b}[0m       \u{1b}[33m↑\u{1b}[0m\n\
drafting-commit-message  1 file changed, +1/-0  \u{1b}[33m↑\u{1b}[0m       \u{1b}[33m↑\u{1b}[0m\n\
\n\
4 updates across 2 targets\n\
\n\
Dry run — no changes were made.\n"
    );
}

/// A skill that exists but is deployed nowhere reports precisely that.
#[test]
fn update_names_a_skill_that_is_deployed_nowhere() {
    let (home, source) = update_review_fixture();
    create_skill(&source, "wait-what", "# Wait\n");

    let output = cli(home.path())
        .args(["update", "wait-what"])
        .output()
        .expect("run undeployed update");
    assert!(
        output.status.success(),
        "an accurate no-op is not a failure"
    );
    assert_eq!(
        stdout_of(output),
        "wait-what is not deployed to any enabled target in global or project scope.\n"
    );
}

/// A noninteractive human-facing run must authorize its plan explicitly.
#[test]
fn update_requires_explicit_authorization_without_input() {
    let (home, source) = update_review_fixture();
    change_skill(&source, "writing-for-agents", "# Writing\nmore\n");

    let output = cli(home.path())
        .args(["update", "--no-input"])
        .output()
        .expect("run noninteractive update");
    assert!(!output.status.success());
    assert!(stdout_of(output.clone()).contains("2 updates across 2 targets"));
    assert!(
        stderr_of(&output).contains("applying this plan noninteractively requires --yes."),
        "{}",
        stderr_of(&output)
    );

    cli(home.path())
        .args(["update", "--no-input", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("completed: 2 deployments updated"));
}

/// The structured plan event mirrors the rendered plan before anything applies.
#[test]
fn update_emits_a_structured_plan_event_before_applying() {
    let (home, source) = update_review_fixture();
    change_skill(&source, "writing-for-agents", "# Writing\nmore\n");

    let events = json_events(
        cli(home.path())
            .args(["--json", "update"])
            .output()
            .expect("run machine update"),
    );
    let plans = events_of(&events, "plan");
    assert_eq!(plans.len(), 1, "one revision was reviewed");
    let data = plans[0]["data"].clone();
    assert_eq!(data["plan_id"], "update:writing-for-agents");
    assert_eq!(data["revision"], 0);
    assert_eq!(data["command"], "update");
    assert_eq!(data["dry_run"], false);
    assert_eq!(data["authorization"]["kind"], "binary");
    assert_eq!(data["authorization"]["mode"], "noninteractive");
    assert_eq!(data["selection"]["targets"]["mode"], "inferred");
    assert_eq!(
        data["selection"]["targets"]["names"],
        serde_json::json!(["claude", "shared", "antigravity"]),
        "the machine stream reports the resolved selection, never the gated columns"
    );
    assert_eq!(
        data["selection"]["scope"],
        serde_json::json!({ "mode": "inferred", "value": "global" })
    );
    assert_eq!(
        data["destinations"],
        serde_json::json!([
            {
                "id": "claude:global",
                "kind": "deployment",
                "label": "claude · global",
                "target": "claude",
                "scope": "global"
            },
            {
                "id": "shared:global",
                "kind": "deployment",
                "label": "shared · global",
                "target": "shared",
                "scope": "global"
            }
        ])
    );
    assert_eq!(data["entries"][0]["skill"], "writing-for-agents");
    assert_eq!(data["entries"][0]["actions"][0]["operation"], "update");
    assert_eq!(
        data["entries"][0]["actions"][0]["destination"],
        "claude:global"
    );
    assert_eq!(data["entries"][0]["actions"][0]["existed"], true);
    assert_eq!(
        data["summary"],
        serde_json::json!({ "skills": 1, "actions": 2, "update": 2 })
    );
    let plan_index = events
        .iter()
        .position(|event| event["event"] == "plan")
        .expect("plan event position");
    let first_write = events
        .iter()
        .position(|event| event["event"] == "skill.updated")
        .expect("applied event position");
    assert!(plan_index < first_write, "the plan precedes every write");
}

/// Two skills discovered from one source, nothing deployed yet.
///
/// `load`'s plan-then-confirm tests start from a genuinely empty install so
/// every deployment in the plan is new, unless a test explicitly pre-installs
/// something to exercise the overwrite/identical paths.
fn load_review_fixture() -> (TempDir, PathBuf) {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "writing-for-agents", "# Writing\n");
    create_skill(&source, "drafting-commit-message", "# Drafting\n");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    (home, source)
}

/// The approved invocation, whose positional order the plan must preserve.
const LOAD_REVIEW_ARGS: [&str; 3] = ["load", "writing-for-agents", "drafting-commit-message"];

/// Both skills are brand new everywhere, so target and scope are both
/// inferred (three enabled targets, global scope from an empty home).
const LOAD_REVIEW_PLAN: &str = "\
Load plan

Scope  global (inferred)

skill                    change                         claude  shared  antigravity
-----------------------  -----------------------------  ------  ------  -----------
writing-for-agents       new deployment, 1 file, +1/-0  load    load    load
drafting-commit-message  new deployment, 1 file, +1/-0  load    load    load

6 changes across 3 enabled targets: 6 new
";

/// The whole plan is rendered before the single confirmation, and declining
/// it says exactly which decisions were inferred.
#[test]
fn load_renders_its_whole_plan_before_one_confirmation_and_cancels_with_a_flag_hint() {
    let (home, _source) = load_review_fixture();

    let declined = cli(home.path())
        .args(LOAD_REVIEW_ARGS)
        .write_stdin("n\n")
        .output()
        .expect("run declined load");
    assert!(declined.status.success(), "cancelling is not a failure");
    assert_eq!(
        stdout_of(declined.clone()),
        format!(
            "{LOAD_REVIEW_PLAN}Cancelled.\n\
Hint: targets and scope were inferred. Re-run with --claude, --shared, --antigravity, --all, or --target NAME, and --global or --project, to change this plan.\n"
        )
    );
    assert_eq!(
        stderr_of(&declined),
        "Apply this load plan to 3 enabled targets? [Y/n] "
    );
    assert!(
        !home
            .path()
            .join(".claude/skills/writing-for-agents")
            .exists(),
        "declining a load plan writes nothing"
    );
}

/// When target and scope are both stated explicitly, nothing was inferred,
/// so cancelling teaches nothing — the hint line is entirely absent.
#[test]
fn load_cancel_omits_the_hint_when_target_and_scope_are_explicit() {
    let (home, _source) = load_review_fixture();

    let declined = cli(home.path())
        .args([
            "load",
            "writing-for-agents",
            "drafting-commit-message",
            "--claude",
            "--shared",
            "--global",
        ])
        .write_stdin("n\n")
        .output()
        .expect("run declined explicit load");
    assert!(declined.status.success());
    assert_eq!(
        stdout_of(declined.clone()),
        "\
Load plan

skill                    change                         claude  shared
-----------------------  -----------------------------  ------  ------
writing-for-agents       new deployment, 1 file, +1/-0  load    load
drafting-commit-message  new deployment, 1 file, +1/-0  load    load

4 changes across 2 selected targets: 4 new
Cancelled.\n"
    );
    assert_eq!(
        stderr_of(&declined),
        "Apply this load plan to 2 selected targets? [Y/n] "
    );
}

/// A dry run reviews the plan and concludes once instead of echoing every item.
#[test]
fn load_dry_run_renders_the_plan_and_concludes_once() {
    let (home, _source) = load_review_fixture();

    let output = cli(home.path())
        .args([LOAD_REVIEW_ARGS.as_slice(), ["--dry-run"].as_slice()].concat())
        .output()
        .expect("run dry-run load");
    assert!(output.status.success());
    let stdout = stdout_of(output);
    assert_eq!(
        stdout,
        format!("{LOAD_REVIEW_PLAN}\nDry run — no changes were made.\n")
    );
    assert!(
        !stdout.contains("(dry-run)"),
        "a dry run concludes once instead of echoing every item:\n{stdout}"
    );
    assert!(
        !home.path().join(".claude/skills").exists(),
        "dry run writes nothing"
    );
}

/// `--yes` still renders the plan, then applies it with a styled summary footer.
#[test]
fn load_yes_renders_the_plan_then_applies_with_a_summary_footer() {
    let (home, _source) = load_review_fixture();

    let output = cli(home.path())
        .args([LOAD_REVIEW_ARGS.as_slice(), ["--yes"].as_slice()].concat())
        .output()
        .expect("run preconfirmed load");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output.clone()),
        format!(
            "{LOAD_REVIEW_PLAN}\n\
Loaded writing-for-agents -> claude\n\
Loaded writing-for-agents -> shared\n\
Loaded writing-for-agents -> antigravity\n\
Loaded drafting-commit-message -> claude\n\
Loaded drafting-commit-message -> shared\n\
Loaded drafting-commit-message -> antigravity\n\
\n\
completed: 6 deployments changed (6 loaded)\n"
        )
    );
    assert!(!stderr_of(&output).contains("Apply this load plan"));
    assert_eq!(
        fs::read_to_string(
            home.path()
                .join(".claude/skills/writing-for-agents/SKILL.md")
        )
        .expect("loaded skill exists"),
        "# Writing\n"
    );
}

/// A noninteractive human-facing run must authorize its plan explicitly.
#[test]
fn load_requires_explicit_authorization_without_input() {
    let (home, _source) = load_review_fixture();

    let output = cli(home.path())
        .args(
            [
                LOAD_REVIEW_ARGS.as_slice(),
                ["--claude", "--shared", "--global", "--no-input"].as_slice(),
            ]
            .concat(),
        )
        .output()
        .expect("run noninteractive load without --yes");
    assert!(!output.status.success());
    assert!(
        stderr_of(&output).contains("applying this plan noninteractively requires --yes."),
        "{}",
        stderr_of(&output)
    );
    assert!(!home.path().join(".claude/skills").exists());
}

/// The plan distinguishes new installs from overwrites, breaking a
/// multi-destination skill's row out into a per-destination explanation.
#[test]
fn load_distinguishes_new_installs_from_overwrites() {
    let (home, source) = load_review_fixture();
    cli(home.path())
        .args([
            "--json",
            "load",
            "writing-for-agents",
            "--claude",
            "--global",
        ])
        .assert()
        .success();
    change_skill(&source, "writing-for-agents", "# Writing\nmore\n");

    let stdout = stdout_of(
        cli(home.path())
            .args(["load", "--claude", "--shared", "--global", "--dry-run"])
            .output()
            .expect("run new-vs-overwrite load"),
    );
    assert_eq!(
        stdout,
        "\
Load plan

skill                    change                          claude  shared
-----------------------  ------------------------------  ------  ------
drafting-commit-message  new deployment, 1 file, +1/-0   load    load
writing-for-agents       2 destination-specific changes  update  load

Destination-specific changes
  writing-for-agents
    claude  1 file changed, +1/-0
    shared  new deployment, 1 file, +2/-0

4 changes across 2 selected targets: 3 new, 1 overwrite

Dry run — no changes were made.\n"
    );
}

/// Once every requested skill is byte-identical everywhere selected, load
/// says so in a single sentence instead of rendering an empty table.
#[test]
fn load_hides_identical_deployments_and_reports_a_footer_count() {
    let (home, source) = load_review_fixture();
    cli(home.path())
        .args([
            "--json",
            "load",
            "writing-for-agents",
            "--claude",
            "--global",
        ])
        .assert()
        .success();
    change_skill(&source, "writing-for-agents", "# Writing\nmore\n");
    cli(home.path())
        .args(["load", "--claude", "--shared", "--global", "--yes"])
        .assert()
        .success();

    let stdout = stdout_of(
        cli(home.path())
            .args(["load", "--claude", "--shared", "--global"])
            .output()
            .expect("run fully-identical load"),
    );
    assert_eq!(
        stdout,
        "All requested skills are already identical across 2 selected targets.\n"
    );
}

/// A destination column is dropped only when every one of its cells is the
/// none value; the dropped destination's identical deployments still count
/// toward the footer.
#[test]
fn load_drops_a_target_column_whose_every_cell_is_the_none_value() {
    let (home, _source) = load_review_fixture();
    cli(home.path())
        .args([
            "--json",
            "load",
            "writing-for-agents",
            "drafting-commit-message",
            "--claude",
            "--global",
        ])
        .assert()
        .success();

    let stdout = stdout_of(
        cli(home.path())
            .args(["load", "--claude", "--shared", "--global", "--dry-run"])
            .output()
            .expect("run column-drop load"),
    );
    assert_eq!(
        stdout,
        "\
Load plan

skill                    change                         shared
-----------------------  -----------------------------  ------
drafting-commit-message  new deployment, 1 file, +1/-0  load
writing-for-agents       new deployment, 1 file, +1/-0  load

2 changes across 1 target: 2 new, 2 already identical

Dry run — no changes were made.\n"
    );
    assert!(
        !stdout.contains("claude"),
        "an all-none column must be dropped entirely, not merely hidden in spirit:\n{stdout}"
    );
}

/// A syntactically valid pattern matching nothing keeps the existing
/// `NotFound` contract: nonzero exit, and no plan is ever rendered.
#[test]
fn load_names_an_unmatched_glob_pattern_as_not_found() {
    let (home, _source) = load_review_fixture();

    let output = cli(home.path())
        .args(["load", "zzz-nomatch-*", "--claude", "--global", "--yes"])
        .output()
        .expect("run unmatched glob load");
    assert!(!output.status.success());
    assert_eq!(
        stderr_of(&output),
        "Warning: skill pattern matched nothing: zzz-nomatch-*\n\
Error: skill matching positional pattern not found: zzz-nomatch-*\n"
    );
    assert!(
        stdout_of(output).is_empty(),
        "no plan is rendered for a NotFound pattern"
    );
}

/// Plan order must equal apply order; reversing the requested names reverses
/// both the rendered rows and the applied progress lines identically.
#[test]
fn load_reviews_and_applies_skills_in_the_order_they_were_requested() {
    let (home, _source) = load_review_fixture();

    let stdout = stdout_of(
        cli(home.path())
            .args([
                "load",
                "drafting-commit-message",
                "writing-for-agents",
                "--claude",
                "--shared",
                "--global",
                "--yes",
            ])
            .output()
            .expect("run reversed load"),
    );
    let rows = stdout
        .lines()
        .filter(|line| line.contains("new deployment"))
        .map(|line| line.split_whitespace().next().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        ["drafting-commit-message", "writing-for-agents"],
        "the plan preserves the requested order:\n{stdout}"
    );
    let applied = stdout
        .lines()
        .filter(|line| line.starts_with("Loaded "))
        .collect::<Vec<_>>();
    assert_eq!(
        applied,
        [
            "Loaded drafting-commit-message -> claude",
            "Loaded drafting-commit-message -> shared",
            "Loaded writing-for-agents -> claude",
            "Loaded writing-for-agents -> shared",
        ],
        "apply must honour the order the plan promised:\n{stdout}"
    );
}

/// A terminal user reviews the same plan in the symbol vocabulary, in color.
#[test]
fn load_renders_the_interactive_symbol_and_color_plan_for_a_terminal_user() {
    let (home, _source) = load_review_fixture();

    let stdout = stdout_of(
        cli(home.path())
            .env_remove("NO_COLOR")
            .env("SKILL_MANAGER_FORCE_INTERACTIVE", "1")
            .args(
                [
                    LOAD_REVIEW_ARGS.as_slice(),
                    ["--claude", "--shared", "--color", "always", "--dry-run"].as_slice(),
                ]
                .concat(),
            )
            .output()
            .expect("run interactive dry-run load"),
    );
    assert_eq!(
        stdout,
        "\u{1b}[1;36mLoad plan\u{1b}[0m\n\
\n\
Scope  🌐 global (inferred)\n\
\n\
skill                    change                         claude  shared\n\
-----------------------  -----------------------------  ------  ------\n\
writing-for-agents       new deployment, 1 file, +1/-0  \u{1b}[32m+\u{1b}[0m       \u{1b}[32m+\u{1b}[0m\n\
drafting-commit-message  new deployment, 1 file, +1/-0  \u{1b}[32m+\u{1b}[0m       \u{1b}[32m+\u{1b}[0m\n\
\n\
4 changes across 2 selected targets: \u{1b}[32m+ 4 new\u{1b}[0m\n\
\n\
Dry run — no changes were made.\n"
    );
}

/// The NDJSON stream carries a single `plan` event at revision 0, ahead of
/// every write, with the resolved (never gated) selection and destinations.
#[test]
fn load_emits_a_structured_plan_event_before_applying() {
    let (home, _source) = load_review_fixture();

    let events = json_events(
        cli(home.path())
            .args([
                "--json",
                "load",
                "writing-for-agents",
                "--claude",
                "--shared",
            ])
            .output()
            .expect("run machine load"),
    );
    let plans = events_of(&events, "plan");
    assert_eq!(plans.len(), 1, "one revision was reviewed");
    let data = plans[0]["data"].clone();
    assert_eq!(data["plan_id"], "load:writing-for-agents");
    assert_eq!(data["revision"], 0);
    assert_eq!(data["command"], "load");
    assert_eq!(data["dry_run"], false);
    assert_eq!(data["authorization"]["kind"], "binary");
    assert_eq!(data["authorization"]["mode"], "noninteractive");
    assert_eq!(data["selection"]["targets"]["mode"], "explicit");
    assert_eq!(
        data["selection"]["targets"]["names"],
        serde_json::json!(["claude", "shared"])
    );
    assert_eq!(
        data["selection"]["scope"],
        serde_json::json!({ "mode": "inferred", "value": "global" }),
        "the machine stream reports the resolved selection, never gated columns"
    );
    let claude_destination = home.path().join(".claude").join("skills");
    let shared_destination = home.path().join(".agents").join("skills");
    assert_eq!(
        data["destinations"],
        serde_json::json!([
            {
                "id": "claude:global",
                "kind": "deployment",
                "label": "claude · global",
                "target": "claude",
                "scope": "global",
                "path": claude_destination.display().to_string()
            },
            {
                "id": "shared:global",
                "kind": "deployment",
                "label": "shared · global",
                "target": "shared",
                "scope": "global",
                "path": shared_destination.display().to_string()
            }
        ])
    );
    assert_eq!(data["entries"][0]["skill"], "writing-for-agents");
    assert_eq!(data["entries"][0]["source"], "Primary");
    assert_eq!(data["entries"][0]["actions"][0]["operation"], "load");
    assert_eq!(
        data["entries"][0]["actions"][0]["destination"],
        "claude:global"
    );
    assert_eq!(data["entries"][0]["actions"][0]["existed"], false);
    assert_eq!(
        data["summary"],
        serde_json::json!({ "skills": 1, "actions": 2, "new": 2 }),
        "load's summary buckets by the plan's own new/overwrite categories, \
         not the generic load/update action word"
    );
    let plan_index = events
        .iter()
        .position(|event| event["event"] == "plan")
        .expect("plan event position");
    let first_write = events
        .iter()
        .position(|event| event["event"] == "skill.loaded")
        .expect("applied event position");
    assert!(plan_index < first_write, "the plan precedes every write");
}

/// A row hidden from the human table for being fully identical everywhere
/// must still appear, complete, in the machine `entries`/`summary`: gating
/// is a property of rendering only, never of the structured stream.
#[test]
fn load_keeps_a_fully_hidden_identical_row_complete_in_the_json_stream() {
    let (home, _source) = load_review_fixture();
    cli(home.path())
        .args([
            "--json",
            "load",
            "writing-for-agents",
            "--claude",
            "--shared",
            "--global",
        ])
        .assert()
        .success();

    // The same row is entirely absent from the human-facing table; only its
    // footer count survives ("2 already identical"). Use --dry-run so this
    // observation and the machine check below share identical prior state.
    let stdout = stdout_of(
        cli(home.path())
            .args([
                "load",
                "writing-for-agents",
                "drafting-commit-message",
                "--claude",
                "--shared",
                "--global",
                "--dry-run",
            ])
            .output()
            .expect("run mixed identical-and-new human load"),
    );
    assert_eq!(
        stdout,
        "\
Load plan

skill                    change                         claude  shared
-----------------------  -----------------------------  ------  ------
drafting-commit-message  new deployment, 1 file, +1/-0  load    load

2 changes across 2 selected targets: 2 new, 2 already identical

Dry run — no changes were made.\n"
    );
    assert!(
        !stdout.contains("writing-for-agents"),
        "the fully-identical row never earns a table row, only a footer count:\n{stdout}"
    );

    let events = json_events(
        cli(home.path())
            .args([
                "--json",
                "load",
                "writing-for-agents",
                "drafting-commit-message",
                "--claude",
                "--shared",
                "--global",
                "--dry-run",
            ])
            .output()
            .expect("run mixed identical-and-new load"),
    );
    let plans = events_of(&events, "plan");
    let data = plans[0]["data"].clone();
    assert_eq!(
        data["summary"],
        serde_json::json!({ "skills": 2, "actions": 4, "new": 2, "skip": 2 }),
        "the hidden identical row's skip actions still count in the machine summary:\n{data}"
    );
    let entries = data["entries"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        entries.len(),
        2,
        "both skills are complete entries:\n{data}"
    );
    let hidden = entries
        .iter()
        .find(|entry| entry["skill"] == "writing-for-agents")
        .cloned()
        .unwrap_or_else(|| unreachable!("writing-for-agents entry is present"));
    assert_eq!(
        hidden["actions"],
        serde_json::json!([
            { "operation": "skip", "destination": "claude:global", "existed": true },
            { "operation": "skip", "destination": "shared:global", "existed": true }
        ]),
        "the fully-identical row is a complete machine entry despite being human-hidden"
    );
}

/// Two skills discovered from one source, ready to copy to an arbitrary path.
fn copy_review_fixture() -> (TempDir, PathBuf) {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "writing-for-agents", "# Writing\n");
    create_skill(&source, "drafting-commit-message", "# Drafting\n");
    (home, source)
}

/// The whole plan renders before the single confirmation, and copy has
/// nothing to teach on cancel: source, destination, and filters are always
/// stated explicitly, so nothing was inferred.
#[test]
fn copy_renders_its_whole_plan_before_one_confirmation_and_cancels() {
    let (home, source) = copy_review_fixture();
    let dest = home.path().join("vendor").join("skills");

    let declined = cli(home.path())
        .args([
            "copy",
            source.to_str().expect("utf8 source"),
            dest.to_str().expect("utf8 dest"),
        ])
        .write_stdin("n\n")
        .output()
        .expect("run declined copy");
    assert!(declined.status.success());
    assert_eq!(
        stdout_of(declined.clone()),
        format!(
            "\
Copy plan

Destination  {dest_display}

skill                    change                   action
-----------------------  -----------------------  ------
drafting-commit-message  new copy, 1 file, +1/-0  copy
writing-for-agents       new copy, 1 file, +1/-0  copy

2 changes to 1 destination: 2 new
Cancelled.\n",
            dest_display = dest.display()
        )
    );
    assert_eq!(
        stderr_of(&declined),
        format!("Copy these 2 skills to {}? [Y/n] ", dest.display())
    );
    assert!(!dest.exists(), "declining a copy plan writes nothing");
}

/// A dry run reviews the plan and concludes once instead of echoing every item.
#[test]
fn copy_dry_run_renders_the_plan_and_concludes_once() {
    let (home, source) = copy_review_fixture();
    let dest = home.path().join("vendor").join("skills");

    let stdout = stdout_of(
        cli(home.path())
            .args([
                "copy",
                source.to_str().expect("utf8 source"),
                dest.to_str().expect("utf8 dest"),
                "--dry-run",
            ])
            .output()
            .expect("run dry-run copy"),
    );
    assert_eq!(
        stdout,
        format!(
            "\
Copy plan

Destination  {dest_display}

skill                    change                   action
-----------------------  -----------------------  ------
drafting-commit-message  new copy, 1 file, +1/-0  copy
writing-for-agents       new copy, 1 file, +1/-0  copy

2 changes to 1 destination: 2 new

Dry run — no changes were made.\n",
            dest_display = dest.display()
        )
    );
    assert!(!dest.exists());
}

/// `--yes` still renders the plan, then applies it with a styled summary footer.
#[test]
fn copy_yes_renders_the_plan_then_applies_with_a_summary_footer() {
    let (home, source) = copy_review_fixture();
    let dest = home.path().join("vendor").join("skills");
    let copied_drafting = dest.join("drafting-commit-message");
    let copied_writing = dest.join("writing-for-agents");

    let output = cli(home.path())
        .args([
            "copy",
            source.to_str().expect("utf8 source"),
            dest.to_str().expect("utf8 dest"),
            "--yes",
        ])
        .output()
        .expect("run preconfirmed copy");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output.clone()),
        format!(
            "\
Copy plan

Destination  {dest_display}

skill                    change                   action
-----------------------  -----------------------  ------
drafting-commit-message  new copy, 1 file, +1/-0  copy
writing-for-agents       new copy, 1 file, +1/-0  copy

2 changes to 1 destination: 2 new

Copied drafting-commit-message -> {copied_drafting}\n\
Copied writing-for-agents -> {copied_writing}\n\
\n\
completed: 2 skills copied (2 new)\n",
            dest_display = dest.display(),
            copied_drafting = copied_drafting.display(),
            copied_writing = copied_writing.display()
        )
    );
    assert!(!stderr_of(&output).contains("Copy these"));
    assert_eq!(
        fs::read_to_string(dest.join("writing-for-agents/SKILL.md")).expect("copied skill exists"),
        "# Writing\n"
    );
}

/// A single matched skill collapses the table into the spec's degenerate
/// sentence rather than a one-row table.
#[test]
fn copy_renders_as_a_degenerate_sentence_for_a_single_matched_skill() {
    let (home, source) = copy_review_fixture();
    let dest = home.path().join("vendor").join("skills");

    let stdout = stdout_of(
        cli(home.path())
            .args([
                "copy",
                source.to_str().expect("utf8 source"),
                dest.to_str().expect("utf8 dest"),
                "--filter",
                "writing-for-agents",
                "--dry-run",
            ])
            .output()
            .expect("run single-skill copy"),
    );
    assert_eq!(
        stdout,
        format!(
            "\
Copy plan

Destination  {dest_display}

copy writing-for-agents: new copy, 1 file, +1/-0

1 change to 1 destination: 1 new

Dry run — no changes were made.\n",
            dest_display = dest.display()
        )
    );
}

/// Unlike `load`, `copy` never hides byte-identical content: re-copying to
/// the same destination still reports every skill as an overwrite.
#[test]
fn copy_has_no_identical_hiding_and_reports_overwrites_even_when_content_matches() {
    let (home, source) = copy_review_fixture();
    let dest = home.path().join("vendor").join("skills");
    let overwritten_drafting = dest.join("drafting-commit-message");
    let overwritten_writing = dest.join("writing-for-agents");
    cli(home.path())
        .args([
            "copy",
            source.to_str().expect("utf8 source"),
            dest.to_str().expect("utf8 dest"),
            "--yes",
        ])
        .assert()
        .success();

    let stdout = stdout_of(
        cli(home.path())
            .args([
                "copy",
                source.to_str().expect("utf8 source"),
                dest.to_str().expect("utf8 dest"),
                "--yes",
            ])
            .output()
            .expect("run re-copy"),
    );
    assert_eq!(
        stdout,
        format!(
            "\
Copy plan

Destination  {dest_display}

skill                    change           action
-----------------------  ---------------  ------
drafting-commit-message  no file changes  update
writing-for-agents       no file changes  update

2 changes to 1 destination: 2 overwrite

Overwrote drafting-commit-message -> {overwritten_drafting}\n\
Overwrote writing-for-agents -> {overwritten_writing}\n\
\n\
completed: 2 skills copied (2 overwritten)\n",
            dest_display = dest.display(),
            overwritten_drafting = overwritten_drafting.display(),
            overwritten_writing = overwritten_writing.display()
        ),
        "copy reports the unchanged content as an overwrite rather than hiding the row"
    );
    assert!(
        !stdout.contains(VERBATIM_PREFIX),
        "human paths must not use verbatim spellings:\n{stdout}"
    );
}

/// With a filter given, an empty match names the filter; without one, it
/// names the empty source.
#[test]
fn copy_reports_no_skills_matched_the_filter_or_found_in_the_source() {
    let (home, source) = copy_review_fixture();
    let dest = home.path().join("vendor").join("skills");

    let filtered = stdout_of(
        cli(home.path())
            .args([
                "copy",
                source.to_str().expect("utf8 source"),
                dest.to_str().expect("utf8 dest"),
                "--filter",
                "nonexistent",
                "--yes",
            ])
            .output()
            .expect("run filtered-empty copy"),
    );
    assert_eq!(
        filtered,
        format!(
            "No skills from {} matched --filter \"nonexistent\".\n",
            source.display()
        )
    );

    let empty_source = home.path().join("empty-source");
    fs::create_dir_all(&empty_source).expect("create empty source");
    let empty = stdout_of(
        cli(home.path())
            .args([
                "copy",
                empty_source.to_str().expect("utf8 empty source"),
                dest.to_str().expect("utf8 dest"),
                "--yes",
            ])
            .output()
            .expect("run empty-source copy"),
    );
    assert_eq!(
        empty,
        format!("No skills found in {}.\n", empty_source.display())
    );
}

/// A noninteractive human-facing run must authorize its plan explicitly.
#[test]
fn copy_requires_explicit_authorization_without_input() {
    let (home, source) = copy_review_fixture();
    let dest = home.path().join("vendor").join("skills");

    let output = cli(home.path())
        .args([
            "copy",
            source.to_str().expect("utf8 source"),
            dest.to_str().expect("utf8 dest"),
            "--no-input",
        ])
        .output()
        .expect("run noninteractive copy without --yes");
    assert!(!output.status.success());
    assert_eq!(
        stderr_of(&output),
        "Error: applying this plan noninteractively requires --yes.\n"
    );
    assert!(!dest.exists());
}

/// Plan order equals apply order for `copy` too, following discovery order
/// since copy has no positional skill-name selector.
#[test]
fn copy_reviews_and_applies_skills_in_discovery_order() {
    let (home, source) = copy_review_fixture();
    let dest = home.path().join("vendor").join("skills");

    let stdout = stdout_of(
        cli(home.path())
            .args([
                "copy",
                source.to_str().expect("utf8 source"),
                dest.to_str().expect("utf8 dest"),
                "--yes",
            ])
            .output()
            .expect("run ordered copy"),
    );
    let rows = stdout
        .lines()
        .filter(|line| line.contains("new copy"))
        .map(|line| line.split_whitespace().next().unwrap_or_default())
        .collect::<Vec<_>>();
    let applied = stdout
        .lines()
        .filter(|line| line.starts_with("Copied "))
        .map(|line| {
            line.strip_prefix("Copied ")
                .and_then(|rest| rest.split(" -> ").next())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rows, applied,
        "plan order must equal apply order:\n{stdout}"
    );
}

/// A terminal user reviews the same degenerate-sentence plan in the symbol
/// vocabulary, in color.
#[test]
fn copy_renders_the_interactive_symbol_and_color_plan_for_a_terminal_user() {
    let (home, source) = copy_review_fixture();
    let dest = home.path().join("vendor").join("skills");

    let stdout = stdout_of(
        cli(home.path())
            .env_remove("NO_COLOR")
            .env("SKILL_MANAGER_FORCE_INTERACTIVE", "1")
            .args([
                "copy",
                source.to_str().expect("utf8 source"),
                dest.to_str().expect("utf8 dest"),
                "--filter",
                "writing-for-agents",
                "--color",
                "always",
                "--dry-run",
            ])
            .output()
            .expect("run interactive dry-run copy"),
    );
    assert_eq!(
        stdout,
        format!(
            "\u{1b}[1;36mCopy plan\u{1b}[0m\n\
\n\
Destination  {dest_display}\n\
\n\
\u{1b}[32m+\u{1b}[0m writing-for-agents: new copy, 1 file, +1/-0\n\
\n\
1 change to 1 destination: \u{1b}[32m+ 1 new\u{1b}[0m\n\
\n\
Dry run — no changes were made.\n",
            dest_display = dest.display()
        )
    );
}

/// The NDJSON stream carries a single `plan` event at revision 0, ahead of
/// every write.
#[test]
fn copy_emits_a_structured_plan_event_before_applying() {
    let (home, source) = copy_review_fixture();
    let dest = home.path().join("vendor").join("skills");

    let events = json_events(
        cli(home.path())
            .args([
                "--json",
                "copy",
                source.to_str().expect("utf8 source"),
                dest.to_str().expect("utf8 dest"),
                "--filter",
                "writing-for-agents",
            ])
            .output()
            .expect("run machine copy"),
    );
    let plans = events_of(&events, "plan");
    assert_eq!(plans.len(), 1, "one revision was reviewed");
    let data = plans[0]["data"].clone();
    assert_eq!(data["plan_id"], "copy:writing-for-agents");
    assert_eq!(data["revision"], 0);
    assert_eq!(data["command"], "copy");
    assert_eq!(data["dry_run"], false);
    assert_eq!(data["authorization"]["kind"], "binary");
    assert_eq!(data["authorization"]["mode"], "noninteractive");
    assert_eq!(
        data["destinations"],
        serde_json::json!([
            {
                "id": "action",
                "kind": "path",
                "label": "action",
                "path": dest.display().to_string()
            }
        ])
    );
    assert_eq!(data["entries"][0]["skill"], "writing-for-agents");
    assert_eq!(data["entries"][0]["actions"][0]["operation"], "copy");
    assert_eq!(data["entries"][0]["actions"][0]["destination"], "action");
    assert_eq!(data["entries"][0]["actions"][0]["existed"], false);
    assert_eq!(
        data["summary"],
        serde_json::json!({ "skills": 1, "actions": 1, "new": 1 }),
        "copy's summary buckets by the plan's own new/overwrite categories, \
         not the generic copy/update action word"
    );
    let plan_index = events
        .iter()
        .position(|event| event["event"] == "plan")
        .expect("plan event position");
    let first_write = events
        .iter()
        .position(|event| event["event"] == "skill.copied")
        .expect("applied event position");
    assert!(plan_index < first_write, "the plan precedes every write");
}

/// `copy --dry-run` still emits the machine `summary` event last, matching
/// the pre-existing NDJSON contract that a summary always concludes the
/// stream, even when nothing was written.
#[test]
fn copy_dry_run_still_emits_a_trailing_summary_event() {
    let (home, source) = copy_review_fixture();
    let dest = home.path().join("vendor").join("skills");

    let events = json_events(
        cli(home.path())
            .args([
                "--json",
                "copy",
                source.to_str().expect("utf8 source"),
                dest.to_str().expect("utf8 dest"),
                "--dry-run",
            ])
            .output()
            .expect("run machine dry-run copy"),
    );
    assert_eq!(
        events.last().map(|event| &event["event"]),
        Some(&Value::from("summary"))
    );
    let summary = events_of(&events, "summary");
    assert_eq!(summary.len(), 1, "exactly one summary event is emitted");
    assert_eq!(
        summary[0]["data"],
        serde_json::json!({ "action": "copy", "copied": 2, "dry_run": true })
    );
    assert!(!dest.exists(), "a dry run never writes the destination");
}

#[test]
fn machine_and_recipe_updates_implicitly_use_only_enabled_installed_targets() {
    let home = sandbox();
    let source = home.path().join("source");
    let alpha = create_skill(&source, "alpha", "# Alpha v1\n");
    create_skill(&source, "beta", "# Beta stable\n");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "add", "offline", ".offline/skills"])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json", "load", "--claude", "--shared", "--target", "offline", "--global",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "target", "disable", "offline"])
        .assert()
        .success();
    let disabled_alpha = home.path().join(".offline/skills/alpha/SKILL.md");

    for (body, recipe_mode) in [("# Alpha v2\n", false), ("# Alpha v3\n", true)] {
        fs::write(alpha.join("SKILL.md"), body).expect("change source alpha");
        let output = if recipe_mode {
            cli(home.path())
                .arg("--json-input")
                .write_stdin(serde_json::json!({ "command": "update" }).to_string())
                .output()
                .expect("run implicit recipe update")
        } else {
            cli(home.path())
                .args(["--json", "update"])
                .output()
                .expect("run implicit JSON update")
        };
        assert!(output.stderr.is_empty(), "machine mode must be NDJSON-only");
        let events = json_events(output);
        let updated = events_of(&events, "skill.updated");
        let skipped = events_of(&events, "skill.skipped");
        assert_eq!(updated.len(), 2);
        assert_eq!(skipped.len(), 2);
        let updated_targets = updated
            .iter()
            .map(|event| event["data"]["target"].as_str().expect("updated target"))
            .collect::<BTreeSet<_>>();
        let skipped_targets = skipped
            .iter()
            .map(|event| event["data"]["target"].as_str().expect("skipped target"))
            .collect::<BTreeSet<_>>();
        assert_eq!(updated_targets, BTreeSet::from(["claude", "shared"]));
        assert_eq!(skipped_targets, BTreeSet::from(["claude", "shared"]));
        assert!(events.iter().all(Value::is_object));
        let summary = events_of(&events, "summary")[0];
        assert_eq!(summary["data"]["changed"], 2);
        assert_eq!(summary["data"]["skipped"], 2);
        assert_eq!(
            fs::read_to_string(&disabled_alpha).expect("disabled deployment remains installed"),
            "# Alpha v1\n"
        );
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "One config fixture validates default, verbose, color, backup, and exact raw views together."
)]
fn configs_human_output_is_layered_and_raw_output_remains_exact() {
    let home = sandbox();
    let source = home.path().join("source");
    let alternate = home.path().join("alternate");
    fs::create_dir_all(&source).expect("create source");
    fs::create_dir_all(&alternate).expect("create alternate");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
            "--label",
            "Primary Skills",
        ])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json",
            "source",
            "alternate",
            "primary",
            alternate.to_str().expect("utf8 alternate"),
        ])
        .assert()
        .success();
    let source_id = read_config(home.path())["sources"][0]["id"]
        .as_str()
        .expect("source id")
        .to_owned();
    cli(home.path())
        .args(["configs", "reset", "--yes"])
        .assert()
        .success();
    cli(home.path())
        .args(["configs", "restore", "--yes"])
        .assert()
        .success();

    let config_path = home.path().join(".skill-manager/config.json");
    let mut configured = read_config(home.path());
    configured["exclude"] = serde_json::json!(["draft-*", "private"]);
    configured["builtins"]["claude"] = serde_json::json!({
        "enabled": false,
        "presentation": { "theme": "quiet" }
    });
    configured["legacy_target_overrides"]["shared"] = serde_json::json!({
        "path": ".legacy/shared-skills",
        "label": "Legacy Shared",
        "enabled": false,
        "migration": { "owner": "team" }
    });
    configured["display_preferences"] = serde_json::json!({
        "details": { "compact": true, "labels": ["source", "target"] }
    });
    let mut configured_bytes =
        serde_json::to_vec_pretty(&configured).expect("serialize advanced config fixture");
    configured_bytes.push(b'\n');
    fs::write(&config_path, configured_bytes).expect("write advanced config fixture");

    let backups_root = home.path().join(".skill-manager/backups");
    let invalid_backup = fs::read_dir(&backups_root)
        .expect("read backups")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path().join("metadata.json"))
        .find(|path| path.is_file())
        .expect("backup metadata fixture");
    let mut metadata: Value =
        serde_json::from_slice(&fs::read(&invalid_backup).expect("read backup metadata fixture"))
            .expect("parse backup metadata fixture");
    metadata["valid"] = Value::Bool(false);
    fs::write(
        &invalid_backup,
        serde_json::to_vec_pretty(&metadata).expect("serialize invalid backup metadata"),
    )
    .expect("write invalid backup metadata fixture");

    let default = cli(home.path())
        .arg("configs")
        .output()
        .expect("default configs");
    assert!(default.status.success());
    let stdout = String::from_utf8(default.stdout).expect("utf8 configs");
    for heading in ["Configuration", "Sources", "Targets", "Backups"] {
        assert!(stdout.contains(heading));
    }
    assert!(stdout.contains("alternate available"));
    assert!(stdout.contains("created (UTC)"));
    assert!(stdout.contains("restore displaced"));
    assert!(stdout.contains("Use --verbose"));
    assert!(!stdout.contains("Configuration document"));
    assert!(!stdout.contains(&source_id));

    let verbose = cli(home.path())
        .args(["--verbose", "configs"])
        .output()
        .expect("verbose configs");
    assert!(verbose.status.success());
    let verbose_stdout = String::from_utf8(verbose.stdout).expect("utf8 verbose configs");
    assert!(verbose_stdout.contains(&source_id));
    assert!(verbose_stdout.contains("template"));
    let canonical_alternate = portable_canonicalize(&alternate).expect("canonical alternate");
    assert!(verbose_stdout.contains(&canonical_alternate.display().to_string()));
    assert!(verbose_stdout.contains("Advanced settings"));
    assert!(verbose_stdout.contains("Legacy target overrides"));
    assert!(verbose_stdout.contains("draft-*"));
    assert!(verbose_stdout.contains("presentation.theme"));
    assert!(verbose_stdout.contains("quiet"));
    assert!(verbose_stdout.contains("Legacy Shared"));
    assert!(verbose_stdout.contains("shared-skills"));
    assert!(verbose_stdout.contains("migration.owner"));
    assert!(verbose_stdout.contains("display_preferences.details.compact"));
    assert!(verbose_stdout.contains("display_preferences.details.labels"));
    assert!(!verbose_stdout.contains("\"enabled\":"));
    assert!(!verbose_stdout.contains("\"compact\":"));
    assert!(!verbose_stdout.contains("Use --verbose"));
    assert!(verbose_stdout.contains("Use --raw"));

    let always = cli(home.path())
        .args(["--color", "always", "configs"])
        .output()
        .expect("always-color configs");
    assert!(always.status.success());
    let always_stdout = String::from_utf8(always.stdout).expect("utf8 colored configs");
    assert!(always_stdout.contains("\u{1b}[1;36mConfiguration\u{1b}[0m"));
    assert!(always_stdout.contains("\u{1b}[32menabled\u{1b}[0m"));
    assert!(always_stdout.contains("\u{1b}[2mdisabled\u{1b}[0m"));
    assert!(always_stdout.contains("\u{1b}[32mvalid\u{1b}[0m"));
    assert!(always_stdout.contains("\u{1b}[31minvalid\u{1b}[0m"));

    for policy in ["never", "auto"] {
        let mut command = cli(home.path());
        command
            .env_remove("NO_COLOR")
            .args(["--color", policy, "configs"])
            .assert()
            .success()
            .stdout(predicate::str::contains('\u{1b}').not());
    }

    let expected = fs::read(&config_path).expect("read exact config bytes");
    let raw = cli(home.path())
        .args(["configs", "--raw"])
        .output()
        .expect("raw configs");
    assert!(raw.status.success());
    assert_eq!(raw.stdout, expected);
}

/// A bare literal `load`/`update` operand now resolves against discovered
/// skill names, case-insensitively, from any CWD. This is the reported bug:
/// before the fix, a bare skill name was always treated as a CWD-relative
/// source path and failed with "source directory not found".
#[test]
fn load_bare_skill_name_resolves_from_an_unrelated_cwd() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "knowing-camber-me", "# Camber");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "sernst-skills",
        ])
        .assert()
        .success();

    let elsewhere = home.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).expect("create unrelated cwd");
    let mut load = cli(home.path());
    load.current_dir(&elsewhere);
    load.args(["load", "knowing-camber-me", "--claude", "--global"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Loaded knowing-camber-me"));
    assert!(
        home.path()
            .join(".claude/skills/knowing-camber-me/SKILL.md")
            .is_file()
    );
}

/// `install` (the `load` alias) resolves a bare skill name case-insensitively.
#[test]
fn install_uppercase_skill_name_folds_case() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "sample-skill", "# Sample");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();

    cli(home.path())
        .args(["install", "SAMPLE-SKILL", "--claude", "--global"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Loaded sample-skill"));
    assert!(
        home.path()
            .join(".claude/skills/sample-skill/SKILL.md")
            .is_file()
    );
}

/// A bare literal that names no configured source, CWD directory, or
/// discovered skill is a hard error with an actionable message, unlike an
/// unmatched glob pattern, which only warns.
#[test]
fn load_unknown_literal_is_a_hard_error() {
    let home = sandbox();
    cli(home.path())
        .args([
            "load",
            "totally-unknown-thing",
            "--claude",
            "--global",
            "--no-input",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no configured source, directory, or skill named \"totally-unknown-thing\"",
        ))
        .stderr(predicate::str::contains("skill-manager ls"));

    cli(home.path())
        .args([
            "--json",
            "load",
            "totally-unknown-thing",
            "--claude",
            "--global",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "no configured source, directory, or skill named \\\"totally-unknown-thing\\\"",
        ))
        .stdout(predicate::str::contains("\"event\":\"command.failed\""));
}

/// When a bare word is both a discovered skill name and a same-named CWD
/// directory, the skill wins and the command warns about the ambiguity,
/// pointing at `./name` to force the directory interpretation. Both the
/// human warning and the NDJSON `diagnostic`/`Warning` event are asserted.
#[test]
fn ambiguous_bare_word_prefers_the_skill_and_warns() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "dup-name", "# Dup");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    fs::create_dir_all(home.path().join("dup-name")).expect("create ambiguous cwd directory");

    cli(home.path())
        .args([
            "load",
            "dup-name",
            "--claude",
            "--global",
            "--no-input",
            "--yes",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "\"dup-name\" matches both a discovered skill and a directory",
        ))
        .stderr(predicate::str::contains("./dup-name"));
    assert!(
        home.path()
            .join(".claude/skills/dup-name/SKILL.md")
            .is_file()
    );

    let events = json_events(
        cli(home.path())
            .args(["--json", "load", "dup-name", "--shared", "--global"])
            .output()
            .expect("json ambiguous load"),
    );
    let warnings = events_of(&events, "diagnostic");
    assert!(
        warnings.iter().any(|event| {
            event["level"] == "warning"
                && event["data"]["message"].as_str().is_some_and(|message| {
                    message.contains("matches both a discovered skill and a directory")
                        && message.contains("./dup-name")
                })
                && event["data"].get("pattern").is_none()
        }),
        "expected an NDJSON diagnostic warning naming the ambiguity with a message-only payload: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event["event"] == "skill.loaded" && event["data"]["skill"] == "dup-name")
    );
}

/// Regression: a bare relative directory name that is not a discovered skill
/// still resolves as a source path, exactly as before this fix.
#[test]
fn bare_directory_name_that_is_not_a_skill_still_resolves_as_a_source() {
    let home = sandbox();
    let plain_dir = home.path().join("plain-dir");
    create_skill(&plain_dir, "widget", "# Widget");

    cli(home.path())
        .args([
            "load",
            "plain-dir",
            "--claude",
            "--global",
            "--no-input",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Loaded widget"));
    assert!(home.path().join(".claude/skills/widget/SKILL.md").is_file());
}

/// Regression: a bare configured-source name and a bare source label still
/// resolve as source references, exactly as before this fix.
#[test]
fn bare_configured_source_name_and_label_still_resolve_as_sources() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "gizmo", "# Gizmo");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
            "--label",
            "Primary Label",
        ])
        .assert()
        .success();

    cli(home.path())
        .args([
            "load",
            "primary",
            "--claude",
            "--global",
            "--no-input",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Loaded gizmo"));
    assert!(home.path().join(".claude/skills/gizmo/SKILL.md").is_file());

    cli(home.path())
        .args([
            "load",
            "Primary Label",
            "--shared",
            "--global",
            "--no-input",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Loaded gizmo"));
    assert!(home.path().join(".agents/skills/gizmo/SKILL.md").is_file());
}

/// Regression (A6): when a mixed operand list contains both a literal skill
/// name and a bare directory name that must be promoted to a source (a
/// second discovery pass), any collision from the configured sources must be
/// reported exactly once, not once per discovery pass.
#[test]
fn collision_diagnostic_is_emitted_exactly_once_across_a_second_discovery_pass() {
    let home = sandbox();
    let first = home.path().join("first");
    let second = home.path().join("second");
    create_skill(&first, "common", "# First");
    create_skill(&second, "common", "# Second");
    for (path, name) in [(&first, "first"), (&second, "second")] {
        cli(home.path())
            .args([
                "--json",
                "source",
                "add",
                path.to_str().expect("utf8 path"),
                name,
            ])
            .assert()
            .success();
    }
    let plain_dir = home.path().join("plain-dir");
    create_skill(&plain_dir, "extra", "# Extra");

    let events = json_events(
        cli(home.path())
            .args([
                "--json",
                "load",
                "common",
                "plain-dir",
                "--claude",
                "--global",
            ])
            .output()
            .expect("mixed skill and promoted-directory load"),
    );
    let collisions = events_of(&events, "collision.detected");
    assert_eq!(
        collisions.len(),
        1,
        "collision.detected must be emitted exactly once across both discovery passes: {events:?}"
    );
    // The bare literal skill name narrows selection to itself, same as the
    // mixed source+skill case above; the promoted directory only widens the
    // discovery *candidate pool* (which is what surfaces the collision from
    // "first"/"second"), not the final selection.
    let loaded = events_of(&events, "skill.loaded");
    let names = loaded
        .iter()
        .filter_map(|event| event["data"]["skill"].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(names, BTreeSet::from(["common"]));
}

/// `load <bare-dir> <exact-skill-inside-that-dir>` must succeed: the skill
/// name only becomes resolvable after `plain-dir` is promoted to a source
/// and the second discovery pass runs, so it must not be hard-errored
/// against the preliminary (pre-promotion) discovery. This is the reported
/// sequencing bug: an equivalent glob (`load plain-dir "widg*"`) already
/// worked, but the exact literal name did not.
#[test]
fn bare_dir_operand_and_a_skill_name_discovered_only_inside_it_both_resolve() {
    let home = sandbox();
    let plain_dir = home.path().join("plain-dir");
    create_skill(&plain_dir, "widget", "# Widget");

    // The glob-based equivalent already worked before this fix; assert it
    // still does, alongside the newly-fixed exact-name case.
    cli(home.path())
        .args([
            "load",
            "plain-dir",
            "widg*",
            "--claude",
            "--global",
            "--no-input",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Loaded widget"));

    let events = json_events(
        cli(home.path())
            .args([
                "--json",
                "load",
                "plain-dir",
                "widget",
                "--shared",
                "--global",
            ])
            .output()
            .expect("bare directory plus exact skill name inside it"),
    );
    assert!(
        !events
            .iter()
            .any(|event| event["event"] == "command.failed"),
        "a skill discovered only after directory promotion must resolve: {events:?}"
    );
    let loaded = events_of(&events, "skill.loaded");
    assert_eq!(loaded.len(), 1, "exactly one skill must deploy: {events:?}");
    assert_eq!(loaded[0]["data"]["skill"], "widget");
    assert!(home.path().join(".agents/skills/widget/SKILL.md").is_file());
}

/// `load <bare-dir> <exact-skill-inside-it> <name-that-exists-nowhere>` must
/// still be a hard error naming the truly unresolvable word, even after the
/// directory is promoted, `widget` resolves against the final discovery, and
/// only `nowhere-to-be-found` remains unresolved. This is rollback-sensitive:
/// on the pre-fix implementation, `resolve_deferred_sync_operands` hard-errors
/// immediately against the PRELIMINARY discovery, before `plain-dir` is ever
/// promoted -- so it would report `widget` (the first deferred word it fails
/// to resolve) rather than `nowhere-to-be-found`, and it would do so even
/// though `widget` is in fact resolvable once promotion runs.
#[test]
fn bare_dir_operand_with_an_unresolvable_sibling_name_is_still_a_hard_error() {
    let home = sandbox();
    let plain_dir = home.path().join("plain-dir");
    create_skill(&plain_dir, "widget", "# Widget");

    cli(home.path())
        .args([
            "load",
            "plain-dir",
            "widget",
            "nowhere-to-be-found",
            "--claude",
            "--global",
            "--no-input",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no configured source, directory, or skill named \"nowhere-to-be-found\"",
        ))
        .stderr(predicate::str::contains("skill-manager ls"))
        .stderr(predicate::str::contains("\"widget\"").not());
    assert!(!home.path().join(".claude/skills/widget").exists());
}

// A word that becomes resolvable only after directory promotion (the
// provisional path) still applies the discovered-skill-vs-CWD-directory
// ambiguity rule identically to the preliminary resolver: see the
// `provisional_resolution_still_warns_when_a_same_named_cwd_directory_exists`
// and `provisional_resolution_does_not_warn_without_a_same_named_cwd_directory`
// unit tests in `src/app.rs`. Those exercise `resolve_provisional_sync_operands`
// directly against a synthetic CWD and a hand-built discovery result, because
// the combination cannot be expressed as a single CLI invocation: any bare
// word that is itself a real top-level CWD directory is always classified as
// a directory to promote (step 5) during the *preliminary* pass -- before
// discovery can know whether that same word would also resolve as a skill --
// so it can never simultaneously reach the provisional bucket. A directory
// literally named after the provisionally-resolved skill can therefore never
// coexist with that skill being "provisional" at the CLI level; the ambiguity
// check inside `resolve_provisional_sync_operands` is still required (so the
// helper cannot silently drift from the preliminary resolver as the code
// evolves), which is exactly what the unit tests isolate and prove.

/// The single extra discovery pass triggered by a bare-directory promotion,
/// combined with a provisionally-unresolved sibling word that only resolves
/// against that final discovery, must not run discovery (or emit its
/// collision diagnostics) more than once. Reuses the same
/// exactly-once-across-passes counting approach as
/// `collision_diagnostic_is_emitted_exactly_once_across_a_second_discovery_pass`.
/// A literal skill name resolved in the preliminary pass keeps the
/// configured sources (rather than replacing them with just the promoted
/// directory), so the pre-existing collision stays observable in the final
/// discovery and its diagnostic count proves discovery ran exactly once
/// beyond the preliminary pass, not once per resolved word.
#[test]
fn provisional_word_resolution_does_not_trigger_a_third_discovery_pass() {
    let home = sandbox();
    let first = home.path().join("first");
    let second = home.path().join("second");
    create_skill(&first, "shared-name", "# First");
    create_skill(&second, "shared-name", "# Second");
    for (path, name) in [(&first, "first"), (&second, "second")] {
        cli(home.path())
            .args([
                "--json",
                "source",
                "add",
                path.to_str().expect("utf8 path"),
                name,
            ])
            .assert()
            .success();
    }
    let plain_dir = home.path().join("plain-dir");
    create_skill(&plain_dir, "widget", "# Widget");

    let events = json_events(
        cli(home.path())
            .args([
                "--json",
                "load",
                "shared-name",
                "plain-dir",
                "widget",
                "--claude",
                "--global",
            ])
            .output()
            .expect("literal skill, bare directory promotion, and a provisionally-resolved word"),
    );
    let collisions = events_of(&events, "collision.detected");
    assert_eq!(
        collisions.len(),
        1,
        "collision.detected must be emitted exactly once, proving discovery ran at most once beyond the preliminary pass: {events:?}"
    );
    let loaded = events_of(&events, "skill.loaded");
    let names = loaded
        .iter()
        .filter_map(|event| event["data"]["skill"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names.len(),
        2,
        "each selected skill must load exactly once, not be duplicated by the extra pass: {events:?}"
    );
    assert_eq!(
        names.iter().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from(["shared-name", "widget"])
    );
    assert!(home.path().join(".claude/skills/widget/SKILL.md").is_file());
    assert!(
        home.path()
            .join(".claude/skills/shared-name/SKILL.md")
            .is_file()
    );
}

/// `update <skill name>` selects only that skill, resolving it through the
/// new literal-skill-name path rather than a configured source or label.
#[test]
fn update_bare_skill_name_selects_only_that_skill() {
    let home = sandbox();
    let source = home.path().join("source");
    let keep = create_skill(&source, "keep-me", "# Keep v1");
    let drop = create_skill(&source, "drop-me", "# Drop v1");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--claude", "--global"])
        .assert()
        .success();

    fs::write(keep.join("SKILL.md"), "# Keep v2").expect("update keep-me source");
    fs::write(drop.join("SKILL.md"), "# Drop v2").expect("update drop-me source");

    cli(home.path())
        .args(["update", "keep-me", "--claude", "--global", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated keep-me"))
        .stdout(predicate::str::contains("Updated drop-me").not());
    assert_eq!(
        fs::read_to_string(home.path().join(".claude/skills/keep-me/SKILL.md"))
            .expect("read updated keep-me deployment"),
        "# Keep v2"
    );
    assert_eq!(
        fs::read_to_string(home.path().join(".claude/skills/drop-me/SKILL.md"))
            .expect("read unchanged drop-me deployment"),
        "# Drop v1"
    );
}

/// Mixed operands narrow to both the named source and the named skill: a
/// second, unrelated source and its skills are excluded entirely.
#[test]
fn mixed_source_and_skill_operands_narrow_to_both() {
    let home = sandbox();
    let source_a = home.path().join("source-a");
    let source_b = home.path().join("source-b");
    create_skill(&source_a, "widget-a", "# Widget A");
    create_skill(&source_a, "gadget-a", "# Gadget A");
    create_skill(&source_b, "widget-b", "# Widget B");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source_a.to_str().expect("utf8 source a"),
            "alpha-source",
            "--label",
            "Alpha Source",
        ])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source_b.to_str().expect("utf8 source b"),
            "beta-source",
        ])
        .assert()
        .success();

    let events = json_events(
        cli(home.path())
            .args([
                "--json",
                "load",
                "Alpha Source",
                "widget-a",
                "--claude",
                "--global",
            ])
            .output()
            .expect("mixed operand load"),
    );
    let loaded = events_of(&events, "skill.loaded");
    assert_eq!(loaded.len(), 1, "only widget-a should deploy: {events:?}");
    assert_eq!(loaded[0]["data"]["skill"], "widget-a");
    assert!(home.path().join(".claude/skills/widget-a").is_dir());
    assert!(!home.path().join(".claude/skills/gadget-a").exists());
    assert!(!home.path().join(".claude/skills/widget-b").exists());
}

/// Glob behavior is unchanged: a wildcard still selects matching skills, and
/// an unmatched glob still warns rather than hard-erroring like an
/// unresolvable literal does.
#[test]
fn glob_patterns_are_unaffected_by_literal_skill_name_resolution() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "knowing-camber-me", "# Camber");
    create_skill(&source, "knowing-other-thing", "# Other");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();

    cli(home.path())
        .args([
            "load",
            "knowing-*",
            "--claude",
            "--global",
            "--no-input",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Loaded knowing-camber-me"))
        .stdout(predicate::str::contains("Loaded knowing-other-thing"));

    cli(home.path())
        .args(["--json", "load", "missing-glob-*", "--claude", "--global"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "skill pattern matched nothing: missing-glob-*",
        ));
}

/// A valid literal skill name paired with an unmatched glob must still
/// succeed: the unmatched glob only warns, and the literal skill deploys.
/// This is the reported "mutation followed by hard failure" bug: before the
/// fix, `load known-skill missing-*` deployed `known-skill` and then still
/// exited non-zero because `positional_matched` only tracked glob matches.
#[test]
fn literal_skill_name_with_an_unmatched_glob_still_succeeds_and_only_deploys_the_literal() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "known-skill", "# Known");
    create_skill(&source, "other-skill", "# Other");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();

    let events = json_events(
        cli(home.path())
            .args([
                "--json",
                "load",
                "known-skill",
                "missing-*",
                "--claude",
                "--global",
            ])
            .output()
            .expect("literal plus unmatched glob load"),
    );
    let warnings = events_of(&events, "diagnostic");
    assert!(
        warnings.iter().any(|event| {
            event["data"]["message"] == "skill pattern matched nothing: missing-*"
        }),
        "expected the unmatched-glob warning: {events:?}"
    );
    let loaded = events_of(&events, "skill.loaded");
    assert_eq!(
        loaded.len(),
        1,
        "only the literal skill should deploy: {events:?}"
    );
    assert_eq!(loaded[0]["data"]["skill"], "known-skill");
    assert!(
        events.iter().any(|event| event["event"] == "summary"),
        "the invocation must succeed and emit a summary, not command.failed: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| event["event"] == "command.failed"),
        "an unmatched glob must not fail the invocation when a literal matched: {events:?}"
    );
    assert!(
        home.path()
            .join(".claude/skills/known-skill/SKILL.md")
            .is_file()
    );
    assert!(!home.path().join(".claude/skills/other-skill").exists());

    // Plain (non-JSON) exit code must be zero too.
    cli(home.path())
        .args([
            "load",
            "known-skill",
            "missing-*",
            "--shared",
            "--global",
            "--no-input",
            "--yes",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "skill pattern matched nothing: missing-*",
        ));
}

/// The inverse of the above: an unresolvable literal must still be a hard
/// error even when another glob operand DOES match something. A matching
/// glob must not mask an unresolvable literal.
#[test]
fn unresolvable_literal_with_a_matching_glob_is_still_a_hard_error() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "knowing-camber-me", "# Camber");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();

    cli(home.path())
        .args([
            "load",
            "totally-unknown-thing",
            "knowing-*",
            "--claude",
            "--global",
            "--no-input",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no configured source, directory, or skill named \"totally-unknown-thing\"",
        ))
        .stderr(predicate::str::contains("skill-manager ls"));
    assert!(
        !home
            .path()
            .join(".claude/skills/knowing-camber-me")
            .exists()
    );
}

/// JSON recipe input (`--json-input`) overlays into the same `SyncArgs` as
/// the CLI, so a bare literal skill name supplied through a recipe's
/// `"source"` field must resolve identically: narrowing to that one skill,
/// not falling back to "deploy everything".
#[test]
fn recipe_literal_skill_name_resolution_matches_the_cli() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "recipe-target", "# Recipe target");
    create_skill(&source, "recipe-other", "# Recipe other");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();

    let recipe = serde_json::json!({
        "command": "load",
        "source": "recipe-target",
        "claude": true,
        "global": true
    });
    let events = json_events(
        cli(home.path())
            .arg("--json-input")
            .write_stdin(recipe.to_string())
            .output()
            .expect("run literal-skill-name recipe"),
    );
    let loaded = events_of(&events, "skill.loaded");
    assert_eq!(
        loaded.len(),
        1,
        "recipe literal skill name must narrow like the CLI does: {events:?}"
    );
    assert_eq!(loaded[0]["data"]["skill"], "recipe-target");
    assert!(
        home.path()
            .join(".claude/skills/recipe-target/SKILL.md")
            .is_file()
    );
    assert!(!home.path().join(".claude/skills/recipe-other").exists());
}

/// With no positional operands at all, `load` still deploys every discovered
/// skill; this must remain true even though literal skill names now
/// participate in selection.
#[test]
fn load_with_no_positional_operands_still_deploys_everything() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha");
    create_skill(&source, "beta", "# Beta");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();

    let events = json_events(
        cli(home.path())
            .args(["--json", "load", "--claude", "--global"])
            .output()
            .expect("load everything"),
    );
    let loaded = events_of(&events, "skill.loaded");
    let names = loaded
        .iter()
        .filter_map(|event| event["data"]["skill"].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        names,
        BTreeSet::from(["alpha", "beta"]),
        "no-operand load must deploy every discovered skill"
    );
}

// ---------------------------------------------------------------------------
// `remove`: plan-first review, scope-ambiguity selection, and preserved
// blast-radius contracts (Stage 3).
// ---------------------------------------------------------------------------

/// `teach` deployed to `claude`+`shared` at both global and project scope:
/// the canonical scope-ambiguity fixture for the removal-scope branch.
/// Returns the home sandbox and its `project` subdirectory; the ambiguity is
/// only visible once the working directory is under `project`, mirroring how
/// project scope is discovered elsewhere in this suite.
fn remove_ambiguous_fixture() -> (TempDir, PathBuf) {
    let home = sandbox();
    let source = home.path().join("source");
    let project = home.path().join("project");
    fs::create_dir_all(&project).expect("create project directory");
    create_skill(&source, "teach", "# teach");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json", "load", "teach", "--claude", "--shared", "--global",
        ])
        .assert()
        .success();
    let mut project_load = cli(home.path());
    project_load.current_dir(&project);
    project_load
        .args([
            "--json",
            "load",
            "teach",
            "--claude",
            "--shared",
            "--project",
        ])
        .assert()
        .success();
    (home, project)
}

/// The rendered scope-branch table shared by every ambiguous-`teach` test:
/// availability evidence, then the three numbered alternatives with their own
/// blast radius, before any prompt is asked. Ends with exactly one trailing
/// newline, mirroring `UPDATE_REVIEW_PLAN`'s convention, so callers splice
/// their own conclusion after it.
const REMOVE_BRANCH_TABLE: &str = "\
Remove plan

Available deployments

skill  files/deploy  claude  shared
-----  ------------  ------  ------
teach  1             both    both

  1  Remove project copies  − 2 deployments, 2 files
  2  Remove global copies   − 2 deployments, 2 files
  3  Remove both copies     − 4 deployments, 4 files
";

/// The both-scopes branch renders every alternative's blast radius before
/// asking anything, and a dry run enumerates them without offering to
/// cancel, per the Stage 1 fixture
/// (`a_dry_run_remove_enumerates_alternatives_without_offering_to_cancel`).
#[test]
fn remove_dry_run_enumerates_scope_alternatives_without_offering_to_cancel() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args(["remove", "teach", "--claude", "--shared", "--dry-run"])
        .output()
        .expect("run ambiguous dry-run remove");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output.clone()),
        format!(
            "{REMOVE_BRANCH_TABLE}\n\
Dry run — 3 alternatives shown; no option selected and no changes were made.\n"
        )
    );
    assert!(stderr_of(&output).is_empty(), "a dry run never prompts");
}

/// Cancelling the numbered selection (`c`) exits 0 with no writes and prints
/// exactly `Cancelled.`, no hint: the branch itself just taught the scope
/// decision, so there is nothing left to teach.
#[test]
fn remove_scope_selection_cancels_with_no_hint_and_no_writes() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args(["remove", "teach", "--claude", "--shared"])
        .write_stdin("c\n")
        .output()
        .expect("run cancelled selection remove");
    assert!(output.status.success(), "cancelling is not a failure");
    assert_eq!(
        stdout_of(output.clone()),
        format!("{REMOVE_BRANCH_TABLE}  c  Cancel\n\nCancelled.\n")
    );
    assert_eq!(
        stderr_of(&output),
        "Select removal scope [1-3, c to cancel]: "
    );
    assert!(
        home.path().join(".claude/skills/teach/SKILL.md").is_file(),
        "cancelling writes nothing"
    );
    assert!(
        project.join(".claude/skills/teach/SKILL.md").is_file(),
        "cancelling writes nothing"
    );
}

/// Two empty answers and one invalid token all reprompt with the exact same
/// instruction; the selection never auto-picks an option, and only the final
/// `c` resolves it.
#[test]
fn remove_scope_selection_reprompts_on_invalid_and_empty_input_without_auto_selecting() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args(["remove", "teach", "--claude", "--shared"])
        .write_stdin("\nbogus\nc\n")
        .output()
        .expect("run reprompted selection remove");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output.clone()),
        format!("{REMOVE_BRANCH_TABLE}  c  Cancel\n\nCancelled.\n")
    );
    assert_eq!(
        stderr_of(&output),
        "Select removal scope [1-3, c to cancel]: Enter 1, 2, 3, or c.\n\
Select removal scope [1-3, c to cancel]: Enter 1, 2, 3, or c.\n\
Select removal scope [1-3, c to cancel]: "
    );
}

/// Selecting `1` at the branch removes only the project copies, leaving the
/// global copies untouched.
#[test]
fn remove_scope_selection_option_1_removes_only_project_copies() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args(["remove", "teach", "--claude", "--shared"])
        .write_stdin("1\n")
        .output()
        .expect("run project-only selection remove");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output),
        format!(
            "{REMOVE_BRANCH_TABLE}  c  Cancel\n\n\n\
Removed teach from claude (project)\n\
Removed teach from shared (project)\n\
\n\
completed: 2 deployments removed (1 skill, 2 files)\n"
        )
    );
    assert!(
        home.path().join(".claude/skills/teach/SKILL.md").is_file(),
        "the global copy survives choosing the project-only option"
    );
    assert!(
        !project.join(".claude/skills/teach").exists(),
        "the project copy is gone"
    );
}

/// Selecting `2` at the branch removes only the global copies, leaving the
/// project copies untouched.
#[test]
fn remove_scope_selection_option_2_removes_only_global_copies() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args(["remove", "teach", "--claude", "--shared"])
        .write_stdin("2\n")
        .output()
        .expect("run global-only selection remove");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output),
        format!(
            "{REMOVE_BRANCH_TABLE}  c  Cancel\n\n\n\
Removed teach from claude (global)\n\
Removed teach from shared (global)\n\
\n\
completed: 2 deployments removed (1 skill, 2 files)\n"
        )
    );
    assert!(
        !home.path().join(".claude/skills/teach").exists(),
        "the global copy is gone"
    );
    assert!(
        project.join(".claude/skills/teach/SKILL.md").is_file(),
        "the project copy survives choosing the global-only option"
    );
}

/// Selecting `3` at the branch removes both copies.
#[test]
fn remove_scope_selection_option_3_removes_both_copies() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args(["remove", "teach", "--claude", "--shared"])
        .write_stdin("3\n")
        .output()
        .expect("run both-scopes selection remove");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output),
        format!(
            "{REMOVE_BRANCH_TABLE}  c  Cancel\n\n\n\
Removed teach from claude (global)\n\
Removed teach from claude (project)\n\
Removed teach from shared (global)\n\
Removed teach from shared (project)\n\
\n\
completed: 4 deployments removed (1 skill, 4 files)\n"
        )
    );
    assert!(!home.path().join(".claude/skills/teach").exists());
    assert!(!project.join(".claude/skills/teach").exists());
}

/// The remove-only `--both` flag reaches the same both-scopes outcome
/// noninteractively, collapsing the branch to a plain action table (each
/// cell annotated `remove both`, since the destination still spans both
/// scopes even though the choice is no longer open).
#[test]
fn remove_both_flag_collapses_the_branch_and_removes_both_copies_noninteractively() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args(["remove", "teach", "--claude", "--shared", "--both", "--yes"])
        .output()
        .expect("run --both --yes remove");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output),
        "\
Remove plan

skill  files/deploy  claude       shared
-----  ------------  -----------  -----------
teach  1             remove both  remove both

4 deployment removals across 2 selected targets: 4 remove; 1 skill, 4 files

Removed teach from claude (global)
Removed teach from claude (project)
Removed teach from shared (global)
Removed teach from shared (project)

completed: 4 deployments removed (1 skill, 4 files)
"
    );
    assert!(!home.path().join(".claude/skills/teach").exists());
    assert!(!project.join(".claude/skills/teach").exists());
}

/// Without `--yes`/`--both`, `--no-input` refuses a genuinely ambiguous
/// remove rather than silently guessing a scope — the plan still renders
/// (without the interactive `c Cancel` row, since nothing will be asked),
/// and nothing is written.
#[test]
fn remove_no_input_refuses_an_ambiguous_scope_without_yes_or_both() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args(["remove", "teach", "--claude", "--shared", "--no-input"])
        .output()
        .expect("run no-input ambiguous remove");
    assert!(!output.status.success());
    assert_eq!(
        stdout_of(output.clone()),
        format!("{REMOVE_BRANCH_TABLE}\n")
    );
    assert_eq!(
        stderr_of(&output),
        "Error: selected skills exist in both scopes; choose --project, --global, or --both before using --yes.\n"
    );
    assert!(home.path().join(".claude/skills/teach/SKILL.md").is_file());
    assert!(project.join(".claude/skills/teach/SKILL.md").is_file());
}

/// `--yes` alone (no `--project`/`--global`/`--both`) is refused the same
/// way: `remove` never auto-picks a scope just because the user pre-approved
/// applying the plan.
#[test]
fn remove_yes_without_a_scope_refuses_when_genuinely_ambiguous() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args(["remove", "teach", "--claude", "--shared", "--yes"])
        .output()
        .expect("run --yes ambiguous remove");
    assert!(!output.status.success());
    assert_eq!(
        stdout_of(output.clone()),
        format!("{REMOVE_BRANCH_TABLE}\n")
    );
    assert_eq!(
        stderr_of(&output),
        "Error: selected skills exist in both scopes; choose --project, --global, or --both before using --yes.\n"
    );
    assert!(home.path().join(".claude/skills/teach/SKILL.md").is_file());
    assert!(project.join(".claude/skills/teach/SKILL.md").is_file());
}

/// An explicit scope collapses the branch entirely: the plan is a plain
/// action table, authorized by a `[y/N]` confirmation defaulting to No.
/// `--yes` still renders the plan before applying it.
#[test]
fn remove_explicit_scope_collapses_to_a_plain_action_table_and_yes_applies_it() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args([
            "remove", "teach", "--claude", "--shared", "--global", "--yes",
        ])
        .output()
        .expect("run explicit-scope --yes remove");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output),
        "\
Remove plan

skill  files/deploy  claude  shared
-----  ------------  ------  ------
teach  1             remove  remove

2 deployment removals across 2 selected targets: 2 remove; 1 skill, 2 files

Removed teach from claude (global)
Removed teach from shared (global)

completed: 2 deployments removed (1 skill, 2 files)
"
    );
    assert!(!home.path().join(".claude/skills/teach").exists());
    assert!(
        project.join(".claude/skills/teach/SKILL.md").is_file(),
        "an explicit --global scope must never touch the project copy"
    );
}

/// Cancelling the plain `[y/N]` confirmation for an explicit, unambiguous
/// scope prints `Cancelled.` with no hint: nothing was inferred, so there is
/// no flag left to teach.
#[test]
fn remove_explicit_scope_cancel_prints_no_hint_when_nothing_was_inferred() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args(["remove", "teach", "--claude", "--shared", "--global"])
        .write_stdin("n\n")
        .output()
        .expect("run explicit-scope cancel remove");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output.clone()),
        "\
Remove plan

skill  files/deploy  claude  shared
-----  ------------  ------  ------
teach  1             remove  remove

2 deployment removals across 2 selected targets: 2 remove; 1 skill, 2 files
Cancelled.
"
    );
    assert_eq!(
        stderr_of(&output),
        "Remove these 2 deployments from 2 selected targets? [y/N] "
    );
    assert!(home.path().join(".claude/skills/teach/SKILL.md").is_file());
    assert!(project.join(".claude/skills/teach/SKILL.md").is_file());
}

/// Cancelling the plain `[y/N]` confirmation when targets and scope were
/// *inferred* (not stated) teaches exactly which flags to add next time,
/// mirroring `update`'s and `load`'s cancel hints.
#[test]
fn remove_cancel_teaches_inferred_target_and_scope_hints() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "solo-skill", "# solo");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "solo-skill", "--claude", "--global"])
        .assert()
        .success();

    let output = cli(home.path())
        .args(["remove", "solo-skill"])
        .write_stdin("n\n")
        .output()
        .expect("run inferred-target cancel remove");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output.clone()),
        "\
Remove plan

remove solo-skill from claude: 1 file

1 deployment removal across 1 target: 1 remove; 1 skill, 1 file
Cancelled.
Hint: targets and deployed scopes were inferred. Re-run with --claude, --shared, --antigravity, --all, or --target NAME, and --global or --project, to narrow this plan.
"
    );
    assert_eq!(
        stderr_of(&output),
        "Remove this deployment from 1 target? [y/N] "
    );
    assert!(
        home.path()
            .join(".claude/skills/solo-skill/SKILL.md")
            .is_file(),
        "cancelling writes nothing"
    );
}

/// `teach` (ambiguous, both scope), plus two unrelated deployed skills and a
/// skill deployed to only one scope: the shared fixture for the bare-remove
/// full-blast-radius test and the originating-scenario regression.
fn remove_originating_scenario_fixture() -> (TempDir, PathBuf) {
    let home = sandbox();
    let source = home.path().join("source");
    let project = home.path().join("project");
    fs::create_dir_all(&project).expect("create project directory");
    for name in ["teach", "solo-skill", "other-one", "other-two"] {
        create_skill(&source, name, &format!("# {name}"));
    }
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json", "load", "teach", "--claude", "--shared", "--global",
        ])
        .assert()
        .success();
    let mut project_load = cli(home.path());
    project_load.current_dir(&project);
    project_load
        .args([
            "--json",
            "load",
            "teach",
            "--claude",
            "--shared",
            "--project",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "solo-skill", "--claude", "--global"])
        .assert()
        .success();
    for name in ["other-one", "other-two"] {
        cli(home.path())
            .args(["--json", "load", name, "--claude", "--shared", "--global"])
            .assert()
            .success();
    }
    (home, project)
}

/// **Originating-scenario regression.** A single named skill that exists
/// across many deployments — while other, unrelated skills are also
/// deployed — must render a plan naming exactly that skill and its real
/// deployments: never a bare aggregate count (`Remove 30 skill
/// deployment(s)?`), and never the other skills. This is the exact defect
/// that triggered this whole effort.
#[test]
fn remove_originating_scenario_names_exactly_the_requested_skill_and_its_deployments() {
    let (home, project) = remove_originating_scenario_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args(["remove", "teach", "--claude", "--shared", "--dry-run"])
        .output()
        .expect("run originating-scenario dry-run remove");
    assert!(output.status.success());
    let stdout = stdout_of(output);
    assert_eq!(
        stdout,
        format!(
            "{REMOVE_BRANCH_TABLE}\n\
Dry run — 3 alternatives shown; no option selected and no changes were made.\n"
        ),
        "the plan must name exactly teach and its own deployments, \
         never a bare count and never the other deployed skills:\n{stdout}"
    );
    assert!(!stdout.contains("solo-skill"));
    assert!(!stdout.contains("other-one"));
    assert!(!stdout.contains("other-two"));
    assert!(
        !stdout.contains("skill deployment(s)"),
        "the bare-count prompt that triggered this effort must never reappear"
    );
}

/// A literal skill name that is not deployed anywhere reports precisely
/// that, exits 0, and renders neither a table nor a prompt.
#[test]
fn remove_names_a_literal_skill_that_is_not_deployed_anywhere() {
    let (home, _project) = remove_originating_scenario_fixture();
    let output = cli(home.path())
        .args(["remove", "nonexistent-skill"])
        .output()
        .expect("run undeployed-literal remove");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output),
        "nonexistent-skill is not deployed to any enabled target in global or project scope.\n"
    );
}

/// A syntactically valid pattern matching nothing keeps the existing
/// `NotFound` contract: nonzero exit, and no plan is ever rendered.
#[test]
fn remove_names_an_unmatched_glob_pattern_as_not_found() {
    let (home, _project) = remove_originating_scenario_fixture();
    let output = cli(home.path())
        .args(["remove", "zzz-nomatch-*"])
        .output()
        .expect("run unmatched-pattern remove");
    assert!(!output.status.success());
    assert_eq!(
        stderr_of(&output),
        "Warning: skill pattern matched nothing: zzz-nomatch-*\n\
Error: deployed skill matching positional pattern not found: zzz-nomatch-*\n"
    );
    assert!(
        stdout_of(output).is_empty(),
        "no plan is rendered for a NotFound pattern"
    );
}

/// Bare `remove` (no operands) still means "every discovered source
/// winner," and the plan is what makes that blast radius visible before
/// anything is asked or applied — run from a cwd where project scope is not
/// reachable, so `teach` resolves unambiguously alongside the rest.
#[test]
fn remove_bare_invocation_shows_the_full_blast_radius_before_applying() {
    let (home, _project) = remove_originating_scenario_fixture();
    let output = cli(home.path())
        .args(["remove", "--yes"])
        .output()
        .expect("run bare remove --yes");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output),
        "\
Remove plan

skill       files/deploy  claude  shared
----------  ------------  ------  ------
other-one   1             remove  remove
other-two   1             remove  remove
solo-skill  1             remove  none
teach       1             remove  remove

7 deployment removals across 2 targets: 7 remove; 4 skills, 7 files

Removed other-one from claude (global)
Removed other-one from shared (global)
Removed other-two from claude (global)
Removed other-two from shared (global)
Removed solo-skill from claude (global)
Removed teach from claude (global)
Removed teach from shared (global)

completed: 7 deployments removed (4 skills, 7 files)
"
    );
}

/// Plan order must equal apply order: reversing the requested names reverses
/// both the rendered rows and the applied progress lines identically.
#[test]
fn remove_reviews_and_applies_skills_in_the_order_they_were_requested() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "zebra-skill", "# zebra");
    create_skill(&source, "alpha-skill", "# alpha");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--claude", "--global"])
        .assert()
        .success();

    let output = cli(home.path())
        .args([
            "remove",
            "zebra-skill",
            "alpha-skill",
            "--claude",
            "--global",
            "--yes",
        ])
        .output()
        .expect("run reversed-order remove");
    assert!(output.status.success());
    let stdout = stdout_of(output);
    let rows = stdout
        .lines()
        .filter(|line| line.contains("1             remove"))
        .map(|line| line.split_whitespace().next().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        ["zebra-skill", "alpha-skill"],
        "reviewing in request order is not alphabetical order:\n{stdout}"
    );
    let applied = stdout
        .lines()
        .filter(|line| line.starts_with("Removed "))
        .collect::<Vec<_>>();
    assert_eq!(
        applied,
        [
            "Removed zebra-skill from claude (global)",
            "Removed alpha-skill from claude (global)",
        ],
        "apply must honour the order the plan promised:\n{stdout}"
    );
}

/// A terminal user reviews the branch plan in the symbol vocabulary, in
/// color, and a dry run still enumerates alternatives without a cancel row.
#[test]
fn remove_renders_the_interactive_symbol_and_color_branch_plan_for_a_terminal_user() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let stdout = stdout_of(
        remove
            .env_remove("NO_COLOR")
            .env("SKILL_MANAGER_FORCE_INTERACTIVE", "1")
            .args([
                "remove",
                "teach",
                "--claude",
                "--shared",
                "--color",
                "always",
                "--dry-run",
            ])
            .output()
            .expect("run interactive dry-run remove"),
    );
    assert_eq!(
        stdout,
        "\u{1b}[1;36mRemove plan\u{1b}[0m\n\
\n\
\u{1b}[1;36mAvailable deployments\u{1b}[0m\n\
\n\
skill  files/deploy  claude  shared\n\
-----  ------------  ------  ------\n\
teach  1             ↕ both  ↕ both\n\
\n\
\x20\x201  Remove project copies  \u{1b}[31m− 2 deployments, 2 files\u{1b}[0m\n\
\x20\x202  Remove global copies   \u{1b}[31m− 2 deployments, 2 files\u{1b}[0m\n\
\x20\x203  Remove both copies     \u{1b}[31m− 4 deployments, 4 files\u{1b}[0m\n\
\n\
Dry run — 3 alternatives shown; no option selected and no changes were made.\n"
    );
}

/// The same terminal user reviewing an explicit, unambiguous scope sees the
/// plain action table in the symbol vocabulary: a bare `−` (colored) with no
/// location suffix, since the destination no longer varies across scopes.
#[test]
fn remove_renders_the_interactive_symbol_and_color_collapsed_plan_for_a_terminal_user() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let stdout = stdout_of(
        remove
            .env_remove("NO_COLOR")
            .env("SKILL_MANAGER_FORCE_INTERACTIVE", "1")
            .args([
                "remove",
                "teach",
                "--claude",
                "--shared",
                "--global",
                "--color",
                "always",
                "--dry-run",
            ])
            .output()
            .expect("run interactive collapsed dry-run remove"),
    );
    assert_eq!(
        stdout,
        "\u{1b}[1;36mRemove plan\u{1b}[0m\n\
\n\
skill  files/deploy  claude  shared\n\
-----  ------------  ------  ------\n\
teach  1             \u{1b}[31m−\u{1b}[0m       \u{1b}[31m−\u{1b}[0m\n\
\n\
2 deployment removals across 2 selected targets: \u{1b}[31m− 2 remove\u{1b}[0m; 1 skill, 2 files\n\
\n\
Dry run — no changes were made.\n"
    );
}

/// The NDJSON stream carries a single `plan` event at revision 0, ahead of
/// every write, whose `decisions[0].options` carry each alternative's own
/// typed consequence (`operation` and `totals`) — never gated columns, and
/// never a bare count.
#[test]
fn remove_emits_a_structured_plan_event_with_per_option_consequences_before_applying() {
    let (home, project) = remove_ambiguous_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let events = json_events(
        remove
            .args([
                "--json",
                "remove",
                "teach",
                "--claude",
                "--shared",
                "--dry-run",
            ])
            .output()
            .expect("run machine ambiguous dry-run remove"),
    );
    let plans = events_of(&events, "plan");
    assert_eq!(plans.len(), 1, "one revision was reviewed");
    let data = plans[0]["data"].clone();
    assert_eq!(data["plan_id"], "remove:teach");
    assert_eq!(data["revision"], 0);
    assert_eq!(data["command"], "remove");
    assert_eq!(data["dry_run"], true);
    assert_eq!(data["authorization"]["kind"], "selection");
    assert_eq!(data["authorization"]["mode"], "dry-run");
    assert_eq!(data["selection"]["targets"]["mode"], "explicit");
    assert_eq!(
        data["selection"]["targets"]["names"],
        serde_json::json!(["claude", "shared"])
    );
    assert_eq!(
        data["selection"]["scope"],
        serde_json::json!({ "mode": "inferred" }),
        "an unresolved branch reports scope as inferred, with no value yet chosen"
    );
    assert_eq!(data["entries"][0]["skill"], "teach");
    assert_eq!(
        data["entries"][0]["available"],
        serde_json::json!([
            "claude:global",
            "claude:project",
            "shared:global",
            "shared:project"
        ]),
        "availability is evidence, never an action, while the branch is open"
    );
    assert_eq!(
        data["entries"][0]["actions"],
        serde_json::json!([]),
        "no action is machine-recorded until a scope is actually chosen"
    );
    let decision = data["decisions"][0].clone();
    assert_eq!(decision["id"], "removal_scope");
    let options = decision["options"].as_array().expect("decision options");
    assert_eq!(options.len(), 3);
    assert_eq!(options[0]["id"], "project");
    assert_eq!(options[0]["token"], "1");
    assert_eq!(
        options[0]["consequence"],
        serde_json::json!({ "operation": "remove", "totals": { "deployments": 2, "files": 2 } })
    );
    assert_eq!(options[1]["id"], "global");
    assert_eq!(options[1]["token"], "2");
    assert_eq!(
        options[1]["consequence"],
        serde_json::json!({ "operation": "remove", "totals": { "deployments": 2, "files": 2 } })
    );
    assert_eq!(options[2]["id"], "both");
    assert_eq!(options[2]["token"], "3");
    assert_eq!(
        options[2]["consequence"],
        serde_json::json!({ "operation": "remove", "totals": { "deployments": 4, "files": 4 } }),
        "every alternative's own blast radius travels with it in the machine stream"
    );
}

/// `drifted` deployed to `claude`+`shared` at both scopes, with every one of
/// its four deployments carrying a genuinely different file count (1, 2, 3,
/// and 1 files respectively). This is the regression fixture for the
/// blast-radius-understatement defect: a per-skill count borrowed from
/// "whichever copy discovery found first" and then multiplied across cells
/// would report the same number for every option regardless of which real
/// deployments that option actually deletes. Divergence spans both scope
/// (global vs. project) and target (claude vs. shared) so neither axis alone
/// could hide a regression.
fn remove_divergent_deployments_fixture() -> (TempDir, PathBuf) {
    let home = sandbox();
    let source = home.path().join("source");
    let project = home.path().join("project");
    fs::create_dir_all(&project).expect("create project directory");
    create_skill(&source, "drifted", "# drifted");
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            source.to_str().expect("utf8 source"),
            "primary",
        ])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json", "load", "drifted", "--claude", "--shared", "--global",
        ])
        .assert()
        .success();
    // Global shared gains one extra file beyond the one every deployment
    // starts with: 2 files.
    fs::write(home.path().join(".agents/skills/drifted/extra.md"), "extra")
        .expect("drift global shared deployment");
    let mut project_load = cli(home.path());
    project_load.current_dir(&project);
    project_load
        .args([
            "--json",
            "load",
            "drifted",
            "--claude",
            "--shared",
            "--project",
        ])
        .assert()
        .success();
    // Project claude gains two extra files beyond its starting one: 3 files.
    // Project shared and global claude are left at their starting 1 file
    // each, so every one of the four deployments ends up with its own
    // distinct count: global claude 1, global shared 2, project claude 3,
    // project shared 1.
    fs::write(project.join(".claude/skills/drifted/a.md"), "a").expect("drift project claude");
    fs::write(project.join(".claude/skills/drifted/b.md"), "b").expect("drift project claude");
    (home, project)
}

/// Each branch option's advertised blast radius is derived from that exact
/// option's real apply list, never from a representative per-skill count
/// multiplied across cells — so it stays correct once deployments have
/// genuinely drifted apart across both scope and target. Project totals
/// (project claude 3 + project shared 1 = 4 files) and global totals (global
/// claude 1 + global shared 2 = 3 files) differ from each other and from
/// what a flawed "first discovered copy" count would have reported for
/// either.
#[test]
fn remove_scope_alternatives_report_the_true_blast_radius_when_deployments_have_drifted_apart() {
    let (home, project) = remove_divergent_deployments_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args(["remove", "drifted", "--claude", "--shared", "--dry-run"])
        .output()
        .expect("run divergent dry-run remove");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output.clone()),
        "Remove plan\n\
\n\
Available deployments\n\
\n\
skill    files/deploy  claude  shared\n\
-------  ------------  ------  ------\n\
drifted  1-3           both    both\n\
\n\
\x20\x201  Remove project copies  − 2 deployments, 4 files\n\
\x20\x202  Remove global copies   − 2 deployments, 3 files\n\
\x20\x203  Remove both copies     − 4 deployments, 7 files\n\
\n\
Dry run — 3 alternatives shown; no option selected and no changes were made.\n"
    );
    assert!(stderr_of(&output).is_empty(), "a dry run never prompts");
}

/// Selecting the project alternative deletes exactly the files that
/// alternative's own blast radius promised — project claude's 3 files and
/// project shared's 1 file, 4 total — and leaves both global deployments,
/// with their own different counts, completely untouched. The post-apply
/// result footer's file count matches the pre-apply option's promise exactly
/// because both are derived from the same real apply list.
#[test]
fn remove_selecting_an_option_deletes_exactly_the_files_its_blast_radius_promised() {
    let (home, project) = remove_divergent_deployments_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let output = remove
        .args(["remove", "drifted", "--claude", "--shared"])
        .write_stdin("1\n")
        .output()
        .expect("run divergent option-1 remove");
    assert!(output.status.success());
    assert_eq!(
        stdout_of(output.clone()),
        "Remove plan\n\
\n\
Available deployments\n\
\n\
skill    files/deploy  claude  shared\n\
-------  ------------  ------  ------\n\
drifted  1-3           both    both\n\
\n\
\x20\x201  Remove project copies  − 2 deployments, 4 files\n\
\x20\x202  Remove global copies   − 2 deployments, 3 files\n\
\x20\x203  Remove both copies     − 4 deployments, 7 files\n\
\x20\x20c  Cancel\n\
\n\
\n\
Removed drifted from claude (project)\n\
Removed drifted from shared (project)\n\
\n\
completed: 2 deployments removed (1 skill, 4 files)\n"
    );
    assert_eq!(
        stderr_of(&output),
        "Select removal scope [1-3, c to cancel]: "
    );
    assert!(
        !project.join(".claude/skills/drifted").exists(),
        "the chosen project claude deployment (3 files) must be gone"
    );
    assert!(
        !project.join(".agents/skills/drifted").exists(),
        "the chosen project shared deployment (1 file) must be gone"
    );
    let global_claude = home.path().join(".claude/skills/drifted");
    let global_shared = home.path().join(".agents/skills/drifted");
    assert!(
        global_claude.join("SKILL.md").is_file() && !global_claude.join("a.md").exists(),
        "global claude's own 1-file deployment must be untouched"
    );
    assert!(
        global_shared.join("SKILL.md").is_file() && global_shared.join("extra.md").is_file(),
        "global shared's own 2-file deployment must be untouched"
    );
}

/// The `plan` event's per-option consequences carry the same true,
/// per-option totals as the rendered table and the eventual apply — proving
/// the machine stream cannot drift from the human rendering or from reality
/// even when deployments have genuinely diverged across scope and target.
#[test]
fn remove_plan_event_reports_true_per_option_totals_when_deployments_have_drifted_apart() {
    let (home, project) = remove_divergent_deployments_fixture();
    let mut remove = cli(home.path());
    remove.current_dir(&project);
    let events = json_events(
        remove
            .args([
                "--json",
                "remove",
                "drifted",
                "--claude",
                "--shared",
                "--dry-run",
            ])
            .output()
            .expect("run machine divergent dry-run remove"),
    );
    let plans = events_of(&events, "plan");
    assert_eq!(plans.len(), 1);
    let data = plans[0]["data"].clone();
    let decision = data["decisions"][0].clone();
    let options = decision["options"].as_array().expect("decision options");
    assert_eq!(options[0]["id"], "project");
    assert_eq!(
        options[0]["consequence"],
        serde_json::json!({ "operation": "remove", "totals": { "deployments": 2, "files": 4 } }),
        "project claude (3 files) + project shared (1 file) = 4, not a borrowed representative count"
    );
    assert_eq!(options[1]["id"], "global");
    assert_eq!(
        options[1]["consequence"],
        serde_json::json!({ "operation": "remove", "totals": { "deployments": 2, "files": 3 } }),
        "global claude (1 file) + global shared (2 files) = 3, genuinely different from the project total"
    );
    assert_eq!(options[2]["id"], "both");
    assert_eq!(
        options[2]["consequence"],
        serde_json::json!({ "operation": "remove", "totals": { "deployments": 4, "files": 7 } }),
        "every deployment's own count summed: 1 + 2 + 3 + 1 = 7"
    );
}
