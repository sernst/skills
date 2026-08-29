//! Black-box contracts for `configs copy`, which seeds a destination manager
//! home (configuration plus resolved target directories) from an existing
//! one using merge/never-delete semantics.

#![allow(
    clippy::expect_used,
    reason = "Fixture and process failures are unrecoverable test harness failures."
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

/// Every invocation passes `--home` pointed at a scratch temp directory and
/// additionally overrides `HOME`/`USERPROFILE`/`SKILL_MANAGER_HOME` to the
/// same scratch directory as defense in depth, so no invocation in this file
/// can ever resolve to the real machine home even if `--home` were ignored.
fn cli(cwd: &Path, home: &Path) -> Command {
    let mut command = Command::cargo_bin("skill-manager").expect("test binary");
    command
        .current_dir(cwd)
        .env("SKILL_MANAGER_HOME", home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env("NO_COLOR", "1")
        .args(["--home"])
        .arg(home);
    command
}

fn events(output: std::process::Output) -> Vec<Value> {
    String::from_utf8(output.stdout)
        .expect("utf8 output")
        .lines()
        .map(|line| serde_json::from_str(line).expect("NDJSON event"))
        .collect()
}

fn write_file(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directories");
    }
    fs::write(path, body).expect("write fixture file");
}

/// Recursively snapshot every regular file under `root` as a map of
/// slash-joined relative path to bytes, plus a marker entry for each
/// directory, so two snapshots compare equal only when the tree is
/// byte-for-byte identical. A missing `root` snapshots as an empty map, so a
/// directory that was never created reads the same as one that stayed empty.
fn snapshot(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut map = BTreeMap::new();
    if root.is_dir() {
        snapshot_into(root, root, &mut map);
    }
    map
}

fn snapshot_into(root: &Path, current: &Path, map: &mut BTreeMap<String, Option<Vec<u8>>>) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .expect("entry under root")
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let metadata = fs::symlink_metadata(&path).expect("entry metadata");
        if metadata.is_dir() {
            map.insert(format!("{relative}/"), None);
            snapshot_into(root, &path, map);
        } else {
            map.insert(relative, Some(fs::read(&path).expect("read file bytes")));
        }
    }
}

/// Try to create a file symlink at `link` pointing at `target`, returning
/// `false` when the platform refuses (an unprivileged Windows session without
/// Developer Mode). Tests that need a real symlink skip themselves rather than
/// fail when the OS cannot make one; on Linux CI this always succeeds.
fn try_symlink_file(target: &Path, link: &Path) -> bool {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).expect("create symlink parent");
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}

/// Try to create a directory symlink at `link` pointing at `target`,
/// returning `false` when the platform refuses. Cross-platform companion to
/// [`try_symlink_file`] used to plant a linked destination ancestor.
fn try_symlink_dir(target: &Path, link: &Path) -> bool {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).expect("create symlink parent");
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}

/// Create a REAL Windows directory junction at `link` pointing at `target`.
///
/// Junctions are link-like reparse points (`IO_REPARSE_TAG_MOUNT_POINT`). The
/// production classifier uses Windows' reparse-tag-aware file type methods so
/// junctions are links without treating every unrelated reparse-point family
/// as one. Junction creation needs NO administrator privilege (unlike symlink
/// creation), so this is strictly more reliable than [`try_symlink_dir`]; it
/// panics loudly rather than skipping if `mklink /J` fails, so a broken guard
/// cannot pass silently.
#[cfg(windows)]
fn make_junction(target: &Path, link: &Path) {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).expect("create junction parent");
    }
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("spawn mklink /J");
    assert!(
        output.status.success(),
        "mklink /J failed to create a junction: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = fs::symlink_metadata(link).expect("junction metadata");
    assert!(
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0,
        "mklink /J must produce a real reparse point, or this test would not exercise junction handling"
    );
}

