//! Locked, journaled per-skill filesystem transactions and recovery.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::acquire_lock;
use crate::error::{Result, SkillManagerError};
use crate::skills::{skill_name, validate_skill_name, validate_skill_tree};

/// Failure-injection boundary used by transaction tests.
pub trait TransactionHook {
    /// Called after a durable transaction state is recorded.
    ///
    /// # Errors
    ///
    /// Test implementations may return an injected failure.
    fn after_state(&self, state: TransactionState) -> Result<()>;
}

/// Production hook which never injects a failure.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopTransactionHook;

impl TransactionHook for NoopTransactionHook {
    fn after_state(&self, _state: TransactionState) -> Result<()> {
        Ok(())
    }
}

/// Durable transaction states.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransactionState {
    /// Replacement is fully staged.
    Prepared,
    /// Previous deployment has moved to backup.
    OldMoved,
    /// New deployment is visible.
    Committed,
}

#[derive(Debug, Deserialize, Serialize)]
struct Journal {
    state: TransactionState,
    destination: PathBuf,
    stage: Option<PathBuf>,
    backup: PathBuf,
}

/// Transactionally replace one deployed skill.
///
/// # Errors
///
/// Returns an error for unsafe source data, lock contention, injected failure, or I/O.
pub fn deploy_skill<H: TransactionHook>(
    source: &Path,
    target_root: &Path,
    cache_root: &Path,
    hook: &H,
) -> Result<PathBuf> {
    validate_skill_tree(source)?;
    let name = skill_name(source)?;
    fs::create_dir_all(target_root).map_err(|error| SkillManagerError::io(target_root, error))?;
    let paths = transaction_paths(target_root, cache_root, &name);
    let _lock = acquire_lock(
        &paths.lock,
        &format!("target {}", target_root.display()),
        Duration::from_secs(10),
    )?;
    recover_journal(&paths.journal)?;

    let staging_parent = target_root.join(".skill-manager-staging");
    fs::create_dir_all(&staging_parent)
        .map_err(|error| SkillManagerError::io(&staging_parent, error))?;
    let staging = tempfile::Builder::new()
        .prefix(&format!("{name}-"))
        .tempdir_in(&staging_parent)
        .map_err(|error| SkillManagerError::io(&staging_parent, error))?;
    let staged_content = staging.path().join("content");
    copy_tree(source, &staged_content)?;
    validate_skill_tree(&staged_content)?;

    let destination = target_root.join(&name);
    if let Some(parent) = paths.backup.parent() {
        fs::create_dir_all(parent).map_err(|error| SkillManagerError::io(parent, error))?;
    }
    let mut journal = Journal {
        state: TransactionState::Prepared,
        destination: destination.clone(),
        stage: Some(staged_content.clone()),
        backup: paths.backup.clone(),
    };
    write_journal(&paths.journal, &journal)?;
    hook.after_state(TransactionState::Prepared)?;

    if destination.exists() {
        fs::rename(&destination, &paths.backup)
            .map_err(|error| SkillManagerError::io(&destination, error))?;
        journal.state = TransactionState::OldMoved;
        write_journal(&paths.journal, &journal)?;
        hook.after_state(TransactionState::OldMoved)?;
    }
    if let Err(error) = fs::rename(&staged_content, &destination) {
        if paths.backup.exists() && !destination.exists() {
            let _rollback = fs::rename(&paths.backup, &destination);
        }
        return Err(SkillManagerError::io(&staged_content, error));
    }
    journal.state = TransactionState::Committed;
    journal.stage = None;
    write_journal(&paths.journal, &journal)?;
    hook.after_state(TransactionState::Committed)?;
    cleanup_committed(&paths.journal, &paths.backup)?;
    cleanup_empty_dir(&staging_parent);
    cleanup_empty_parent(&paths.backup);
    Ok(destination)
}

