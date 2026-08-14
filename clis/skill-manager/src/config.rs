//! Versioned configuration, source normalization, and target resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
#[cfg(windows)]
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
};

use chrono::{DateTime, Utc};
use fs2::FileExt;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;
use url::Url;

use crate::domain::{
    Scope, ScopedTarget, SourceEntry, SourceLocation, SourceMode, SourceType, Target, TargetEntry,
};
use crate::error::{Result, SkillManagerError};
use crate::storage_migration::{self, LayoutMigrationResult, LayoutPaths};

/// Current configuration schema.
pub const CONFIG_SCHEMA_VERSION: u32 = 2;
/// Default remote cache lifetime.
pub const DEFAULT_CACHE_TTL_HOURS: i64 = 24;
#[cfg(windows)]
const VERBATIM_PREFIX: &[u16] = &[92, 92, 63, 92];
#[cfg(windows)]
const VERBATIM_UNC_PREFIX: &[u16] = &[92, 92, 63, 92, 85, 78, 67, 92];

/// Built-in target enablement stored separately from custom definitions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BuiltinTargetSettings {
    /// Whether implicit selection includes the target.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Unknown fields preserved across updates.
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

const fn default_true() -> bool {
    true
}

/// Persisted version-two configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// On-disk schema.
    pub schema_version: u32,
    /// Ordered skill sources.
    #[serde(default)]
    pub sources: Vec<SourceEntry>,
    /// Ordered custom targets.
    #[serde(default)]
    pub targets: IndexMap<String, TargetEntry>,
    /// Migrated custom definitions that shadow built-in names.
    #[serde(default)]
    pub legacy_target_overrides: IndexMap<String, TargetEntry>,
    /// Built-in target state.
    #[serde(default)]
    pub builtins: IndexMap<String, BuiltinTargetSettings>,
    /// Global skill exclusions.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Unknown root fields preserved across updates.
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            sources: Vec::new(),
            targets: IndexMap::new(),
            legacy_target_overrides: IndexMap::new(),
            builtins: IndexMap::new(),
            exclude: Vec::new(),
            extra: IndexMap::new(),
        }
    }
}

/// Configuration together with the active path selected during legacy handling.
#[derive(Clone, Debug)]
pub struct LoadedConfig {
    /// Validated configuration.
    pub config: Config,
    /// Current or legacy path used by this invocation.
    pub active_path: PathBuf,
    /// Compatibility warning generated while selecting the path.
    pub warning: Option<String>,
    /// Whether an on-disk configuration existed when loading began.
    pub persisted: bool,
    /// Startup layout migration details for diagnostic event emission.
    pub layout_migration: LayoutMigrationResult,
}

/// Immutable backup metadata persisted beside exact configuration bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackupMetadata {
    /// Stable path-safe identifier.
    pub id: String,
    /// UTC creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Operation that created the backup.
    pub reason: String,
    /// Configuration path represented by the record.
    pub original_path: PathBuf,
    /// Whether the represented state had a configuration file.
    pub present: bool,
    /// Best-effort schema discovered from the raw bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<u64>,
    /// Whether the bytes are syntactically valid JSON.
    pub valid: bool,
}

/// Backup record returned by repository operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigBackup {
    /// Persisted metadata.
    pub metadata: BackupMetadata,
    /// Exact-byte payload location. Absent-state records do not create it.
    pub raw_path: PathBuf,
}

/// Outcome of restoring a configuration backup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreOutcome {
    /// Backup whose state was restored.
    pub restored: ConfigBackup,
    /// Backup of the state displaced by the restore.
    pub displaced: ConfigBackup,
}

/// Persistence port used by the application service.
pub trait ConfigRepository {
    /// Run the isolated startup layout migration.
    ///
    /// # Errors
    ///
    /// Returns an error when legacy data cannot be migrated safely.
    fn migrate_layout(&self) -> Result<LayoutMigrationResult>;
    /// Load and migrate configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when configuration I/O, migration, or validation fails.
    fn load(&self, dry_run: bool) -> Result<LoadedConfig>;
    /// Atomically persist configuration at the previously selected active path.
    ///
    /// # Errors
    ///
    /// Returns an error when validation, locking, serialization, or replacement fails.
    fn save(&self, active_path: &Path, config: &Config) -> Result<()>;
    /// Return the cache root.
    fn cache_root(&self) -> &Path;
    /// Return the consolidated storage root.
    fn storage_root(&self) -> &Path;
    /// Return the canonical configuration path.
    fn config_path(&self) -> &Path;
    /// Read exact active bytes without parsing.
    ///
    /// # Errors
    ///
    /// Returns an error when the active configuration cannot be read.
    fn read_raw(&self) -> Result<Option<Vec<u8>>>;
    /// Read exact active bytes, creating canonical empty v2 when absent.
    ///
    /// # Errors
    ///
    /// Returns an error when configuration storage cannot be read or initialized.
    fn read_raw_or_create(&self) -> Result<Vec<u8>>;
    /// Return backup records ordered oldest to newest.
    ///
    /// # Errors
    ///
    /// Returns an error when backup storage cannot be inspected safely.
    fn list_backups(&self) -> Result<Vec<ConfigBackup>>;
    /// Archive the current state and install canonical empty v2 bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the state cannot be locked, archived, or replaced.
    fn reset_config(&self) -> Result<ConfigBackup>;
    /// Restore a selected or latest immutable backup.
    ///
    /// # Errors
    ///
    /// Returns an error when the backup is unavailable or the state cannot be replaced safely.
    fn restore_config(&self, backup_id: Option<&str>) -> Result<RestoreOutcome>;
}

/// User-home-backed configuration repository.
#[derive(Clone, Debug)]
pub struct FileConfigRepository {
    user_home: PathBuf,
    storage_root: PathBuf,
    config_path: PathBuf,
    cache_root: PathBuf,
    backups_root: PathBuf,
    locks_root: PathBuf,
    layout_paths: LayoutPaths,
}

impl FileConfigRepository {
    /// Build a repository for the current user's home directory.
    ///
    /// `home_override` takes the same explicit `--home` override
    /// [`manager_home`] accepts, ahead of `SKILL_MANAGER_HOME` and the OS
    /// home; pass `None` to resolve the unoverridden current-user home.
    ///
    /// # Errors
    ///
    /// Returns an error when the user home directory is unavailable.
    pub fn for_current_user(home_override: Option<&Path>) -> Result<Self> {
        Ok(Self::new(manager_home(home_override)?))
    }

    /// Build a repository rooted at an explicit home, primarily for tests.
    #[must_use]
    pub fn new(home: impl AsRef<Path>) -> Self {
        let user_home = home.as_ref().to_path_buf();
        let layout_paths = LayoutPaths::new(&user_home);
        Self {
            user_home,
            storage_root: layout_paths.storage_root.clone(),
            config_path: layout_paths.config.clone(),
            cache_root: layout_paths.cache.clone(),
            backups_root: layout_paths.backups.clone(),
            locks_root: layout_paths.storage_root.join("locks"),
            layout_paths,
        }
    }

    /// Manager/user home used for global targets and `~/` expansion.
    #[must_use]
    pub fn user_home(&self) -> &Path {
        &self.user_home
    }

    fn lock_path(&self) -> PathBuf {
        self.locks_root.join("config.lock")
    }

    fn save_unlocked(active_path: &Path, config: &Config) -> Result<()> {
        let mut normalized = config.clone();
        normalize_config_locations(&mut normalized)?;
        normalize_config_targets(&mut normalized)?;
        validate_config(&normalized, active_path)?;
        let mut bytes = serde_json::to_vec_pretty(&normalized)
            .map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
        bytes.push(b'\n');
        atomic_write(active_path, &bytes)
    }

    fn backup_unlocked(&self, reason: &str) -> Result<ConfigBackup> {
        let bytes = match fs::read(&self.config_path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(SkillManagerError::io(&self.config_path, error)),
        };
        self.create_backup_unlocked(reason, bytes.as_deref())
    }

    fn create_backup_unlocked(&self, reason: &str, bytes: Option<&[u8]>) -> Result<ConfigBackup> {
        fs::create_dir_all(&self.backups_root)
            .map_err(|error| SkillManagerError::io(&self.backups_root, error))?;
        let now = Utc::now();
        let safe_reason = reason
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '-' {
                    character
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_owned();
        let prefix = format!(
            "{}-{}",
            now.format("%Y%m%dT%H%M%S%.3fZ"),
            if safe_reason.is_empty() {
                "backup"
            } else {
                &safe_reason
            }
        );
        let mut id = prefix.clone();
        let mut suffix = 2_u32;
        while self.backups_root.join(&id).exists() {
            id = format!("{prefix}-{suffix}");
            suffix += 1;
        }
        let final_directory = self.backups_root.join(&id);
        let staging = tempfile::Builder::new()
            .prefix(".backup-")
            .tempdir_in(&self.backups_root)
            .map_err(|error| SkillManagerError::io(&self.backups_root, error))?;
        let raw_path = final_directory.join("config.raw");
        if let Some(raw) = bytes {
            let staged_raw = staging.path().join("config.raw");
            fs::write(&staged_raw, raw)
                .and_then(|()| {
                    OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&staged_raw)?
                        .sync_all()
                })
                .map_err(|error| SkillManagerError::io(&staged_raw, error))?;
        }
        let parsed = bytes.and_then(|raw| serde_json::from_slice::<Value>(raw).ok());
        let metadata = BackupMetadata {
            id,
            created_at: now,
            reason: reason.to_owned(),
            original_path: self.config_path.clone(),
            present: bytes.is_some(),
            schema_version: parsed
                .as_ref()
                .and_then(|value| value.get("schema_version"))
                .and_then(Value::as_u64),
            valid: bytes.is_none() || parsed.is_some(),
        };
        let metadata_bytes = serde_json::to_vec_pretty(&metadata)
            .map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
        let staged_metadata = staging.path().join("metadata.json");
        fs::write(&staged_metadata, metadata_bytes)
            .and_then(|()| {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&staged_metadata)?
                    .sync_all()
            })
            .map_err(|error| SkillManagerError::io(&staged_metadata, error))?;
        fs::rename(staging.keep(), &final_directory)
            .map_err(|error| SkillManagerError::io(&final_directory, error))?;
        Ok(ConfigBackup { metadata, raw_path })
    }

    fn list_backups_unlocked(&self) -> Result<Vec<ConfigBackup>> {
        if !self.backups_root.exists() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        let entries = fs::read_dir(&self.backups_root)
            .map_err(|error| SkillManagerError::io(&self.backups_root, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| SkillManagerError::io(&self.backups_root, error))?;
            let directory = entry.path();
            let entry_kind = entry
                .file_type()
                .map_err(|error| SkillManagerError::io(&directory, error))?;
            if !entry_kind.is_dir() || entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let metadata_path = directory.join("metadata.json");
            if !metadata_path.exists() {
                continue;
            }
            let metadata_kind = fs::symlink_metadata(&metadata_path)
                .map_err(|error| SkillManagerError::io(&metadata_path, error))?
                .file_type();
            if !metadata_kind.is_file() {
                continue;
            }
            let bytes = fs::read(&metadata_path)
                .map_err(|error| SkillManagerError::io(&metadata_path, error))?;
            let Ok(metadata) = serde_json::from_slice::<BackupMetadata>(&bytes) else {
                continue;
            };
            let directory_name = entry.file_name();
            if !safe_backup_id(&metadata.id)
                || directory_name.to_str() != Some(metadata.id.as_str())
            {
                continue;
            }
            let raw_path = directory.join("config.raw");
            match fs::symlink_metadata(&raw_path) {
                Ok(raw_metadata) if metadata.present && raw_metadata.file_type().is_file() => {}
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && !metadata.present => {
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(SkillManagerError::io(&raw_path, error)),
            }
            records.push(ConfigBackup { metadata, raw_path });
        }
        records.sort_by(|left, right| {
            left.metadata
                .created_at
                .cmp(&right.metadata.created_at)
                .then_with(|| left.metadata.id.cmp(&right.metadata.id))
        });
        Ok(records)
    }

    fn prune_backups_unlocked(&self, preserve_id: Option<&str>) -> Result<()> {
        let records = self.list_backups_unlocked()?;
        let canonical_root = if records.is_empty() {
            None
        } else {
            Some(
                fs::canonicalize(&self.backups_root)
                    .map_err(|error| SkillManagerError::io(&self.backups_root, error))?,
            )
        };
        let newest = records.last().map(|record| record.metadata.id.clone());
        let cutoff = Utc::now() - chrono::Duration::days(30);
        for record in records {
            if record.metadata.created_at >= cutoff
                || Some(record.metadata.id.as_str()) == newest.as_deref()
                || Some(record.metadata.id.as_str()) == preserve_id
            {
                continue;
            }
            let directory = record.raw_path.parent().ok_or_else(|| {
                invalid_backup_record(&record.raw_path, "backup payload has no parent directory")
            })?;
            let expected = self.backups_root.join(&record.metadata.id);
            if directory != expected || directory.parent() != Some(self.backups_root.as_path()) {
                return Err(invalid_backup_record(
                    directory,
                    "backup directory is outside the configured backup root",
                ));
            }
            let directory_kind = fs::symlink_metadata(directory)
                .map_err(|error| SkillManagerError::io(directory, error))?
                .file_type();
            if !directory_kind.is_dir() {
                return Err(invalid_backup_record(
                    directory,
                    "backup directory must be a regular directory",
                ));
            }
            let canonical_directory = fs::canonicalize(directory)
                .map_err(|error| SkillManagerError::io(directory, error))?;
            if canonical_directory.parent() != canonical_root.as_deref() {
                return Err(invalid_backup_record(
                    directory,
                    "backup directory resolves outside the configured backup root",
                ));
            }
            fs::remove_dir_all(directory)
                .map_err(|error| SkillManagerError::io(directory, error))?;
        }
        Ok(())
    }
}