/// A raw [`std::process::Command`] for the binary, configured with the same
/// home isolation as [`cli`], for the few tests that need piped stdin/stderr
/// to control an interactive invocation mid-flight.
fn std_cli(cwd: &Path, home: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(assert_cmd::cargo::cargo_bin("skill-manager"));
    command
        .current_dir(cwd)
        .env("SKILL_MANAGER_HOME", home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env("NO_COLOR", "1")
        .arg("--home")
        .arg(home);
    command
}

/// than hand-authoring JSON: `source add` + `load --claude --global`
/// together exercise the same persistence path a real user's home would
/// have gone through.
fn seed_real_home_with_claude_skill(scratch: &TempDir, home: &Path, skill_name: &str) {
    let collection = scratch.path().join("collection");
    write_file(&collection.join(skill_name).join("SKILL.md"), "# skill\n");
    cli(scratch.path(), home)
        .args([
            "--json",
            "source",
            "add",
            collection.to_str().expect("utf8 collection path"),
            "--name=collection",
        ])
        .assert()
        .success();
    cli(scratch.path(), home)
        .args(["--json", "load", "--claude", "--global", "--yes"])
        .assert()
        .success();
}

#[test]
fn seeds_an_empty_destination_with_configuration_and_the_deployed_target() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    let output = cli(scratch.path(), &active_home)
        .args([
            "--json",
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .output()
        .expect("run configs copy");
    assert!(output.status.success(), "configs copy failed: {output:?}");

    assert!(to.join(".skill-manager").join("config.json").exists());
    assert!(
        to.join(".claude")
            .join("skills")
            .join("alpha")
            .join("SKILL.md")
            .is_file()
    );
    let deployed = fs::read_to_string(
        to.join(".claude")
            .join("skills")
            .join("alpha")
            .join("SKILL.md"),
    )
    .expect("read deployed skill");
    assert_eq!(deployed, "# skill\n");

    let lines = events(output);
    assert!(!lines.is_empty(), "expected at least one NDJSON event");
    let plan = lines
        .iter()
        .find(|event| event["event"] == "plan" && event["data"]["command"] == "configs.copy")
        .expect("configs.copy plan event");
    assert_eq!(plan["data"]["dry_run"], false);
    assert_eq!(plan["data"]["target_source"], "from-config");
    let items = plan["data"]["items"].as_array().expect("plan items array");
    assert!(
        items.len() >= 2,
        "expected a configuration item and at least one target item"
    );

    let summary = lines
        .iter()
        .find(|event| event["event"] == "summary" && event["data"]["action"] == "configs.copy")
        .expect("configs.copy summary event");
    assert_eq!(summary["data"]["dry_run"], false);
    assert_eq!(summary["data"]["items"], items.len() as u64);

    let item_events: Vec<_> = lines
        .iter()
        .filter(|event| event["event"] == "configs.copy.item")
        .collect();
    assert_eq!(item_events.len(), items.len());
    assert!(
        item_events
            .iter()
            .all(|event| event["data"]["action"] == "copied"),
        "seeding an empty destination should report every item as newly copied"
    );
}

#[test]
fn merging_into_an_existing_destination_preserves_unrelated_content() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    // Pre-existing, unrelated destination content: a sibling skill inside a
    // directory this command will merge into, and a top-level file outside
    // any item this command ever touches.
    write_file(
        &to.join(".claude")
            .join("skills")
            .join("bystander")
            .join("SKILL.md"),
        "# bystander\n",
    );
    write_file(&to.join("untouched-top-level.txt"), "leave me alone\n");

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .success();

    assert!(
        to.join(".claude")
            .join("skills")
            .join("alpha")
            .join("SKILL.md")
            .is_file(),
        "the seeded skill must be merged in"
    );
    let bystander = fs::read_to_string(
        to.join(".claude")
            .join("skills")
            .join("bystander")
            .join("SKILL.md"),
    )
    .expect("read bystander skill");
    assert_eq!(
        bystander, "# bystander\n",
        "an unrelated sibling inside a merged directory must survive untouched"
    );
    let untouched = fs::read_to_string(to.join("untouched-top-level.txt"))
        .expect("read untouched top-level file");
    assert_eq!(
        untouched, "leave me alone\n",
        "content outside every copied item must never be touched"
    );
}

#[test]
fn target_discovery_prefers_froms_own_configuration_over_the_active_home() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");

    // `from` registers its own custom target that the active home's
    // configuration has no knowledge of, proving discovery reads `from`'s
    // configuration rather than falling back.
    cli(scratch.path(), &from)
        .args(["target", "add", "custom-dir", "--name=custom-target"])
        .assert()
        .success();
    write_file(
        &from.join("custom-dir").join("marker.txt"),
        "from custom target\n",
    );

    // The active home has its own, unrelated persisted configuration so the
    // fallback path (if it were mistakenly used) would resolve nothing at
    // `custom-dir`.
    cli(scratch.path(), &active_home)
        .args(["target", "add", "unrelated-dir", "--name=unrelated-target"])
        .assert()
        .success();

    let output = cli(scratch.path(), &active_home)
        .args([
            "--json",
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .output()
        .expect("run configs copy");
    assert!(output.status.success(), "configs copy failed: {output:?}");

    assert!(to.join("custom-dir").join("marker.txt").is_file());
    assert!(!to.join("unrelated-dir").exists());

    let plan = events(output)
        .into_iter()
        .find(|event| event["event"] == "plan")
        .expect("plan event");
    assert_eq!(plan["data"]["target_source"], "from-config");
}

#[test]
fn target_discovery_falls_back_to_the_active_home_configuration_when_from_has_none() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");

    // The active home persists a custom target; `from` has no
    // `.skill-manager` at all, only a directory that happens to match that
    // custom target's path, so only the active-home fallback can discover it.
    cli(scratch.path(), &active_home)
        .args(["target", "add", "special-dir", "--name=special"])
        .assert()
        .success();
    write_file(
        &from.join("special-dir").join("marker.txt"),
        "from active home config\n",
    );

    let output = cli(scratch.path(), &active_home)
        .args([
            "--json",
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .output()
        .expect("run configs copy");
    assert!(output.status.success(), "configs copy failed: {output:?}");

    assert!(to.join("special-dir").join("marker.txt").is_file());
    assert!(
        !to.join(".skill-manager").join("config.json").exists(),
        "from has no configuration of its own, so none should be copied"
    );

    let plan = events(output)
        .into_iter()
        .find(|event| event["event"] == "plan")
        .expect("plan event");
    assert_eq!(plan["data"]["target_source"], "active-config");
}

#[test]
fn target_discovery_falls_back_to_builtin_defaults_when_neither_has_configuration() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    // A fresh active home that has never persisted a configuration.
    let active_home = scratch.path().join("active-home");

    write_file(
        &from
            .join(".claude")
            .join("skills")
            .join("alpha")
            .join("SKILL.md"),
        "# builtin default target\n",
    );

    let output = cli(scratch.path(), &active_home)
        .args([
            "--json",
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .output()
        .expect("run configs copy");
    assert!(output.status.success(), "configs copy failed: {output:?}");

    assert!(
        to.join(".claude")
            .join("skills")
            .join("alpha")
            .join("SKILL.md")
            .is_file()
    );

    let plan = events(output)
        .into_iter()
        .find(|event| event["event"] == "plan")
        .expect("plan event");
    assert_eq!(plan["data"]["target_source"], "defaults");
}

#[test]
fn cache_backup_and_lock_directories_are_excluded_by_default_and_included_with_the_flag() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    // Regenerable directories that must be excluded from an ordinary seed.
    write_file(
        &from.join(".skill-manager").join("cache").join("entry.json"),
        "{}",
    );
    write_file(
        &from
            .join(".skill-manager")
            .join("backups")
            .join("config.json.bak"),
        "{}",
    );
    write_file(
        &from
            .join(".skill-manager")
            .join("locks")
            .join("lock-marker"),
        "",
    );

    let excluded_destination = scratch.path().join("to-default");
    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            excluded_destination
                .to_str()
                .expect("utf8 destination path"),
            "--yes",
        ])
        .assert()
        .success();
    assert!(
        !excluded_destination
            .join(".skill-manager")
            .join("cache")
            .exists()
    );
    assert!(
        !excluded_destination
            .join(".skill-manager")
            .join("backups")
            .exists()
    );
    assert!(
        !excluded_destination
            .join(".skill-manager")
            .join("locks")
            .exists()
    );
    assert!(
        excluded_destination
            .join(".skill-manager")
            .join("config.json")
            .is_file(),
        "the configuration file itself is not excluded, only cache/backup/lock directories"
    );

    let included_destination = scratch.path().join("to-included");
    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            included_destination
                .to_str()
                .expect("utf8 destination path"),
            "--yes",
            "--include-cache",
        ])
        .assert()
        .success();
    assert!(
        included_destination
            .join(".skill-manager")
            .join("cache")
            .join("entry.json")
            .is_file()
    );
    assert!(
        included_destination
            .join(".skill-manager")
            .join("backups")
            .join("config.json.bak")
            .is_file()
    );
    assert!(
        included_destination
            .join(".skill-manager")
            .join("locks")
            .join("lock-marker")
            .is_file()
    );
}

#[test]
fn dry_run_reports_the_plan_and_makes_no_filesystem_change() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    let output = cli(scratch.path(), &active_home)
        .args([
            "--json",
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--dry-run",
        ])
        .output()
        .expect("run configs copy --dry-run");
    assert!(
        output.status.success(),
        "configs copy --dry-run failed: {output:?}"
    );

    assert!(
        !to.exists(),
        "a dry run must not create the destination at all"
    );

    let lines = events(output);
    let plan = lines
        .iter()
        .find(|event| event["event"] == "plan")
        .expect("plan event");
    assert_eq!(plan["data"]["dry_run"], true);
    let summary = lines
        .iter()
        .find(|event| event["event"] == "summary" && event["data"]["action"] == "configs.copy")
        .expect("configs.copy summary event");
    assert_eq!(summary["data"]["dry_run"], true);
    assert!(
        !lines
            .iter()
            .any(|event| event["event"] == "configs.copy.item"),
        "a dry run must never emit apply-time item events"
    );
}