/// Transactionally remove one deployed skill.
///
/// # Errors
///
/// Returns an error for an invalid name, lock contention, injected failure, or I/O.
pub fn remove_skill<H: TransactionHook>(
    name: &str,
    target_root: &Path,
    cache_root: &Path,
    hook: &H,
) -> Result<bool> {
    validate_skill_name(name)?;
    if !target_root.exists() {
        return Ok(false);
    }
    let paths = transaction_paths(target_root, cache_root, name);
    let _lock = acquire_lock(
        &paths.lock,
        &format!("target {}", target_root.display()),
        Duration::from_secs(10),
    )?;
    recover_journal(&paths.journal)?;
    let destination = target_root.join(name);
    if !destination.exists() {
        return Ok(false);
    }
    if let Some(parent) = paths.backup.parent() {
        fs::create_dir_all(parent).map_err(|error| SkillManagerError::io(parent, error))?;
    }
    let mut journal = Journal {
        state: TransactionState::Prepared,
        destination: destination.clone(),
        stage: None,
        backup: paths.backup.clone(),
    };
    write_journal(&paths.journal, &journal)?;
    hook.after_state(TransactionState::Prepared)?;
    fs::rename(&destination, &paths.backup)
        .map_err(|error| SkillManagerError::io(&destination, error))?;
    journal.state = TransactionState::OldMoved;
    write_journal(&paths.journal, &journal)?;
    hook.after_state(TransactionState::OldMoved)?;
    journal.state = TransactionState::Committed;
    write_journal(&paths.journal, &journal)?;
    hook.after_state(TransactionState::Committed)?;
    cleanup_committed(&paths.journal, &paths.backup)?;
    cleanup_empty_parent(&paths.backup);
    Ok(true)
}

/// Recover an interrupted transaction from a durable journal.
///
/// # Errors
///
/// Returns an error when journal parsing or recovery I/O fails.
#[allow(
    clippy::too_many_lines,
    reason = "Recovery validates every journal-controlled path before a single mutation, then handles all durable states together for auditability."
)]
pub fn recover_journal(path: &Path) -> Result<()> {
    let journal_root = path.parent().ok_or_else(|| {
        SkillManagerError::InvalidInput("transaction journal has no parent".into())
    })?;
    if journal_root.file_name().and_then(std::ffi::OsStr::to_str) != Some(".skill-manager-journals")
    {
        return Err(SkillManagerError::InvalidInput(format!(
            "unexpected transaction journal path: {}",
            path.display()
        )));
    }
    let target_root = journal_root.parent().ok_or_else(|| {
        SkillManagerError::InvalidInput("transaction journal has no target root".into())
    })?;
    if !path.exists() {
        return Ok(());
    }
    let bytes = fs::read(path).map_err(|error| SkillManagerError::io(path, error))?;
    let journal: Journal = serde_json::from_slice(&bytes).map_err(|error| {
        SkillManagerError::InvalidInput(format!(
            "transaction journal {} is invalid: {error}",
            path.display()
        ))
    })?;
    let name = journal
        .destination
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| SkillManagerError::InvalidInput("invalid journal destination".into()))?;
    validate_skill_name(name)?;
    let expected = transaction_paths(target_root, Path::new(""), name);
    let expected_destination = target_root.join(name);
    let staging_root = target_root.join(".skill-manager-staging");
    let valid_stage = journal.stage.as_ref().is_none_or(|stage| {
        stage.file_name().is_some_and(|value| value == "content")
            && stage
                .parent()
                .and_then(Path::parent)
                .is_some_and(|parent| parent == staging_root)
            && stage
                .parent()
                .and_then(Path::file_name)
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|directory| directory.starts_with(&format!("{name}-")))
    });
    if path != expected.journal
        || journal.destination != expected_destination
        || journal.backup != expected.backup
        || !valid_stage
    {
        return Err(SkillManagerError::InvalidInput(format!(
            "transaction journal {} names paths outside its transaction",
            path.display()
        )));
    }
    for manager_root in [
        target_root.join(".skill-manager-staging"),
        target_root.join(".skill-manager-backups"),
        target_root.join(".skill-manager-journals"),
    ] {
        if manager_root.exists()
            && fs::symlink_metadata(&manager_root)
                .map_err(|error| SkillManagerError::io(&manager_root, error))?
                .file_type()
                .is_symlink()
        {
            return Err(SkillManagerError::InvalidInput(format!(
                "transaction manager path must not be linked: {}",
                manager_root.display()
            )));
        }
    }
    match journal.state {
        TransactionState::Prepared => {
            if journal.backup.exists() && !journal.destination.exists() {
                fs::rename(&journal.backup, &journal.destination)
                    .map_err(|error| SkillManagerError::io(&journal.backup, error))?;
            } else if journal.backup.exists() {
                fs::remove_dir_all(&journal.backup)
                    .map_err(|error| SkillManagerError::io(&journal.backup, error))?;
            }
            if let Some(stage) = journal.stage
                && stage.exists()
            {
                fs::remove_dir_all(&stage).map_err(|error| SkillManagerError::io(&stage, error))?;
            }
        }
        TransactionState::OldMoved => {
            if journal.backup.exists() && !journal.destination.exists() {
                fs::rename(&journal.backup, &journal.destination)
                    .map_err(|error| SkillManagerError::io(&journal.backup, error))?;
            } else if journal.backup.exists() {
                fs::remove_dir_all(&journal.backup)
                    .map_err(|error| SkillManagerError::io(&journal.backup, error))?;
            }
        }
        TransactionState::Committed => {
            if journal.backup.exists() {
                fs::remove_dir_all(&journal.backup)
                    .map_err(|error| SkillManagerError::io(&journal.backup, error))?;
            }
        }
    }
    fs::remove_file(path).map_err(|error| SkillManagerError::io(path, error))
}

