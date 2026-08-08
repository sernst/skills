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
use serde_json::Value;
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
        .stderr(predicate::str::contains("Use all 3 enabled target(s)?"));
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
            "target selection is required in noninteractive mode",
        ));
    cli(home.path())
        .args(["load", "--filter", "alpha", "--no-input", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("claude (global) (dry-run)"));

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
        .stderr(predicate::str::contains("Remove 1 skill deployment(s)?"));
    assert!(target.join("alpha/SKILL.md").is_file());

    cli(home.path())
        .args(["remove", "alpha", "--target", "prompt-target", "--global"])
        .write_stdin("y\n")
        .assert()
        .success()
        .stderr(predicate::str::contains("Remove 1 skill deployment(s)?"));
    assert!(!target.join("alpha").exists());
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
        .stdout(predicate::str::contains("--global or --project"));

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
fn load_scope_prompt_uses_exact_cwd_vendor_directory_as_its_default() {
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

    let mut project_load = cli(home.path());
    project_load.current_dir(&project);
    project_load
        .args(["load", "--shared", "--filter", "alpha"])
        .write_stdin("\n")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Install skills at project scope? [Y/n]",
        ));
    assert!(project.join(".agents/skills/alpha/SKILL.md").is_file());

    let mut global_load = cli(home.path());
    global_load.current_dir(&other_project);
    global_load
        .args(["load", "--shared", "--filter", "alpha"])
        .write_stdin("\n")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Install skills at project scope? [y/N]",
        ));
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

/// A single changed deployment is adopted as the new source content in full.
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "One end-to-end import contract keeps the idle, dry-run, and committed phases auditable together."
)]
fn import_mirrors_one_changed_deployment_and_dry_runs_change_nothing() {
    let home = sandbox();
    let source = home.path().join("source");
    let skill = create_skill(&source, "alpha", "# Alpha\nline two\n");
    fs::create_dir_all(skill.join("reference")).expect("create source reference directory");
    fs::write(skill.join("reference/stale.md"), "stale\n").expect("write stale reference");

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

    let idle = json_events(
        cli(home.path())
            .args([
                "--json",
                "import",
                "alpha",
                "--claude",
                "--global",
                "--yes",
                "--no-input",
            ])
            .output()
            .expect("run idle import"),
    );
    assert_eq!(events_of(&idle, "skill.import-skipped").len(), 1);
    assert!(events_of(&idle, "skill.imported").is_empty());
    let summary = events_of(&idle, "summary")[0]["data"].clone();
    assert_eq!(summary["action"], "import");
    assert_eq!(summary["imported"], 0);
    assert_eq!(summary["skipped"], 1);

    let deployed = home.path().join(".claude/skills/alpha");
    fs::write(
        deployed.join("SKILL.md"),
        "# Alpha\nline two edited\nline three\n",
    )
    .expect("agent edits the deployed skill");
    fs::remove_file(deployed.join("reference/stale.md")).expect("agent deletes a reference");
    fs::write(deployed.join("reference/new.md"), "fresh\n").expect("agent adds a reference");

    let config_before = fs::read(home.path().join(".skill-manager/config.json"))
        .expect("read configuration before dry run");
    let planned = json_events(
        cli(home.path())
            .args([
                "--json",
                "import",
                "alpha",
                "--claude",
                "--global",
                "--yes",
                "--no-input",
                "--dry-run",
            ])
            .output()
            .expect("run dry-run import"),
    );
    let plan = events_of(&planned, "skill.import-planned");
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0]["data"]["dry_run"], true);
    assert_eq!(plan[0]["data"]["files_changed"], 3);
    assert_eq!(plan[0]["data"]["target"], "claude");
    assert_eq!(plan[0]["data"]["scope"], "global");
    assert_eq!(
        events_of(&planned, "skill.imported")[0]["data"]["dry_run"],
        true
    );
    assert_eq!(
        fs::read_to_string(skill.join("SKILL.md")).expect("source is untouched by a dry run"),
        "# Alpha\nline two\n"
    );
    assert!(skill.join("reference/stale.md").is_file());
    assert!(!skill.join("reference/new.md").exists());
    assert_eq!(
        fs::read(home.path().join(".skill-manager/config.json"))
            .expect("read configuration after dry run"),
        config_before
    );

    let committed = json_events(
        cli(home.path())
            .args([
                "--json",
                "import",
                "alpha",
                "--claude",
                "--global",
                "--yes",
                "--no-input",
            ])
            .output()
            .expect("run committed import"),
    );
    let imported = events_of(&committed, "skill.imported");
    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0]["data"]["action"], "imported");
    assert_eq!(imported[0]["data"]["dry_run"], false);
    assert_eq!(
        fs::read_to_string(skill.join("SKILL.md")).expect("imported source content"),
        "# Alpha\nline two edited\nline three\n"
    );
    assert_eq!(
        fs::read_to_string(skill.join("reference/new.md")).expect("imported new reference"),
        "fresh\n"
    );
    assert!(
        !skill.join("reference/stale.md").exists(),
        "import must mirror deletions into the source"
    );
    assert!(!source.join(".skill-manager-journals").exists());
    assert!(!source.join(".skill-manager-staging").exists());

    cli(home.path())
        .args([
            "--json",
            "import",
            "alpha",
            "--claude",
            "--global",
            "--yes",
            "--no-input",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"event\":\"skill.import-skipped\"",
        ));
}