fn safe_backup_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
}

fn invalid_backup_record(path: &Path, message: &str) -> SkillManagerError {
    SkillManagerError::InvalidConfig {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

impl ConfigRepository for FileConfigRepository {
    fn migrate_layout(&self) -> Result<LayoutMigrationResult> {
        let _lock = acquire_lock(&self.lock_path(), "configuration", Duration::from_secs(10))?;
        storage_migration::migrate(&self.layout_paths)
    }

    fn load(&self, _dry_run: bool) -> Result<LoadedConfig> {
        let _lock = acquire_lock(&self.lock_path(), "configuration", Duration::from_secs(10))?;
        let layout_migration = storage_migration::migrate(&self.layout_paths)?;
        let active_path = self.config_path.clone();
        if !active_path.exists() {
            return Ok(LoadedConfig {
                config: Config::default(),
                active_path,
                warning: layout_migration.warnings.first().cloned(),
                persisted: false,
                layout_migration,
            });
        }
        let raw = fs::read(&active_path)
            .map_err(|error| SkillManagerError::io(active_path.clone(), error))?;
        let mut value: Value =
            serde_json::from_slice(&raw).map_err(|error| SkillManagerError::InvalidConfig {
                path: active_path.clone(),
                message: error.to_string(),
            })?;
        let schema = parse_schema_version(&value, &active_path)?;
        if schema > u64::from(CONFIG_SCHEMA_VERSION) {
            return Err(SkillManagerError::InvalidConfig {
                path: active_path,
                message: format!(
                    "schema {schema} is newer than supported schema {CONFIG_SCHEMA_VERSION}"
                ),
            });
        }
        let migrated = schema < u64::from(CONFIG_SCHEMA_VERSION);
        if schema == 0 {
            value = migrate_v0(&value, &self.user_home)?;
        }
        if value
            .get("schema_version")
            .and_then(Value::as_u64)
            .is_some_and(|schema| schema == 1)
        {
            value = migrate_v1(&value)?;
        }
        let mut config: Config =
            serde_json::from_value(value).map_err(|error| SkillManagerError::InvalidConfig {
                path: active_path.clone(),
                message: error.to_string(),
            })?;
        normalize_config_locations(&mut config)?;
        normalize_config_targets(&mut config)?;
        validate_config(&config, &active_path)?;
        if migrated {
            self.create_backup_unlocked(
                if schema == 0 {
                    "schema-v0-migration"
                } else {
                    "schema-v1-migration"
                },
                Some(&raw),
            )?;
            Self::save_unlocked(&active_path, &config)?;
            self.prune_backups_unlocked(None)?;
        }
        Ok(LoadedConfig {
            config,
            active_path,
            warning: layout_migration.warnings.first().cloned(),
            persisted: true,
            layout_migration,
        })
    }

    fn save(&self, active_path: &Path, config: &Config) -> Result<()> {
        let _lock = acquire_lock(&self.lock_path(), "configuration", Duration::from_secs(10))?;
        Self::save_unlocked(active_path, config)?;
        self.prune_backups_unlocked(None)
    }

    fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    fn storage_root(&self) -> &Path {
        &self.storage_root
    }

    fn config_path(&self) -> &Path {
        &self.config_path
    }

    fn read_raw(&self) -> Result<Option<Vec<u8>>> {
        let _lock = acquire_lock(&self.lock_path(), "configuration", Duration::from_secs(10))?;
        storage_migration::migrate(&self.layout_paths)?;
        match fs::read(&self.config_path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(SkillManagerError::io(&self.config_path, error)),
        }
    }

    fn read_raw_or_create(&self) -> Result<Vec<u8>> {
        let _lock = acquire_lock(&self.lock_path(), "configuration", Duration::from_secs(10))?;
        storage_migration::migrate(&self.layout_paths)?;
        match fs::read(&self.config_path) {
            Ok(bytes) => Ok(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let bytes = canonical_config_bytes()?;
                atomic_write(&self.config_path, &bytes)?;
                Ok(bytes)
            }
            Err(error) => Err(SkillManagerError::io(&self.config_path, error)),
        }
    }

    fn list_backups(&self) -> Result<Vec<ConfigBackup>> {
        let _lock = acquire_lock(&self.lock_path(), "configuration", Duration::from_secs(10))?;
        self.list_backups_unlocked()
    }

    fn reset_config(&self) -> Result<ConfigBackup> {
        let _lock = acquire_lock(&self.lock_path(), "configuration", Duration::from_secs(10))?;
        storage_migration::migrate(&self.layout_paths)?;
        let backup = self.backup_unlocked("reset")?;
        atomic_write(&self.config_path, &canonical_config_bytes()?)?;
        self.prune_backups_unlocked(Some(&backup.metadata.id))?;
        Ok(backup)
    }

    fn restore_config(&self, backup_id: Option<&str>) -> Result<RestoreOutcome> {
        let _lock = acquire_lock(&self.lock_path(), "configuration", Duration::from_secs(10))?;
        storage_migration::migrate(&self.layout_paths)?;
        let records = self.list_backups_unlocked()?;
        let selected = match backup_id {
            Some(id) => records
                .iter()
                .find(|record| record.metadata.id == id)
                .cloned(),
            None => records.last().cloned(),
        }
        .ok_or_else(|| {
            SkillManagerError::InvalidInput(match backup_id {
                Some(id) => format!("configuration backup '{id}' was not found"),
                None => "no configuration backups are available".into(),
            })
        })?;
        let selected_bytes = if selected.metadata.present {
            Some(
                fs::read(&selected.raw_path)
                    .map_err(|error| SkillManagerError::io(&selected.raw_path, error))?,
            )
        } else {
            None
        };
        let displaced = self.backup_unlocked("restore-displaced")?;
        if let Some(bytes) = selected_bytes {
            atomic_write(&self.config_path, &bytes)?;
        } else {
            match fs::remove_file(&self.config_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(SkillManagerError::io(&self.config_path, error)),
            }
        }
        self.prune_backups_unlocked(Some(&selected.metadata.id))?;
        Ok(RestoreOutcome {
            restored: selected,
            displaced,
        })
    }
}

fn parse_schema_version(value: &Value, path: &Path) -> Result<u64> {
    match value.get("schema_version") {
        None => Ok(0),
        Some(Value::Number(number)) => {
            number
                .as_u64()
                .ok_or_else(|| SkillManagerError::InvalidConfig {
                    path: path.to_path_buf(),
                    message: "schema_version must be a non-negative integer".into(),
                })
        }
        Some(_) => Err(SkillManagerError::InvalidConfig {
            path: path.to_path_buf(),
            message: "schema_version must be a non-negative integer".into(),
        }),
    }
}

/// Serialize the canonical empty schema-v2 document.
///
/// # Errors
///
/// Returns an error only when the in-memory canonical value cannot serialize.
pub fn canonical_config_bytes() -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(&Config::default())
        .map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
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
    Ok(())
}

/// Advisory lock held for one resource.
pub struct ResourceLock {
    file: fs::File,
}

impl Drop for ResourceLock {
    fn drop(&mut self) {
        let _result = self.file.unlock();
    }
}

/// Acquire an advisory lock with a bounded wait.
///
/// # Errors
///
/// Returns an error when the lock file cannot be opened or the timeout expires.
pub fn acquire_lock(path: &Path, resource: &str, timeout: Duration) -> Result<ResourceLock> {
    let parent = path.parent().ok_or_else(|| {
        SkillManagerError::InvalidInput(format!("lock path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent).map_err(|error| SkillManagerError::io(parent, error))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| SkillManagerError::io(path, error))?;
    let started = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(ResourceLock { file }),
            Err(error) if lock_is_contended(&error) => {
                if started.elapsed() >= timeout {
                    return Err(SkillManagerError::LockTimeout {
                        resource: resource.to_owned(),
                        seconds: timeout.as_secs(),
                    });
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(SkillManagerError::io(path, error)),
        }
    }
}

fn lock_is_contended(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        // Windows reports advisory byte-range contention as sharing/lock
        // violations rather than `WouldBlock`.
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    false
}

fn migrate_v0(value: &Value, home: &Path) -> Result<Value> {
    let mut root = value.as_object().cloned().ok_or_else(|| {
        SkillManagerError::InvalidInput("configuration root must be a JSON object".into())
    })?;
    migrate_v0_sources(&mut root, home)?;
    migrate_v0_targets(&mut root)?;
    root.insert("schema_version".into(), Value::Number(1_u32.into()));
    migrate_v1(&Value::Object(root))
}

fn migrate_v1(value: &Value) -> Result<Value> {
    let mut root = value.as_object().cloned().ok_or_else(|| {
        SkillManagerError::InvalidInput("configuration root must be a JSON object".into())
    })?;
    for field in ["targets", "legacy_target_overrides"] {
        let targets = root
            .entry(field)
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| {
                SkillManagerError::InvalidInput(format!(
                    "configuration '{field}' must be an object"
                ))
            })?;
        for (name, target) in targets {
            let object = target.as_object_mut().ok_or_else(|| {
                SkillManagerError::InvalidInput(format!("target '{name}' must be an object"))
            })?;
            let raw = object.get("path").and_then(Value::as_str).ok_or_else(|| {
                SkillManagerError::InvalidInput(format!("target '{name}' requires path"))
            })?;
            let template = migrate_v1_target_template(raw)?;
            object.insert(
                "path".into(),
                Value::String(template.to_string_lossy().replace('\\', "/")),
            );
        }
    }
    root.entry("sources")
        .or_insert_with(|| Value::Array(Vec::new()));
    root.entry("builtins")
        .or_insert_with(|| Value::Object(Map::new()));
    root.entry("exclude")
        .or_insert_with(|| Value::Array(Vec::new()));
    root.insert(
        "schema_version".into(),
        Value::Number(CONFIG_SCHEMA_VERSION.into()),
    );
    Ok(Value::Object(root))
}

fn migrate_v1_target_template(raw: &str) -> Result<PathBuf> {
    let normalized = raw.replace('\\', "/");
    let segments = normalized
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>();
    let start = segments
        .iter()
        .rposition(|segment| segment.starts_with('.') && segment.len() > 1)
        .unwrap_or_else(|| segments.len().saturating_sub(1));
    normalize_target_template(&segments[start..].join("/"))
}

fn migrate_v0_sources(root: &mut Map<String, Value>, home: &Path) -> Result<()> {
    let sources = match root.remove("sources") {
        Some(Value::Array(values)) => values,
        Some(_) => {
            return Err(SkillManagerError::InvalidInput(
                "configuration 'sources' must be an array".into(),
            ));
        }
        None => Vec::new(),
    };
    let mut normalized_sources = Vec::new();
    // Hybrid v0 files may describe the same local source in both representations.
    // Normalize the explicit array first so its identity, metadata, and order win
    // even when the legacy map uses a symlink or another equivalent spelling.
    let mut explicit_local_paths = BTreeSet::new();
    let mut seen_source_ids = BTreeSet::new();
    for raw in sources {
        let entry = coerce_v0_source(&raw, home)?;
        if entry.source_type == SourceType::Local
            && let Some(path) = &entry.path
        {
            explicit_local_paths.insert(path.clone());
        }
        if seen_source_ids.insert(entry.id.clone()) {
            normalized_sources.push(serde_json::to_value(entry).map_err(|error| {
                SkillManagerError::InvalidInput(format!("could not migrate source: {error}"))
            })?);
        }
    }
    if let Some(legacy) = root.remove("skills_directories") {
        let directories = legacy.as_object().ok_or_else(|| {
            SkillManagerError::InvalidInput(
                "configuration 'skills_directories' must be an object".into(),
            )
        })?;
        for (path, metadata) in directories {
            let mut entry = source_from_reference(path, None, home)?;
            entry.id = legacy_local_source_id(&entry);
            let meta = metadata.as_object().ok_or_else(|| {
                SkillManagerError::InvalidInput(format!(
                    "legacy skills_directories metadata for {path} must be an object"
                ))
            })?;
            if let Some(raw_name) = meta.get("name").or_else(|| meta.get("alias")) {
                let name = raw_name.as_str().ok_or_else(|| {
                    SkillManagerError::InvalidInput(format!(
                        "legacy source name for {path} must be a string"
                    ))
                })?;
                if !name.is_empty() {
                    name.clone_into(&mut entry.name);
                }
            }
            if let Some(raw_label) = meta.get("label") {
                let label = raw_label.as_str().ok_or_else(|| {
                    SkillManagerError::InvalidInput(format!(
                        "legacy source label for {path} must be a string"
                    ))
                })?;
                if !label.is_empty() {
                    label.clone_into(&mut entry.label);
                }
            }
            if let Some(raw_exclude) = meta.get("exclude") {
                let exclude = raw_exclude.as_array().ok_or_else(|| {
                    SkillManagerError::InvalidInput(format!(
                        "legacy source exclude for {path} must be an array"
                    ))
                })?;
                entry.exclude = exclude
                    .iter()
                    .map(|value| {
                        value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                            SkillManagerError::InvalidInput(format!(
                                "legacy source exclude entries for {path} must be strings"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
            }
            if entry
                .path
                .as_ref()
                .is_some_and(|path| explicit_local_paths.contains(path))
            {
                continue;
            }
            if seen_source_ids.insert(entry.id.clone()) {
                normalized_sources.push(serde_json::to_value(entry).map_err(|error| {
                    SkillManagerError::InvalidInput(format!("could not migrate source: {error}"))
                })?);
            }
        }
    }
    root.insert("sources".into(), Value::Array(normalized_sources));
    Ok(())
}

fn migrate_v0_targets(root: &mut Map<String, Value>) -> Result<()> {
    let mut custom_targets = Map::new();
    let mut legacy_overrides = Map::new();
    let builtins = Map::new();
    if let Some(raw_targets) = root.remove("targets") {
        let targets = raw_targets.as_object().ok_or_else(|| {
            SkillManagerError::InvalidInput("configuration 'targets' must be an object".into())
        })?;
        for (name, target) in targets {
            let mut normalized_target = target.clone();
            let object = normalized_target.as_object_mut().ok_or_else(|| {
                SkillManagerError::InvalidInput(format!("legacy target {name} must be an object"))
            })?;
            for field in ["enabled", "disabled"] {
                if object.get(field).is_some_and(|value| !value.is_boolean()) {
                    return Err(SkillManagerError::InvalidInput(format!(
                        "legacy target {name} field {field} must be a boolean"
                    )));
                }
            }
            if object.get("path").is_some_and(|value| !value.is_string()) {
                return Err(SkillManagerError::InvalidInput(format!(
                    "legacy target {name} path must be a string"
                )));
            }
            {
                let enabled = object
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .or_else(|| {
                        object
                            .get("disabled")
                            .and_then(Value::as_bool)
                            .map(|disabled| !disabled)
                    })
                    .unwrap_or(true);
                object.remove("disabled");
                object.insert("enabled".into(), Value::Bool(enabled));
            }
            if let Some(canonical_name) = canonical_builtin_name(name) {
                if legacy_overrides.contains_key(canonical_name) {
                    return Err(SkillManagerError::InvalidInput(format!(
                        "legacy target name '{name}' collides with another '{canonical_name}' built-in target"
                    )));
                }
                legacy_overrides.insert(canonical_name.into(), normalized_target);
            } else {
                custom_targets.insert(name.clone(), normalized_target);
            }
        }
    }
    root.insert("targets".into(), Value::Object(custom_targets));
    root.insert(
        "legacy_target_overrides".into(),
        Value::Object(legacy_overrides),
    );
    root.insert("builtins".into(), Value::Object(builtins));
    Ok(())
}

fn legacy_local_source_id(source: &SourceEntry) -> String {
    let identity = source
        .path
        .as_ref()
        .map_or_else(String::new, |path| path.to_string_lossy().into_owned());
    let digest = Sha256::digest(identity.as_bytes());
    format!("src_local_{}", &hex::encode(digest)[..12])
}

fn coerce_v0_source(raw: &Value, home: &Path) -> Result<SourceEntry> {
    if let Some(reference) = raw.as_str() {
        return source_from_reference(reference, None, home);
    }
    if let Some(alias) = raw.as_object().and_then(|object| object.get("alias"))
        && !alias.is_string()
    {
        return Err(SkillManagerError::InvalidInput(
            "legacy source alias must be a string".into(),
        ));
    }
    let mut entry: SourceEntry = serde_json::from_value(raw.clone()).map_err(|error| {
        SkillManagerError::InvalidInput(format!("invalid legacy source entry: {error}"))
    })?;
    let original_id = entry.id.clone();
    let original_name = entry.name.clone();
    let original_label = entry.label.clone();
    let alias = entry
        .extra
        .get("alias")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    match entry.source_type {
        SourceType::Local => {
            let path = entry.path.as_ref().ok_or_else(|| {
                SkillManagerError::InvalidInput("legacy local source requires path".into())
            })?;
            let mut normalized =
                source_from_reference(&path.to_string_lossy(), Some(entry.mode), home)?;
            normalized.exclude = entry.exclude;
            normalized.cache_ttl_hours = entry.cache_ttl_hours;
            normalized.extra = entry.extra;
            entry = normalized;
        }
        SourceType::GitHub => {
            if entry.repo_path.is_none() {
                entry.repo_path = entry
                    .path
                    .take()
                    .map(|path| path.to_string_lossy().trim_matches(['/', '\\']).to_owned())
                    .filter(|path| !path.is_empty());
            }
            if entry.owner.as_deref().is_none_or(str::is_empty)
                || entry.repo.as_deref().is_none_or(str::is_empty)
            {
                return Err(SkillManagerError::InvalidInput(
                    "legacy GitHub source requires owner and repo".into(),
                ));
            }
            entry.id = derive_source_id(&entry);
            entry.name = entry.repo.clone().unwrap_or_default();
            entry.label = title_case(&entry.name);
        }
    }
    if !original_id.trim().is_empty() {
        entry.id = original_id;
    }
    if !original_name.trim().is_empty() {
        entry.name = original_name;
    } else if let Some(alias) = alias.filter(|value| !value.trim().is_empty()) {
        entry.name = alias;
    }
    if original_label.trim().is_empty() {
        entry.label = title_case(&entry.name);
    } else {
        entry.label = original_label;
    }
    Ok(entry)
}

fn validate_config(config: &Config, path: &Path) -> Result<()> {
    if config.schema_version != CONFIG_SCHEMA_VERSION {
        return Err(SkillManagerError::InvalidConfig {
            path: path.to_path_buf(),
            message: format!(
                "expected schema {CONFIG_SCHEMA_VERSION}, got {}",
                config.schema_version
            ),
        });
    }
    let mut names = BTreeMap::<String, &str>::new();
    for source in &config.sources {
        validate_source(source)?;
        let key = fold(&source.name);
        if let Some(existing) = names.insert(key, &source.id)
            && existing != source.id
        {
            return Err(SkillManagerError::InvalidConfig {
                path: path.to_path_buf(),
                message: format!("duplicate source name '{}'", source.name),
            });
        }
    }
    for name in config.targets.keys() {
        if is_builtin_name(name) {
            return Err(SkillManagerError::InvalidConfig {
                path: path.to_path_buf(),
                message: format!("custom target '{name}' uses a reserved built-in name"),
            });
        }
    }
    Ok(())
}

fn normalize_config_locations(config: &mut Config) -> Result<()> {
    for source in &mut config.sources {
        let active = normalize_location(raw_source_location(source)?)?;
        set_source_location(source, &active);
        if let Some(alternate) = source.alternate.take() {
            source.alternate = Some(normalize_location(alternate)?);
        }
    }
    Ok(())
}

pub(crate) fn normalize_config_targets(config: &mut Config) -> Result<()> {
    for (name, target) in config
        .targets
        .iter_mut()
        .chain(config.legacy_target_overrides.iter_mut())
    {
        target.path =
            normalize_target_template(&target.path.to_string_lossy()).map_err(|error| {
                SkillManagerError::InvalidInput(format!(
                    "target '{name}' has invalid path template: {error}"
                ))
            })?;
    }
    Ok(())
}

/// Normalize a target path into a safe scope-root-relative template.
///
/// A single leading `~/` is accepted for compatibility and stripped. Absolute
/// paths, named-home forms, blank values, and parent traversal are rejected.
///
/// # Errors
///
/// Returns an invalid-input error when the value cannot remain below a scope
/// root after lexical normalization.
pub fn normalize_target_template(raw: &str) -> Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(SkillManagerError::InvalidInput(
            "target path template must not be empty".into(),
        ));
    }
    let unified = trimmed.replace('\\', "/");
    let without_home = unified.strip_prefix("~/").unwrap_or(unified.as_str());
    if without_home.starts_with('~') {
        return Err(SkillManagerError::InvalidInput(
            "named-home target paths are not supported".into(),
        ));
    }
    if without_home.starts_with('/')
        || without_home.starts_with("//")
        || without_home
            .as_bytes()
            .get(1)
            .is_some_and(|character| *character == b':')
    {
        return Err(SkillManagerError::InvalidInput(
            "target path template must be relative".into(),
        ));
    }
    let mut segments = Vec::new();
    for segment in without_home.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(SkillManagerError::InvalidInput(
                        "target path template escapes its scope root".into(),
                    ));
                }
            }
            value => segments.push(value),
        }
    }
    if segments.is_empty() {
        return Err(SkillManagerError::InvalidInput(
            "target path template must not be empty".into(),
        ));
    }
    Ok(segments.iter().collect())
}

fn validate_source(source: &SourceEntry) -> Result<()> {
    if source.id.trim().is_empty() || source.name.trim().is_empty() {
        return Err(SkillManagerError::InvalidInput(
            "source ID and name must not be blank".into(),
        ));
    }
    if source.cache_ttl_hours.is_some_and(|ttl| ttl < 0) {
        return Err(SkillManagerError::InvalidInput(format!(
            "source '{}' has negative cache TTL",
            source.name
        )));
    }
    let active = source_location(source)?;
    validate_location(&active, &source.name)?;
    if let Some(alternate) = &source.alternate {
        validate_location(alternate, &source.name)?;
        if locations_equal(&active, alternate) {
            return Err(SkillManagerError::InvalidInput(format!(
                "source '{}' active and alternate locations must differ",
                source.name
            )));
        }
    }
    Ok(())
}

/// Normalize a local path or supported GitHub reference into a source.
///
/// `home` is the caller's already-resolved manager home (see
/// [`manager_home`]), used only to expand a leading `~` in a local path
/// reference; it is ignored for GitHub references.
///
/// # Errors
///
/// Returns an error when a local path cannot be absolutized or the user home is unavailable.
pub fn source_from_reference(
    raw: &str,
    mode: Option<SourceMode>,
    home: &Path,
) -> Result<SourceEntry> {
    if let Some(reference) = parse_github_reference(raw)? {
        let mut entry = SourceEntry {
            id: String::new(),
            source_type: SourceType::GitHub,
            mode: mode.unwrap_or(SourceMode::Collection),
            name: reference.repo.clone(),
            label: title_case(&reference.repo),
            exclude: Vec::new(),
            cache_ttl_hours: None,
            path: None,
            owner: Some(reference.owner),
            repo: Some(reference.repo),
            r#ref: reference.reference,
            repo_path: reference.repo_path,
            alternate: None,
            extra: IndexMap::new(),
        };
        entry.id = derive_source_id(&entry);
        return Ok(entry);
    }
    let expanded = expand_home(raw, home);
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map_err(|error| SkillManagerError::io(".", error))?
            .join(expanded)
    };
    let normalized = portable_canonicalize(&absolute);
    let source_mode = mode.unwrap_or_else(|| {
        if normalized.join("SKILL.md").is_file() {
            SourceMode::Single
        } else {
            SourceMode::Collection
        }
    });
    let name = normalized
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("skills")
        .to_owned();
    let mut entry = SourceEntry {
        id: String::new(),
        source_type: SourceType::Local,
        mode: source_mode,
        name: name.clone(),
        label: title_case(&name),
        exclude: Vec::new(),
        cache_ttl_hours: None,
        path: Some(normalized),
        owner: None,
        repo: None,
        r#ref: None,
        repo_path: None,
        alternate: None,
        extra: IndexMap::new(),
    };
    entry.id = derive_source_id(&entry);
    Ok(entry)
}

/// Parse and normalize a reference into a location without changing source metadata.
///
/// `mode` is supplied so local location changes never infer or change a source's
/// collection/single layout based on whether the path currently exists. `home`
/// is the caller's already-resolved manager home, forwarded to
/// [`source_from_reference`] for `~` expansion.
///
/// # Errors
///
/// Returns an error when a reference cannot be normalized safely.
pub fn location_from_reference(raw: &str, mode: SourceMode, home: &Path) -> Result<SourceLocation> {
    let entry = source_from_reference(raw, Some(mode), home)?;
    source_location(&entry)
}

/// Return the active location stored in a flattened source entry.
///
/// # Errors
///
/// Returns an error when fields for the active source type are missing or
/// forbidden fields for the other type are present.
pub fn source_location(source: &SourceEntry) -> Result<SourceLocation> {
    normalize_location(raw_source_location(source)?)
}

fn raw_source_location(source: &SourceEntry) -> Result<SourceLocation> {
    match source.source_type {
        SourceType::Local => {
            if source.owner.is_some()
                || source.repo.is_some()
                || source.r#ref.is_some()
                || source.repo_path.is_some()
            {
                return Err(SkillManagerError::InvalidInput(format!(
                    "local source '{}' forbids GitHub location fields",
                    source.name
                )));
            }
            let path = source.path.clone().ok_or_else(|| {
                SkillManagerError::InvalidInput(format!(
                    "local source '{}' requires path",
                    source.name
                ))
            })?;
            Ok(SourceLocation::Local { path })
        }
        SourceType::GitHub => {
            if source.path.is_some() {
                return Err(SkillManagerError::InvalidInput(format!(
                    "GitHub source '{}' forbids path",
                    source.name
                )));
            }
            let owner = source.owner.clone().ok_or_else(|| {
                SkillManagerError::InvalidInput(format!(
                    "GitHub source '{}' requires owner",
                    source.name
                ))
            })?;
            let repo = source.repo.clone().ok_or_else(|| {
                SkillManagerError::InvalidInput(format!(
                    "GitHub source '{}' requires repo",
                    source.name
                ))
            })?;
            Ok(SourceLocation::GitHub {
                owner,
                repo,
                r#ref: source.r#ref.clone(),
                repo_path: source.repo_path.clone(),
            })
        }
    }
}

fn normalize_location(location: SourceLocation) -> Result<SourceLocation> {
    match location {
        SourceLocation::Local { path } => {
            if path.as_os_str().is_empty() || !path.is_absolute() {
                return Err(SkillManagerError::InvalidInput(
                    "local source location must be an absolute non-blank path".into(),
                ));
            }
            Ok(SourceLocation::Local {
                path: portable_canonicalize(&path),
            })
        }
        SourceLocation::GitHub {
            owner,
            repo,
            r#ref,
            repo_path,
        } => Ok(SourceLocation::GitHub {
            owner,
            repo,
            r#ref,
            repo_path: repo_path
                .map(|path| normalize_repo_path(&path))
                .transpose()?,
        }),
    }
}

fn normalize_repo_path(path: &str) -> Result<String> {
    let normalized = path.replace('\\', "/");
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.ends_with('/')
        || normalized
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(SkillManagerError::InvalidInput(
            "GitHub repository path must be a normalized relative path".into(),
        ));
    }
    Ok(normalized)
}