struct TransactionPaths {
    journal: PathBuf,
    backup: PathBuf,
    lock: PathBuf,
}

fn transaction_paths(target: &Path, cache: &Path, name: &str) -> TransactionPaths {
    let canonical_target = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let target_key = hex::encode(Sha256::digest(
        canonical_target.to_string_lossy().as_bytes(),
    ));
    let skill_key = hex::encode(Sha256::digest(name.as_bytes()));
    TransactionPaths {
        journal: target
            .join(".skill-manager-journals")
            .join(format!("{}.json", &skill_key[..24])),
        backup: target.join(".skill-manager-backups").join(&skill_key[..24]),
        lock: cache
            .join(".locks")
            .join(format!("target-{}.lock", &target_key[..24])),
    }
}

fn write_journal(path: &Path, journal: &Journal) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        SkillManagerError::InvalidInput(format!(
            "transaction journal has no parent: {}",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| SkillManagerError::io(parent, error))?;
    let mut data = serde_json::to_vec(journal)
        .map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
    data.push(b'\n');
    fs::write(path, data).map_err(|error| SkillManagerError::io(path, error))?;
    FileSync::sync(path)
}

struct FileSync;

impl FileSync {
    fn sync(path: &Path) -> Result<()> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| SkillManagerError::io(path, error))?;
        file.sync_all()
            .map_err(|error| SkillManagerError::io(path, error))
    }
}

fn cleanup_committed(journal: &Path, backup: &Path) -> Result<()> {
    if backup.exists() {
        fs::remove_dir_all(backup).map_err(|error| SkillManagerError::io(backup, error))?;
    }
    fs::remove_file(journal).map_err(|error| SkillManagerError::io(journal, error))
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir(destination).map_err(|error| SkillManagerError::io(destination, error))?;
    for item in walkdir::WalkDir::new(source).follow_links(false) {
        let item = item.map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
        let relative = item.path().strip_prefix(source).map_err(|error| {
            SkillManagerError::InvalidInput(format!("invalid skill source path: {error}"))
        })?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let output = destination.join(relative);
        if item.file_type().is_dir() {
            fs::create_dir_all(&output).map_err(|error| SkillManagerError::io(&output, error))?;
        } else if item.file_type().is_file() {
            fs::copy(item.path(), &output)
                .map_err(|error| SkillManagerError::io(&output, error))?;
        } else {
            return Err(SkillManagerError::InvalidInput(format!(
                "skill contains an unsupported entry: {}",
                item.path().display()
            )));
        }
    }
    Ok(())
}

fn cleanup_empty_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        cleanup_empty_dir(parent);
    }
}