#[test]
fn recipe_driven_invocations_seed_the_destination_without_argv_flags() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    // Inline `--json=OBJECT` recipe: no `--yes`/argv command at all, and the
    // JSON carrier alone must still auto-authorize the plan and emit NDJSON,
    // exactly like a sibling `copy`/`load` recipe invocation does.
    let inline_to = scratch.path().join("inline-to");
    let inline = serde_json::json!({
        "command": "configs.copy",
        "from": from,
        "to": inline_to,
    });
    cli(scratch.path(), &active_home)
        .arg(format!("--json={inline}"))
        .assert()
        .success()
        .stdout(predicate::str::contains("\"event\":\"summary\""))
        .stdout(predicate::str::contains("\"action\":\"configs.copy\""));
    assert!(
        inline_to
            .join(".claude")
            .join("skills")
            .join("alpha")
            .join("SKILL.md")
            .is_file(),
        "an inline --json recipe must seed the destination"
    );

    // `--json-input` recipe delivered over stdin.
    let stdin_to = scratch.path().join("stdin-to");
    let stdin_recipe = serde_json::json!({
        "command": "configs.copy",
        "from": from,
        "to": stdin_to,
    });
    cli(scratch.path(), &active_home)
        .arg("--json-input")
        .write_stdin(stdin_recipe.to_string())
        .assert()
        .success();
    assert!(
        stdin_to
            .join(".claude")
            .join("skills")
            .join("alpha")
            .join("SKILL.md")
            .is_file(),
        "a --json-input recipe over stdin must seed the destination"
    );

    // `--input FILE` recipe with relative `from`/`to` rebased against the
    // recipe file's own directory, proving `rebase_seed_path` runs.
    let recipe_dir = scratch.path().join("recipes");
    fs::create_dir_all(&recipe_dir).expect("create recipe directory");
    fs::write(
        recipe_dir.join("seed.json"),
        serde_json::json!({
            "command": "configs.copy",
            "from": "../from",
            "to": "../file-to",
        })
        .to_string(),
    )
    .expect("write recipe file");
    cli(&recipe_dir, &active_home)
        .args(["--input", "seed.json"])
        .assert()
        .success();
    assert!(
        scratch
            .path()
            .join("file-to")
            .join(".claude")
            .join("skills")
            .join("alpha")
            .join("SKILL.md")
            .is_file(),
        "a relative --input recipe path must rebase against the recipe file's directory"
    );
}

#[test]
fn recipe_rejects_unknown_and_malformed_fields() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    let unknown_field = serde_json::json!({
        "command": "configs.copy",
        "from": from,
        "to": to,
        "bogus": true,
    });
    cli(scratch.path(), &active_home)
        .arg(format!("--json={unknown_field}"))
        .assert()
        .failure();
    assert!(
        !to.exists(),
        "an unknown recipe field must fail before any mutation"
    );

    let malformed_bool = serde_json::json!({
        "command": "configs.copy",
        "from": from,
        "to": to,
        "include_cache": "yes",
    });
    cli(scratch.path(), &active_home)
        .arg(format!("--json={malformed_bool}"))
        .assert()
        .failure();
    assert!(
        !to.exists(),
        "a malformed recipe field type must fail before any mutation"
    );

    let missing_to = serde_json::json!({
        "command": "configs.copy",
        "from": from,
    });
    cli(scratch.path(), &active_home)
        .arg(format!("--json={missing_to}"))
        .assert()
        .failure();
}

#[test]
fn dry_run_leaves_existing_destination_content_untouched() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");
    write_file(&to.join("preexisting.txt"), "already here\n");

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--dry-run",
        ])
        .assert()
        .success();

    assert!(
        !to.join(".claude").exists(),
        "a dry run must not deploy the seeded target directory"
    );
    let preexisting =
        fs::read_to_string(to.join("preexisting.txt")).expect("read preexisting file");
    assert_eq!(preexisting, "already here\n");
}

#[test]
fn nonexistent_from_directory_is_a_clean_error() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("does-not-exist");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist"));
}

#[test]
fn from_with_nothing_to_copy_is_a_clean_error() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    // A fresh active home with no configuration and a `from` directory that
    // exists but has neither a configuration nor any built-in target
    // directory beneath it.
    let active_home = scratch.path().join("active-home");
    fs::create_dir_all(&from).expect("create empty from directory");

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no configuration"));
}

#[test]
fn destination_that_is_a_file_is_a_clean_error() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to-is-a-file");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");
    write_file(&to, "not a directory\n");

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a directory"));
}

/// A destination inside a source root that is ACTUALLY copied (here the
/// resolved `.claude/skills` target) is a real recursion hazard and must be
/// rejected — walking the root would descend into the destination as it is
/// being written. Contrast with the headline case below, where a destination
/// merely under `<FROM>` but outside every copied root is allowed.
#[test]
fn destination_inside_a_copied_target_root_is_rejected() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = from
        .join(".claude")
        .join("skills")
        .join("nested-destination");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("recurse"));
}

/// A destination inside the copied `.skill-manager` configuration root is the
/// same recursion hazard as a target root and must be rejected too.
#[test]
fn destination_inside_the_configuration_root_is_rejected() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = from.join(".skill-manager").join("nested-destination");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("recurse"));
}

/// The headline case (finding N): a destination that lives under `<FROM>` — as
/// a repo or `TEMP` directory under the user's home routinely does — but
/// OUTSIDE every copied root must SUCCEED. This is `configs copy ~ ./temp/...`,
/// the single most important path the feature exists to serve, which the old
/// blanket nesting guard wrongly rejected.
#[test]
fn destination_under_from_but_outside_copied_roots_succeeds() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    // A realistic destination: a scratch folder nested under the source home,
    // but not inside `.skill-manager` or any resolved target root.
    let to = from.join("repo").join("temp").join("smoke-testing");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .success();

    assert!(
        to.join(".skill-manager").join("config.json").exists(),
        "the headline nested-destination case must seed the configuration"
    );
    assert!(
        to.join(".claude")
            .join("skills")
            .join("alpha")
            .join("SKILL.md")
            .is_file(),
        "the headline nested-destination case must seed the resolved target"
    );
}

/// A source root inside the destination is a self-overwrite hazard: writing
/// `<TO>` could clobber the source mid-read. When `<FROM>` itself is inside
/// `<TO>`, its `.skill-manager` root is too, so the copy is rejected.
#[test]
fn source_nested_inside_destination_is_rejected() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let to = scratch.path().join("to");
    let from = to.join("nested-source");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("overwrite the source"));
}

#[test]
fn identical_source_and_destination_are_rejected() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("home");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            from.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("same directory"));
}

/// Regression for defect 1: a `--dry-run` must change nothing anywhere,
/// including the active `--home`, which older builds mutated up front with
/// `.config-layout-migrated`, `locks/`, and `config.lock` before any
/// validation ran. The source here has no configuration of its own, forcing
/// target discovery to consult the active home — which must be *read*, never
/// written.
#[test]
fn dry_run_never_mutates_the_active_home() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    // A source with a resolved target directory but NO `.skill-manager`
    // configuration, so discovery falls through to the active home.
    write_file(
        &from
            .join(".claude")
            .join("skills")
            .join("alpha")
            .join("SKILL.md"),
        "# skill\n",
    );

    let before = snapshot(&active_home);
    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--dry-run",
        ])
        .assert()
        .success();
    let after = snapshot(&active_home);

    assert_eq!(
        before, after,
        "a dry run left the active home changed; it must never migrate, lock, or write it"
    );
    assert!(
        !active_home.join(".skill-manager").exists(),
        "a dry run must not create `.skill-manager` under the active home"
    );
    assert!(!to.exists(), "a dry run must not create the destination");
}

