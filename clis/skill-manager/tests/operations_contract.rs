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
use skill_manager::config::acquire_lock;
use tempfile::TempDir;

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

fn read_config(home: &Path) -> Value {
    let bytes =
        fs::read(home.join(".skill-manager.config.json")).expect("read generated config file");
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
    assert_eq!(added["schema_version"], 1);
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
        PathBuf::from(
            config["sources"][0]["path"]
                .as_str()
                .expect("stored local source path")
        )
        .canonicalize()
        .expect("canonical stored source"),
        home.path().canonicalize().expect("canonical sandbox")
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
        home.path().join(".skill-manager.config.json"),
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
    let first = home.path().join("first-target");
    let second = home.path().join("second-target");

    cli(home.path())
        .args([
            "--json",
            "target",
            "add",
            "custom",
            first.to_str().expect("utf8 path"),
        ])
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
        .args([
            "--json",
            "target",
            "set-path",
            "custom",
            second.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();
    assert_eq!(
        read_config(home.path())["targets"]["custom"]["path"],
        second.to_str().expect("utf8 path")
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
        ])
        .assert()
        .success();
    assert!(home.path().join(".claude/skills/alpha/SKILL.md").is_file());
}

#[test]
fn explicit_named_and_builtin_target_selectors_form_a_deduplicated_union() {
    let home = sandbox();
    let source = home.path().join("source");
    let custom = home.path().join("custom");
    create_skill(&source, "alpha", "# Alpha");
    cli(home.path())
        .args([
            "--json",
            "target",
            "add",
            "custom",
            custom.to_str().expect("utf8 path"),
        ])
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
        .args([
            "--json",
            "target",
            "add",
            "test-target",
            managed_target.to_str().expect("utf8 path"),
        ])
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
        .args(["--json", "load", "--target", "test-target", "--no-input"])
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
        .args([
            "--json",
            "target",
            "add",
            "custom",
            target.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();

    let loaded = json_events(
        cli(home.path())
            .args(["--json", "load", "--target", "custom"])
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
            .args(["--json", "load", "--target", "custom"])
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
    assert!(updated.iter().any(|event| {
        event["event"] == "skill.skipped"
            && event["data"]["skill"] == "beta"
            && event["data"]["action"] == "skipped"
    }));

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
            .args(["--json", "remove", "alpha", "--target", "custom", "--yes"])
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
                path.to_str().expect("utf8 path"),
            ])
            .assert()
            .success();
    }
    cli(home.path())
        .args(["--json", "load", "--target", "one"])
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
    let target = home.path().join("target");
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
        .args([
            "--json",
            "target",
            "add",
            "custom",
            target.to_str().expect("utf8 path"),
        ])
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
fn human_status_renders_sources_header_rows_summary_and_empty_state() {
    let home = sandbox();
    let source = home.path().join("source");
    let target = home.path().join("target");
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
    cli(home.path())
        .args([
            "--json",
            "target",
            "add",
            "custom",
            target.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();

    cli(home.path())
        .args(["status", "--target", "custom"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sources:"))
        .stdout(predicate::str::contains("primary"))
        .stdout(predicate::str::contains("(Primary Label)"))
        .stdout(predicate::str::contains("skill\tsource\tcustom"))
        .stdout(predicate::str::contains("alpha\t"))
        .stdout(predicate::str::contains("custom:not-loaded"))
        .stdout(predicate::str::contains(
            "Summary: up-to-date: 0, needs-update: 0, not-loaded: 1, no-connection: 0",
        ))
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
fn dry_run_never_writes_deployments_or_configuration() {
    let home = sandbox();
    let source = home.path().join("source");
    let target = home.path().join("target");
    create_skill(&source, "alpha", "# Alpha");

    cli(home.path())
        .args([
            "--json",
            "target",
            "add",
            "custom",
            target.to_str().expect("utf8 path"),
        ])
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
    let config_before =
        fs::read(home.path().join(".skill-manager.config.json")).expect("read config");

    cli(home.path())
        .args(["--json", "load", "--target", "custom", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"dry_run\":true"));

    assert!(!target.exists());
    assert_eq!(
        fs::read(home.path().join(".skill-manager.config.json")).expect("read config again"),
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
        .args([
            "--json",
            "target",
            "add",
            "custom",
            target.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();

    cli(home.path())
        .args([
            "--json",
            "load",
            "--target",
            "custom",
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
            "--json", "load", "--target", "custom", "--no-cd", "--filter", "alpha",
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
        .args([
            "--json",
            "target",
            "add",
            "custom",
            managed_target.to_str().expect("utf8 path"),
        ])
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
        .args(["--json", "load", "--target", "custom"])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json",
            "remove",
            "alpha",
            "--target",
            "custom",
            "--yes",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"dry_run\":true"));
    assert!(managed_target.join("alpha/SKILL.md").is_file());
    cli(home.path())
        .args(["remove", "alpha", "--target", "custom", "--no-input"])
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
    assert!(home.path().join(".skill-manager.config.json").exists());
    assert!(
        home.path()
            .join(".skill-manager.config.json.v0.bak")
            .exists()
    );

    let dry_home = sandbox();
    let dry_legacy = dry_home.path().join(".skills-syncer.config.json");
    fs::write(&dry_legacy, "{}").expect("write dry-run legacy config");
    cli(dry_home.path())
        .args(["--json", "load", "--all", "--dry-run"])
        .assert()
        .success();
    assert!(dry_legacy.exists());
    assert!(!dry_home.path().join(".skill-manager.config.json").exists());
    assert!(
        !dry_home
            .path()
            .join(".skill-manager.config.json.v0.bak")
            .exists()
    );
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
    create_skill(
        &recipe_dir.join("relative-source"),
        "from-recipe",
        "# rebased",
    );
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
        fs::read_to_string(home.path().join(".skill-manager.config.json"))
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
        assert_eq!(fs::read(&path).expect("read unchanged config"), raw);
        assert!(
            !home
                .path()
                .join(".skill-manager.config.json.v0.bak")
                .exists()
        );
    }
}

#[test]
fn completion_and_man_generation_hooks_produce_installable_assets() {
    let home = sandbox();
    for shell in ["bash", "zsh", "fish", "powershell"] {
        cli(home.path())
            .args(["generate-completions", "--shell", shell])
            .assert()
            .success()
            .stdout(predicate::str::contains("skill-manager"));
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
    let target = home.path().join("target");
    cli(home.path())
        .args([
            "--json",
            "target",
            "add",
            "custom",
            target.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();

    let mut colored = cli(home.path());
    colored.env_remove("NO_COLOR");
    colored
        .args(["--color", "always", "target", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[36m"))
        .stderr(predicate::str::is_empty());

    let mut plain = cli(home.path());
    plain.env_remove("NO_COLOR");
    plain
        .args(["--color", "never", "target", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[").not());

    fs::write(home.path().join(".skill-manager.config.json"), "{broken")
        .expect("write malformed config");
    let mut diagnostic = cli(home.path());
    diagnostic.env_remove("NO_COLOR");
    diagnostic
        .args(["--color", "always", "status"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("\u{1b}[31mError:"));
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
                path.to_str().expect("utf8 path"),
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
    let lock_path = home.path().join(".skill-manager-cache/.locks/config.lock");
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
    assert!(!home.path().join(".skill-manager.config.json").exists());
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

    let changed = home.path().join("legacy-claude");
    cli(home.path())
        .args([
            "--json",
            "target",
            "set-path",
            "claude",
            changed.to_str().expect("utf8 path"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"target.path-set\""));
    assert_eq!(
        read_config(home.path())["legacy_target_overrides"]["claude"]["path"],
        changed.to_str().expect("utf8 path")
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
    assert_eq!(
        migrated["legacy_target_overrides"]["claude"]["path"],
        legacy_path.to_str().expect("utf8 path")
    );

    let changed_path = home.path().join("changed-mixed-case-claude");
    cli(home.path())
        .args([
            "--json",
            "target",
            "set-path",
            "CLAUDE",
            changed_path.to_str().expect("utf8 path"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"target.path-set\""));
    assert_eq!(
        read_config(home.path())["legacy_target_overrides"]["claude"]["path"],
        changed_path.to_str().expect("utf8 path")
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
            "common",
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
        .success();

    cli(home.path())
        .args(["load", "--filter", "alpha"])
        .write_stdin("perhaps\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("expected 'yes' or 'no'"));

    let target = home.path().join("prompt-target");
    cli(home.path())
        .args([
            "--json",
            "target",
            "add",
            "prompt-target",
            target.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();
    cli(home.path())
        .args(["--json", "load", "--target", "prompt-target"])
        .assert()
        .success();

    cli(home.path())
        .args(["remove", "alpha", "--target", "prompt-target"])
        .write_stdin("n\n")
        .assert()
        .success()
        .stderr(predicate::str::contains("Remove 1 skill deployment(s)?"));
    assert!(target.join("alpha/SKILL.md").is_file());

    cli(home.path())
        .args(["remove", "alpha", "--target", "prompt-target"])
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
        .args([
            "--json",
            "target",
            "add",
            "managed",
            managed.to_str().expect("utf8 path"),
        ])
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
    let target = home.path().join("target");
    cli(home.path())
        .args([
            "--json",
            "target",
            "add",
            "custom",
            target.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json",
            "target",
            "add",
            "custom",
            home.path().to_str().expect("utf8 path"),
        ])
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
