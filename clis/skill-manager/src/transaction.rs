//! Locked, journaled per-skill filesystem transactions and recovery.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::acquire_lock;
use crate::error::{Result, SkillManagerError};
use crate::fs_retry as fs;
use crate::skills::{skill_name, validate_skill_name, validate_skill_tree};
use crate::staging::{reject_link, remove_tree};

/// Failure-injection boundary used by transaction tests.
pub trait TransactionHook {
    /// Called after a durable transaction state is recorded.
    ///
    /// # Errors
    ///
    /// Test implementations may return an injected failure.
    fn after_state(&self, state: TransactionState) -> Result<()>;

    /// Called after replacement placement, immediately before its commit record.
    ///
    /// # Errors
    /// Test implementations may simulate a failed commit-record write.
    fn before_commit(&self) -> Result<()> {
        Ok(())
    }

    /// Called before committed cleanup, for deterministic cleanup failure tests.
    ///
    /// # Errors
    /// An error defers cleanup while preserving the committed result and journal.
    fn before_cleanup(&self) -> Result<()> {
        Ok(())
    }
}

/// A committed operation whose housekeeping may still need recovery.
#[derive(Debug)]
pub struct TransactionOutcome<T> {
    /// Committed filesystem result.
    pub value: T,
    /// Actionable warning when the durable cleanup journal remains.
    pub cleanup_pending: Option<String>,
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
    /// The journal owns a scratch directory which may not yet be populated.
    Staging,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    staging_root: Option<PathBuf>,
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
) -> Result<TransactionOutcome<PathBuf>> {
    let name = skill_name(source)?;
    replace_directory(source, &target_root.join(name), cache_root, hook)
}

/// Transactionally replace one source skill directory with a deployed copy.
///
/// This is the reverse of [`deploy_skill`]: the destination is named
/// explicitly, so a source checkout whose directory name differs from the
/// deployed directory name is still replaced in place.
///
/// # Errors
///
/// Returns an error for unsafe source data, an unusable destination, lock
/// contention, injected failure, or I/O.
pub fn import_skill<H: TransactionHook>(
    deployment: &Path,
    destination: &Path,
    cache_root: &Path,
    hook: &H,
) -> Result<TransactionOutcome<PathBuf>> {
    replace_directory(deployment, destination, cache_root, hook)
}