/// Replace only the flattened active-location fields of a source.
pub fn set_source_location(source: &mut SourceEntry, location: &SourceLocation) {
    source.path = None;
    source.owner = None;
    source.repo = None;
    source.r#ref = None;
    source.repo_path = None;
    match location {
        SourceLocation::Local { path } => {
            source.source_type = SourceType::Local;
            source.path = Some(path.clone());
        }
        SourceLocation::GitHub {
            owner,
            repo,
            r#ref,
            repo_path,
        } => {
            source.source_type = SourceType::GitHub;
            source.owner = Some(owner.clone());
            source.repo = Some(repo.clone());
            source.r#ref.clone_from(r#ref);
            source.repo_path.clone_from(repo_path);
        }
    }
}

/// Compare two locations by their normalized platform-aware identities.
#[must_use]
pub fn locations_equal(left: &SourceLocation, right: &SourceLocation) -> bool {
    location_identity(left) == location_identity(right)
}

/// Canonical identity used for collision and equality checks.
#[must_use]
pub fn location_identity(location: &SourceLocation) -> String {
    match location {
        SourceLocation::Local { path } => {
            let normalized = portable_canonicalize(path);
            let value = normalized.to_string_lossy().replace('\\', "/");
            #[cfg(windows)]
            let value = value.to_lowercase();
            format!("local\0{value}")
        }
        SourceLocation::GitHub {
            owner,
            repo,
            r#ref,
            repo_path,
        } => {
            let normalized_repo_path = repo_path
                .as_deref()
                .map(|path| path.replace('\\', "/"))
                .unwrap_or_default();
            format!(
                "github\0{}\0{}\0{}\0{}",
                owner.to_ascii_lowercase(),
                repo.to_ascii_lowercase(),
                r#ref.as_deref().unwrap_or_default(),
                normalized_repo_path
            )
        }
    }
}