fn cleanup_empty_dir(path: &Path) {
    let _result = fs::remove_dir(path);
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        Journal, NoopTransactionHook, TransactionHook, TransactionState, deploy_skill,
        recover_journal, remove_skill, transaction_paths, write_journal,
    };
    use crate::error::{Result, SkillManagerError};

    struct FailAt(TransactionState);

    impl TransactionHook for FailAt {
        fn after_state(&self, state: TransactionState) -> Result<()> {
            if state == self.0 {
                Err(SkillManagerError::InvalidInput(
                    "injected transaction failure".into(),
                ))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn deploy_replaces_a_skill() {
        let source_root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let source = source_root.path().join("demo");
        std::fs::create_dir(&source).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(source.join("SKILL.md"), "new")
            .unwrap_or_else(|error| unreachable!("{error}"));
        let target = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let cache = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let deployed = deploy_skill(&source, target.path(), cache.path(), &NoopTransactionHook)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            std::fs::read_to_string(deployed.join("SKILL.md"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            "new"
        );
    }

    #[test]
    fn interrupted_deploy_and_remove_recover_on_the_next_operation() {
        let source_root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let source = source_root.path().join("demo");
        std::fs::create_dir(&source).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(source.join("SKILL.md"), "one")
            .unwrap_or_else(|error| unreachable!("{error}"));
        let target = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let cache = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        deploy_skill(&source, target.path(), cache.path(), &NoopTransactionHook)
            .unwrap_or_else(|error| unreachable!("{error}"));

        std::fs::write(source.join("SKILL.md"), "two")
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            deploy_skill(
                &source,
                target.path(),
                cache.path(),
                &FailAt(TransactionState::OldMoved),
            )
            .is_err()
        );
        deploy_skill(&source, target.path(), cache.path(), &NoopTransactionHook)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            std::fs::read_to_string(target.path().join("demo").join("SKILL.md"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            "two"
        );

        assert!(
            remove_skill(
                "demo",
                target.path(),
                cache.path(),
                &FailAt(TransactionState::OldMoved),
            )
            .is_err()
        );
        assert!(
            remove_skill("demo", target.path(), cache.path(), &NoopTransactionHook,)
                .unwrap_or_else(|error| unreachable!("{error}"))
        );
        assert!(!target.path().join("demo").exists());
    }

    #[test]
    fn prepared_and_committed_deploy_failures_are_recovered() {
        let source_root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let source = source_root.path().join("demo");
        std::fs::create_dir(&source).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(source.join("SKILL.md"), "one")
            .unwrap_or_else(|error| unreachable!("{error}"));
        let target = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let cache = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));

        assert!(
            deploy_skill(
                &source,
                target.path(),
                cache.path(),
                &FailAt(TransactionState::Prepared),
            )
            .is_err()
        );
        let paths = transaction_paths(target.path(), cache.path(), "demo");
        assert!(paths.journal.exists());
        deploy_skill(&source, target.path(), cache.path(), &NoopTransactionHook)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!paths.journal.exists());

        std::fs::write(source.join("SKILL.md"), "two")
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            deploy_skill(
                &source,
                target.path(),
                cache.path(),
                &FailAt(TransactionState::Committed),
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(target.path().join("demo").join("SKILL.md"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            "two"
        );
        deploy_skill(&source, target.path(), cache.path(), &NoopTransactionHook)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!paths.backup.exists());
        assert!(!paths.journal.exists());
    }

    #[test]
    fn remove_handles_absent_targets_invalid_names_and_failure_states() {
        let target = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let cache = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            !remove_skill(
                "demo",
                &target.path().join("missing-root"),
                cache.path(),
                &NoopTransactionHook,
            )
            .unwrap_or_else(|error| unreachable!("{error}"))
        );
        assert!(
            !remove_skill("demo", target.path(), cache.path(), &NoopTransactionHook)
                .unwrap_or_else(|error| unreachable!("{error}"))
        );
        assert!(
            remove_skill(
                "../unsafe",
                target.path(),
                cache.path(),
                &NoopTransactionHook
            )
            .is_err()
        );

        let deployed = target.path().join("demo");
        std::fs::create_dir(&deployed).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(deployed.join("SKILL.md"), "one")
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            remove_skill(
                "demo",
                target.path(),
                cache.path(),
                &FailAt(TransactionState::Prepared),
            )
            .is_err()
        );
        assert!(deployed.exists());
        assert!(
            remove_skill(
                "demo",
                target.path(),
                cache.path(),
                &FailAt(TransactionState::Committed),
            )
            .is_err()
        );
        assert!(!deployed.exists());
        assert!(
            !remove_skill("demo", target.path(), cache.path(), &NoopTransactionHook)
                .unwrap_or_else(|error| unreachable!("{error}"))
        );
    }

    #[test]
    fn direct_recovery_restores_prepared_backup_and_rejects_corrupt_journal() {
        let target = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let cache = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let paths = transaction_paths(target.path(), cache.path(), "demo");
        let destination = target.path().join("demo");
        let stage = target
            .path()
            .join(".skill-manager-staging")
            .join("demo-interrupted")
            .join("content");
        std::fs::create_dir_all(&stage).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(stage.join("SKILL.md"), "staged")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::create_dir_all(&paths.backup).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(paths.backup.join("SKILL.md"), "old")
            .unwrap_or_else(|error| unreachable!("{error}"));
        write_journal(
            &paths.journal,
            &Journal {
                state: TransactionState::Prepared,
                destination: destination.clone(),
                stage: Some(stage.clone()),
                backup: paths.backup.clone(),
            },
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        recover_journal(&paths.journal).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            std::fs::read_to_string(destination.join("SKILL.md"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            "old"
        );
        assert!(!stage.exists());

        if let Some(parent) = paths.journal.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|error| unreachable!("{error}"));
        }
        std::fs::write(&paths.journal, "{broken").unwrap_or_else(|error| unreachable!("{error}"));
        assert!(recover_journal(&paths.journal).is_err());
        let missing = transaction_paths(target.path(), cache.path(), "missing");
        recover_journal(&missing.journal).unwrap_or_else(|error| unreachable!("{error}"));
    }

    #[test]
    fn recovery_discards_redundant_backups_for_every_durable_state() {
        for state in [
            TransactionState::Prepared,
            TransactionState::OldMoved,
            TransactionState::Committed,
        ] {
            let target = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
            let cache = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
            let paths = transaction_paths(target.path(), cache.path(), "demo");
            let destination = target.path().join("demo");
            std::fs::create_dir(&destination).unwrap_or_else(|error| unreachable!("{error}"));
            std::fs::write(destination.join("SKILL.md"), "current")
                .unwrap_or_else(|error| unreachable!("{error}"));
            std::fs::create_dir_all(&paths.backup).unwrap_or_else(|error| unreachable!("{error}"));
            std::fs::write(paths.backup.join("SKILL.md"), "backup")
                .unwrap_or_else(|error| unreachable!("{error}"));
            let stage = target
                .path()
                .join(".skill-manager-staging")
                .join("demo-redundant")
                .join("content");
            if state == TransactionState::Prepared {
                std::fs::create_dir_all(&stage).unwrap_or_else(|error| unreachable!("{error}"));
            }
            write_journal(
                &paths.journal,
                &Journal {
                    state,
                    destination: destination.clone(),
                    stage: (state == TransactionState::Prepared).then_some(stage.clone()),
                    backup: paths.backup.clone(),
                },
            )
            .unwrap_or_else(|error| unreachable!("{error}"));
            recover_journal(&paths.journal).unwrap_or_else(|error| unreachable!("{error}"));
            assert!(!paths.backup.exists());
            assert!(!stage.exists());
            assert_eq!(
                std::fs::read_to_string(destination.join("SKILL.md"))
                    .unwrap_or_else(|error| unreachable!("{error}")),
                "current"
            );
        }
    }

    #[test]
    fn deployment_creates_missing_roots_and_copies_nested_tree() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let source = root.path().join("source").join("nested-skill");
        std::fs::create_dir_all(source.join("assets").join("nested"))
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(source.join("SKILL.md"), "# Nested")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(
            source.join("assets").join("nested").join("data.txt"),
            "data",
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        let target = root.path().join("missing-target");
        let cache = root.path().join("missing-cache");
        let deployed = deploy_skill(&source, &target, &cache, &NoopTransactionHook)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            std::fs::read_to_string(deployed.join("assets").join("nested").join("data.txt"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            "data"
        );
        assert!(
            write_journal(
                Path::new(""),
                &Journal {
                    state: TransactionState::Prepared,
                    destination: deployed,
                    stage: None,
                    backup: target.join("backup"),
                }
            )
            .is_err()
        );
    }

    #[test]
    fn crafted_journals_cannot_mutate_paths_outside_the_expected_transaction() {
        for state in [TransactionState::Prepared, TransactionState::Committed] {
            let target = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
            let cache = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
            let outside = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
            let sentinel = outside.path().join("sentinel");
            std::fs::create_dir(&sentinel).unwrap_or_else(|error| unreachable!("{error}"));
            std::fs::write(sentinel.join("keep"), "safe")
                .unwrap_or_else(|error| unreachable!("{error}"));
            let paths = transaction_paths(target.path(), cache.path(), "demo");
            write_journal(
                &paths.journal,
                &Journal {
                    state,
                    destination: sentinel.clone(),
                    stage: Some(sentinel.clone()),
                    backup: sentinel.clone(),
                },
            )
            .unwrap_or_else(|error| unreachable!("{error}"));
            assert!(recover_journal(&paths.journal).is_err());
            assert_eq!(
                std::fs::read_to_string(sentinel.join("keep"))
                    .unwrap_or_else(|error| unreachable!("{error}")),
                "safe"
            );
        }
    }
}
