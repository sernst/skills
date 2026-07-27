//! One-time migration from the historical flat user-home layout.
//!
//! This module intentionally knows nothing about configuration schemas. It can
//! be deleted after the legacy adoption window without changing configuration
//! parsing, target resolution, or backup/restore behavior.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::json;

use crate::error::{Result, SkillManagerError};

/// One legacy component moved into the consolidated storage tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutMigrationItem {
    /// Kind of component moved.
    pub component: &'static str,
    /// Original location.
    pub from: PathBuf,
    /// Installed location.
    pub to: PathBuf,
}

/// Structured result suitable for later diagnostic/event emission.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LayoutMigrationResult {
    /// Successfully migrated components.
    pub migrated: Vec<LayoutMigrationItem>,
    /// Conflicts that were deliberately left untouched.
    pub warnings: Vec<String>,
}

/// Paths participating in the isolated layout migration.
#[derive(Clone, Debug)]
pub struct LayoutPaths {
    /// User/manager home containing historical flat files.
    pub user_home: PathBuf,
    /// Consolidated storage root.
    pub storage_root: PathBuf,
    /// Canonical configuration.
    pub config: PathBuf,
    /// Consolidated cache directory.
    pub cache: PathBuf,
    /// Consolidated backup directory.
    pub backups: PathBuf,
    /// Current Rust flat configuration.
    pub current_flat_config: PathBuf,
    /// Older Python flat configuration.
    pub python_flat_config: PathBuf,
    /// Historical cache directory.
    pub legacy_cache: PathBuf,
    /// Durable marker indicating that legacy configuration selection completed.
    pub config_migration_marker: PathBuf,
}

impl LayoutPaths {
    /// Construct all migration paths from the manager/user home.
    #[must_use]
    pub fn new(user_home: &Path) -> Self {
        let storage_root = user_home.join(".skill-manager");
        Self {
            user_home: user_home.to_path_buf(),
            config: storage_root.join("config.json"),
            cache: storage_root.join("cache"),
            backups: storage_root.join("backups"),
            current_flat_config: user_home.join(".skill-manager.config.json"),
            python_flat_config: user_home.join(".skills-syncer.config.json"),
            legacy_cache: user_home.join(".skill-manager-cache"),
            config_migration_marker: storage_root.join(".config-layout-migrated"),
            storage_root,
        }
    }
}

/// Migrate all known legacy layout components.
///
/// Destination files always win. Conflicting legacy data remains in place and
/// is reported as a warning. Every source is removed only after its destination
/// has been durably staged.
///
/// # Errors
///
/// Returns an error when a component cannot be staged, installed, or cleaned up safely.
pub fn migrate(paths: &LayoutPaths) -> Result<LayoutMigrationResult> {
    fs::create_dir_all(&paths.storage_root)
        .map_err(|error| SkillManagerError::io(&paths.storage_root, error))?;
    let mut result = LayoutMigrationResult::default();
    migrate_config(paths, &mut result)?;
    migrate_cache(paths, &mut result)?;
    migrate_v0_backups(paths, &mut result)?;
    Ok(result)
}

fn migrate_config(paths: &LayoutPaths, result: &mut LayoutMigrationResult) -> Result<()> {
    let current_exists = paths.current_flat_config.exists();
    let python_exists = paths.python_flat_config.exists();
    if paths.config_migration_marker.exists() {
        for legacy in [&paths.current_flat_config, &paths.python_flat_config] {
            if legacy.exists() {
                result.warnings.push(format!(
                    "configuration layout migration was already completed; leaving legacy configuration {} untouched",
                    legacy.display()
                ));
            }
        }
        return Ok(());
    }
    if paths.config.exists() {
        for legacy in [&paths.current_flat_config, &paths.python_flat_config] {
            if legacy.exists() {
                result.warnings.push(format!(
                    "{} already exists; leaving legacy configuration {} untouched",
                    paths.config.display(),
                    legacy.display()
                ));
            }
        }
        write_file_atomic(&paths.config_migration_marker, b"completed\n")?;
        return Ok(());
    }

    let selected = if current_exists {
        Some(&paths.current_flat_config)
    } else if python_exists {
        Some(&paths.python_flat_config)
    } else {
        None
    };
    if let Some(source) = selected {
        copy_file_atomic(source, &paths.config)?;
        fs::remove_file(source).map_err(|error| SkillManagerError::io(source, error))?;
        result.migrated.push(LayoutMigrationItem {
            component: "config",
            from: source.clone(),
            to: paths.config.clone(),
        });
    }
    if current_exists && python_exists {
        result.warnings.push(format!(
            "preferred {} and left lower-priority legacy configuration {} untouched",
            paths.current_flat_config.display(),
            paths.python_flat_config.display()
        ));
    }
    write_file_atomic(&paths.config_migration_marker, b"completed\n")?;
    Ok(())
}