/// Import renders a reviewable plain-text plan and accepts recipe invocation.
#[test]
fn import_renders_a_plain_plan_and_runs_from_a_recipe() {
    let home = sandbox();
    let source = home.path().join("source");
    let skill = create_skill(&source, "alpha", "# Alpha\n");
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
    let deployed = home.path().join(".claude/skills/alpha");
    fs::write(deployed.join("SKILL.md"), "# Alpha\nsecond line\n").expect("agent edit");
    fs::write(deployed.join("logo.bin"), [0_u8, 1, 2, 3]).expect("agent adds binary content");

    let output = cli(home.path())
        .args([
            "import",
            "alpha",
            "--claude",
            "--global",
            "--no-input",
            "--dry-run",
        ])
        .output()
        .expect("run human dry-run import");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 plan");
    assert!(stdout.contains("Import alpha"));
    assert!(stdout.contains("From   claude · global"));
    assert!(stdout.contains("Into   Primary (source)"));
    assert!(stdout.contains("Changes"));
    assert!(stdout.contains("modified  SKILL.md"));
    assert!(stdout.contains("added     logo.bin"));
    assert!(stdout.contains("bin +4 bytes"));
    assert!(stdout.contains("2 files changed, +1/-0, 1 binary"));
    assert!(!stdout.contains('\u{1b}'), "plain output must be ANSI-free");
    assert!(!stdout.contains('\u{2190}'), "plain output uses words");
    assert!(
        !stdout.contains(&source.display().to_string()),
        "default import output must hide the source collection path: {stdout}"
    );
    assert!(
        !stdout.contains(&skill.display().to_string()),
        "default import output must hide the source path: {stdout}"
    );
    assert!(
        !stdout.contains(&deployed.display().to_string()),
        "default import output must hide the deployment path: {stdout}"
    );

    let recipe = serde_json::json!({
        "command": "import",
        "skill": "alpha",
        "claude": true,
        "global": true,
        "yes": true
    });
    cli(home.path())
        .args(["--json-input"])
        .write_stdin(recipe.to_string())
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"skill.imported\""))
        .stdout(predicate::str::contains("\"action\":\"import\""));
    assert_eq!(
        fs::read_to_string(skill.join("SKILL.md")).expect("recipe import applied"),
        "# Alpha\nsecond line\n"
    );
    assert!(skill.join("logo.bin").is_file());
}

/// Several changed deployments require selection, which recipes cannot supply.
#[test]
fn import_requires_narrowing_when_several_deployments_changed() {
    let home = sandbox();
    let source = home.path().join("source");
    let skill = create_skill(&source, "alpha", "# Alpha\n");
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
            "load",
            "--claude",
            "--shared",
            "--global",
            "--no-input",
        ])
        .assert()
        .success();
    fs::write(
        home.path().join(".claude/skills/alpha/SKILL.md"),
        "# Alpha claude\n",
    )
    .expect("edit claude copy");
    fs::write(
        home.path().join(".agents/skills/alpha/SKILL.md"),
        "# Alpha shared\n",
    )
    .expect("edit shared copy");

    cli(home.path())
        .args([
            "--json", "import", "alpha", "--claude", "--shared", "--global", "--yes",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "2 changed deployments of alpha are importable",
        ));
    cli(home.path())
        .args(["--json", "import", "alph*", "--claude", "--global", "--yes"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("does not accept patterns"));
    cli(home.path())
        .args([
            "--json", "import", "missing", "--claude", "--global", "--yes",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("source skill"));
    assert_eq!(
        fs::read_to_string(skill.join("SKILL.md")).expect("source is untouched"),
        "# Alpha\n"
    );

    cli(home.path())
        .args(["import", "alpha", "--claude", "--shared", "--global"])
        .write_stdin("2\ny\n")
        .assert()
        .success()
        .stderr(predicate::str::contains("Choose the alpha copy to import"))
        .stderr(predicate::str::contains("shared · global"));
    assert_eq!(
        fs::read_to_string(skill.join("SKILL.md")).expect("selected copy is imported"),
        "# Alpha shared\n"
    );

    fs::write(
        home.path().join(".claude/skills/alpha/SKILL.md"),
        "# Alpha declined\n",
    )
    .expect("create another import candidate");
    cli(home.path())
        .args(["import", "alpha", "--claude", "--global"])
        .write_stdin("n\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Cancelled."));
    assert_eq!(
        fs::read_to_string(skill.join("SKILL.md")).expect("declined import changes nothing"),
        "# Alpha shared\n"
    );
}

