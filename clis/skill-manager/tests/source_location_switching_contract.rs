//! Black-box contracts for active/alternate source location switching.

#![allow(
    clippy::expect_used,
    reason = "Fixture and process failures are unrecoverable test harness failures."
)]

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn cli(home: &Path) -> Command {
    let mut command = Command::cargo_bin("skill-manager").expect("test binary");
    command
        .current_dir(home)
        .env("SKILL_MANAGER_HOME", home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("NO_COLOR", "1")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN");
    command
}

fn read_config(home: &Path) -> Value {
    serde_json::from_slice(&fs::read(home.join(".skill-manager.config.json")).expect("read config"))
        .expect("parse config")
}

fn events(output: std::process::Output) -> Vec<Value> {
    assert!(output.status.success(), "command failed: {output:?}");
    String::from_utf8(output.stdout)
        .expect("utf8 output")
        .lines()
        .map(|line| serde_json::from_str(line).expect("NDJSON event"))
        .collect()
}

fn add_local(home: &Path, location: &Path, name: &str) -> Value {
    fs::create_dir_all(location).expect("create source");
    let output = cli(home)
        .args([
            "--json",
            "source",
            "add",
            location.to_str().expect("utf8 path"),
            name,
            "--label",
            "Personal Skills",
            "--exclude",
            "draft-*",
        ])
        .output()
        .expect("add source");
    events(output).remove(0)
}

fn assert_exact_paired_list(home: &Path, active: &str, alternate: &Path) {
    let list = cli(home)
        .args(["source", "list"])
        .output()
        .expect("list paired source");
    assert!(list.status.success());
    assert_eq!(
        String::from_utf8(list.stdout)
            .expect("utf8 list")
            .lines()
            .collect::<Vec<_>>(),
        [
            format!("personal     (Personal Skills)  {active}"),
            format!("  alternate  (inactive)         {}", alternate.display()),
        ]
    );
}

#[test]
fn human_status_shows_the_active_and_indented_inactive_location_exactly() {
    let home = TempDir::new().expect("temporary home");
    let local = home.path().join("local-skills");
    add_local(home.path(), &local, "personal");
    cli(home.path())
        .args(["source", "alternate", "personal", "sernst/skills"])
        .assert()
        .success();

    let status = cli(home.path())
        .args(["status", "--all"])
        .output()
        .expect("render status");
    assert!(status.status.success(), "status failed: {status:?}");
    assert_eq!(
        String::from_utf8(status.stdout).expect("utf8 status"),
        format!(
            "Sources:\npersonal     (Personal Skills)  {}\n  alternate  (inactive)         \
             sernst/skills\n\nNo skills found in sources or deployed targets.\n",
            local.display()
        )
    );
}

#[test]
fn alternate_swap_noops_events_metadata_and_aligned_display_are_stable() {
    let home = TempDir::new().expect("temporary home");
    let local = home.path().join("local-skills");
    add_local(home.path(), &local, "personal");
    let mut config = read_config(home.path());
    let original_id = config["sources"][0]["id"].clone();
    config["sources"][0]["source_extension"] = Value::String("preserved".into());
    config["root_extension"] = Value::String("also-preserved".into());
    fs::write(
        home.path().join(".skill-manager.config.json"),
        serde_json::to_vec_pretty(&config).expect("serialize config"),
    )
    .expect("inject extensions");

    let alternate = events(
        cli(home.path())
            .args(["--json", "source", "alternate", "personal", "sernst/skills"])
            .output()
            .expect("set alternate"),
    );
    let event = &alternate[0];
    assert_eq!(event["event"], "source.alternate-set");
    assert_eq!(event["data"]["changed"], true);
    assert_eq!(event["data"]["source_type"], "local");
    assert_eq!(event["data"]["alternate"]["source"], "sernst/skills");
    assert_eq!(event["data"]["alternate"]["source_type"], "github");
    assert!(event["data"]["previous"]["alternate"].is_null());

    let before_noop =
        fs::read(home.path().join(".skill-manager.config.json")).expect("read config before no-op");
    let repeated = events(
        cli(home.path())
            .args(["--json", "source", "alternate", "personal", "SERNST/skills"])
            .output()
            .expect("repeat alternate"),
    );
    assert_eq!(repeated[0]["data"]["changed"], false);
    assert_eq!(
        repeated[0]["data"]["previous"]["alternate"],
        repeated[0]["data"]["alternate"]
    );
    assert_eq!(
        fs::read(home.path().join(".skill-manager.config.json")).expect("read no-op config"),
        before_noop,
        "a no-op must not rewrite configuration"
    );

    let swapped = events(
        cli(home.path())
            .args(["--json", "source", "swap", "personal"])
            .output()
            .expect("swap locations"),
    );
    assert_eq!(swapped[0]["event"], "source.locations-swapped");
    assert_eq!(swapped[0]["data"]["changed"], true);
    assert_eq!(swapped[0]["data"]["source"], "sernst/skills");
    assert_eq!(
        swapped[0]["data"]["previous"]["source"],
        local.to_string_lossy().as_ref()
    );
    let after_swap = read_config(home.path());
    assert_eq!(after_swap["sources"][0]["id"], original_id);
    assert_eq!(after_swap["sources"][0]["name"], "personal");
    assert_eq!(after_swap["sources"][0]["label"], "Personal Skills");
    assert_eq!(after_swap["sources"][0]["exclude"][0], "draft-*");
    assert_eq!(after_swap["sources"][0]["source_extension"], "preserved");
    assert_eq!(after_swap["root_extension"], "also-preserved");

    assert_exact_paired_list(home.path(), "sernst/skills", &local);

    let swapped_back = events(
        cli(home.path())
            .args(["--json", "source", "swap", "personal"])
            .output()
            .expect("swap back"),
    );
    assert_eq!(
        swapped_back[0]["data"]["source"],
        local.to_string_lossy().as_ref()
    );

    cli(home.path())
        .args(["source", "locate", "personal", "sernst/skills"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("source swap"));
    cli(home.path())
        .args(["source", "swap", "sernst/skills"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "alternate locations are not source selectors",
        ));
}

#[test]
fn locate_aliases_update_location_atomically_and_salted_add_reuses_the_old_location() {
    let home = TempDir::new().expect("temporary home");
    let original = home.path().join("original");
    add_local(home.path(), &original, "personal");
    let original_id = read_config(home.path())["sources"][0]["id"]
        .as_str()
        .expect("source id")
        .to_owned();

    let updated = events(
        cli(home.path())
            .args([
                "--json",
                "source",
                "update",
                "personal",
                "--location",
                "sernst/skills",
                "--label",
                "Remote Personal",
            ])
            .output()
            .expect("combined update"),
    );
    assert_eq!(updated[0]["event"], "source.updated");
    assert_eq!(updated[0]["data"]["changed"], true);
    assert_eq!(updated[0]["data"]["source"], "sernst/skills");
    assert_eq!(
        updated[0]["data"]["previous"]["source"],
        original.to_string_lossy().as_ref()
    );
    let config = read_config(home.path());
    assert_eq!(config["sources"][0]["id"], original_id);
    assert_eq!(config["sources"][0]["label"], "Remote Personal");

    add_local(home.path(), &original, "local-again");
    let added = read_config(home.path());
    let replacement_id = added["sources"][1]["id"].as_str().expect("salted id");
    assert_ne!(replacement_id, original_id);
    assert!(replacement_id.starts_with("src_"));
    assert_eq!(replacement_id.len(), 16);

    for (alias, location) in [
        ("relocate", "owner/one"),
        ("move", "owner/two"),
        ("mv", "owner/three"),
        ("locate", "owner/four"),
    ] {
        let result = events(
            cli(home.path())
                .args(["--json", "source", alias, "personal", location])
                .output()
                .expect("locate alias"),
        );
        assert_eq!(result[0]["event"], "source.location-set");
        assert_eq!(result[0]["data"]["changed"], true);
    }
}

#[test]
fn pairing_validation_clear_and_new_cross_source_collisions_are_enforced() {
    let home = TempDir::new().expect("temporary home");
    let first = home.path().join("first");
    let second = home.path().join("second");
    let third = home.path().join("third-does-not-need-to-exist");
    add_local(home.path(), &first, "first");
    add_local(home.path(), &second, "second");

    cli(home.path())
        .args([
            "source",
            "alternate",
            "first",
            second.to_str().expect("utf8 path"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "already configured by source 'second'",
        ));
    cli(home.path())
        .args([
            "source",
            "alternate",
            "first",
            first.to_str().expect("utf8 path"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "must differ from the active location",
        ));

    cli(home.path())
        .args([
            "source",
            "alternate",
            "first",
            third.to_str().expect("utf8 path"),
        ])
        .assert()
        .success();
    cli(home.path())
        .args([
            "source",
            "locate",
            "second",
            third.to_str().expect("utf8 path"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "already configured by source 'first'",
        ));

    let cleared = events(
        cli(home.path())
            .args(["--json", "source", "alternate", "first", "--clear"])
            .output()
            .expect("clear alternate"),
    );
    assert_eq!(cleared[0]["event"], "source.alternate-cleared");
    assert_eq!(cleared[0]["data"]["changed"], true);
    let cleared_again = events(
        cli(home.path())
            .args(["--json", "source", "alternate", "first", "--clear"])
            .output()
            .expect("clear absent alternate"),
    );
    assert_eq!(cleared_again[0]["data"]["changed"], false);

    cli(home.path())
        .args(["source", "swap", "first"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("has no alternate"));
    cli(home.path())
        .args(["source", "alternate", "first", "owner/repo", "--clear"])
        .assert()
        .code(2);
}

#[test]
fn github_pairs_replace_and_inactive_selectors_are_rejected_before_ad_hoc_fallbacks() {
    let home = TempDir::new().expect("temporary home");
    let local = home.path().join("local");
    add_local(home.path(), &local, "personal");
    cli(home.path())
        .args(["source", "locate", "personal", "owner/active"])
        .assert()
        .success();
    cli(home.path())
        .args(["source", "alternate", "personal", "owner/first-alt"])
        .assert()
        .success();
    cli(home.path())
        .args(["source", "alternate", "personal", "owner/replacement"])
        .assert()
        .success();
    let paired = read_config(home.path());
    assert_eq!(paired["sources"][0]["type"], "github");
    assert_eq!(paired["sources"][0]["repo"], "active");
    assert_eq!(paired["sources"][0]["alternate"]["repo"], "replacement");
    assert_eq!(paired["sources"][0]["alternate"]["type"], "github");

    let copy_output = home.path().join("copy-output");
    let copy_output = copy_output.to_str().expect("utf8 output");
    for arguments in [
        vec!["copy", "owner/replacement", copy_output],
        vec!["load", "owner/replacement", "--all"],
        vec!["update", "owner/replacement", "--all"],
        vec!["source", "remove", "owner/replacement"],
        vec!["source", "swap", "owner/replacement"],
        vec!["resolve", "--prefer-source", "owner/replacement"],
    ] {
        cli(home.path())
            .args(arguments)
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "alternate locations are not source selectors",
            ))
            .stderr(predicate::str::contains("personal"));
    }
}

#[test]
fn normalized_equivalent_local_locations_collide_for_pairs_and_sources() {
    let home = TempDir::new().expect("temporary home");
    let first = home.path().join("first");
    let second = home.path().join("second");
    add_local(home.path(), &first, "first");
    add_local(home.path(), &second, "second");

    let own_equivalent = first.join(".").to_string_lossy().into_owned();
    cli(home.path())
        .args(["source", "alternate", "first", &own_equivalent])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "must differ from the active location",
        ));

    let cross_equivalent = first
        .join("child")
        .join("..")
        .to_string_lossy()
        .into_owned();
    cli(home.path())
        .args(["source", "alternate", "second", &cross_equivalent])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "already configured by source 'first'",
        ));
}