fn migrate_cache(paths: &LayoutPaths, result: &mut LayoutMigrationResult) -> Result<()> {
    if !paths.legacy_cache.exists() {
        return Ok(());
    }
    migrate_directory_entries(
        &paths.legacy_cache,
        &paths.cache,
        &paths.legacy_cache,
        result,
    )?;
    remove_empty_directories(&paths.legacy_cache)?;
    Ok(())
}

fn migrate_directory_entries(
    source: &Path,
    destination: &Path,
    legacy_root: &Path,
    result: &mut LayoutMigrationResult,
) -> Result<()> {
    fs::create_dir_all(destination).map_err(|error| SkillManagerError::io(destination, error))?;
    let entries = fs::read_dir(source).map_err(|error| SkillManagerError::io(source, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| SkillManagerError::io(source, error))?;
        let from = entry.path();
        if stale_lock_path(&from, legacy_root) {
            continue;
        }
        let to = destination.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|error| SkillManagerError::io(&from, error))?;
        if kind.is_dir() {
            if to.exists() {
                let destination_kind = fs::symlink_metadata(&to)
                    .map_err(|error| SkillManagerError::io(&to, error))?
                    .file_type();
                if !destination_kind.is_dir() {
                    result.warnings.push(format!(
                        "{} has an incompatible entry type; leaving legacy cache directory {} untouched",
                        to.display(),
                        from.display()
                    ));
                    continue;
                }
            }
            migrate_directory_entries(&from, &to, legacy_root, result)?;
            remove_empty_directories(&from)?;
        } else if kind.is_file() {
            if to.exists() {
                result.warnings.push(format!(
                    "{} already exists; leaving legacy cache file {} untouched",
                    to.display(),
                    from.display()
                ));
                continue;
            }
            copy_file_atomic(&from, &to)?;
            fs::remove_file(&from).map_err(|error| SkillManagerError::io(&from, error))?;
            result.migrated.push(LayoutMigrationItem {
                component: "cache",
                from,
                to,
            });
        }
    }
    Ok(())
}

fn stale_lock_path(path: &Path, legacy_root: &Path) -> bool {
    let relative = path.strip_prefix(legacy_root).unwrap_or(path);
    relative
        .components()
        .any(|part| part.as_os_str() == ".locks")
        || path
            .extension()
            .is_some_and(|extension| extension == "lock")
}

fn remove_empty_directories(path: &Path) -> Result<()> {
    if !path.is_dir() {
        return Ok(());
    }
    let mut entries = fs::read_dir(path).map_err(|error| SkillManagerError::io(path, error))?;
    if entries.next().is_none() {
        fs::remove_dir(path).map_err(|error| SkillManagerError::io(path, error))?;
    }
    Ok(())
}

fn migrate_v0_backups(paths: &LayoutPaths, result: &mut LayoutMigrationResult) -> Result<()> {
    for config_path in [&paths.current_flat_config, &paths.python_flat_config] {
        let source = PathBuf::from(format!("{}.v0.bak", config_path.display()));
        if !source.exists() {
            continue;
        }
        let stem = config_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("legacy-config");
        let id = format!("legacy-{}", stem.trim_matches('.').replace('.', "-"));
        let directory = paths.backups.join(&id);
        let bytes = fs::read(&source).map_err(|error| SkillManagerError::io(&source, error))?;
        if directory.exists() {
            if legacy_backup_matches(&directory, &id, &source, &bytes)? {
                fs::remove_file(&source).map_err(|error| SkillManagerError::io(&source, error))?;
                result.migrated.push(LayoutMigrationItem {
                    component: "backup",
                    from: source,
                    to: directory,
                });
            } else {
                result.warnings.push(format!(
                    "backup {} already exists; leaving legacy migration backup {} untouched",
                    directory.display(),
                    source.display()
                ));
            }
            continue;
        }
        fs::create_dir_all(&paths.backups)
            .map_err(|error| SkillManagerError::io(&paths.backups, error))?;
        let staging = tempfile::Builder::new()
            .prefix(".legacy-backup-")
            .tempdir_in(&paths.backups)
            .map_err(|error| SkillManagerError::io(&paths.backups, error))?;
        let staged_raw = staging.path().join("config.raw");
        write_synced_file(&staged_raw, &bytes)?;
        let metadata = serde_json::to_vec_pretty(&json!({
            "id": &id,
            "created_at": Utc::now(),
            "reason": "legacy-v0-migration",
            "original_path": &source,
            "present": true,
            "schema_version": serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|value| value.get("schema_version").and_then(serde_json::Value::as_u64)),
            "valid": serde_json::from_slice::<serde_json::Value>(&bytes).is_ok()
        }))
        .map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
        let staged_metadata = staging.path().join("metadata.json");
        write_synced_file(&staged_metadata, &metadata)?;
        sync_directory(staging.path())?;
        fs::rename(staging.keep(), &directory)
            .map_err(|error| SkillManagerError::io(&directory, error))?;
        sync_directory(&paths.backups)?;
        fs::remove_file(&source).map_err(|error| SkillManagerError::io(&source, error))?;
        result.migrated.push(LayoutMigrationItem {
            component: "backup",
            from: source,
            to: directory,
        });
    }
    Ok(())
}