/// Regression for defect 2: when the active `--home` aliases `<FROM>` — the
/// canonical `configs copy ~ ./temp/...` shape — the command must never write
/// to `<FROM>`, with or without `--dry-run`. Older builds migrated layout and
/// took a persistent lock inside the active home unconditionally, which here
/// is the real source home.
#[test]
fn copying_from_the_active_home_never_mutates_it() {
    for dry_run in [false, true] {
        let scratch = tempfile::tempdir().expect("scratch root");
        let from = scratch.path().join("src-home");
        let to = scratch.path().join("to");
        // The source carries only a resolved target directory and no
        // `.skill-manager` at all, so any repository housekeeping would be
        // plainly visible as a newly created `.skill-manager` under it.
        write_file(
            &from
                .join(".claude")
                .join("skills")
                .join("alpha")
                .join("SKILL.md"),
            "# skill\n",
        );

        let before = snapshot(&from);
        let mut command = cli(scratch.path(), &from);
        command.args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ]);
        if dry_run {
            command.arg("--dry-run");
        }
        command.assert().success();
        let after = snapshot(&from);

        assert_eq!(
            before, after,
            "configs copy mutated <FROM> when it aliased the active home (dry_run={dry_run})"
        );
        assert!(
            !from.join(".skill-manager").exists(),
            "the active home == <FROM> gained repository state (dry_run={dry_run})"
        );
    }
}

/// Regression for defect 3: a symlink planted at a destination path must not
/// be followed to overwrite a file outside `<TO>`. The command must reject the
/// link during preflight and leave the outside file byte-for-byte intact.
#[test]
fn destination_symlink_escape_is_rejected_and_leaves_the_outside_file_intact() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    // A sensitive file OUTSIDE the destination that must never be written.
    let outside = scratch.path().join("outside-secret.txt");
    write_file(&outside, "do not overwrite me\n");

    // Plant a symlink where the copy would write `alpha/SKILL.md`, aimed at
    // the outside file. Skip when the platform will not let us make one.
    let link = to
        .join(".claude")
        .join("skills")
        .join("alpha")
        .join("SKILL.md");
    if !try_symlink_file(&outside, &link) {
        eprintln!("skipping: platform refused to create a symlink");
        return;
    }

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("link"));

    let outside_body = fs::read_to_string(&outside).expect("read outside file");
    assert_eq!(
        outside_body, "do not overwrite me\n",
        "the copy followed a destination symlink and overwrote a file outside <TO>"
    );
}

/// Windows regression for the rejected dangling-`TO` bypass: path resolution
/// used to follow the junction first, return its missing target, and thereby
/// erase the evidence that the caller supplied a linked destination. The
/// command must reject the original junction before rendering a plan or
/// creating the outside target. Junction creation is deterministic and does
/// not depend on symlink privilege.
#[cfg(windows)]
#[test]
fn a_dangling_junction_as_to_is_rejected_before_plan_or_write() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    let outside = scratch.path().join("outside");
    fs::create_dir_all(&outside).expect("create outside parent");
    let missing_target = outside.join("missing-target");
    let to = scratch.path().join("dangling-to");
    make_junction(&missing_target, &to);
    assert!(
        fs::symlink_metadata(&to).is_ok() && !missing_target.exists(),
        "fixture must be a present junction whose target is absent"
    );

    let output = cli(scratch.path(), &active_home)
        .args([
            "--json",
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .output()
        .expect("run configs copy");
    assert!(
        !output.status.success(),
        "a dangling junction supplied as <TO> must hard-fail"
    );

    let lines = events(output);
    assert!(
        !lines.iter().any(|event| event["event"] == "plan"),
        "destination-link validation must fail before the plan"
    );
    assert!(
        !lines
            .iter()
            .any(|event| event["event"] == "configs.copy.item"),
        "destination-link validation must fail before apply"
    );
    assert!(
        lines.iter().any(|event| event["event"] == "command.failed"),
        "the hard failure must be reported"
    );
    assert!(
        !missing_target.exists(),
        "the copy followed the dangling <TO> junction and created its outside target"
    );
    assert!(
        fs::symlink_metadata(&to).is_ok(),
        "the rejected junction fixture must remain untouched"
    );
}

/// The strict destination walk must inspect an existing junction before a
/// following `..` is applied. Ordinary path normalization correctly resolves
/// `junction/../destination` against the junction target, but `configs copy`
/// has the stronger policy that any destination-side link is a hard error.
#[cfg(windows)]
#[test]
fn a_junction_component_before_parent_in_to_is_rejected_before_plan_or_write() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    let real_root = scratch.path().join("real-root");
    let inner = real_root.join("inner");
    fs::create_dir_all(&inner).expect("create junction target");
    let junction = scratch.path().join("junction");
    make_junction(&inner, &junction);
    let requested_to = junction.join("..").join("destination");
    let physical_destination = real_root.join("destination");
    let lexical_destination = scratch.path().join("destination");

    let output = cli(scratch.path(), &active_home)
        .args([
            "--json",
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            requested_to.to_str().expect("utf8 destination path"),
            "--yes",
        ])
        .output()
        .expect("run configs copy");
    assert!(
        !output.status.success(),
        "an existing junction component in <TO> must hard-fail"
    );

    let lines = events(output);
    assert!(
        !lines.iter().any(|event| event["event"] == "plan"),
        "the original destination walk must reject the junction before the plan"
    );
    assert!(
        !lines
            .iter()
            .any(|event| event["event"] == "configs.copy.item"),
        "the original destination walk must reject the junction before apply"
    );
    assert!(
        !physical_destination.exists() && !lexical_destination.exists(),
        "the rejected command must not write to either interpretation of the destination"
    );
}

/// Regression for defect 4: a plan must never promise a seed it then only
/// partially applies. With a destination path existing as a FILE where a
/// directory is required, the command must fail during preflight — before
/// rendering apply-time item events or writing `.skill-manager` — so no
/// partial seed is left behind.
#[test]
fn child_path_conflict_fails_before_writing_anything() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    // `.claude/skills` must become a directory, but it already exists as a
    // file at the destination.
    write_file(&to.join(".claude").join("skills"), "i am a file\n");

    let output = cli(scratch.path(), &active_home)
        .args([
            "--json",
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .output()
        .expect("run configs copy");
    assert!(!output.status.success(), "conflict must fail the command");

    assert!(
        !to.join(".skill-manager").exists(),
        "the command wrote a partial seed despite a preflight conflict"
    );
    let skills_still_a_file = fs::symlink_metadata(to.join(".claude").join("skills"))
        .expect("skills path metadata")
        .is_file();
    assert!(
        skills_still_a_file,
        "the conflicting path was mutated instead of left untouched"
    );

    let lines = events(output);
    assert!(
        !lines
            .iter()
            .any(|event| event["event"] == "configs.copy.item"),
        "no apply-time item events may be emitted when preflight rejects the plan"
    );
    let summary = lines
        .iter()
        .find(|event| event["event"] == "summary" && event["data"]["action"] == "configs.copy");
    assert!(
        summary.is_some(),
        "a terminal summary must close even a preflight-failure exit"
    );
}

/// Regression for defect 5: a malformed recipe value must fail the whole
/// invocation even when a valid argv operand supplies the same field, so a
/// recipe cannot smuggle a type error past argv precedence.
#[test]
fn recipe_type_masking_under_argv_precedence_is_rejected() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    // argv supplies valid operands; the recipe's `from`/`to` are the wrong
    // type and must be validated and rejected regardless.
    let recipe = serde_json::json!({
        "command": "configs.copy",
        "from": false,
        "to": false,
    });
    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .arg(format!("--json={recipe}"))
        .assert()
        .failure();

    assert!(
        !to.exists(),
        "a malformed recipe value masked by argv must fail before any mutation"
    );
}