fn replace_directory<H: TransactionHook>(
    source: &Path,
    destination: &Path,
    cache_root: &Path,
    hook: &H,
) -> Result<TransactionOutcome<PathBuf>> {
    validate_skill_tree(source)?;
    let target_root = destination.parent().ok_or_else(|| {
        SkillManagerError::InvalidInput(format!(
            "replacement destination has no parent directory: {}",
            destination.display()
        ))
    })?;
    let name = replacement_name(destination)?;
    fs::create_dir_all(target_root).map_err(|error| SkillManagerError::io(target_root, error))?;
    let paths = transaction_paths(target_root, cache_root, &name);
    let _lock = acquire_lock(
        &paths.lock,
        &format!("target {}", target_root.display()),
        Duration::from_secs(10),
    )?;
    validate_manager_roots(target_root)?;
    reject_link(destination)?;
    recover_journal(&paths.journal)?;

    if crate::staging::exists(&paths.backup)? {
        return Err(SkillManagerError::InvalidInput(format!(
            "unowned transaction backup {}; inspect and move it aside before retrying; no recovery journal exists",
            paths.backup.display()
        )));
    }

    let staging_parent = target_root.join(".skill-manager-staging");
    fs::create_dir_all(&staging_parent)
        .map_err(|error| SkillManagerError::io(&staging_parent, error))?;
    let staging = staging_parent.join(format!("{name}-pending"));
    reject_link(&staging_parent)?;
    reject_link(&staging)?;
    if crate::staging::exists(&staging)? {
        return Err(SkillManagerError::InvalidInput(format!(
            "unowned staging directory {}; inspect and move it aside before retrying; no transaction journal owns it",
            staging.display()
        )));
    }
    let staged_content = staging.join("content");

    if let Some(parent) = paths.backup.parent() {
        fs::create_dir_all(parent).map_err(|error| SkillManagerError::io(parent, error))?;
    }
    let mut journal = Journal {
        state: TransactionState::Staging,
        destination: destination.to_path_buf(),
        stage: Some(staged_content.clone()),
        backup: paths.backup.clone(),
        staging_root: Some(staging.clone()),
    };
    write_journal(&paths.journal, &journal)?;
    fs::create_dir(&staging).map_err(|error| SkillManagerError::io(&staging, error))?;
    if let Err(error) =
        copy_tree(source, &staged_content).and_then(|()| validate_skill_tree(&staged_content))
    {
        return match recover_journal(&paths.journal) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(SkillManagerError::InvalidInput(format!(
                "{error}; staging cleanup pending in {}: {cleanup}; retry to recover",
                paths.journal.display()
            ))),
        };
    }
    journal.state = TransactionState::Prepared;
    write_journal(&paths.journal, &journal)?;
    hook.after_state(TransactionState::Prepared)?;

    if destination.exists() {
        fs::rename(destination, &paths.backup)
            .map_err(|error| SkillManagerError::io(destination, error))?;
        journal.state = TransactionState::OldMoved;
        write_journal(&paths.journal, &journal)?;
        hook.after_state(TransactionState::OldMoved)?;
    }
    if let Err(error) = fs::rename(&staged_content, destination) {
        if paths.backup.exists() && !destination.exists() {
            let _rollback = fs::rename(&paths.backup, destination);
        }
        return Err(SkillManagerError::io(&staged_content, error));
    }
    journal.state = TransactionState::Committed;
    hook.before_commit().and_then(|()| write_journal(&paths.journal, &journal))
        .map_err(|error| SkillManagerError::InvalidInput(format!(
            "replacement data is installed at {}, but recording committed state failed: {error}; operation interrupted; recovery journal {} retains the prior state and recovery may restore prior content",
            destination.display(), paths.journal.display()
        )))?;
    hook.after_state(TransactionState::Committed)?;
    let cleanup_pending = hook
        .before_cleanup()
        .and_then(|()| cleanup_committed(&paths.journal, &journal))
        .err()
        .map(|error| cleanup_warning(destination, &paths.journal, &error));
    cleanup_empty_dir(&staging_parent);
    cleanup_empty_parent(&paths.backup);
    cleanup_empty_parent(&paths.journal);
    Ok(TransactionOutcome {
        value: destination.to_path_buf(),
        cleanup_pending,
    })
}