fn legacy_backup_matches(
    directory: &Path,
    expected_id: &str,
    expected_source: &Path,
    expected_bytes: &[u8],
) -> Result<bool> {
    let directory_kind = fs::symlink_metadata(directory)
        .map_err(|error| SkillManagerError::io(directory, error))?
        .file_type();
    if !directory_kind.is_dir() {
        return Ok(false);
    }
    let raw_path = directory.join("config.raw");
    let metadata_path = directory.join("metadata.json");
    let raw_kind = match fs::symlink_metadata(&raw_path) {
        Ok(metadata) => metadata.file_type(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(SkillManagerError::io(&raw_path, error)),
    };
    let metadata_kind = match fs::symlink_metadata(&metadata_path) {
        Ok(metadata) => metadata.file_type(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(SkillManagerError::io(&metadata_path, error)),
    };
    if !raw_kind.is_file() || !metadata_kind.is_file() {
        return Ok(false);
    }
    let raw = fs::read(&raw_path).map_err(|error| SkillManagerError::io(&raw_path, error))?;
    if raw != expected_bytes {
        return Ok(false);
    }
    let metadata_bytes =
        fs::read(&metadata_path).map_err(|error| SkillManagerError::io(&metadata_path, error))?;
    let Ok(metadata) = serde_json::from_slice::<serde_json::Value>(&metadata_bytes) else {
        return Ok(false);
    };
    Ok(
        metadata.get("id").and_then(serde_json::Value::as_str) == Some(expected_id)
            && metadata.get("reason").and_then(serde_json::Value::as_str)
                == Some("legacy-v0-migration")
            && metadata.get("present").and_then(serde_json::Value::as_bool) == Some(true)
            && metadata
                .get("original_path")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|path| Path::new(path) == expected_source),
    )
}

fn copy_file_atomic(source: &Path, destination: &Path) -> Result<()> {
    let bytes = fs::read(source).map_err(|error| SkillManagerError::io(source, error))?;
    write_file_atomic(destination, &bytes)
}

fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        SkillManagerError::InvalidInput(format!("path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent).map_err(|error| SkillManagerError::io(parent, error))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| SkillManagerError::io(parent, error))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| SkillManagerError::io(temporary.path(), error))?;
    temporary
        .persist(path)
        .map_err(|error| SkillManagerError::io(path, error.error))?;
    sync_directory(parent)
}

fn write_synced_file(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes)
        .and_then(|()| {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)?
                .sync_all()
        })
        .map_err(|error| SkillManagerError::io(path, error))
}