/// Human-safe canonical reference for a standalone location.
#[must_use]
pub fn location_reference(location: &SourceLocation) -> String {
    match location {
        SourceLocation::Local { path } => path.display().to_string(),
        SourceLocation::GitHub {
            owner,
            repo,
            r#ref,
            repo_path,
        } => {
            let mut value = format!("{owner}/{repo}");
            if let Some(reference) = r#ref {
                value.push(':');
                value.push_str(reference);
            }
            if let Some(path) = repo_path {
                value.push('/');
                value.push_str(path);
            }
            value
        }
    }
}

fn validate_location(location: &SourceLocation, source_name: &str) -> Result<()> {
    match location {
        SourceLocation::Local { path } => {
            if path.as_os_str().is_empty() || !path.is_absolute() {
                return Err(SkillManagerError::InvalidInput(format!(
                    "local location for source '{source_name}' must be an absolute non-blank path"
                )));
            }
            if path.to_string_lossy().contains('\0') {
                return Err(SkillManagerError::InvalidInput(format!(
                    "local location for source '{source_name}' contains a NUL byte"
                )));
            }
        }
        SourceLocation::GitHub {
            owner,
            repo,
            r#ref,
            repo_path,
        } => {
            if !valid_github_segment(owner) || !valid_github_segment(repo) {
                return Err(SkillManagerError::InvalidInput(format!(
                    "GitHub location for source '{source_name}' requires a valid owner and repo"
                )));
            }
            if r#ref.as_ref().is_some_and(|value| value.trim().is_empty()) {
                return Err(SkillManagerError::InvalidInput(format!(
                    "GitHub location for source '{source_name}' has a blank ref"
                )));
            }
            if let Some(path) = repo_path {
                validate_github_repo_path(path, source_name)?;
            }
        }
    }
    Ok(())
}

fn validate_github_repo_path(path: &str, source_name: &str) -> Result<()> {
    if normalize_repo_path(path).is_err() || path.contains('\\') {
        return Err(SkillManagerError::InvalidInput(format!(
            "GitHub repository path for source '{source_name}' must be a normalized relative path"
        )));
    }
    Ok(())
}

/// Return a physical path when available, using the portable spelling used by
/// persisted configuration and human-facing paths.
///
/// Existing paths are canonicalized so equivalent symlinked spellings render
/// consistently. Missing paths receive the same lexical and Windows-prefix
/// normalization used by configuration persistence.
#[must_use]
pub(crate) fn portable_canonicalize(path: &Path) -> PathBuf {
    portable_path(
        &path
            .canonicalize()
            .unwrap_or_else(|_| lexically_normalized(path)),
    )
}

/// Compare filesystem locations using physical identity when possible.
///
/// Existing paths are canonicalized so symlinked spellings compare equal. If
/// canonicalization is unavailable, both paths receive the same lexical and
/// Windows-prefix normalization used for persisted configuration paths.
#[must_use]
pub fn paths_equal(left: &Path, right: &Path) -> bool {
    let left = portable_canonicalize(left);
    let right = portable_canonicalize(right);
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

/// Remove Windows verbatim prefixes so a path is displayed and stored plainly.
///
/// `std::fs::canonicalize` can return `\\?\C:\...` or `\\?\UNC\...` spellings.
/// Those are valid but unfamiliar, so every persisted and user-facing path uses
/// the ordinary spelling instead. Non-Windows paths are returned unchanged.
#[must_use]
pub fn portable_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if let Some(rest) = wide.strip_prefix(VERBATIM_UNC_PREFIX) {
            let mut normal = vec![u16::from(b'\\'), u16::from(b'\\')];
            normal.extend_from_slice(rest);
            return PathBuf::from(OsString::from_wide(&normal));
        }
        if let Some(rest) = wide.strip_prefix(VERBATIM_PREFIX) {
            return PathBuf::from(OsString::from_wide(rest));
        }
    }
    path.to_path_buf()
}

fn lexically_normalized(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _removed = normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}

/// Expand a leading `~` in `raw` against `home`, the already-resolved manager
/// home (see [`manager_home`]). `home` must be the SAME value the caller
/// resolved `--home`/`SKILL_MANAGER_HOME`/the OS home into; this function
/// never re-resolves it, so a caller cannot silently fall back to the real
/// OS home behind an overridden home.
pub(crate) fn expand_home(raw: &str, home: &Path) -> PathBuf {
    if raw == "~" || raw.starts_with("~/") || raw.starts_with("~\\") {
        if raw == "~" {
            return home.to_path_buf();
        }
        return home.join(&raw[2..]);
    }
    PathBuf::from(raw)
}

/// Resolve skill-manager's home directory.
///
/// Precedence is `home_override` (the `--home` flag, threaded explicitly by
/// every caller from the value `main` parsed once), then `SKILL_MANAGER_HOME`
/// as an explicit automation/test override, then the operating system home.
/// The winning value controls configuration, cache, `~` source expansion, and
/// all built-in targets.
///
/// # Errors
///
/// Returns an error when neither override nor the operating system supplies a home.
pub fn manager_home(home_override: Option<&Path>) -> Result<PathBuf> {
    if let Some(home) = home_override {
        return absolutize_home(home.to_path_buf());
    }
    if let Some(override_home) = std::env::var_os("SKILL_MANAGER_HOME")
        && !override_home.is_empty()
    {
        return absolutize_home(PathBuf::from(override_home));
    }
    home::home_dir()
        .ok_or_else(|| {
            SkillManagerError::InvalidInput("could not determine the user home directory".into())
        })
        .and_then(absolutize_home)
}

/// Resolve a manager-home value to an absolute, lexically clean path.
///
/// A relative `--home` or `SKILL_MANAGER_HOME` value must be made absolute
/// before it is threaded into any derived path, or its raw `.`/`..`/mixed
/// separators leak into cache staging paths and trip the journal's own
/// path-safety validation (which is correct — the input was wrong). Both
/// override sources funnel through here so they cannot diverge.
///
/// `canonicalize` is deliberately NOT used: the home is frequently created by
/// the very command being run, so it may not exist yet, and `canonicalize`
/// fails on missing paths. Instead a relative value is joined against the
/// current directory and `.`/`..` segments are collapsed lexically, which also
/// normalizes mixed separators, a trailing separator, and a bare `.`.
fn absolutize_home(path: PathBuf) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|error| SkillManagerError::io(".", error))?
            .join(path)
    };
    Ok(portable_path(&lexically_normalized(&absolute)))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitHubReference {
    owner: String,
    repo: String,
    reference: Option<String>,
    repo_path: Option<String>,
}

/// Parse a canonical GitHub source reference.
///
/// `Ok(None)` means the operand is not a canonical GitHub reference and may
/// be interpreted as a local path. An explicit `github.com` URL is never
/// silently downgraded to a local path: malformed URLs return an input error.
pub(crate) fn parse_github_reference(raw: &str) -> Result<Option<GitHubReference>> {
    // Local spelling is intentional, even when its first two path components
    // happen to satisfy the GitHub shorthand grammar (for example `./skills`).
    // Keep this boundary here so every source-reference consumer agrees before
    // add-operand inference gets a chance to probe the filesystem.
    if is_explicit_local_reference(raw) {
        return Ok(None);
    }
    if let Ok(url) = Url::parse(raw) {
        if url.scheme() != "http" && url.scheme() != "https" {
            return Ok(None);
        }
        if !matches!(url.host_str(), Some("github.com" | "www.github.com")) {
            return Ok(None);
        }
        return parse_github_url(raw, &url).map(Some);
    }
    if is_explicit_github_url(raw) {
        return Err(invalid_github_reference(raw));
    }
    if raw.contains("://") || raw.contains('\\') {
        return Ok(None);
    }
    let parts = raw.split('/').collect::<Vec<_>>();
    if parts.len() < 2 || parts.iter().any(|part| part.is_empty()) {
        return Ok(None);
    }
    let owner = parts[0];
    let (repo, reference) = match parts[1].split_once(':') {
        Some((repo, reference)) if is_safe_github_path_component(reference) => {
            (repo, Some(reference.to_owned()))
        }
        Some(_) => return Ok(None),
        None => (parts[1], None),
    };
    if !valid_github_segment(owner) || !valid_github_segment(repo) {
        return Ok(None);
    }
    let repo_path = (parts.len() > 2).then(|| parts[2..].join("/"));
    if repo_path
        .as_deref()
        .is_some_and(|path| !is_normalized_github_repo_path(path))
    {
        return Ok(None);
    }
    Ok(Some(GitHubReference {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        reference,
        repo_path,
    }))
}

/// Return whether the complete operand is a canonical GitHub source
/// reference. This is the sole predicate used by argv role inference and JSON
/// recipe rebasing, so neither can drift from source parsing.
#[must_use]
pub(crate) fn is_github_reference(raw: &str) -> bool {
    parse_github_reference(raw).is_ok_and(|parsed| parsed.is_some())
}

fn parse_github_url(raw: &str, url: &Url) -> Result<GitHubReference> {
    if url.username() != "" || url.password().is_some() || url.port().is_some() {
        return Err(invalid_github_reference(raw));
    }
    let mut parts = url
        .path_segments()
        .ok_or_else(|| invalid_github_reference(raw))?
        .collect::<Vec<_>>();
    while parts.last() == Some(&"") {
        parts.pop();
    }
    if parts.iter().any(|part| part.is_empty()) || raw_url_path_has_unsafe_components(raw) {
        return Err(invalid_github_reference(raw));
    }
    if parts.len() < 2 {
        return Err(invalid_github_reference(raw));
    }
    let owner = parts[0];
    let repo = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
    if !valid_github_segment(owner) || !valid_github_segment(repo) {
        return Err(invalid_github_reference(raw));
    }
    let (reference, repo_path) = match parts.as_slice() {
        [_, _] => (None, None),
        [_, _, "tree", reference, tail @ ..]
            if is_safe_github_path_component(reference)
                && tail.iter().all(|part| is_safe_github_path_component(part)) =>
        {
            (
                Some((*reference).to_owned()),
                (!tail.is_empty()).then(|| tail.join("/")),
            )
        }
        _ => return Err(invalid_github_reference(raw)),
    };
    Ok(GitHubReference {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        reference,
        repo_path,
    })
}