/// Regression for defect 6: every exit path, including an error, must end with
/// a terminal `summary` event (see `docs/json.md`). A nonexistent source
/// previously emitted only `command.failed`.
#[test]
fn error_exits_still_emit_a_terminal_summary() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("does-not-exist");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");

    let output = cli(scratch.path(), &active_home)
        .args([
            "--json",
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .output()
        .expect("run configs copy");
    assert!(!output.status.success(), "a missing source must fail");

    let lines = events(output);
    let summary = lines
        .iter()
        .find(|event| event["event"] == "summary" && event["data"]["action"] == "configs.copy")
        .expect("a terminal summary must be emitted even on an error exit");
    assert_eq!(
        summary["data"]["items"], 0,
        "a pre-apply failure committed nothing, so the summary must report zero items"
    );
    assert!(
        lines.iter().any(|event| event["event"] == "command.failed"),
        "the error exit must still report command.failed after its summary"
    );
    let summary_index = lines
        .iter()
        .position(|event| event["event"] == "summary")
        .expect("summary present");
    let failed_index = lines
        .iter()
        .position(|event| event["event"] == "command.failed")
        .expect("command.failed present");
    assert!(
        summary_index < failed_index,
        "the summary must precede command.failed as the terminal accounting"
    );
}

/// Regression for defect 10 / finding E: an identical second copy is a genuine
/// no-op. In HUMAN mode (no `--json`, no `--yes`, no stdin) it must NOT render
/// the `Configs copy plan` table or a `0 changes` footer, must NOT prompt for
/// confirmation (so it cannot hang or fail on the missing stdin), must exit 0,
/// and must state the specific no-op result. Using `--json` here — as an
/// earlier version of this test did — cannot exercise the interactive plan
/// rendering or prompting this contract is about, so this runs in human mode.
#[test]
fn an_identical_second_copy_is_a_noop_without_a_prompt() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    // First copy: applies normally.
    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .success();
    let after_first = snapshot(&to);

    // Second copy: HUMAN mode, no `--yes` and no stdin. A no-op must NOT prompt,
    // so this cannot hang or fail waiting for input; it must succeed and render
    // the concise no-op result WITHOUT the plan table or a `0 changes` footer.
    let output = cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
        ])
        .output()
        .expect("run identical second configs copy");
    assert!(
        output.status.success(),
        "an identical second copy must succeed without a prompt: {output:?}"
    );
    let after_second = snapshot(&to);
    assert_eq!(
        after_first, after_second,
        "an identical second copy must not change the destination"
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("Nothing to copy"),
        "the no-op must state the specific no-op result, got: {stdout}"
    );
    assert!(
        !stdout.contains("Configs copy plan"),
        "a genuine no-op must not render the plan table, got: {stdout}"
    );
    assert!(
        !stdout.contains(" changes") && !stdout.contains("0 change"),
        "a genuine no-op must not render a plan `changes` footer, got: {stdout}"
    );
    assert!(
        !stdout.contains("Seed "),
        "a genuine no-op must not render a confirmation prompt, got: {stdout}"
    );
}

/// Regression for defect 7: the reserved `cache`/`backups`/`locks` exclusion
/// is global and cannot be bypassed by a resolved target that points inside
/// one of them. A target configured at `.skill-manager/cache` must not smuggle
/// its bytes into the destination when `--include-cache` is absent.
#[test]
fn a_target_configured_inside_the_cache_is_still_excluded() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");

    // Configure a custom target whose path resolves inside the reserved cache
    // directory, then plant a secret there.
    cli(scratch.path(), &from)
        .args(["target", "add", ".skill-manager/cache", "--name=cachey"])
        .assert()
        .success();
    write_file(
        &from.join(".skill-manager").join("cache").join("secret.bin"),
        "top secret\n",
    );

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .success();

    assert!(
        !to.join(".skill-manager").join("cache").exists(),
        "a target pointing inside the cache smuggled reserved bytes past the exclusion"
    );
    assert!(
        !to.join(".skill-manager")
            .join("cache")
            .join("secret.bin")
            .exists(),
        "the reserved cache secret was copied despite the default exclusion"
    );
}

/// Regression for defect 9: a `<FROM>` configuration that is present but on an
/// unsupported schema must be a hard error that names the offending file, not
/// a silent fall-through that copies the bytes while resolving targets from
/// somewhere else. `<FROM>` must not be mutated by the attempt.
#[test]
fn a_malformed_source_configuration_is_an_actionable_error() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");

    // An older-schema configuration the command must refuse rather than
    // silently ignore.
    let config = from.join(".skill-manager").join("config.json");
    write_file(&config, "{\"schema_version\": 1}\n");
    let before = snapshot(&from);

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("schema_version"))
        .stderr(predicate::str::contains("config.json"));

    assert!(
        !to.exists(),
        "a rejected source config must not seed anything"
    );
    assert_eq!(
        before,
        snapshot(&from),
        "reading a malformed source config must never mutate <FROM>"
    );
}

/// Regression for finding A: a source-configuration target path that escapes
/// `<FROM>` via `..` must be a hard error that names the offending target,
/// never a silent read outside `<FROM>` and write outside `<TO>`. Writing
/// outside `<TO>` must be impossible by construction.
#[test]
fn a_source_target_escaping_from_is_rejected() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");

    // Sensitive content OUTSIDE both directories that must never be touched.
    let outside = scratch.path().join("outside-source");
    write_file(&outside.join("secret.txt"), "do not read me\n");
    let outside_before = snapshot(&outside);

    // Hand-author a schema-v2 config whose custom target escapes <FROM>.
    let config = serde_json::json!({
        "schema_version": 2,
        "sources": [],
        "targets": { "evil": { "path": "../outside-source", "label": "Evil", "enabled": true } },
        "legacy_target_overrides": {},
        "builtins": {},
        "exclude": [],
    });
    write_file(
        &from.join(".skill-manager").join("config.json"),
        &format!("{config}\n"),
    );

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("config.json"));

    assert!(
        !to.exists(),
        "an escaping target must fail before writing anything to <TO>"
    );
    assert_eq!(
        outside_before,
        snapshot(&outside),
        "an escaping target must never read or write outside <FROM>/<TO>"
    );
}