// The cross-platform call contract stays fallible because Unix directory fsync can fail.
#[allow(clippy::unnecessary_wraps)]
fn sync_directory(_path: &Path) -> Result<()> {
    // Windows cannot open a directory as a regular file. The staged file was
    // still flushed before replacement; Unix additionally flushes the entry.
    #[cfg(unix)]
    {
        let directory =
            fs::File::open(_path).map_err(|error| SkillManagerError::io(_path, error))?;
        directory
            .sync_all()
            .map_err(|error| SkillManagerError::io(_path, error))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{LayoutPaths, migrate};

    #[test]
    fn current_config_wins_and_migration_is_idempotent() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let paths = LayoutPaths::new(home.path());
        std::fs::write(&paths.current_flat_config, b"current")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(&paths.python_flat_config, b"python")
            .unwrap_or_else(|error| unreachable!("{error}"));

        let first = migrate(&paths).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            std::fs::read(&paths.config).unwrap_or_else(|error| unreachable!("{error}")),
            b"current"
        );
        assert!(!paths.current_flat_config.exists());
        assert!(paths.python_flat_config.exists());
        assert_eq!(first.migrated.len(), 1);
        assert_eq!(first.warnings.len(), 1);

        let second = migrate(&paths).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(second.migrated.is_empty());
        assert_eq!(second.warnings.len(), 1);
    }

    #[test]
    fn cache_conflicts_leave_legacy_data_and_skip_locks() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let paths = LayoutPaths::new(home.path());
        std::fs::create_dir_all(paths.legacy_cache.join(".locks"))
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::create_dir_all(&paths.cache).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(paths.legacy_cache.join("same"), b"old")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(paths.cache.join("same"), b"new")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(paths.legacy_cache.join("move"), b"value")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(paths.legacy_cache.join(".locks").join("stale.lock"), b"")
            .unwrap_or_else(|error| unreachable!("{error}"));

        let result = migrate(&paths).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            std::fs::read(paths.cache.join("same")).unwrap_or_else(|error| unreachable!("{error}")),
            b"new"
        );
        assert!(paths.legacy_cache.join("same").exists());
        assert!(paths.cache.join("move").exists());
        assert!(
            paths
                .legacy_cache
                .join(".locks")
                .join("stale.lock")
                .exists()
        );
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn completed_config_migration_blocks_stale_reimport_without_blocking_cache() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let paths = LayoutPaths::new(home.path());
        migrate(&paths).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(paths.config_migration_marker.exists());

        std::fs::write(&paths.python_flat_config, b"stale")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::create_dir_all(&paths.legacy_cache)
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(paths.legacy_cache.join("entry"), b"cached")
            .unwrap_or_else(|error| unreachable!("{error}"));

        let result = migrate(&paths).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!paths.config.exists());
        assert!(paths.python_flat_config.exists());
        assert_eq!(
            std::fs::read(paths.cache.join("entry"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            b"cached"
        );
        assert!(!paths.legacy_cache.join("entry").exists());
        assert_eq!(result.warnings.len(), 1);
    }

    #[test]
    fn cross_kind_cache_collisions_warn_and_preserve_both_sides() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let paths = LayoutPaths::new(home.path());
        std::fs::create_dir_all(paths.legacy_cache.join("directory"))
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(
            paths.legacy_cache.join("directory/item"),
            b"legacy-directory",
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(paths.legacy_cache.join("file"), b"legacy-file")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::create_dir_all(paths.cache.join("file"))
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(paths.cache.join("directory"), b"current-file")
            .unwrap_or_else(|error| unreachable!("{error}"));

        let result = migrate(&paths).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(result.warnings.len(), 2);
        assert_eq!(
            std::fs::read(paths.legacy_cache.join("directory/item"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            b"legacy-directory"
        );
        assert_eq!(
            std::fs::read(paths.cache.join("directory"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            b"current-file"
        );
        assert_eq!(
            std::fs::read(paths.legacy_cache.join("file"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            b"legacy-file"
        );
        assert!(paths.cache.join("file").is_dir());
    }

    #[test]
    fn legacy_backup_install_recovers_after_install_before_source_cleanup() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let paths = LayoutPaths::new(home.path());
        let source =
            std::path::PathBuf::from(format!("{}.v0.bak", paths.current_flat_config.display()));
        let bytes = br#"{"legacy":true}"#;
        std::fs::write(&source, bytes).unwrap_or_else(|error| unreachable!("{error}"));

        let first = migrate(&paths).unwrap_or_else(|error| unreachable!("{error}"));
        let record = paths.backups.join("legacy-skill-manager-config-json");
        assert_eq!(first.migrated.len(), 1);
        assert!(!source.exists());
        assert_eq!(
            std::fs::read(record.join("config.raw"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            bytes
        );
        assert!(record.join("metadata.json").is_file());

        std::fs::write(&source, bytes).unwrap_or_else(|error| unreachable!("{error}"));
        let recovered = migrate(&paths).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!source.exists());
        assert_eq!(recovered.migrated.len(), 1);
        assert!(recovered.warnings.is_empty());
    }

    #[test]
    fn partial_legacy_backup_record_is_not_completed_in_place() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let paths = LayoutPaths::new(home.path());
        let source =
            std::path::PathBuf::from(format!("{}.v0.bak", paths.current_flat_config.display()));
        let record = paths.backups.join("legacy-skill-manager-config-json");
        std::fs::create_dir_all(&record).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(record.join("config.raw"), b"partial")
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(&source, b"legacy").unwrap_or_else(|error| unreachable!("{error}"));

        let result = migrate(&paths).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(source.exists());
        assert!(!record.join("metadata.json").exists());
        assert_eq!(
            std::fs::read(record.join("config.raw"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            b"partial"
        );
        assert_eq!(result.warnings.len(), 1);
    }
}
