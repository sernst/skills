//! End-to-end contract coverage for the `describe` command.

#![allow(
    clippy::expect_used,
    reason = "Fixture construction and a missing test binary are unrecoverable harness failures."
)]

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

mod support;

use support::portable_canonicalize;

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

fn create_skill(collection: &Path, name: &str, description: &str) -> PathBuf {
    let root = collection.join(name);
    fs::create_dir_all(&root).expect("create skill directory");
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
    )
    .expect("write skill");
    root
}

fn add_source(home: &Path, collection: &Path, name: &str, extra: &[&str]) {
    let mut args = vec![
        "--json",
        "source",
        "add",
        collection.to_str().expect("UTF-8 source path"),
        "--name",
        name,
    ];
    args.extend_from_slice(extra);
    cli(home).args(args).assert().success();
}

fn json_events(output: std::process::Output) -> Vec<Value> {
    assert!(output.status.success(), "command failed: {output:?}");
    String::from_utf8(output.stdout)
        .expect("UTF-8 output")
        .lines()
        .map(|line| serde_json::from_str(line).expect("NDJSON event"))
        .collect()
}

#[test]
fn skill_human_output_preserves_bounded_readme_without_ansi() {
    let home = sandbox();
    let source = home.path().join("source");
    let skill = create_skill(&source, "teach", "Teach carefully.");
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: teach\ndescription:\n  Teach\n  carefully.\n---\n\n# Teach\n",
    )
    .expect("write multiline trigger");
    let readme = (1..=105)
        .map(|line| format!("README line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(skill.join("README.md"), readme).expect("write README");
    add_source(home.path(), &source, "personal", &[]);

    cli(home.path())
        .args(["describe", "teach", "--color", "never"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Skill: teach"))
        .stdout(predicate::str::contains("Trigger  Teach carefully."))
        .stdout(predicate::str::contains("README line 100"))
        .stdout(predicate::str::contains("truncated after 100 of 105 lines"))
        .stdout(predicate::str::contains("README line 101").not())
        .stdout(predicate::str::contains("\u{1b}[").not());
}

#[test]
fn qualified_selection_exposes_excluded_and_shadowed_physical_copies() {
    let home = sandbox();
    let first = home.path().join("first");
    let second = home.path().join("second");
    create_skill(&first, "alpha", "First alpha.");
    create_skill(&first, "beta", "First beta.");
    create_skill(&second, "alpha", "Excluded alpha.");
    create_skill(&second, "beta", "Shadowed beta.");
    add_source(home.path(), &first, "first", &[]);
    add_source(home.path(), &second, "second", &["--exclude", "alpha"]);

    let events = json_events(
        cli(home.path())
            .args(["--json", "describe", "second:*"])
            .output()
            .expect("describe source-qualified skills"),
    );
    let skills = events
        .iter()
        .filter(|event| event["event"] == "describe.skill")
        .collect::<Vec<_>>();
    assert_eq!(skills.len(), 2);
    assert_eq!(skills[0]["data"]["skill"], "alpha");
    assert_eq!(skills[0]["data"]["resolver_status"], "excluded");
    assert_eq!(skills[1]["data"]["skill"], "beta");
    assert_eq!(skills[1]["data"]["resolver_status"], "shadowed");

    let narrowed = json_events(
        cli(home.path())
            .args(["--json", "describe", "beta", "--source", "second"])
            .output()
            .expect("describe source-narrowed skill"),
    );
    let narrowed_skills = narrowed
        .iter()
        .filter(|event| event["event"] == "describe.skill")
        .collect::<Vec<_>>();
    assert_eq!(narrowed_skills.len(), 1);
    assert_eq!(narrowed_skills[0]["data"]["resolver_status"], "shadowed");

    let effective = json_events(
        cli(home.path())
            .args(["--json", "describe", "--all-skills"])
            .output()
            .expect("describe effective skills"),
    );
    let described_sources = effective
        .iter()
        .filter(|event| event["event"] == "describe.skill")
        .map(|event| event["data"]["source"]["source_name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(described_sources, vec![Some("first"), Some("first")]);
}

#[test]
fn installation_filters_are_ored_and_outdated_is_still_installed() {
    let home = sandbox();
    let source = home.path().join("source");
    let alpha = create_skill(&source, "alpha", "Alpha.");
    create_skill(&source, "beta", "Beta.");
    add_source(home.path(), &source, "personal", &[]);
    cli(home.path())
        .args(["--json", "load", "alpha", "--claude", "--global", "--yes"])
        .assert()
        .success();
    fs::write(
        alpha.join("SKILL.md"),
        "---\nname: alpha\ndescription: Changed.\n---\n",
    )
    .expect("make alpha outdated");

    let outdated = json_events(
        cli(home.path())
            .args(["--json", "describe", "--outdated"])
            .output()
            .expect("describe outdated"),
    );
    let alpha_event = outdated
        .iter()
        .find(|event| event["event"] == "describe.skill")
        .expect("outdated alpha");
    assert_eq!(alpha_event["data"]["skill"], "alpha");
    assert_eq!(alpha_event["data"]["installation"]["installed"], true);
    assert_eq!(alpha_event["data"]["installation"]["outdated"], true);

    let union = json_events(
        cli(home.path())
            .args(["--json", "describe", "--outdated", "--not-installed"])
            .output()
            .expect("describe state union"),
    );
    let names = union
        .iter()
        .filter(|event| event["event"] == "describe.skill")
        .map(|event| event["data"]["skill"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec![Some("alpha"), Some("beta")]);
}

#[test]
fn installation_state_aggregates_across_all_configured_targets() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "Alpha.");
    add_source(home.path(), &source, "personal", &[]);
    cli(home.path())
        .args([
            "--json",
            "target",
            "add",
            "custom-skills",
            "--name",
            "custom",
        ])
        .assert()
        .success();
    cli(home.path())
        .args([
            "--json", "load", "alpha", "--claude", "--target", "custom", "--global", "--yes",
        ])
        .assert()
        .success();
    fs::write(
        home.path().join("custom-skills/alpha/SKILL.md"),
        "drifted deployment\n",
    )
    .expect("drift one target only");

    let events = json_events(
        cli(home.path())
            .args(["--json", "describe", "alpha", "--outdated"])
            .output()
            .expect("describe aggregate installation state"),
    );
    let skill = events
        .iter()
        .find(|event| event["event"] == "describe.skill")
        .expect("outdated skill");
    assert_eq!(skill["data"]["installation"]["installed"], true);
    assert_eq!(skill["data"]["installation"]["outdated"], true);
    let states = skill["data"]["installation"]["deployments"]
        .as_array()
        .expect("deployment list")
        .iter()
        .map(|deployment| deployment["state"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(states.len(), 2);
    assert!(states.contains(&Some("up-to-date")));
    assert!(states.contains(&Some("needs-update")));
}

#[test]
fn partial_selector_misses_warn_but_an_empty_result_fails() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "Alpha.");
    add_source(home.path(), &source, "personal", &[]);

    let events = json_events(
        cli(home.path())
            .args(["--json", "describe", "alpha", "missing"])
            .output()
            .expect("describe partial match"),
    );
    assert!(events.iter().any(|event| {
        event["event"] == "diagnostic"
            && event["level"] == "warning"
            && event["data"]["pattern"] == "missing"
    }));
    assert!(
        events
            .iter()
            .any(|event| event["event"] == "describe.skill")
    );
    assert_eq!(
        events.last().map(|event| &event["event"]),
        Some(&Value::from("summary"))
    );

    cli(home.path())
        .args(["--json", "describe", "missing"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"event\":\"command.failed\""));
}

#[test]
fn source_inspection_failure_is_reported_before_the_empty_result_error() {
    let home = sandbox();
    cli(home.path())
        .args([
            "--json",
            "source",
            "add",
            "missing-source",
            "--name",
            "broken",
        ])
        .assert()
        .success();

    let output = cli(home.path())
        .args(["--json", "describe", "alpha"])
        .output()
        .expect("describe a skill from an unavailable source");
    assert!(!output.status.success());
    let events = String::from_utf8(output.stdout)
        .expect("UTF-8 events")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("NDJSON event"))
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 2, "diagnostic must precede terminal failure");
    assert_eq!(events[0]["event"], "diagnostic");
    assert_eq!(events[0]["level"], "warning");
    let cause = events[0]["data"]["message"]
        .as_str()
        .expect("materialization diagnostic message");
    assert!(cause.contains("could not inspect skills in source 'broken'"));
    assert!(cause.contains("source directory not found"));
    assert_eq!(events[1]["event"], "command.failed");

    let human = cli(home.path())
        .args(["describe", "alpha"])
        .output()
        .expect("describe unavailable source for human output");
    assert!(!human.status.success());
    let stderr = String::from_utf8(human.stderr).expect("UTF-8 diagnostics");
    let warning = stderr
        .find("Warning: could not inspect skills in source 'broken'")
        .expect("materialization warning");
    let failure = stderr
        .find("Error: skill or source description not found")
        .expect("terminal empty-result failure");
    assert!(warning < failure, "the real cause must be reported first");
}

#[test]
fn source_fallback_and_type_specific_commands_emit_full_source_records() {
    let home = sandbox();
    let source = home.path().join("source");
    create_skill(&source, "alpha", "Alpha trigger.");
    fs::write(source.join("README.md"), "Collection README\n").expect("write source README");
    add_source(home.path(), &source, "personal", &[]);
    let expected_source = portable_canonicalize(&source).expect("canonical source path");

    for args in [
        vec!["--json", "describe", "personal"],
        vec!["--json", "describe", "source", "personal"],
        vec!["--json", "describe", "--all-sources"],
    ] {
        let events = json_events(
            cli(home.path())
                .args(args)
                .output()
                .expect("describe source"),
        );
        let source_event = events
            .iter()
            .find(|event| event["event"] == "describe.source")
            .expect("source event");
        assert_eq!(source_event["data"]["source"]["name"], "personal");
        let reported_source = source_event["data"]["source"]["location"]
            .as_str()
            .map(PathBuf::from)
            .expect("source location");
        assert_eq!(
            portable_canonicalize(reported_source).expect("canonical reported source path"),
            expected_source
        );
        assert_eq!(
            source_event["data"]["content"]["lines"][0],
            "Collection README"
        );
        assert_eq!(
            source_event["data"]["skills"][0]["trigger"],
            "Alpha trigger."
        );
    }
}

#[test]
fn empty_describe_forms_show_relevant_help_and_type_flags_conflict() {
    let home = sandbox();
    for args in [
        vec!["describe"],
        vec!["describe", "skill"],
        vec!["describe", "source"],
    ] {
        cli(home.path())
            .args(args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("Usage:"));
    }
    cli(home.path())
        .args(["describe", "--skills", "--sources"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "the argument '--skills' cannot be used with '--sources'",
        ));
}