#[test]
fn every_new_or_replaced_slot_rejects_other_active_and_alternate_collisions() {
    enum Mutation {
        Add,
        Locate,
        Alternate,
    }

    let cases = [
        ("add -> other active", Mutation::Add, "owner/second"),
        ("add -> other alternate", Mutation::Add, "owner/second-alt"),
        (
            "active replacement -> other active",
            Mutation::Locate,
            "owner/second",
        ),
        (
            "active replacement -> other alternate",
            Mutation::Locate,
            "owner/second-alt",
        ),
        (
            "alternate replacement -> other active",
            Mutation::Alternate,
            "owner/second",
        ),
        (
            "alternate replacement -> other alternate",
            Mutation::Alternate,
            "owner/second-alt",
        ),
    ];

    for (description, mutation, collision) in cases {
        let home = TempDir::new().expect("temporary home");
        cli(home.path())
            .args(["source", "add", "owner/first", "first"])
            .assert()
            .success();
        cli(home.path())
            .args(["source", "add", "owner/second", "second"])
            .assert()
            .success();
        cli(home.path())
            .args(["source", "alternate", "second", "owner/second-alt"])
            .assert()
            .success();
        cli(home.path())
            .args(["source", "alternate", "first", "owner/first-alt"])
            .assert()
            .success();

        let mut command = cli(home.path());
        match mutation {
            Mutation::Add => {
                command.args(["source", "add", collision, "third"]);
            }
            Mutation::Locate => {
                command.args(["source", "locate", "first", collision]);
            }
            Mutation::Alternate => {
                command.args(["source", "alternate", "first", collision]);
            }
        }
        command.assert().failure().stderr(predicate::str::contains(
            "already configured by source 'second'",
        ));
        assert_eq!(
            read_config(home.path())["sources"].as_array().map(Vec::len),
            Some(2),
            "{description}"
        );
    }
}