/// Regression for finding A: the reserved cache/backup/lock exclusion must be
/// applied to the LEXICALLY NORMALIZED target path, so a `..`-obfuscated
/// spelling like `.skill-manager/x/../cache` cannot slip reserved bytes past
/// it (nor escape `<TO>` through a `..` in the destination join).
#[test]
fn a_dotdot_obfuscated_cache_target_is_still_excluded() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");

    let config = serde_json::json!({
        "schema_version": 2,
        "sources": [],
        "targets": {
            "bypass": { "path": ".skill-manager/x/../cache", "label": "Bypass", "enabled": true }
        },
        "legacy_target_overrides": {},
        "builtins": {},
        "exclude": [],
    });
    write_file(
        &from.join(".skill-manager").join("config.json"),
        &format!("{config}\n"),
    );
    write_file(
        &from.join(".skill-manager").join("cache").join("secret.bin"),
        "top secret\n",
    );

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .success();

    assert!(
        !to.join(".skill-manager").join("cache").exists(),
        "a `..`-obfuscated cache target smuggled reserved bytes past the exclusion"
    );
    assert!(
        !to.join(".skill-manager").join("x").exists(),
        "the destination join must not materialize the pre-normalized `x` segment"
    );
}

/// Regression for finding B: an incoming empty DIRECTORY colliding with an
/// existing destination FILE must be caught in preflight — before the plan is
/// rendered or a byte is written — not discovered only when apply fails at
/// `create_dir_all` after already overwriting `config.json`.
#[test]
fn incoming_directory_over_existing_file_fails_before_writing_anything() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    // An empty directory in the source configuration...
    fs::create_dir_all(from.join(".skill-manager").join("emptydir"))
        .expect("create source empty directory");
    // ...collides with a FILE at the same destination path.
    write_file(&to.join(".skill-manager").join("emptydir"), "i am a file\n");

    let output = cli(scratch.path(), &active_home)
        .args([
            "--json",
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .output()
        .expect("run configs copy");
    assert!(
        !output.status.success(),
        "the conflict must fail the command"
    );

    assert!(
        !to.join(".skill-manager").join("config.json").exists(),
        "apply wrote a partial seed despite an incoming-directory conflict"
    );
    assert!(
        fs::symlink_metadata(to.join(".skill-manager").join("emptydir"))
            .expect("emptydir metadata")
            .is_file(),
        "the conflicting path was mutated instead of left untouched"
    );
    let lines = events(output);
    assert!(
        !lines
            .iter()
            .any(|event| event["event"] == "configs.copy.item"),
        "no apply-time item events may be emitted when preflight rejects the plan"
    );
    assert!(
        lines
            .iter()
            .any(|event| event["event"] == "summary" && event["data"]["action"] == "configs.copy"),
        "a terminal summary must close even a preflight-failure exit"
    );
}

/// Regression for finding B, the other direction: an incoming FILE colliding
/// with an existing destination DIRECTORY must also be caught in preflight.
#[test]
fn incoming_file_over_existing_directory_fails_before_writing_anything() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    // `config.json` is a file in the source, but a DIRECTORY at the destination.
    fs::create_dir_all(to.join(".skill-manager").join("config.json"))
        .expect("create destination directory where a file is incoming");

    let output = cli(scratch.path(), &active_home)
        .args([
            "--json",
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .output()
        .expect("run configs copy");
    assert!(
        !output.status.success(),
        "the conflict must fail the command"
    );

    assert!(
        fs::symlink_metadata(to.join(".skill-manager").join("config.json"))
            .expect("config.json metadata")
            .is_dir(),
        "the conflicting directory was mutated instead of left untouched"
    );
    let lines = events(output);
    assert!(
        !lines
            .iter()
            .any(|event| event["event"] == "configs.copy.item"),
        "no apply-time item events may be emitted when preflight rejects the plan"
    );
}

/// Regression for finding C: a destination ancestor swapped for a link AFTER
/// preflight, while the confirmation prompt is waiting, must be re-rejected at
/// apply time so the approved copy cannot write outside `<TO>`. This shrinks
/// the TOCTOU window to "checked immediately before the write"; it is NOT a
/// per-handle TOCTOU guarantee. Preflight passes because the link does not yet
/// exist when it runs — it is planted only after the prompt appears — so this
/// exercises the apply-time recheck specifically.
#[test]
fn a_destination_ancestor_linked_after_preflight_is_rejected_at_apply() {
    use std::io::{Read, Write};
    use std::process::Stdio;
    use std::sync::{Arc, Mutex};

    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    // A directory OUTSIDE <TO> that the escape would write into.
    let outside = scratch.path().join("outside-dir");
    fs::create_dir_all(&outside).expect("create outside dir");
    write_file(&outside.join("keep.txt"), "keep me\n");
    let outside_before = snapshot(&outside);

    // Confirm the platform can make a directory link at all; otherwise skip.
    let probe = scratch.path().join("probe-link");
    if !try_symlink_dir(&outside, &probe) {
        eprintln!("skipping: platform refused to create a directory symlink");
        return;
    }
    fs::remove_dir(&probe)
        .or_else(|_| fs::remove_file(&probe))
        .ok();

    let mut child = std_cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn interactive configs copy");

    // Drain stderr on a thread so we can watch for the confirmation prompt.
    let stderr = child.stderr.take().expect("child stderr");
    let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
    let seen_writer = Arc::clone(&seen);
    let reader = std::thread::spawn(move || {
        let mut stderr = stderr;
        let mut buffer = [0_u8; 256];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => seen_writer
                    .lock()
                    .expect("stderr lock")
                    .extend_from_slice(&buffer[..count]),
            }
        }
    });

    // Wait for the prompt, proving preflight and plan rendering already ran.
    let mut prompted = false;
    for _ in 0..100 {
        if String::from_utf8_lossy(&seen.lock().expect("stderr lock")).contains("Seed") {
            prompted = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(prompted, "the interactive copy never reached its prompt");

    // Now plant the link at `TO/.claude`, then approve. `TO/.skill-manager`
    // and `TO/.claude` did not exist during preflight.
    assert!(
        try_symlink_dir(&outside, &to.join(".claude")),
        "failed to plant the post-preflight link"
    );
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(b"y\n")
        .expect("approve the copy");
    let status = child.wait().expect("await configs copy");
    reader.join().expect("join stderr reader");

    assert!(
        !status.success(),
        "an ancestor linked after preflight must fail the approved copy, not exit 0"
    );
    assert_eq!(
        outside_before,
        snapshot(&outside),
        "the approved copy wrote outside <TO> through a post-preflight link"
    );
}

/// Finding D: a recipe-VALIDATION failure emits only `command.failed`, with no
/// terminal `summary`. This is verified to match sibling commands (see the
/// investigation in the report): recipe validation fails in the shared
/// pre-dispatch path before any command lifecycle begins, so no `summary` is
/// emitted for ANY command. `configs copy` is consistent, so this locks that
/// consistency in rather than special-casing a summary here. A genuine RUNTIME
/// failure still emits `summary` then `command.failed` (see
/// `error_exits_still_emit_a_terminal_summary`).
#[test]
fn a_recipe_validation_failure_emits_only_command_failed() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let active_home = scratch.path().join("active-home");

    // `from` is the wrong type: a pre-dispatch recipe validation failure.
    let recipe = serde_json::json!({
        "command": "configs.copy",
        "from": false,
        "to": "x",
    });
    let output = cli(scratch.path(), &active_home)
        .arg(format!("--json={recipe}"))
        .output()
        .expect("run configs copy recipe");
    assert!(!output.status.success(), "a malformed recipe must fail");

    let lines = events(output);
    assert!(
        lines.iter().any(|event| event["event"] == "command.failed"),
        "a recipe-validation failure must report command.failed"
    );
    assert!(
        !lines.iter().any(|event| event["event"] == "summary"),
        "a pre-dispatch recipe-validation failure emits no summary, matching siblings"
    );

    // Sibling parity: `remove` behaves identically for the same failure class.
    let sibling = serde_json::json!({ "command": "remove", "skills": false });
    let sibling_output = cli(scratch.path(), &active_home)
        .arg(format!("--json={sibling}"))
        .output()
        .expect("run remove recipe");
    let sibling_lines = events(sibling_output);
    assert!(
        sibling_lines
            .iter()
            .any(|event| event["event"] == "command.failed")
            && !sibling_lines
                .iter()
                .any(|event| event["event"] == "summary"),
        "sibling recipe-validation failures also emit only command.failed"
    );
}

/// Author a schema-v2 `<FROM>` config whose single custom target is
/// `linked-target`, and stage outside content the target's link would expose.
/// Returns the outside directory so the caller can assert it stays untouched.
fn from_with_linked_target(scratch: &TempDir, from: &Path) -> std::path::PathBuf {
    let outside = scratch.path().join("outside-target");
    write_file(&outside.join("demo").join("SKILL.md"), "# outside\n");
    let config = serde_json::json!({
        "schema_version": 2,
        "sources": [],
        "targets": {
            "linked": { "path": "linked-target", "label": "Linked", "enabled": true }
        },
        "legacy_target_overrides": {},
        "builtins": {},
        "exclude": [],
    });
    write_file(
        &from.join(".skill-manager").join("config.json"),
        &format!("{config}\n"),
    );
    outside
}

/// Regression for finding G: a configured target ROOT that is a directory
/// SYMLINK pointing outside `<FROM>` must be skipped, never descended, so the
/// copy cannot read outside `<FROM>` and write outside content into `<TO>`.
/// `WalkDir` follows a linked root even with `follow_links(false)`, and
/// `is_dir()` follows links, so the fix must stat the root with
/// `symlink_metadata` before descending. Skips only if the platform refuses to
/// create a directory symlink at all (the junction test below always runs on
/// Windows and needs no privilege).
#[test]
fn a_symlinked_target_root_is_not_followed_outside_from() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    let outside = from_with_linked_target(&scratch, &from);
    let outside_before = snapshot(&outside);

    if !try_symlink_dir(&outside, &from.join("linked-target")) {
        eprintln!("skipping: platform refused to create a directory symlink");
        return;
    }

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .success();

    assert!(
        !to.join("linked-target").exists(),
        "a linked target root must be skipped, not materialized under <TO>"
    );
    assert_eq!(
        outside_before,
        snapshot(&outside),
        "a linked target root must never read or write the outside directory"
    );
}