fn invalid_github_reference(raw: &str) -> SkillManagerError {
    SkillManagerError::InvalidInput(format!("invalid GitHub source reference: {raw}"))
}

fn is_explicit_github_url(raw: &str) -> bool {
    let lowercase = raw.to_ascii_lowercase();
    [
        "http://github.com",
        "https://github.com",
        "http://www.github.com",
        "https://www.github.com",
    ]
    .iter()
    .any(|prefix| {
        lowercase
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(['/', '?', '#', ':']))
    })
}

fn raw_url_path_has_unsafe_components(raw: &str) -> bool {
    let Some((_, authority_and_path)) = raw.split_once("://") else {
        return true;
    };
    let path = authority_and_path
        .find('/')
        .map(|index| &authority_and_path[index + 1..])
        .unwrap_or_default()
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    let mut components = path.split('/').collect::<Vec<_>>();
    while components.last() == Some(&"") {
        components.pop();
    }
    components
        .iter()
        .any(|component| !is_safe_github_path_component(component))
}

fn is_normalized_github_repo_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.starts_with('/')
        && !path.ends_with('/')
        && path.split('/').all(is_safe_github_path_component)
}

fn is_safe_github_path_component(component: &str) -> bool {
    if component.is_empty() || matches!(component, "." | ".." | "~") {
        return false;
    }
    let lowercase = component.to_ascii_lowercase();
    !(component.contains('\\')
        || lowercase.contains("%2f")
        || lowercase.contains("%5c")
        || matches!(lowercase.as_str(), "%2e" | "%2e%2e" | ".%2e" | "%2e.")
        || (component.len() == 2
            && component.as_bytes()[0].is_ascii_alphabetic()
            && component.as_bytes()[1] == b':'))
}

/// Return whether `raw` uses a filesystem spelling that takes precedence over
/// GitHub shorthand parsing on every supported platform.
fn is_explicit_local_reference(raw: &str) -> bool {
    raw == "~"
        || raw.starts_with("~/")
        || raw.starts_with("~\\")
        || raw.starts_with("./")
        || raw.starts_with("../")
        || raw.starts_with(".\\")
        || raw.starts_with("..\\")
        || raw.starts_with(['/', '\\'])
        || Path::new(raw).is_absolute()
        // Recognize Windows drive-rooted paths even when parsing a persisted
        // configuration on a non-Windows host.
        || raw.as_bytes().get(1) == Some(&b':')
            && raw
                .as_bytes()
                .get(2)
                .is_some_and(|separator| matches!(separator, b'/' | b'\\'))
}

fn valid_github_segment(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
}

/// Derive the Python-compatible canonical source ID.
#[must_use]
pub fn derive_source_id(source: &SourceEntry) -> String {
    source_id_from_bytes(&canonical_source_identity_bytes(source))
}

/// Derive a deterministic fallback ID when the canonical ID remains occupied
/// by a source that has since moved to another location.
#[must_use]
pub fn derive_salted_source_id(source: &SourceEntry, salt: u64) -> String {
    let mut bytes = canonical_source_identity_bytes(source);
    bytes.push(0);
    bytes.extend_from_slice(b"skill-manager-source-id-salt=");
    bytes.extend_from_slice(salt.to_string().as_bytes());
    source_id_from_bytes(&bytes)
}

fn canonical_source_identity_bytes(source: &SourceEntry) -> Vec<u8> {
    let value = json!({
        "mode": source.mode,
        "owner": source.owner,
        "path": source.path.as_ref().map(|path| path.to_string_lossy().into_owned()),
        "ref": source.r#ref,
        "repo": source.repo,
        "repo_path": source.repo_path,
        "type": source.source_type,
    });
    let serialized = serde_json::to_string(&value).unwrap_or_default();
    ensure_ascii(&serialized).into_bytes()
}

fn source_id_from_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("src_{}", &hex::encode(digest)[..12])
}

fn ensure_ascii(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii() {
            output.push(character);
        } else {
            let code = u32::from(character);
            if code <= 0xffff {
                push_unicode_escape(&mut output, code);
            } else {
                let offset = code - 0x1_0000;
                let high = 0xd800 + (offset >> 10);
                let low = 0xdc00 + (offset & 0x3ff);
                push_unicode_escape(&mut output, high);
                push_unicode_escape(&mut output, low);
            }
        }
    }
    output
}

fn push_unicode_escape(output: &mut String, code: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    output.push('\\');
    output.push('u');
    for shift in [12, 8, 4, 0] {
        let nibble = ((code >> shift) & 0x0f) as usize;
        output.push(char::from(HEX[nibble]));
    }
}

/// Find a configured source by ID, name, unique label, path, or canonical reference.
///
/// `home` is the caller's already-resolved manager home, forwarded to
/// [`location_from_reference`] for `~` expansion of a local-path selector.
///
/// # Errors
///
/// Returns an ambiguity error when multiple sources share the selected label.
pub fn find_source_index(config: &Config, selector: &str, home: &Path) -> Result<Option<usize>> {
    let selector_folded = fold(selector);
    let direct = config.sources.iter().position(|source| {
        fold(&source.id) == selector_folded || fold(&source.name) == selector_folded
    });
    if direct.is_some() {
        return Ok(direct);
    }
    if let Ok(location) = location_from_reference(selector, SourceMode::Collection, home)
        && let Some(index) = config.sources.iter().position(|source| {
            source_location(source).is_ok_and(|active| locations_equal(&active, &location))
        })
    {
        return Ok(Some(index));
    }
    let mut labels = config
        .sources
        .iter()
        .enumerate()
        .filter(|(_, source)| fold(&source.label) == selector_folded)
        .map(|(index, _)| index);
    let first = labels.next();
    if first.is_some() && labels.next().is_some() {
        return Err(SkillManagerError::AmbiguousSourceLabel {
            label: selector.to_owned(),
        });
    }
    Ok(first)
}

/// Human-safe canonical source reference.
#[must_use]
pub fn source_reference(source: &SourceEntry) -> String {
    match source.source_type {
        SourceType::Local => source
            .path
            .as_ref()
            .map_or_else(|| source.name.clone(), |path| path.display().to_string()),
        SourceType::GitHub => {
            let mut value = format!(
                "{}/{}",
                source.owner.as_deref().unwrap_or_default(),
                source.repo.as_deref().unwrap_or_default()
            );
            if let Some(reference) = &source.r#ref {
                value.push(':');
                value.push_str(reference);
            }
            if let Some(path) = &source.repo_path {
                value.push('/');
                value.push_str(path);
            }
            value
        }
    }
}

/// Resolve built-in, custom, and legacy override targets in deterministic order.
#[must_use]
pub fn resolved_targets(config: &Config, home: &Path) -> IndexMap<String, Target> {
    resolved_targets_for_scope(config, home, home, Scope::Global)
        .into_iter()
        .map(|(name, scoped)| (name, scoped.target))
        .collect()
}

/// Resolve every target against one explicit installation scope.
#[must_use]
pub fn resolved_targets_for_scope(
    config: &Config,
    user_home: &Path,
    project_root: &Path,
    scope: Scope,
) -> IndexMap<String, ScopedTarget> {
    let root = scope.root(user_home, project_root);
    target_templates(config)
        .into_iter()
        .map(|(name, mut target)| {
            let template = target.path.clone();
            target.path = root.join(&template);
            (
                name,
                ScopedTarget {
                    target,
                    template,
                    scope,
                },
            )
        })
        .collect()
}

/// Resolve every target in both scopes, global first and project second.
#[must_use]
pub fn resolved_targets_by_scope(
    config: &Config,
    user_home: &Path,
    project_root: &Path,
) -> IndexMap<Scope, IndexMap<String, ScopedTarget>> {
    IndexMap::from([
        (
            Scope::Global,
            resolved_targets_for_scope(config, user_home, project_root, Scope::Global),
        ),
        (
            Scope::Project,
            resolved_targets_for_scope(config, user_home, project_root, Scope::Project),
        ),
    ])
}

fn target_templates(config: &Config) -> IndexMap<String, Target> {
    let defaults = builtin_targets();
    let mut result = IndexMap::new();
    for (name, mut target) in defaults {
        if let Some(settings) = config.builtins.get(&name) {
            target.enabled = settings.enabled;
        }
        if let Some(legacy) = config.legacy_target_overrides.get(&name) {
            target.path.clone_from(&legacy.path);
            if !legacy.label.is_empty() {
                target.label.clone_from(&legacy.label);
            }
            target.enabled = legacy.enabled;
            target.legacy_override = true;
        }
        result.insert(name, target);
    }
    for (name, target) in &config.targets {
        result.insert(
            name.clone(),
            Target {
                name: name.clone(),
                label: if target.label.is_empty() {
                    title_case(name)
                } else {
                    target.label.clone()
                },
                path: target.path.clone(),
                enabled: target.enabled,
                builtin: false,
                legacy_override: false,
            },
        );
    }
    result
}

/// Return whether a target name is reserved by a built-in.
#[must_use]
pub fn is_builtin_name(name: &str) -> bool {
    canonical_builtin_name(name).is_some()
}

fn canonical_builtin_name(name: &str) -> Option<&'static str> {
    match fold(name).as_str() {
        "claude" => Some("claude"),
        "shared" => Some("shared"),
        "antigravity" => Some("antigravity"),
        _ => None,
    }
}

fn builtin_targets() -> IndexMap<String, Target> {
    IndexMap::from([
        (
            "claude".into(),
            Target {
                name: "claude".into(),
                label: "Claude Code".into(),
                path: PathBuf::from(".claude").join("skills"),
                enabled: true,
                builtin: true,
                legacy_override: false,
            },
        ),
        (
            "shared".into(),
            Target {
                name: "shared".into(),
                label: "Shared (VS Code / Gemini / Copilot / Codex)".into(),
                path: PathBuf::from(".agents").join("skills"),
                enabled: true,
                builtin: true,
                legacy_override: false,
            },
        ),
        (
            "antigravity".into(),
            Target {
                name: "antigravity".into(),
                label: "Google Antigravity".into(),
                path: PathBuf::from(".gemini").join("antigravity").join("skills"),
                enabled: true,
                builtin: true,
                legacy_override: false,
            },
        ),
    ])
}