/// GitHub-backed sources import only into a confirmed local alternate location.
#[test]
fn import_into_a_github_source_requires_a_confirmed_local_alternate() {
    let home = sandbox();
    cli(home.path())
        .args(["--json", "source", "add", "acme/skills", "remote"])
        .assert()
        .success();
    seed_github_cache(home.path(), "alpha", "# Alpha remote\n");
    cli(home.path())
        .args(["--json", "load", "--claude", "--global", "--no-input"])
        .assert()
        .success();
    let idle = cli(home.path())
        .args(["import", "alpha", "--claude", "--global", "--yes"])
        .output()
        .expect("run idle import against a GitHub source");
    assert!(idle.status.success());
    let idle_stdout = String::from_utf8(idle.stdout).expect("utf8 idle output");
    let idle_stderr = String::from_utf8(idle.stderr).expect("utf8 idle diagnostics");
    assert!(idle_stdout.contains("Nothing to import"));
    assert!(
        !idle_stdout.contains("GitHub-backed") && !idle_stderr.contains("GitHub-backed"),
        "an up-to-date GitHub source must not ask about a destination it will not write"
    );

    fs::write(
        home.path().join(".claude/skills/alpha/SKILL.md"),
        "# Alpha remote\nagent addition\n",
    )
    .expect("agent edits the deployed remote skill");

    cli(home.path())
        .args(["--json", "import", "alpha", "--claude", "--global", "--yes"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "import writes to local source checkouts only",
        ))
        .stdout(predicate::str::contains("source alternate remote"));

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

    cli(home.path())
        .args(["--json", "import", "alpha", "--claude", "--global", "--yes"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "requires interactive confirmation",
        ));
    assert!(!checkout.join("alpha").exists());

    cli(home.path())
        .args(["import", "alpha", "--claude", "--global", "--yes"])
        .write_stdin("n\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Cancelled."))
        .stderr(predicate::str::contains("is GitHub-backed (acme/skills)"));
    assert!(!checkout.join("alpha").exists());

    cli(home.path())
        .args(["import", "alpha", "--claude", "--global", "--yes"])
        .write_stdin("y\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Imported alpha"));
    assert_eq!(
        fs::read_to_string(checkout.join("alpha/SKILL.md")).expect("alternate receives the import"),
        "# Alpha remote\nagent addition\n"
    );
}