/// Regression for finding G on Windows specifically: a configured target ROOT
/// that is a directory JUNCTION (an `IO_REPARSE_TAG_MOUNT_POINT` reparse
/// point) pointing outside `<FROM>` must also be skipped. This proves the
/// reparse-tag-aware link classifier recognizes a real junction — the exact
/// filesystem object a user plants with `mklink /J` — as a source link.
/// Junction creation needs no privilege, so it always runs.
#[cfg(windows)]
#[test]
fn a_junctioned_target_root_is_not_followed_outside_from() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    let outside = from_with_linked_target(&scratch, &from);
    let outside_before = snapshot(&outside);

    make_junction(&outside, &from.join("linked-target"));

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .success();

    assert!(
        !to.join("linked-target").exists(),
        "a junctioned target root must be skipped, not materialized under <TO>"
    );
    assert_eq!(
        outside_before,
        snapshot(&outside),
        "a junctioned target root must never read or write the outside directory"
    );
}

/// Regression for finding K: a configured source target whose ROOT is a link is
/// not silently dropped — it is reported as an EXPLICIT skip in the human
/// output, so a user seeding a scratch home can see the seed is deliberately
/// incomplete rather than assuming everything was copied. The configuration is
/// still copied (a real write), so this run is NOT a no-op. Skips only if the
/// platform refuses to create a directory symlink at all.
#[test]
fn a_linked_target_root_is_reported_as_an_explicit_skip() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    let outside = from_with_linked_target(&scratch, &from);

    if !try_symlink_dir(&outside, &from.join("linked-target")) {
        eprintln!("skipping: platform refused to create a directory symlink");
        return;
    }

    let output = cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .output()
        .expect("run configs copy");
    assert!(output.status.success(), "configs copy failed: {output:?}");

    // The configuration was genuinely copied, so this is not a no-op.
    assert!(
        to.join(".skill-manager").join("config.json").exists(),
        "the real configuration must still be copied alongside the link-skip"
    );
    assert!(
        !to.join("linked-target").exists(),
        "a linked target root must never be materialized under <TO>"
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("linked source"),
        "a linked target root must be reported as an explicit link-skip, got: {stdout}"
    );
    assert!(
        !stdout.contains("Nothing to copy"),
        "a run with a real write plus a link-skip is not a no-op, got: {stdout}"
    );
}

/// Regression for finding K: a run whose ONLY work is a link-skip (no writes at
/// all) must still surface that skip — it must render its plan and the
/// `linked source` skip line, and must NOT masquerade as a genuine no-op
/// (`Nothing to copy`). It also must not prompt, because there is nothing to
/// authorize, so this runs in human mode without `--yes` and cannot hang.
#[test]
fn a_link_skip_only_copy_is_not_treated_as_a_noop() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");

    // No `.skill-manager` config in FROM, so targets resolve from the built-in
    // defaults. Make the default `.claude/skills` root a link pointing outside
    // FROM: it is the only planned item and it is a link-skip.
    let outside = scratch.path().join("outside-claude");
    write_file(&outside.join("demo").join("SKILL.md"), "# outside\n");
    let outside_before = snapshot(&outside);
    if !try_symlink_dir(&outside, &from.join(".claude").join("skills")) {
        eprintln!("skipping: platform refused to create a directory symlink");
        return;
    }

    let output = cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
        ])
        .output()
        .expect("run link-skip-only configs copy");
    assert!(
        output.status.success(),
        "a link-skip-only copy must succeed without a prompt: {output:?}"
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        !stdout.contains("Nothing to copy"),
        "a link-skip is not a no-op, so it must not report `Nothing to copy`, got: {stdout}"
    );
    assert!(
        stdout.contains("linked source"),
        "a link-skip-only run must still render the explicit link-skip, got: {stdout}"
    );
    assert!(
        !to.join(".claude").join("skills").exists(),
        "a linked default target root must never be materialized under <TO>"
    );
    assert_eq!(
        outside_before,
        snapshot(&outside),
        "a link-skip-only run must never read or write the outside directory"
    );
}