fn replacement_name(destination: &Path) -> Result<String> {
    let name = destination
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| {
            SkillManagerError::InvalidInput(format!(
                "replacement destination is not a portable name: {}",
                destination.display()
            ))
        })?
        .to_owned();
    validate_skill_name(&name)?;
    Ok(name)
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
) -> Result<TransactionOutcome<bool>> {
    validate_skill_name(name)?;
    if !target_root.exists() {
        return Ok(TransactionOutcome {
            value: false,
            cleanup_pending: None,
        });
    }
    let paths = transaction_paths(target_root, cache_root, name);
    let _lock = acquire_lock(
        &paths.lock,
        &format!("target {}", target_root.display()),
        Duration::from_secs(10),
    )?;
    validate_manager_roots(target_root)?;
    recover_journal(&paths.journal)?;
    let destination = target_root.join(name);
    reject_link(&destination)?;
    if crate::staging::exists(&paths.backup)? {
        return Err(SkillManagerError::InvalidInput(format!(
            "unowned transaction backup {}; inspect and move it aside before retrying; no recovery journal exists",
            paths.backup.display()
        )));
    }
    if !destination.exists() {
        return Ok(TransactionOutcome {
            value: false,
            cleanup_pending: None,
        });
    }
    if let Some(parent) = paths.backup.parent() {
        fs::create_dir_all(parent).map_err(|error| SkillManagerError::io(parent, error))?;
    }
    let mut journal = Journal {
        state: TransactionState::Prepared,
        destination: destination.clone(),
        stage: None,
        backup: paths.backup.clone(),
        staging_root: None,
    };
    write_journal(&paths.journal, &journal)?;
    hook.after_state(TransactionState::Prepared)?;
    fs::rename(&destination, &paths.backup)
        .map_err(|error| SkillManagerError::io(&destination, error))?;
    journal.state = TransactionState::OldMoved;
    write_journal(&paths.journal, &journal)?;
    hook.after_state(TransactionState::OldMoved)?;
    journal.state = TransactionState::Committed;
    if let Err(error) = write_journal(&paths.journal, &journal) {
        // A removal has no installed stage proving commitment. Restore the old
        // content while the durable OldMoved record still describes rollback.
        fs::rename(&paths.backup, &destination).map_err(|rollback| {
            SkillManagerError::InvalidInput(format!(
                "{error}; removal rollback pending in {}: {rollback}",
                paths.journal.display()
            ))
        })?;
        return Err(error);
    }
    hook.after_state(TransactionState::Committed)?;
    let cleanup_pending = hook
        .before_cleanup()
        .and_then(|()| cleanup_committed(&paths.journal, &journal))
        .err()
        .map(|error| cleanup_warning(&destination, &paths.journal, &error));
    cleanup_empty_parent(&paths.backup);
    cleanup_empty_parent(&paths.journal);
    Ok(TransactionOutcome {
        value: true,
        cleanup_pending,
    })
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
        || journal.staging_root.as_ref().is_some_and(|root| {
            root != &staging_root.join(format!("{name}-pending"))
                || journal.stage.as_ref() != Some(&root.join("content"))
        })
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
        reject_link(&manager_root)?;
    }
    for managed in [
        Some(path),
        Some(journal.destination.as_path()),
        Some(journal.backup.as_path()),
        journal.stage.as_deref(),
        journal.stage.as_deref().and_then(Path::parent),
    ]
    .into_iter()
    .flatten()
    {
        reject_link(managed)?;
    }
    match journal.state {
        TransactionState::Staging => {}
        TransactionState::Prepared => {
            if journal.backup.exists() && !journal.destination.exists() {
                fs::rename(&journal.backup, &journal.destination)
                    .map_err(|error| SkillManagerError::io(&journal.backup, error))?;
            } else if journal.backup.exists() {
                fs::remove_dir_all(&journal.backup)
                    .map_err(|error| SkillManagerError::io(&journal.backup, error))?;
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
    cleanup_stage(&journal)?;
    fs::remove_file(path).map_err(|error| SkillManagerError::io(path, error))
}

struct TransactionPaths {
    journal: PathBuf,
    backup: PathBuf,
    lock: PathBuf,
}

fn validate_manager_roots(target_root: &Path) -> Result<()> {
    for name in [
        ".skill-manager-staging",
        ".skill-manager-backups",
        ".skill-manager-journals",
    ] {
        reject_link(&target_root.join(name))?;
    }
    Ok(())
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
    fs::atomic_write(path, &data).map_err(|error| SkillManagerError::io(path, error))
}

fn cleanup_stage(journal: &Journal) -> Result<()> {
    if let Some(root) = &journal.staging_root {
        remove_tree(root)?;
    } else if let Some(stage) = &journal.stage {
        // Legacy records own the content child, not arbitrary siblings in its parent.
        remove_tree(stage)?;
        if let Some(parent) = stage.parent() {
            cleanup_empty_dir(parent);
        }
    }
    Ok(())
}

fn cleanup_committed(path: &Path, journal: &Journal) -> Result<()> {
    remove_tree(&journal.backup)?;
    cleanup_stage(journal)?;
    fs::remove_file(path).map_err(|error| SkillManagerError::io(path, error))
}

fn cleanup_warning(destination: &Path, journal: &Path, error: &SkillManagerError) -> String {
    format!(
        "change committed at {}; cleanup pending: {error}. Recovery journal: {}; cleanup is retried when a later operation applies this skill; no-op and dry-run commands do not retry cleanup",
        destination.display(),
        journal.display()
    )
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
    if fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none()) {
        let _result = fs::remove_dir(path);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        Journal, NoopTransactionHook, TransactionHook, TransactionState, deploy_skill,
        import_skill, recover_journal, remove_skill, transaction_paths, write_journal,
    };
    use crate::error::{Result, SkillManagerError};

    struct FailAt(TransactionState);

    struct CleanupBlocked;
    impl TransactionHook for CleanupBlocked {
        fn after_state(&self, _: TransactionState) -> Result<()> {
            Ok(())
        }
        fn before_cleanup(&self) -> Result<()> {
            Err(SkillManagerError::InvalidInput(
                "held cleanup handle".into(),
            ))
        }
    }

    #[test]
    fn staging_recovery_owns_only_the_exact_recorded_directory() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let paths = transaction_paths(root.path(), root.path(), "demo");
        let staging = root
            .path()
            .join(".skill-manager-staging")
            .join("demo-pending");
        let unrelated = root
            .path()
            .join(".skill-manager-staging")
            .join("demo-unowned");
        std::fs::create_dir_all(staging.join("content"))
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::create_dir_all(&unrelated).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(unrelated.join("keep"), "safe")
            .unwrap_or_else(|error| unreachable!("{error}"));
        let mut journal = Journal {
            state: TransactionState::Staging,
            destination: root.path().join("demo"),
            stage: Some(staging.join("content")),
            backup: paths.backup.clone(),
            staging_root: Some(unrelated.clone()),
        };
        write_journal(&paths.journal, &journal).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(recover_journal(&paths.journal).is_err());
        assert!(staging.exists());
        assert!(unrelated.join("keep").exists());
        journal.staging_root = Some(staging.clone());
        write_journal(&paths.journal, &journal).unwrap_or_else(|error| unreachable!("{error}"));
        recover_journal(&paths.journal).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!staging.exists());
        assert!(unrelated.join("keep").exists());
    }

    #[cfg(windows)]
    #[test]
    fn recovery_rejects_a_junction_before_mutating_any_backup() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let paths = transaction_paths(root.path(), root.path(), "demo");
        let staging = root
            .path()
            .join(".skill-manager-staging")
            .join("demo-pending");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(staging.parent().unwrap_or(root.path()))
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::create_dir_all(&outside).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::create_dir_all(&paths.backup).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(outside.join("keep"), "safe")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(paths.backup.join("keep"), "old")
            .unwrap_or_else(|error| unreachable!("{error}"));
        let result = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&staging)
            .arg(&outside)
            .output()
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        write_journal(
            &paths.journal,
            &Journal {
                state: TransactionState::Committed,
                destination: root.path().join("demo"),
                stage: Some(staging.join("content")),
                backup: paths.backup.clone(),
                staging_root: Some(staging.clone()),
            },
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(recover_journal(&paths.journal).is_err());
        assert!(paths.backup.join("keep").exists());
        assert!(outside.join("keep").exists());
        std::fs::remove_dir(staging).unwrap_or_else(|error| unreachable!("{error}"));
    }

    #[test]
    fn import_commit_record_failure_is_interrupted_and_retains_prior_journal() {
        struct CommitBlocked;
        impl TransactionHook for CommitBlocked {
            fn after_state(&self, state: TransactionState) -> Result<()> {
                assert_ne!(state, TransactionState::Committed);
                Ok(())
            }
            fn before_commit(&self) -> Result<()> {
                Err(SkillManagerError::InvalidInput(
                    "commit record unavailable".into(),
                ))
            }
        }
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let deployment = root.path().join("deployment");
        let destination = root.path().join("source").join("demo");
        for path in [&deployment, &destination] {
            std::fs::create_dir_all(path).unwrap_or_else(|error| unreachable!("{error}"));
        }
        std::fs::write(deployment.join("SKILL.md"), "new")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(destination.join("SKILL.md"), "old")
            .unwrap_or_else(|error| unreachable!("{error}"));
        let error = import_skill(&deployment, &destination, root.path(), &CommitBlocked)
            .err()
            .unwrap_or_else(|| unreachable!("commit recording must fail"));
        assert!(error.to_string().contains("data is installed"));
        assert!(error.to_string().contains("operation interrupted"));
        let paths = transaction_paths(&root.path().join("source"), root.path(), "demo");
        let journal: Journal = serde_json::from_slice(
            &std::fs::read(&paths.journal).unwrap_or_else(|error| unreachable!("{error}")),
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(journal.state, TransactionState::OldMoved);
        assert!(journal.staging_root.is_some_and(|path| path.exists()));
        assert_eq!(
            std::fs::read_to_string(destination.join("SKILL.md"))
                .ok()
                .as_deref(),
            Some("new")
        );
        assert_eq!(
            std::fs::read_to_string(paths.backup.join("SKILL.md"))
                .ok()
                .as_deref(),
            Some("old")
        );
    }

    #[test]
    fn committed_import_retains_cleanup_ownership_and_recovers_without_replaying() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let deployment = root.path().join("deployment");
        let destination = root.path().join("source/demo");
        std::fs::create_dir_all(&deployment).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::create_dir_all(&destination).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(deployment.join("SKILL.md"), "new")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(destination.join("SKILL.md"), "old")
            .unwrap_or_else(|error| unreachable!("{error}"));
        let result = import_skill(&deployment, &destination, root.path(), &CleanupBlocked)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            result
                .cleanup_pending
                .as_deref()
                .is_some_and(|message| message.contains("change committed"))
        );
        let paths = transaction_paths(&root.path().join("source"), root.path(), "demo");
        assert!(paths.journal.exists());
        assert!(
            root.path()
                .join("source/.skill-manager-staging/demo-pending")
                .exists()
        );
        std::fs::write(destination.join("SKILL.md"), "edited after commit")
            .unwrap_or_else(|error| unreachable!("{error}"));
        recover_journal(&paths.journal).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!paths.journal.exists());
        assert!(!paths.backup.exists());
        assert!(
            !root
                .path()
                .join("source/.skill-manager-staging/demo-pending")
                .exists()
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("SKILL.md"))
                .ok()
                .as_deref(),
            Some("edited after commit")
        );
    }

    #[cfg(windows)]
    #[test]
    fn import_survives_a_real_temporary_windows_sharing_lock() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let deployment = root.path().join("deployment");
        let destination = root.path().join("source/demo");
        std::fs::create_dir_all(&deployment).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::create_dir_all(&destination).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(deployment.join("SKILL.md"), "new")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(destination.join("SKILL.md"), "old")
            .unwrap_or_else(|error| unreachable!("{error}"));
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(destination.join("SKILL.md"))
            .unwrap_or_else(|error| unreachable!("{error}"));
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            drop(held);
        });
        import_skill(&deployment, &destination, root.path(), &NoopTransactionHook)
            .unwrap_or_else(|error| unreachable!("{error}"));
        release
            .join()
            .unwrap_or_else(|error| unreachable!("{error:?}"));
        assert_eq!(
            std::fs::read_to_string(destination.join("SKILL.md"))
                .ok()
                .as_deref(),
            Some("new")
        );
        assert!(!root.path().join("source/.skill-manager-staging").exists());
    }

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
            .unwrap_or_else(|error| unreachable!("{error}"))
            .value;
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
                .value
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
            .value
        );
        assert!(
            !remove_skill("demo", target.path(), cache.path(), &NoopTransactionHook)
                .unwrap_or_else(|error| unreachable!("{error}"))
                .value
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
                .value
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
                staging_root: None,
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
                    staging_root: None,
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
            .unwrap_or_else(|error| unreachable!("{error}"))
            .value;
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
                    staging_root: None,
                }
            )
            .is_err()
        );
    }

    #[test]
    fn import_mirrors_a_deployment_into_an_explicitly_named_destination() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let cache = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let deployment = root.path().join("target").join("demo");
        std::fs::create_dir_all(deployment.join("reference"))
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(deployment.join("SKILL.md"), "agent edited")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(deployment.join("reference").join("new.md"), "added")
            .unwrap_or_else(|error| unreachable!("{error}"));

        let destination = root.path().join("source").join("Demo");
        std::fs::create_dir_all(&destination).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(destination.join("SKILL.md"), "original")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(destination.join("stale.md"), "removed upstream")
            .unwrap_or_else(|error| unreachable!("{error}"));

        let imported = import_skill(
            &deployment,
            &destination,
            cache.path(),
            &NoopTransactionHook,
        )
        .unwrap_or_else(|error| unreachable!("{error}"))
        .value;
        assert_eq!(imported, destination);
        assert_eq!(
            std::fs::read_to_string(destination.join("SKILL.md"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            "agent edited"
        );
        assert!(destination.join("reference").join("new.md").is_file());
        assert!(!destination.join("stale.md").exists());
        let source_root = root.path().join("source");
        assert!(!source_root.join(".skill-manager-journals").exists());
        assert!(!source_root.join(".skill-manager-staging").exists());
        assert!(!source_root.join(".skill-manager-backups").exists());

        assert!(
            import_skill(
                &deployment,
                std::path::Path::new(""),
                cache.path(),
                &NoopTransactionHook
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
                    staging_root: None,
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