/// Displayed and reported source paths never leak Windows verbatim prefixes.
#[test]
fn import_paths_are_reported_without_verbatim_prefixes() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Alpha\n");
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
        "# Alpha\nedited\n",
    )
    .expect("agent edit");

    let expected = portable_canonicalize(source.join("alpha")).expect("canonical source skill");
    let expected_deployment = portable_canonicalize(home.path().join(".claude/skills/alpha"))
        .expect("canonical deployment");
    let human = cli(home.path())
        .args([
            "import",
            "alpha",
            "--claude",
            "--global",
            "--yes",
            "--no-input",
            "--verbose",
        ])
        .output()
        .expect("run human import");
    assert!(human.status.success());
    let stdout = String::from_utf8(human.stdout).expect("utf8 import output");
    assert!(stdout.contains("Imported alpha"));
    assert!(
        !stdout.contains(VERBATIM_PREFIX),
        "human paths must not use verbatim spellings: {stdout}"
    );
    assert!(stdout.contains(&expected.display().to_string()));
    assert!(stdout.contains(&expected_deployment.display().to_string()));

    fs::write(
        home.path().join(".claude/skills/alpha/SKILL.md"),
        "# Alpha\nedited twice\n",
    )
    .expect("agent edit again");
    let events = json_events(
        cli(home.path())
            .args(["--json", "import", "alpha", "--claude", "--global", "--yes"])
            .output()
            .expect("run machine import"),
    );
    let destination = events_of(&events, "skill.imported")[0]["data"]["destination"]
        .as_str()
        .expect("reported destination path")
        .to_owned();
    assert!(!destination.contains(VERBATIM_PREFIX), "{destination}");
    assert_eq!(PathBuf::from(destination), expected);
    let imported = events_of(&events, "skill.imported")[0];
    assert_eq!(
        portable_canonicalize(Path::new(
            imported["data"]["path"].as_str().expect("source path"),
        ))
        .expect("canonical event source path"),
        expected
    );
    assert_eq!(
        portable_canonicalize(Path::new(
            imported["data"]["deployment"]
                .as_str()
                .expect("deployment path")
        ))
        .expect("canonical event deployment path"),
        expected_deployment
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
        .stdout(predicate::str::contains("skill  change"))
        .stdout(predicate::str::contains("alpha"))
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
    assert!(stdout.contains("Updated alpha -> claude (global)"));
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
    assert!(import_stdout.contains("From   claude · global"));
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
fn import_can_sync_other_installed_targets_after_explicit_review() {
    let home = sandbox();
    let source = home.path().join("source");
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
    fs::write(
        home.path().join(".claude/skills/alpha/SKILL.md"),
        "# Imported\n",
    )
    .expect("edit import source");

    let output = cli(home.path())
        .args(["import", "alpha", "--claude", "--global"])
        .write_stdin("y\n\n\n")
        .output()
        .expect("accept import and sync");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 import output");
    let stderr = String::from_utf8(output.stderr).expect("utf8 prompts");
    assert!(stderr.contains("other installed deployment needs this change"));
    assert!(stderr.contains("Apply this update plan to 3 enabled targets?"));
    assert!(stdout.contains("Update plan"));
    assert!(
        stdout.contains("\n\nUpdate plan"),
        "the embedded import follow-up plan needs a leading separator: {stdout}"
    );
    assert!(stdout.contains("Updated alpha -> shared (global)"));
    assert_eq!(
        fs::read_to_string(source_skill.join("SKILL.md")).expect("imported source"),
        "# Imported\n"
    );
    assert_eq!(
        fs::read_to_string(home.path().join(".agents/skills/alpha/SKILL.md"))
            .expect("synced shared deployment"),
        "# Imported\n"
    );
}

#[test]
fn import_syncs_the_same_targets_opposite_scope_in_both_directions() {
    let home = sandbox();
    let project = home.path().join("project");
    fs::create_dir_all(&project).expect("create project");
    let source = home.path().join("source");
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
        .args(["--json", "load", "--claude", "--global"])
        .assert()
        .success();
    let mut project_load = cli(home.path());
    project_load
        .current_dir(&project)
        .args(["--json", "load", "--claude", "--project"])
        .assert()
        .success();

    let global = home.path().join(".claude/skills/alpha/SKILL.md");
    let project_copy = project.join(".claude/skills/alpha/SKILL.md");
    fs::write(&global, "# From global\n").expect("edit global deployment");
    let mut from_global = cli(home.path());
    let output = from_global
        .current_dir(&project)
        .args(["import", "alpha", "--claude", "--global", "--yes"])
        .write_stdin("\n\n")
        .output()
        .expect("import global then sync project");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 global import output");
    assert!(stdout.contains("Updated alpha -> claude (project)"));
    assert!(!stdout.contains("Updated alpha -> claude (global)"));
    assert_eq!(stdout.matches("Updated alpha -> claude").count(), 1);
    assert_eq!(
        fs::read_to_string(source_skill.join("SKILL.md")).expect("source from global"),
        "# From global\n"
    );
    assert_eq!(
        fs::read_to_string(&project_copy).expect("project synced from global"),
        "# From global\n"
    );

    fs::write(&project_copy, "# From project\n").expect("edit project deployment");
    let mut from_project = cli(home.path());
    let output = from_project
        .current_dir(&project)
        .args(["import", "alpha", "--claude", "--project", "--yes"])
        .write_stdin("\n\n")
        .output()
        .expect("import project then sync global");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 project import output");
    assert!(stdout.contains("Updated alpha -> claude (global)"));
    assert!(!stdout.contains("Updated alpha -> claude (project)"));
    assert_eq!(stdout.matches("Updated alpha -> claude").count(), 1);
    assert_eq!(
        fs::read_to_string(source_skill.join("SKILL.md")).expect("source from project"),
        "# From project\n"
    );
    assert_eq!(
        fs::read_to_string(&global).expect("global synced from project"),
        "# From project\n"
    );
}

#[test]
fn declining_import_sync_plan_keeps_the_successful_import_unambiguous() {
    let home = sandbox();
    let source = home.path().join("source");
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
    fs::write(
        home.path().join(".claude/skills/alpha/SKILL.md"),
        "# Imported\n",
    )
    .expect("edit import source");

    let output = cli(home.path())
        .args(["import", "alpha", "--claude", "--global", "--yes"])
        .write_stdin("\nn\n")
        .output()
        .expect("decline follow-up plan application");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 import output");
    assert!(
        stdout.contains("Imported successfully; other installed deployments were not updated.")
    );
    assert!(!stdout.contains("Cancelled."));
    assert_eq!(
        fs::read_to_string(source_skill.join("SKILL.md")).expect("source was imported"),
        "# Imported\n"
    );
    assert_eq!(
        fs::read_to_string(home.path().join(".agents/skills/alpha/SKILL.md"))
            .expect("shared deployment unchanged"),
        "# Original\n"
    );
}

#[test]
fn import_follow_up_can_be_declined_and_is_skipped_noninteractively() {
    for noninteractive in [false, true] {
        let home = sandbox();
        let source = home.path().join("source");
        create_skill(&source, "alpha", "# Original\n");
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
        fs::write(
            home.path().join(".claude/skills/alpha/SKILL.md"),
            "# Imported\n",
        )
        .expect("edit import source");

        let output = if noninteractive {
            cli(home.path())
                .args([
                    "import",
                    "alpha",
                    "--claude",
                    "--global",
                    "--yes",
                    "--no-input",
                ])
                .output()
                .expect("noninteractive import")
        } else {
            cli(home.path())
                .args(["import", "alpha", "--claude", "--global", "--yes"])
                .write_stdin("n\n")
                .output()
                .expect("declined import sync")
        };
        assert!(output.status.success());
        let stderr = String::from_utf8(output.stderr).expect("utf8 prompts");
        if noninteractive {
            assert!(!stderr.contains("other installed deployment"));
        } else {
            assert!(stderr.contains("other installed deployment"));
        }
        assert_eq!(
            fs::read_to_string(home.path().join(".agents/skills/alpha/SKILL.md"))
                .expect("shared remains unchanged"),
            "# Original\n"
        );
    }
}

#[test]
fn import_follow_up_is_absent_without_an_outdated_target_and_during_dry_run() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "# Original\n");
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
    fs::write(
        home.path().join(".claude/skills/alpha/SKILL.md"),
        "# Imported\n",
    )
    .expect("edit deployment");
    let single = cli(home.path())
        .args(["import", "alpha", "--claude", "--global"])
        .write_stdin("y\n")
        .output()
        .expect("single-target import");
    assert!(single.status.success());
    assert!(
        !String::from_utf8(single.stderr)
            .expect("utf8 prompts")
            .contains("other installed deployment")
    );

    cli(home.path())
        .args(["--json", "load", "--shared", "--global"])
        .assert()
        .success();
    fs::write(
        home.path().join(".claude/skills/alpha/SKILL.md"),
        "# Dry run\n",
    )
    .expect("edit deployment again");
    let dry_run = cli(home.path())
        .args([
            "import",
            "alpha",
            "--claude",
            "--global",
            "--dry-run",
            "--no-input",
        ])
        .output()
        .expect("dry-run import");
    assert!(dry_run.status.success());
    assert!(
        !String::from_utf8(dry_run.stderr)
            .expect("utf8 dry-run diagnostics")
            .contains("other installed deployment")
    );
    assert_eq!(
        fs::read_to_string(source.join("alpha/SKILL.md")).expect("dry-run source"),
        "# Imported\n"
    );
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
    assert!(stdout.contains("antigravity"));
    assert_eq!(stdout.matches("↑ both").count(), 2);
    assert!(stdout.contains("4 updates across 3 enabled targets"));
    assert!(!stderr.contains("Use all"));

    fs::write(source_skill.join("SKILL.md"), "# Alias\n").expect("change source again");
    let mut alias = cli(home.path());
    alias.current_dir(&project);
    alias
        .args(["up", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "4 updates across 3 enabled targets",
        ));

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
        .stdout(predicate::str::contains("2 target-specific changes"))
        .stdout(predicate::str::contains(
            "claude · global  1 file changed, +2/-1",
        ))
        .stdout(predicate::str::contains(
            "shared · global  2 files changed, +2/-2",
        ));
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
        assert!(
            stdout.contains("1 update across 1 selected target\n\nUpdated alpha"),
            "unexpected update section spacing:\n{stdout}"
        );
        assert!(
            !stdout.contains("\n\n\n"),
            "duplicate blank line:\n{stdout}"
        );
        if dry_run {
            cli(home.path())
                .args(["--json", "update", "--claude", "--global"])
                .assert()
                .success();
        }
    }
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
    assert!(verbose_stdout.contains(&alternate.display().to_string()));
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