/// Author an outside schema-v2 config whose custom target
/// `chosen-by-outside-config` would steer the copy if it were ever read, and
/// stage the in-FROM directory that target names. Returns the outside config
/// directory so the caller can link `<FROM>/.skill-manager` at it.
fn outside_config_dir_that_would_steer(scratch: &TempDir, from: &Path) -> std::path::PathBuf {
    let outside = scratch.path().join("outside-config");
    let config = serde_json::json!({
        "schema_version": 2,
        "sources": [],
        "targets": {
            "chosen": {
                "path": "chosen-by-outside-config",
                "label": "Chosen",
                "enabled": true
            }
        },
        "legacy_target_overrides": {},
        "builtins": {},
        "exclude": [],
    });
    write_file(&outside.join("config.json"), &format!("{config}\n"));
    // The directory the outside config would resolve as a target lives inside
    // FROM, so the only thing that could pull it into the copy is READING the
    // outside config through the linked root.
    write_file(
        &from
            .join("chosen-by-outside-config")
            .join("skill")
            .join("SKILL.md"),
        "# steered\n",
    );
    outside
}

/// Assert the outcome shared by both configuration-root link tests below: the
/// outside config was never read, so its custom target never steered the copy,
/// and the linked configuration root was reported as an explicit skip rather
/// than copied or silently dropped.
fn assert_config_root_link_did_not_steer(to: &Path, output: std::process::Output) {
    assert!(output.status.success(), "configs copy failed: {output:?}");
    assert!(
        !to.join("chosen-by-outside-config").exists(),
        "an outside configuration read through a linked root must not steer the copy"
    );
    assert!(
        !to.join(".skill-manager").exists(),
        "a linked configuration root must never be copied into <TO>"
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("linked source"),
        "a linked configuration root must be reported as an explicit link-skip, got: {stdout}"
    );
}

/// Regression for finding J: `read_seed_config` must gate the `.skill-manager`
/// configuration ROOT with the same link check as target roots BEFORE reading
/// `config.json`, or an outside config linked in as `<FROM>/.skill-manager`
/// would be read and could steer the copy to directories the user never
/// configured. This covers the directory-SYMLINK (`mklink /D`) form; skips only
/// if the platform refuses to create a directory symlink.
#[test]
fn a_symlinked_configuration_root_does_not_steer_the_copy() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    let outside = outside_config_dir_that_would_steer(&scratch, &from);

    if !try_symlink_dir(&outside, &from.join(".skill-manager")) {
        eprintln!("skipping: platform refused to create a directory symlink");
        return;
    }

    let output = cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .output()
        .expect("run configs copy");
    assert_config_root_link_did_not_steer(&to, output);
}

/// Regression for finding J on Windows specifically: the configuration ROOT
/// gate must reject a real directory JUNCTION (`mklink /J`) linked in as
/// `<FROM>/.skill-manager`, not just a symlink, before `config.json` is read.
/// Junctions need no privilege, so this always runs on Windows.
#[cfg(windows)]
#[test]
fn a_junctioned_configuration_root_does_not_steer_the_copy() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    let outside = outside_config_dir_that_would_steer(&scratch, &from);

    make_junction(&outside, &from.join(".skill-manager"));

    let output = cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .output()
        .expect("run configs copy");
    assert_config_root_link_did_not_steer(&to, output);
}

/// Regression for finding H: an empty directory present under `<FROM>` but
/// missing at `<TO>` is real work and must be recreated, not silently ignored
/// as a no-op. File-only diffing missed this because an empty folder has no
/// regular files to compare.
#[test]
fn an_empty_directory_missing_at_the_destination_is_recreated() {
    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    // A required, intentionally EMPTY directory inside the copied configuration.
    fs::create_dir_all(from.join(".skill-manager").join("required-empty"))
        .expect("create required-empty");

    cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .assert()
        .success();
    assert!(
        to.join(".skill-manager").join("required-empty").is_dir(),
        "the first copy must materialize the empty directory"
    );

    // Delete it at the destination, then copy again. The contract "copies
    // folders, deletes nothing" means this second copy is NOT a no-op: the
    // missing folder is work and must be recreated.
    fs::remove_dir(to.join(".skill-manager").join("required-empty"))
        .expect("remove required-empty from destination");

    let output = cli(scratch.path(), &active_home)
        .args([
            "--json",
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
            "--yes",
        ])
        .output()
        .expect("run second configs copy");
    assert!(output.status.success(), "second copy must succeed");

    let lines = events(output);
    assert!(
        lines
            .iter()
            .any(|event| event["event"] == "configs.copy.item"),
        "a missing empty directory must count as work, not a silent no-op"
    );
    assert!(
        to.join(".skill-manager").join("required-empty").is_dir(),
        "the second copy must recreate the deleted empty directory"
    );
}

/// Regression for finding C on Windows specifically: the post-plan ancestor
/// recheck must reject a real JUNCTION planted at a destination ancestor after
/// preflight. This proves the exact object a user creates with `mklink /J` is
/// classified as link-like and caught at apply time. Junctions need no
/// privilege, so this is deterministic on Windows CI.
#[cfg(windows)]
#[test]
fn a_junctioned_destination_ancestor_after_preflight_is_rejected_at_apply() {
    use std::io::{Read, Write};
    use std::process::Stdio;
    use std::sync::{Arc, Mutex};

    let scratch = tempfile::tempdir().expect("scratch root");
    let from = scratch.path().join("from");
    let to = scratch.path().join("to");
    let active_home = scratch.path().join("active-home");
    seed_real_home_with_claude_skill(&scratch, &from, "alpha");

    let outside = scratch.path().join("outside-dir");
    fs::create_dir_all(&outside).expect("create outside dir");
    write_file(&outside.join("keep.txt"), "keep me\n");
    let outside_before = snapshot(&outside);

    let mut child = std_cli(scratch.path(), &active_home)
        .args([
            "configs",
            "copy",
            from.to_str().expect("utf8 from path"),
            to.to_str().expect("utf8 to path"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn interactive configs copy");

    let stderr = child.stderr.take().expect("child stderr");
    let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
    let seen_writer = Arc::clone(&seen);
    let reader = std::thread::spawn(move || {
        let mut stderr = stderr;
        let mut buffer = [0_u8; 256];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => seen_writer
                    .lock()
                    .expect("stderr lock")
                    .extend_from_slice(&buffer[..count]),
            }
        }
    });

    let mut prompted = false;
    for _ in 0..100 {
        if String::from_utf8_lossy(&seen.lock().expect("stderr lock")).contains("Seed") {
            prompted = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(prompted, "the interactive copy never reached its prompt");

    // Plant a real junction (NOT a symlink) at `TO/.claude`, then approve.
    make_junction(&outside, &to.join(".claude"));
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(b"y\n")
        .expect("approve the copy");
    let status = child.wait().expect("await configs copy");
    reader.join().expect("join stderr reader");

    assert!(
        !status.success(),
        "a junctioned ancestor planted after preflight must fail the approved copy"
    );
    assert_eq!(
        outside_before,
        snapshot(&outside),
        "the approved copy wrote outside <TO> through a post-preflight junction"
    );
}
