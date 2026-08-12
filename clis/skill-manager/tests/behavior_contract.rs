//! Focused discovery, equality, matching, and status behavior.

#![allow(
    clippy::expect_used,
    reason = "Temporary fixture construction failures are unrecoverable test harness failures."
)]

use std::fs;

use skill_manager::config::source_from_reference;
use skill_manager::domain::SkillState;
use skill_manager::domain::SourceType;
use skill_manager::skills::{directories_equal, matches_patterns, skill_state};

mod support;

use support::portable_canonicalize;

#[test]
fn directory_equality_compares_relative_names_and_contents() {
    let temp = tempfile::tempdir().expect("temporary root");
    let left = temp.path().join("left");
    let right = temp.path().join("right");
    fs::create_dir_all(left.join("empty")).expect("left tree");
    fs::create_dir_all(right.join("different-empty-name")).expect("right tree");
    fs::write(left.join("SKILL.md"), "same").expect("left file");
    fs::write(right.join("SKILL.md"), "same").expect("right file");
    assert!(directories_equal(&left, &right).expect("compare identical files"));

    fs::write(right.join("SKILL.md"), "changed").expect("change content");
    assert!(!directories_equal(&left, &right).expect("compare changed content"));
    fs::write(right.join("SKILL.md"), "same").expect("restore content");

    fs::write(left.join("extra.txt"), "extra").expect("left extra");
    assert!(!directories_equal(&left, &right).expect("compare left extra"));
    fs::remove_file(left.join("extra.txt")).expect("remove left extra");
    fs::write(right.join("extra.txt"), "extra").expect("right extra");
    assert!(!directories_equal(&left, &right).expect("compare right extra"));
    fs::remove_file(right.join("extra.txt")).expect("remove right extra");

    fs::write(left.join("named-left.txt"), "same bytes").expect("left named file");
    fs::write(right.join("named-right.txt"), "same bytes").expect("right named file");
    assert!(!directories_equal(&left, &right).expect("compare different relative names"));
    assert!(!directories_equal(&left, &temp.path().join("missing")).expect("compare missing path"));
}

#[test]
fn status_covers_all_four_states() {
    let temp = tempfile::tempdir().expect("temporary root");
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(&source).expect("source");
    fs::write(source.join("SKILL.md"), "same").expect("source file");
    fs::create_dir_all(target.join("same")).expect("same target");
    fs::write(target.join("same/SKILL.md"), "same").expect("same deployment");
    fs::create_dir_all(target.join("changed")).expect("changed target");
    fs::write(target.join("changed/SKILL.md"), "different").expect("changed deployment");
    fs::create_dir_all(target.join("orphan")).expect("orphan target");
    fs::write(target.join("orphan/SKILL.md"), "orphan").expect("orphan deployment");

    assert_eq!(
        skill_state(Some(&source), &target, "same").expect("same state"),
        SkillState::UpToDate
    );
    assert_eq!(
        skill_state(Some(&source), &target, "changed").expect("changed state"),
        SkillState::NeedsUpdate
    );
    assert_eq!(
        skill_state(Some(&source), &target, "missing").expect("missing state"),
        SkillState::NotLoaded
    );
    assert_eq!(
        skill_state(None, &target, "orphan").expect("orphan state"),
        SkillState::NoConnection
    );
}

#[test]
fn patterns_are_unicode_folded_and_unmatched_brackets_are_literals() {
    assert!(matches_patterns("Straße", &["STRASSE".into()]).expect("Unicode folded match"));
    assert!(matches_patterns("[", &["[".into()]).expect("literal unmatched bracket"));
    assert!(!matches_patterns("alpha", &["[".into()]).expect("unmatched bracket nonmatch"));
    assert!(matches_patterns("alpha", &["a?pha".into()]).expect("question wildcard"));
    assert!(matches_patterns("alpha", &["[a-c]*".into()]).expect("character class"));
}

#[test]
fn source_reference_and_bare_name_resolution_cases() {
    let temp = tempfile::tempdir().expect("temporary root");
    let tree = source_from_reference(
        "https://github.com/acme/skills/tree/main/collection",
        None,
        temp.path(),
    )
    .expect("GitHub tree URL");
    assert_eq!(tree.source_type, SourceType::GitHub);
    assert_eq!(tree.owner.as_deref(), Some("acme"));
    assert_eq!(tree.repo.as_deref(), Some("skills"));
    assert_eq!(tree.r#ref.as_deref(), Some("main"));
    assert_eq!(tree.repo_path.as_deref(), Some("collection"));

    let shorthand = source_from_reference("acme/skills:v2/nested", None, temp.path())
        .expect("GitHub shorthand");
    assert_eq!(shorthand.r#ref.as_deref(), Some("v2"));
    assert_eq!(shorthand.repo_path.as_deref(), Some("nested"));

    let local = source_from_reference(temp.path().to_str().expect("utf8 path"), None, temp.path())
        .expect("local source");
    assert_eq!(local.source_type, SourceType::Local);
    let canonical_temp = portable_canonicalize(temp.path()).expect("canonical temporary path");
    assert_eq!(local.path.as_deref(), Some(canonical_temp.as_path()));
}
