//! Adversarial recovery contracts for untrusted durable journal contents.

#![allow(
    clippy::expect_used,
    reason = "Fixture setup failures are unrecoverable test harness failures."
)]

use std::fs;
use std::path::Path;

use serde_json::json;
use skill_manager::transaction::recover_journal;

fn write_journal(path: &Path, value: &serde_json::Value) {
    fs::create_dir_all(path.parent().expect("journal parent")).expect("create journal directory");
    fs::write(path, value.to_string()).expect("write crafted journal");
}

#[test]
fn recovery_rejects_outside_stage_backup_and_destination_before_mutation() {
    let target = tempfile::tempdir().expect("target root");
    let outside = tempfile::tempdir().expect("outside root");
    let journals = target.path().join(".skill-manager-journals");

    let outside_stage = outside.path().join("stage");
    fs::create_dir(&outside_stage).expect("outside stage");
    fs::write(outside_stage.join("sentinel"), "stage").expect("stage sentinel");
    let stage_journal = journals.join("stage.json");
    write_journal(
        &stage_journal,
        &json!({
            "state": "prepared",
            "destination": target.path().join("demo"),
            "stage": outside_stage,
            "backup": target.path().join(".skill-manager-backups/stage")
        }),
    );
    assert!(recover_journal(&stage_journal).is_err());
    assert_eq!(
        fs::read_to_string(outside.path().join("stage/sentinel")).expect("stage sentinel remains"),
        "stage"
    );
    assert!(stage_journal.exists());

    let outside_backup = outside.path().join("backup");
    fs::create_dir(&outside_backup).expect("outside backup");
    fs::write(outside_backup.join("sentinel"), "backup").expect("backup sentinel");
    let backup_journal = journals.join("backup.json");
    write_journal(
        &backup_journal,
        &json!({
            "state": "committed",
            "destination": target.path().join("demo"),
            "stage": null,
            "backup": outside_backup
        }),
    );
    assert!(recover_journal(&backup_journal).is_err());
    assert_eq!(
        fs::read_to_string(outside.path().join("backup/sentinel"))
            .expect("backup sentinel remains"),
        "backup"
    );
    assert!(backup_journal.exists());

    let local_backup = target.path().join(".skill-manager-backups/destination");
    fs::create_dir_all(&local_backup).expect("local backup");
    fs::write(local_backup.join("sentinel"), "destination").expect("destination sentinel");
    let outside_destination = outside.path().join("injected-destination");
    let destination_journal = journals.join("destination.json");
    write_journal(
        &destination_journal,
        &json!({
            "state": "old-moved",
            "destination": outside_destination,
            "stage": null,
            "backup": local_backup
        }),
    );
    assert!(recover_journal(&destination_journal).is_err());
    assert_eq!(
        fs::read_to_string(
            target
                .path()
                .join(".skill-manager-backups/destination/sentinel")
        )
        .expect("local backup remains"),
        "destination"
    );
    assert!(!outside.path().join("injected-destination").exists());
    assert!(destination_journal.exists());
}