fn title_case(value: &str) -> String {
    value
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// NFKC Unicode case-fold a command-facing identity.
#[must_use]
pub fn fold(value: &str) -> String {
    value.nfkc().case_fold().collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use indexmap::IndexMap;
    use serde_json::json;

    use super::{
        BuiltinTargetSettings, Config, ConfigRepository, FileConfigRepository,
        derive_salted_source_id, derive_source_id, ensure_ascii, find_source_index,
        is_builtin_name, is_github_reference, locations_equal, manager_home, migrate_v0,
        normalize_target_template, parse_github_reference, paths_equal, resolved_targets,
        resolved_targets_for_scope, source_from_reference, source_location, source_reference,
        validate_config, validate_source,
    };
    use crate::domain::{Scope, SourceEntry, SourceLocation, SourceMode, SourceType, TargetEntry};

    /// Placeholder manager home for call sites whose reference is a GitHub
    /// shorthand or an already-absolute local path, neither of which is
    /// `~`-prefixed; `expand_home` never dereferences this value unless the
    /// raw string begins with `~`, so a fixed nonexistent path is safe here.
    fn unused_home() -> PathBuf {
        PathBuf::from("unused-manager-home")
    }

    fn write_absent_backup_record(
        repository: &FileConfigRepository,
        directory_name: &str,
        metadata_id: &str,
        created_at: chrono::DateTime<chrono::Utc>,
    ) {
        let directory = repository.backups_root.join(directory_name);
        std::fs::create_dir_all(&directory).unwrap_or_else(|error| unreachable!("{error}"));
        let metadata = super::BackupMetadata {
            id: metadata_id.into(),
            created_at,
            reason: "test".into(),
            original_path: repository.config_path.clone(),
            present: false,
            schema_version: None,
            valid: true,
        };
        std::fs::write(
            directory.join("metadata.json"),
            serde_json::to_vec(&metadata).unwrap_or_else(|error| unreachable!("{error}")),
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
    }

    #[test]
    fn physical_and_lexical_path_identity_detects_home_aliases() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let child = root.path().join("child");
        std::fs::create_dir_all(&child).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(paths_equal(root.path(), &child.join("..")));
        assert!(!paths_equal(root.path(), &child));

        let alias = root.path().join("physical-home-alias");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.path(), &alias)
                .unwrap_or_else(|error| unreachable!("{error}"));
            assert!(paths_equal(root.path(), &alias));
        }

        #[cfg(windows)]
        {
            let upper = std::path::PathBuf::from(root.path().to_string_lossy().to_uppercase());
            assert!(paths_equal(root.path(), &upper));
            if std::os::windows::fs::symlink_dir(root.path(), &alias).is_ok() {
                assert!(paths_equal(root.path(), &alias));
            }
        }
    }

    #[test]
    fn target_templates_are_safe_and_resolve_from_exact_scope_roots() {
        assert_eq!(
            normalize_target_template("~/.claude/./skills")
                .unwrap_or_else(|error| unreachable!("{error}")),
            std::path::PathBuf::from(".claude").join("skills")
        );
        assert!(normalize_target_template("").is_err());
        assert!(normalize_target_template("~other/skills").is_err());
        assert!(normalize_target_template("../skills").is_err());
        assert!(normalize_target_template("one/../../skills").is_err());
        assert!(normalize_target_template("C:\\absolute\\skills").is_err());

        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let project = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let config = Config::default();
        let global =
            resolved_targets_for_scope(&config, home.path(), project.path(), Scope::Global);
        let local =
            resolved_targets_for_scope(&config, home.path(), project.path(), Scope::Project);
        assert_eq!(
            global
                .get("claude")
                .unwrap_or_else(|| unreachable!())
                .target
                .path,
            home.path().join(".claude").join("skills")
        );
        assert_eq!(
            local
                .get("antigravity")
                .unwrap_or_else(|| unreachable!())
                .target
                .path,
            project
                .path()
                .join(".gemini")
                .join("antigravity")
                .join("skills")
        );
    }

    #[test]
    fn schema_one_targets_migrate_to_templates_with_raw_backup() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let legacy = home.path().join(".skill-manager.config.json");
        let bytes = br#"{"schema_version":1,"sources":[],"targets":{"custom":{"path":"C:\\Users\\me\\.custom\\skills","future":true}},"legacy_target_overrides":{},"builtins":{},"exclude":[],"root_future":42}"#;
        std::fs::write(&legacy, bytes).unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());

        let loaded = repository
            .load(true)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(loaded.config.schema_version, 2);
        assert_eq!(
            loaded
                .config
                .targets
                .get("custom")
                .unwrap_or_else(|| unreachable!())
                .path,
            std::path::PathBuf::from(".custom").join("skills")
        );
        assert_eq!(loaded.config.extra.get("root_future"), Some(&json!(42)));
        assert_eq!(
            loaded
                .config
                .targets
                .get("custom")
                .and_then(|target| target.extra.get("future")),
            Some(&json!(true))
        );
        let backups = repository
            .list_backups()
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read(&backups[0].raw_path).unwrap_or_else(|error| unreachable!("{error}")),
            bytes
        );
        assert!(!legacy.exists());
        assert!(repository.config_path().exists());
    }

    #[test]
    fn reset_and_restore_preserve_malformed_bytes_and_absent_state() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let absent = repository
            .reset_config()
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!absent.metadata.present);

        let malformed = b"{ definitely not json";
        std::fs::write(repository.config_path(), malformed)
            .unwrap_or_else(|error| unreachable!("{error}"));
        let invalid = repository
            .reset_config()
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(invalid.metadata.present);
        assert!(!invalid.metadata.valid);

        repository
            .restore_config(Some(&invalid.metadata.id))
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            repository
                .read_raw()
                .unwrap_or_else(|error| unreachable!("{error}")),
            Some(malformed.to_vec())
        );

        repository
            .restore_config(Some(&absent.metadata.id))
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            repository
                .read_raw()
                .unwrap_or_else(|error| unreachable!("{error}"))
                .is_none()
        );
    }

    #[test]
    fn absent_restore_does_not_reimport_stale_legacy_config_and_cache_still_migrates() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let absent = repository
            .reset_config()
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(
            &repository.layout_paths.python_flat_config,
            br#"{"schema_version":2,"exclude":["stale"]}"#,
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::create_dir_all(&repository.layout_paths.legacy_cache)
            .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(
            repository.layout_paths.legacy_cache.join("entry"),
            b"cached",
        )
        .unwrap_or_else(|error| unreachable!("{error}"));

        repository
            .restore_config(Some(&absent.metadata.id))
            .unwrap_or_else(|error| unreachable!("{error}"));
        let loaded = repository
            .load(false)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!loaded.persisted);
        assert!(loaded.config.exclude.is_empty());
        assert!(repository.layout_paths.python_flat_config.exists());
        assert_eq!(
            std::fs::read(repository.cache_root.join("entry"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            b"cached"
        );
    }

    #[test]
    fn backup_metadata_cannot_redirect_pruning_outside_the_backup_root() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let outside = repository.storage_root.join("outside");
        std::fs::create_dir_all(&outside).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(outside.join("sentinel"), b"keep")
            .unwrap_or_else(|error| unreachable!("{error}"));
        let old = chrono::Utc::now() - chrono::Duration::days(31);
        for (directory, id) in [
            ("traversal", "../../outside"),
            ("windows-traversal", r"..\..\outside"),
            ("mismatch", "different"),
            ("root", ".."),
        ] {
            write_absent_backup_record(&repository, directory, id, old);
        }
        write_absent_backup_record(&repository, "old-valid", "old-valid", old);
        write_absent_backup_record(&repository, "new-valid", "new-valid", chrono::Utc::now());

        let records = repository
            .list_backups_unlocked()
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(records.len(), 2);
        repository
            .prune_backups_unlocked(None)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            std::fs::read(outside.join("sentinel")).unwrap_or_else(|error| unreachable!("{error}")),
            b"keep"
        );
        assert!(!repository.backups_root.join("old-valid").exists());
        assert!(repository.backups_root.join("new-valid").exists());
        assert!(repository.backups_root.is_dir());
        for directory in ["traversal", "windows-traversal", "mismatch", "root"] {
            assert!(repository.backups_root.join(directory).exists());
        }
    }

    #[test]
    fn ordinary_save_prunes_expired_backups_only_after_persistence_succeeds() {
        let successful_home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let successful = FileConfigRepository::new(successful_home.path());
        let expired = chrono::Utc::now() - chrono::Duration::days(31);
        write_absent_backup_record(&successful, "old-valid", "old-valid", expired);
        write_absent_backup_record(&successful, "new-valid", "new-valid", chrono::Utc::now());

        ConfigRepository::save(&successful, successful.config_path(), &Config::default())
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!successful.backups_root.join("old-valid").exists());
        assert!(successful.backups_root.join("new-valid").exists());
        assert!(successful.config_path().is_file());

        let failed_home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let failed = FileConfigRepository::new(failed_home.path());
        write_absent_backup_record(&failed, "old-valid", "old-valid", expired);
        write_absent_backup_record(&failed, "new-valid", "new-valid", chrono::Utc::now());
        let mut invalid = Config::default();
        invalid.targets.insert(
            "custom".into(),
            TargetEntry {
                path: "../outside".into(),
                label: "Invalid".into(),
                enabled: true,
                extra: IndexMap::new(),
            },
        );

        assert!(ConfigRepository::save(&failed, failed.config_path(), &invalid).is_err());
        assert!(failed.backups_root.join("old-valid").exists());
        assert!(failed.backups_root.join("new-valid").exists());
        assert!(!failed.config_path().exists());
    }

    #[cfg(unix)]
    #[test]
    fn backup_listing_does_not_follow_directory_or_metadata_symlinks() {
        use std::os::unix::fs::symlink;

        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let outside = home.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap_or_else(|error| unreachable!("{error}"));
        write_absent_backup_record(&repository, "real", "real", chrono::Utc::now());
        symlink(&outside, repository.backups_root.join("linked-directory"))
            .unwrap_or_else(|error| unreachable!("{error}"));
        let linked_metadata = repository.backups_root.join("linked-metadata");
        std::fs::create_dir_all(&linked_metadata).unwrap_or_else(|error| unreachable!("{error}"));
        symlink(
            repository.backups_root.join("real/metadata.json"),
            linked_metadata.join("metadata.json"),
        )
        .unwrap_or_else(|error| unreachable!("{error}"));

        let records = repository
            .list_backups_unlocked()
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].metadata.id, "real");
        assert!(outside.exists());
        assert!(linked_metadata.exists());
    }

    #[test]
    fn source_id_is_stable() {
        let source = source_from_reference("owner/repo:main/team", None, &unused_home())
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(source.id, derive_source_id(&source));
        assert!(source.id.starts_with("src_"));
        assert_eq!(source.id.len(), 16);
    }

    #[test]
    fn salted_source_id_uses_canonical_identity_bytes_and_decimal_salt() {
        let source =
            source_from_reference("owner/repo", Some(SourceMode::Collection), &unused_home())
                .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(derive_salted_source_id(&source, 1), "src_e5e18bfaf9b8");
        assert_eq!(
            derive_salted_source_id(&source, 1),
            derive_salted_source_id(&source, 1)
        );
        assert_ne!(
            derive_salted_source_id(&source, 1),
            derive_salted_source_id(&source, 2)
        );
    }

    #[test]
    fn migrates_legacy_configuration_once() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let legacy = home.path().join(".skills-syncer.config.json");
        std::fs::write(
            &legacy,
            r#"{"skills_directories":{"C:\\skills":{"name":"mine"}}}"#,
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let loaded = repository
            .load(false)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(loaded.config.schema_version, 2);
        assert_eq!(loaded.config.sources.len(), 1);
        assert!(!legacy.exists());
        assert!(repository.config_path().exists());
        assert_eq!(
            repository
                .list_backups()
                .unwrap_or_else(|error| unreachable!("{error}"))
                .len(),
            1
        );
    }

    #[test]
    fn rejects_non_integer_and_future_schema_without_writing() {
        for schema in [
            serde_json::json!("1"),
            serde_json::json!(-1),
            serde_json::json!(0.5),
            serde_json::Value::Null,
            serde_json::json!(3),
        ] {
            let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
            let path = home.path().join(".skill-manager.config.json");
            let bytes = serde_json::to_vec(&serde_json::json!({ "schema_version": schema }))
                .unwrap_or_else(|error| unreachable!("{error}"));
            std::fs::write(&path, &bytes).unwrap_or_else(|error| unreachable!("{error}"));
            let repository = FileConfigRepository::new(home.path());
            assert!(repository.load(false).is_err());
            assert_eq!(
                std::fs::read(repository.config_path())
                    .unwrap_or_else(|error| unreachable!("{error}")),
                bytes
            );
            assert!(!path.exists());
            assert!(
                repository
                    .list_backups()
                    .unwrap_or_else(|error| unreachable!("{error}"))
                    .is_empty()
            );
        }
    }

    #[test]
    fn migrates_legacy_disabled_target_to_enabled_state() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let path = home.path().join(".skill-manager.config.json");
        std::fs::write(
            &path,
            r#"{"targets":{"custom":{"path":"C:\\custom","disabled":true}}}"#,
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let loaded = repository
            .load(false)
            .unwrap_or_else(|error| unreachable!("{error}"));
        let custom = loaded
            .config
            .targets
            .get("custom")
            .unwrap_or_else(|| unreachable!("migrated target"));
        assert!(!custom.enabled);
        assert!(!custom.extra.contains_key("disabled"));
        let saved = std::fs::read_to_string(repository.config_path())
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!saved.contains("\"disabled\""));
        assert!(!path.exists());
    }

    #[test]
    fn current_config_wins_and_save_round_trips_unknown_fields() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let current = home.path().join(".skill-manager.config.json");
        let legacy = home.path().join(".skills-syncer.config.json");
        std::fs::write(
            &current,
            r#"{"schema_version":1,"sources":[],"root_extension":{"keep":true}}"#,
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(&legacy, r#"{"schema_version":1,"sources":[]}"#)
            .unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let mut loaded = repository
            .load(false)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(loaded.active_path, repository.config_path());
        assert!(loaded.warning.is_some());
        assert_eq!(
            loaded.config.extra.get("root_extension"),
            Some(&json!({"keep": true}))
        );
        loaded.config.exclude.push("draft-*".into());
        repository
            .save(&loaded.active_path, &loaded.config)
            .unwrap_or_else(|error| unreachable!("{error}"));
        let bytes =
            std::fs::read(&loaded.active_path).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert!(!current.exists());
        assert!(legacy.exists());
        let reloaded = repository
            .load(false)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(reloaded.config.exclude, ["draft-*"]);
        assert_eq!(
            reloaded.config.extra.get("root_extension"),
            Some(&json!({"keep": true}))
        );
    }

    #[test]
    fn startup_migration_persists_during_dry_run_and_is_idempotent() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let current = home.path().join(".skill-manager.config.json");
        let original = br#"{"schema_version":0,"sources":[]}"#;
        std::fs::write(&current, original).unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let loaded = repository
            .load(true)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(loaded.config.schema_version, 2);
        assert!(!current.exists());
        let backups = repository
            .list_backups()
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read(&backups[0].raw_path).unwrap_or_else(|error| unreachable!("{error}")),
            original
        );
        let persisted =
            std::fs::read(repository.config_path()).unwrap_or_else(|error| unreachable!("{error}"));
        assert_ne!(persisted, original);
        repository
            .load(true)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            repository
                .list_backups()
                .unwrap_or_else(|error| unreachable!("{error}"))
                .len(),
            1
        );
    }

    #[test]
    fn source_references_cover_urls_shorthand_local_identity_and_lookup() {
        let shorthand =
            source_from_reference("owner/repo:feature/team/skills", None, &unused_home())
                .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(shorthand.source_type, SourceType::GitHub);
        assert_eq!(shorthand.r#ref.as_deref(), Some("feature"));
        assert_eq!(shorthand.repo_path.as_deref(), Some("team/skills"));
        assert_eq!(
            source_reference(&shorthand),
            "owner/repo:feature/team/skills"
        );

        // These local markers must retain their meaning on every host: a
        // Windows spelling may be read while validating configuration on a
        // non-Windows machine, and vice versa.
        for local_spelling in [
            "./skills",
            "../skills",
            r".\skills",
            r"..\skills",
            "~/skills",
            r"~\skills",
            "/skills",
            r"\skills",
            "C:/skills",
            r"C:\skills",
        ] {
            let local = source_from_reference(local_spelling, None, &unused_home())
                .unwrap_or_else(|error| unreachable!("{error}"));
            assert_eq!(local.source_type, SourceType::Local, "{local_spelling}");
        }

        let url = source_from_reference(
            "https://github.com/owner/repo.git/tree/main/nested/skills",
            None,
            &unused_home(),
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(url.owner.as_deref(), Some("owner"));
        assert_eq!(url.repo.as_deref(), Some("repo"));
        assert_eq!(url.r#ref.as_deref(), Some("main"));
        assert_eq!(url.repo_path.as_deref(), Some("nested/skills"));

        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let local = source_from_reference(&root.path().to_string_lossy(), None, root.path())
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(local.source_type, SourceType::Local);
        let config = Config {
            sources: vec![shorthand.clone(), local.clone()],
            ..Config::default()
        };
        for selector in [
            shorthand.id.as_str(),
            shorthand.name.as_str(),
            "OWNER/REPO:feature/team/skills",
        ] {
            assert_eq!(
                find_source_index(&config, selector, root.path())
                    .unwrap_or_else(|error| unreachable!("{error}")),
                Some(0)
            );
        }
        assert_eq!(
            find_source_index(
                &config,
                &local
                    .path
                    .as_ref()
                    .unwrap_or_else(|| unreachable!())
                    .to_string_lossy(),
                root.path(),
            )
            .unwrap_or_else(|error| unreachable!("{error}")),
            Some(1)
        );
        assert_eq!(
            find_source_index(&config, "missing", root.path())
                .unwrap_or_else(|error| unreachable!("{error}")),
            None
        );

        assert_eq!(ensure_ascii("é😀"), "\\u00e9\\ud83d\\ude00");
        assert!(is_builtin_name("CLAUDE"));
        assert!(!is_builtin_name("custom"));
    }

    #[test]
    fn github_reference_classification_validates_the_complete_reference() {
        for reference in [
            "owner/repo",
            "owner/repo/subdir",
            "owner/repo:main/subdir/nested",
            "https://github.com/owner/repo",
            "https://www.github.com/owner/repo.git/",
            "https://github.com/owner/repo/tree/main",
            "https://github.com/owner/repo/tree/main/subdir/nested",
        ] {
            assert!(
                is_github_reference(reference),
                "expected GitHub: {reference}"
            );
            assert!(
                parse_github_reference(reference)
                    .unwrap_or_else(|error| unreachable!("{reference}: {error}"))
                    .is_some(),
                "{reference}"
            );
        }

        for reference in [
            "./bar",
            "../bar",
            r".\bar",
            r"..\bar",
            "/rooted/bar",
            r"\rooted\bar",
            "C:/rooted/bar",
            r"C:\rooted\bar",
            "~/bar",
            r"~\bar",
            "owner/repo/../local",
            "owner/repo:../local",
            "owner/./repo",
            "owner/repo//subdir",
            "owner/repo/~/subdir",
            "owner/repo/C:/subdir",
            r"owner\repo\subdir",
            r"owner/repo/subdir\escape",
        ] {
            assert!(
                !is_github_reference(reference),
                "expected local: {reference}"
            );
            assert!(
                parse_github_reference(reference)
                    .unwrap_or_else(|error| unreachable!("{reference}: {error}"))
                    .is_none(),
                "{reference}"
            );
        }
    }

    #[test]
    fn malformed_explicit_github_urls_are_input_errors() {
        for reference in [
            "https://github.com/owner",
            "https://github.com/owner/repo/not-tree/main",
            "https://github.com/owner/repo/tree",
            "https://github.com/owner/repo/tree/main/../local",
            "https://github.com/owner/repo/tree/main//local",
            "https://github.com/owner/repo/tree/main/%2E%2E/local",
            "https://github.com/owner/repo/tree/main/C:/local",
        ] {
            let Err(error) = source_from_reference(reference, None, &unused_home()) else {
                unreachable!("malformed explicit GitHub URL succeeded: {reference}");
            };
            assert!(
                error
                    .to_string()
                    .contains("invalid GitHub source reference"),
                "{reference}: {error}"
            );
        }
    }

    #[test]
    fn source_lookup_accepts_unique_labels_and_rejects_ambiguous_labels() {
        let mut first = source_from_reference("owner/first", None, &unused_home())
            .unwrap_or_else(|error| unreachable!("{error}"));
        first.label = "Team Skills".into();
        let mut second = source_from_reference("owner/second", None, &unused_home())
            .unwrap_or_else(|error| unreachable!("{error}"));
        second.label = "Other Skills".into();
        let mut config = Config {
            sources: vec![first, second],
            ..Config::default()
        };
        assert_eq!(
            find_source_index(&config, "TEAM SKILLS", &unused_home())
                .unwrap_or_else(|error| unreachable!("{error}")),
            Some(0)
        );
        config.sources[1].label = "team skills".into();
        assert!(find_source_index(&config, "Team Skills", &unused_home()).is_err());
        assert_eq!(
            find_source_index(&config, "first", &unused_home())
                .unwrap_or_else(|error| unreachable!("{error}")),
            Some(0),
            "unique names take precedence even when labels collide"
        );
    }

    #[test]
    fn target_resolution_applies_builtin_state_legacy_override_and_custom_defaults() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let mut config = Config::default();
        config.builtins.insert(
            "shared".into(),
            BuiltinTargetSettings {
                enabled: false,
                extra: IndexMap::new(),
            },
        );
        config.legacy_target_overrides.insert(
            "claude".into(),
            TargetEntry {
                path: home.path().join("legacy-claude"),
                label: "Legacy Claude".into(),
                enabled: false,
                extra: IndexMap::new(),
            },
        );
        config.targets.insert(
            "my-target".into(),
            TargetEntry {
                path: home.path().join("custom"),
                label: String::new(),
                enabled: true,
                extra: IndexMap::new(),
            },
        );
        let targets = resolved_targets(&config, home.path());
        assert_eq!(
            targets
                .get("claude")
                .unwrap_or_else(|| unreachable!("claude"))
                .path,
            home.path().join("legacy-claude")
        );
        assert!(
            targets
                .get("claude")
                .unwrap_or_else(|| unreachable!("claude"))
                .legacy_override
        );
        assert!(
            !targets
                .get("shared")
                .unwrap_or_else(|| unreachable!("shared"))
                .enabled
        );
        assert_eq!(
            targets
                .get("my-target")
                .unwrap_or_else(|| unreachable!("custom"))
                .label,
            "My Target"
        );
    }

    #[test]
    fn invalid_config_sources_and_reserved_custom_targets_are_rejected() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let active = home.path().join(".skill-manager.config.json");
        let invalid_schema = Config {
            schema_version: 0,
            ..Config::default()
        };
        assert!(repository.save(&active, &invalid_schema).is_err());

        let mut negative_ttl = Config::default();
        let mut source = source_from_reference("owner/repo", None, home.path())
            .unwrap_or_else(|error| unreachable!("{error}"));
        source.cache_ttl_hours = Some(-1);
        negative_ttl.sources.push(source);
        assert!(repository.save(&active, &negative_ttl).is_err());

        let mut duplicate = Config::default();
        let one = source_from_reference("owner/one", None, home.path())
            .unwrap_or_else(|error| unreachable!("{error}"));
        let mut two = source_from_reference("owner/two", None, home.path())
            .unwrap_or_else(|error| unreachable!("{error}"));
        two.name.clone_from(&one.name);
        duplicate.sources = vec![one, two];
        assert!(repository.save(&active, &duplicate).is_err());

        let mut reserved = Config::default();
        reserved.targets.insert(
            "Claude".into(),
            TargetEntry {
                path: home.path().join("reserved"),
                label: String::new(),
                enabled: true,
                extra: IndexMap::new(),
            },
        );
        assert!(repository.save(&active, &reserved).is_err());
    }

    #[test]
    fn rich_v0_migration_preserves_sources_targets_and_unknown_fields() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let local = home.path().join("local");
        let mapped = home.path().join("mapped");
        for path in [&local, &mapped] {
            std::fs::create_dir_all(path).unwrap_or_else(|error| unreachable!("{error}"));
        }
        let value = json!({
            "root_extension": {"preserve": true},
            "sources": [
                "owner/repo:main/team",
                {
                    "id": "legacy-local-id",
                    "type": "local",
                    "mode": "collection",
                    "name": "",
                    "label": "",
                    "path": local,
                    "alias": "local-alias",
                    "source_extension": 1
                },
                {
                    "id": "legacy-github-id",
                    "type": "github",
                    "mode": "collection",
                    "name": "",
                    "label": "",
                    "owner": "other",
                    "repo": "project",
                    "path": "nested/skills"
                }
            ],
            "skills_directories": {
                mapped.to_string_lossy().into_owned(): {
                    "name": "mapped-name",
                    "label": "Mapped Label",
                    "exclude": ["draft-*"]
                }
            },
            "targets": {
                "claude": {
                    "path": home.path().join(".claude").join("skills"),
                    "disabled": true
                },
                "shared": {
                    "path": home.path().join("legacy-shared"),
                    "disabled": false,
                    "target_extension": "keep"
                },
                "custom": {
                    "path": home.path().join("custom"),
                    "disabled": true
                }
            }
        });
        let migrated =
            migrate_v0(&value, home.path()).unwrap_or_else(|error| unreachable!("{error}"));
        let config: Config =
            serde_json::from_value(migrated).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(config.sources.len(), 4);
        assert_eq!(config.sources[1].id, "legacy-local-id");
        assert_eq!(config.sources[1].name, "local-alias");
        assert_eq!(
            config.sources[2].repo_path.as_deref(),
            Some("nested/skills")
        );
        assert_eq!(config.sources[3].label, "Mapped Label");
        assert_eq!(config.sources[3].exclude, ["draft-*"]);
        assert!(config.builtins.get("claude").is_none());
        assert!(
            !config
                .legacy_target_overrides
                .get("claude")
                .unwrap_or_else(|| unreachable!("override"))
                .enabled
        );
        assert!(
            config
                .legacy_target_overrides
                .get("shared")
                .unwrap_or_else(|| unreachable!("override"))
                .enabled
        );
        assert!(
            !config
                .targets
                .get("custom")
                .unwrap_or_else(|| unreachable!("custom"))
                .enabled
        );
        assert_eq!(
            config.extra.get("root_extension"),
            Some(&json!({"preserve": true}))
        );
    }

    #[test]
    fn hybrid_v0_migration_keeps_explicit_source_for_overlapping_legacy_path() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let explicit_path = home.path().join("personal");
        let legacy_only_path = home.path().join("legacy-only");
        for path in [&explicit_path, &legacy_only_path] {
            std::fs::create_dir_all(path).unwrap_or_else(|error| unreachable!("{error}"));
        }
        let config_path = home.path().join(".skill-manager.config.json");
        let value = json!({
            "root_extension": "keep",
            "sources": [
                {
                    "id": "explicit-personal-id",
                    "type": "local",
                    "mode": "collection",
                    "name": "personal",
                    "label": "Explicit Personal",
                    "exclude": ["private-*"],
                    "path": explicit_path,
                    "source_extension": {"keep": true}
                },
                "owner/repo:main/skills"
            ],
            "skills_directories": {
                explicit_path.to_string_lossy().into_owned(): {
                    "name": "personal",
                    "label": "Legacy Personal",
                    "exclude": ["legacy-*"]
                },
                legacy_only_path.to_string_lossy().into_owned(): {
                    "name": "legacy-only",
                    "label": "Legacy Only"
                }
            }
        });
        let original = serde_json::to_vec(&value).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(&config_path, &original).unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());

        let dry_run = repository
            .load(true)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(dry_run.config.sources.len(), 3);
        assert_eq!(dry_run.config.sources[0].id, "explicit-personal-id");
        assert_eq!(dry_run.config.sources[0].label, "Explicit Personal");
        assert_eq!(dry_run.config.sources[0].exclude, ["private-*"]);
        assert_eq!(
            dry_run.config.sources[0].extra.get("source_extension"),
            Some(&json!({"keep": true}))
        );
        assert_eq!(dry_run.config.sources[1].name, "repo");
        assert_eq!(dry_run.config.sources[2].name, "legacy-only");
        assert!(!config_path.exists());
        let backups = repository
            .list_backups()
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read(&backups[0].raw_path).unwrap_or_else(|error| unreachable!("{error}")),
            original
        );

        let written = repository
            .load(false)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(written.config.sources.len(), 3);
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(repository.config_path())
                .unwrap_or_else(|error| unreachable!("{error}")),
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(persisted.get("skills_directories").is_none());
        assert_eq!(persisted["root_extension"], "keep");
        assert_eq!(persisted["sources"][0]["id"], "explicit-personal-id");
        assert_eq!(
            persisted["sources"][0]["source_extension"],
            json!({"keep": true})
        );
    }

    #[test]
    fn hybrid_v0_migration_rejects_same_name_at_different_paths() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let explicit_path = home.path().join("explicit");
        let legacy_path = home.path().join("legacy");
        for path in [&explicit_path, &legacy_path] {
            std::fs::create_dir_all(path).unwrap_or_else(|error| unreachable!("{error}"));
        }
        let value = json!({
            "sources": [{
                "id": "explicit-id",
                "type": "local",
                "mode": "collection",
                "name": "personal",
                "label": "Personal",
                "path": explicit_path
            }],
            "skills_directories": {
                legacy_path.to_string_lossy().into_owned(): {
                    "name": "PERSONAL"
                }
            }
        });
        let migrated =
            migrate_v0(&value, home.path()).unwrap_or_else(|error| unreachable!("{error}"));
        let config: Config =
            serde_json::from_value(migrated).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(config.sources.len(), 2);
        let error = validate_config(&config, &home.path().join("config.json"))
            .err()
            .unwrap_or_else(|| unreachable!("different paths must preserve the collision"));
        assert!(
            error
                .to_string()
                .contains("duplicate source name 'PERSONAL'")
        );
    }

    #[test]
    fn hybrid_v0_migration_validates_overlapping_legacy_metadata_before_skipping() {
        let invalid_metadata = [
            json!(null),
            json!({"name": 1}),
            json!({"label": false}),
            json!({"exclude": "not-an-array"}),
            json!({"exclude": ["valid", 1]}),
        ];
        for metadata in invalid_metadata {
            let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
            let source_path = home.path().join("personal");
            std::fs::create_dir_all(&source_path).unwrap_or_else(|error| unreachable!("{error}"));
            let config_path = home.path().join(".skill-manager.config.json");
            let value = json!({
                "sources": [{
                    "id": "explicit-personal-id",
                    "type": "local",
                    "mode": "collection",
                    "name": "personal",
                    "label": "Personal",
                    "path": source_path
                }],
                "skills_directories": {
                    source_path.to_string_lossy().into_owned(): metadata
                }
            });
            let original =
                serde_json::to_vec(&value).unwrap_or_else(|error| unreachable!("{error}"));
            std::fs::write(&config_path, &original).unwrap_or_else(|error| unreachable!("{error}"));
            let repository = FileConfigRepository::new(home.path());

            assert!(repository.load(false).is_err(), "{value}");
            assert_eq!(
                std::fs::read(repository.config_path())
                    .unwrap_or_else(|error| unreachable!("{error}")),
                original
            );
            assert!(!config_path.exists());
            assert!(
                repository
                    .list_backups()
                    .unwrap_or_else(|error| unreachable!("{error}"))
                    .is_empty()
            );
        }
    }

    #[test]
    fn v0_migration_rejects_colliding_builtin_target_aliases() {
        let value = json!({
            "targets": {
                "Claude": {
                    "path": "first",
                    "disabled": true
                },
                "claude": {
                    "path": "second",
                    "disabled": false
                }
            }
        });
        let error = migrate_v0(&value, &unused_home())
            .err()
            .unwrap_or_else(|| unreachable!("case-folded aliases must collide"));
        assert!(
            error
                .to_string()
                .contains("collides with another 'claude' built-in target")
        );
    }

    #[test]
    fn malformed_v0_shapes_and_incomplete_source_entries_are_rejected() {
        for value in [
            json!([]),
            json!({"sources": {}}),
            json!({"skills_directories": []}),
            json!({"targets": []}),
            json!({"sources": [{"type": "github", "owner": "", "repo": ""}]}),
        ] {
            assert!(migrate_v0(&value, &unused_home()).is_err(), "{value}");
        }

        let base = SourceEntry {
            id: "id".into(),
            source_type: SourceType::Local,
            mode: SourceMode::Collection,
            name: "name".into(),
            label: "Label".into(),
            exclude: Vec::new(),
            cache_ttl_hours: None,
            path: None,
            owner: None,
            repo: None,
            r#ref: None,
            repo_path: None,
            alternate: None,
            extra: IndexMap::new(),
        };
        assert!(validate_source(&base).is_err());
        let github = SourceEntry {
            source_type: SourceType::GitHub,
            path: None,
            ..base.clone()
        };
        assert!(validate_source(&github).is_err());
        let blank = SourceEntry {
            id: String::new(),
            ..base
        };
        assert!(validate_source(&blank).is_err());
    }

    #[test]
    fn malformed_nested_v0_values_never_rewrite_or_create_backup() {
        let cases = [
            json!({"skills_directories": {"C:\\skills": null}}),
            json!({"skills_directories": {"C:\\skills": {"name": 1}}}),
            json!({"skills_directories": {"C:\\skills": {"label": false}}}),
            json!({"skills_directories": {"C:\\skills": {"exclude": "bad"}}}),
            json!({"skills_directories": {"C:\\skills": {"exclude": ["ok", 1]}}}),
            json!({"sources": [{"type":"github","owner":"o","repo":"r","cache_ttl_hours":"bad"}]}),
            json!({"sources": [{"type":"github","owner":"o","repo":"r","cache_ttl_hours":-1}]}),
            json!({"sources": [{"type":"local","path":"C:\\skills","alias":[]}]}),
            json!({"targets": {"custom":{"path":"C:\\target","disabled":"yes"}}}),
            json!({"targets": {"custom":{"path":1}}}),
        ];
        for value in cases {
            let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
            let path = home.path().join(".skill-manager.config.json");
            let bytes = serde_json::to_vec(&value).unwrap_or_else(|error| unreachable!("{error}"));
            std::fs::write(&path, &bytes).unwrap_or_else(|error| unreachable!("{error}"));
            let repository = FileConfigRepository::new(home.path());
            assert!(repository.load(false).is_err(), "{value}");
            assert_eq!(
                std::fs::read(repository.config_path())
                    .unwrap_or_else(|error| unreachable!("{error}")),
                bytes
            );
            assert!(!path.exists());
            assert!(
                repository
                    .list_backups()
                    .unwrap_or_else(|error| unreachable!("{error}"))
                    .is_empty()
            );
        }
    }

    #[test]
    fn alternate_locations_are_closed_typed_objects_and_round_trip_in_schema_two() {
        for invalid in [
            json!({"type":"local","path":"C:\\skills","owner":"forbidden"}),
            json!({"type":"github","owner":"o","repo":"r","path":"forbidden"}),
            json!({"type":"github","owner":"o","repo":"r","unknown":true}),
            json!({"type":"local"}),
            json!({"type":"github","owner":"o"}),
        ] {
            assert!(
                serde_json::from_value::<SourceLocation>(invalid.clone()).is_err(),
                "{invalid}"
            );
        }

        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let mut source = source_from_reference("owner/repo:main/skills", None, root.path())
            .unwrap_or_else(|error| unreachable!("{error}"));
        source
            .extra
            .insert("source_extension".into(), json!("kept"));
        source.alternate = Some(SourceLocation::Local {
            path: root.path().to_path_buf(),
        });
        let config = Config {
            sources: vec![source],
            extra: IndexMap::from([("root_extension".into(), json!(42))]),
            ..Config::default()
        };
        let value = serde_json::to_value(&config).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(value["schema_version"], 2);
        assert_eq!(value["sources"][0]["alternate"]["type"], "local");
        let decoded: Config =
            serde_json::from_value(value).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(decoded.extra["root_extension"], 42);
        assert_eq!(decoded.sources[0].extra["source_extension"], "kept");
    }

    #[test]
    fn persistence_normalizes_local_paths_and_github_repo_path_separators() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(root.path());
        let config_path = root.path().join(".skill-manager.config.json");
        let mut source = source_from_reference("owner/repo:main/Skills", None, root.path())
            .unwrap_or_else(|error| unreachable!("{error}"));
        source.repo_path = Some(r"Skills\Team".into());
        source.alternate = Some(SourceLocation::Local {
            path: root.path().join("one").join("..").join("two"),
        });
        repository
            .save(
                &config_path,
                &Config {
                    sources: vec![source],
                    ..Config::default()
                },
            )
            .unwrap_or_else(|error| unreachable!("{error}"));
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&config_path).unwrap_or_else(|error| unreachable!("{error}")),
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(persisted["sources"][0]["repo_path"], json!("Skills/Team"));
        assert_eq!(
            std::path::Path::new(
                persisted["sources"][0]["alternate"]["path"]
                    .as_str()
                    .unwrap_or_else(|| unreachable!("path"))
            ),
            root.path().join("two")
        );

        assert!(locations_equal(
            &SourceLocation::GitHub {
                owner: "OWNER".into(),
                repo: "Repo".into(),
                r#ref: Some("Main".into()),
                repo_path: Some(r"Skills\Team".into()),
            },
            &SourceLocation::GitHub {
                owner: "owner".into(),
                repo: "repo".into(),
                r#ref: Some("Main".into()),
                repo_path: Some("Skills/Team".into()),
            }
        ));
    }

    #[test]
    fn normalized_equivalent_persisted_pairs_are_rejected_on_load() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let location = root.path().join("skills");
        let config_path = root.path().join(".skill-manager.config.json");
        let value = json!({
            "schema_version": 1,
            "sources": [{
                "id": "src_test",
                "type": "local",
                "mode": "collection",
                "name": "test",
                "label": "Test",
                "path": location.join("child").join(".."),
                "alternate": {
                    "type": "local",
                    "path": location
                }
            }]
        });
        std::fs::write(
            &config_path,
            serde_json::to_vec_pretty(&value).unwrap_or_else(|error| unreachable!("{error}")),
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(FileConfigRepository::new(root.path()).load(false).is_err());
    }

    #[test]
    fn repository_load_and_save_reject_equal_active_and_alternate_locations() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(root.path());
        let config_path = root.path().join(".skill-manager.config.json");
        let location = root.path().join("skills");
        let mut source = source_from_reference(&location.to_string_lossy(), None, root.path())
            .unwrap_or_else(|error| unreachable!("{error}"));
        source.alternate = Some(SourceLocation::Local {
            path: location.clone(),
        });
        let config = Config {
            sources: vec![source],
            ..Config::default()
        };

        assert!(repository.save(&config_path, &config).is_err());
        assert!(
            !config_path.exists(),
            "rejected saves must not create a file"
        );

        std::fs::write(
            &config_path,
            serde_json::to_vec_pretty(&config).unwrap_or_else(|error| unreachable!("{error}")),
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(repository.load(false).is_err());
    }

    #[test]
    fn local_normalization_clamps_excess_parent_components_at_the_root() {
        let temporary = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let filesystem_root = temporary
            .path()
            .ancestors()
            .last()
            .unwrap_or_else(|| unreachable!("absolute temporary path has a root"));
        let mut input = temporary.path().to_path_buf();
        for _ in 0..64 {
            input.push("..");
        }
        input.push("clamped-location-that-does-not-exist");

        let source = source_from_reference(&input.to_string_lossy(), None, temporary.path())
            .unwrap_or_else(|error| unreachable!("{error}"));
        let location = source_location(&source).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            location,
            SourceLocation::Local {
                path: filesystem_root.join("clamped-location-that-does-not-exist"),
            }
        );
    }

    #[test]
    fn own_location_duplicates_are_invalid_but_legacy_cross_source_collisions_load() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let mut first = source_from_reference(&root.path().to_string_lossy(), None, root.path())
            .unwrap_or_else(|error| unreachable!("{error}"));
        first.name = "first".into();
        first.alternate =
            Some(source_location(&first).unwrap_or_else(|error| unreachable!("{error}")));
        assert!(validate_source(&first).is_err());

        first.alternate = None;
        let mut second = first.clone();
        second.id = "different-id".into();
        second.name = "second".into();
        let config = Config {
            sources: vec![first, second],
            ..Config::default()
        };
        assert!(
            validate_config(&config, &root.path().join("config.json")).is_ok(),
            "pre-existing cross-source collisions remain loadable"
        );
    }

    /// A `--home` override carrying `.`/`..` segments must resolve to the
    /// lexically collapsed absolute path, not be threaded through verbatim.
    /// The raw form is what leaked into cache staging paths and tripped the
    /// journal's path-safety validation.
    #[test]
    fn manager_home_collapses_dot_and_parent_segments_in_an_absolute_override() {
        let base = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let messy = base.path().join(".").join("a").join("..").join("b");
        let expected = base.path().join("b");
        let resolved = manager_home(Some(&messy)).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(resolved, expected);
    }

    /// A trailing separator on an override must normalize away so the resolved
    /// home matches the same path without it.
    #[test]
    fn manager_home_normalizes_a_trailing_separator() {
        let base = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let expected = base.path().join("c");
        let mut trailing = expected.clone().into_os_string();
        trailing.push(std::path::MAIN_SEPARATOR.to_string());
        let resolved = manager_home(Some(std::path::Path::new(&trailing)))
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(resolved, expected);
    }

    /// On Windows a mixed-separator override must normalize to the platform
    /// spelling rather than leave a foreign separator in a derived path.
    #[cfg(windows)]
    #[test]
    fn manager_home_normalizes_mixed_separators_on_windows() {
        let base = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let mut mixed = base.path().join("mixed").into_os_string();
        mixed.push("/seg");
        let expected = base.path().join("mixed").join("seg");
        let resolved = manager_home(Some(std::path::Path::new(&mixed)))
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(resolved, expected);
    }
}
