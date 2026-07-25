//! Versioned configuration, source normalization, and target resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
#[cfg(windows)]
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
};

use fs2::FileExt;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;
use url::Url;

use crate::domain::{SourceEntry, SourceMode, SourceType, Target, TargetEntry};
use crate::error::{Result, SkillManagerError};

/// Current configuration schema.
pub const CONFIG_SCHEMA_VERSION: u32 = 1;
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

/// Persisted version-one configuration.
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
}

/// Persistence port used by the application service.
pub trait ConfigRepository {
    /// Load and, unless dry-running, migrate configuration.
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
}

/// User-home-backed configuration repository.
#[derive(Clone, Debug)]
pub struct FileConfigRepository {
    current_path: PathBuf,
    legacy_path: PathBuf,
    cache_root: PathBuf,
}

impl FileConfigRepository {
    /// Build a repository for the current user's home directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the user home directory is unavailable.
    pub fn for_current_user() -> Result<Self> {
        Ok(Self::new(manager_home()?))
    }

    /// Build a repository rooted at an explicit home, primarily for tests.
    #[must_use]
    pub fn new(home: impl AsRef<Path>) -> Self {
        let home = home.as_ref();
        Self {
            current_path: home.join(".skill-manager.config.json"),
            legacy_path: home.join(".skills-syncer.config.json"),
            cache_root: home.join(".skill-manager-cache"),
        }
    }

    fn select_active_path(&self, dry_run: bool) -> (PathBuf, Option<String>) {
        if self.current_path.exists() || !self.legacy_path.exists() {
            return (self.current_path.clone(), None);
        }
        if dry_run {
            return (
                self.legacy_path.clone(),
                Some(format!(
                    "would migrate legacy configuration {} to {}",
                    self.legacy_path.display(),
                    self.current_path.display()
                )),
            );
        }
        let mut last_error = None;
        for _ in 0..3 {
            match fs::rename(&self.legacy_path, &self.current_path) {
                Ok(()) => return (self.current_path.clone(), None),
                Err(error) => last_error = Some(error),
            }
        }
        let detail = last_error.map_or_else(|| "unknown error".into(), |error| error.to_string());
        (
            self.legacy_path.clone(),
            Some(format!(
                "could not rename {} to {} ({detail}); using the legacy path",
                self.legacy_path.display(),
                self.current_path.display()
            )),
        )
    }

    fn lock_path(&self) -> PathBuf {
        self.cache_root.join(".locks").join("config.lock")
    }
}

impl ConfigRepository for FileConfigRepository {
    fn load(&self, dry_run: bool) -> Result<LoadedConfig> {
        let (active_path, warning) = self.select_active_path(dry_run);
        if !active_path.exists() {
            return Ok(LoadedConfig {
                config: Config::default(),
                active_path,
                warning,
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
        let migrated = schema == 0;
        if migrated {
            value = migrate_v0(&value)?;
        }
        let config: Config =
            serde_json::from_value(value).map_err(|error| SkillManagerError::InvalidConfig {
                path: active_path.clone(),
                message: error.to_string(),
            })?;
        validate_config(&config, &active_path)?;
        if migrated && !dry_run {
            ensure_v0_backup(&active_path, &raw)?;
            self.save(&active_path, &config)?;
        }
        Ok(LoadedConfig {
            config,
            active_path,
            warning,
        })
    }

    fn save(&self, active_path: &Path, config: &Config) -> Result<()> {
        validate_config(config, active_path)?;
        let _lock = acquire_lock(&self.lock_path(), "configuration", Duration::from_secs(10))?;
        let parent = active_path.parent().ok_or_else(|| {
            SkillManagerError::InvalidInput(format!(
                "configuration path has no parent: {}",
                active_path.display()
            ))
        })?;
        fs::create_dir_all(parent).map_err(|error| SkillManagerError::io(parent, error))?;
        let mut bytes = serde_json::to_vec_pretty(config)
            .map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
        bytes.push(b'\n');
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| SkillManagerError::io(parent, error))?;
        temporary
            .write_all(&bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| SkillManagerError::io(temporary.path(), error))?;
        temporary
            .persist(active_path)
            .map_err(|error| SkillManagerError::io(active_path, error.error))?;
        Ok(())
    }

    fn cache_root(&self) -> &Path {
        &self.cache_root
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

fn ensure_v0_backup(path: &Path, expected: &[u8]) -> Result<()> {
    let backup = PathBuf::from(format!("{}.v0.bak", path.display()));
    match OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&backup)
    {
        Ok(mut file) => {
            file.write_all(expected)
                .and_then(|()| file.sync_all())
                .map_err(|error| SkillManagerError::io(&backup, error))?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = fs::read(&backup)
                .map_err(|read_error| SkillManagerError::io(&backup, read_error))?;
            if existing == expected {
                Ok(())
            } else {
                Err(SkillManagerError::InvalidConfig {
                    path: backup,
                    message: "existing migration backup does not match pre-migration bytes".into(),
                })
            }
        }
        Err(error) => Err(SkillManagerError::io(backup, error)),
    }
}

fn migrate_v0(value: &Value) -> Result<Value> {
    let mut root = value.as_object().cloned().ok_or_else(|| {
        SkillManagerError::InvalidInput("configuration root must be a JSON object".into())
    })?;
    migrate_v0_sources(&mut root)?;
    migrate_v0_targets(&mut root)?;
    root.insert(
        "schema_version".into(),
        Value::Number(CONFIG_SCHEMA_VERSION.into()),
    );
    Ok(Value::Object(root))
}

fn migrate_v0_sources(root: &mut Map<String, Value>) -> Result<()> {
    let mut sources = match root.remove("sources") {
        Some(Value::Array(values)) => values,
        Some(_) => {
            return Err(SkillManagerError::InvalidInput(
                "configuration 'sources' must be an array".into(),
            ));
        }
        None => Vec::new(),
    };
    if let Some(legacy) = root.remove("skills_directories") {
        let directories = legacy.as_object().ok_or_else(|| {
            SkillManagerError::InvalidInput(
                "configuration 'skills_directories' must be an object".into(),
            )
        })?;
        for (path, metadata) in directories {
            let mut entry = source_from_reference(path, None)?;
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
            sources.push(serde_json::to_value(entry).map_err(|error| {
                SkillManagerError::InvalidInput(format!("could not migrate source: {error}"))
            })?);
        }
    }
    let mut normalized_sources = Vec::new();
    let mut seen_source_ids = BTreeSet::new();
    for raw in sources {
        let entry = coerce_v0_source(&raw)?;
        if seen_source_ids.insert(entry.id.clone()) {
            normalized_sources.push(serde_json::to_value(entry).map_err(|error| {
                SkillManagerError::InvalidInput(format!("could not migrate source: {error}"))
            })?);
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

fn coerce_v0_source(raw: &Value) -> Result<SourceEntry> {
    if let Some(reference) = raw.as_str() {
        return source_from_reference(reference, None);
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
            let mut normalized = source_from_reference(&path.to_string_lossy(), Some(entry.mode))?;
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
    match source.source_type {
        SourceType::Local if source.path.is_none() => Err(SkillManagerError::InvalidInput(
            format!("local source '{}' requires path", source.name),
        )),
        SourceType::GitHub if source.owner.is_none() || source.repo.is_none() => {
            Err(SkillManagerError::InvalidInput(format!(
                "GitHub source '{}' requires owner and repo",
                source.name
            )))
        }
        _ => Ok(()),
    }
}

/// Normalize a local path or supported GitHub reference into a source.
///
/// # Errors
///
/// Returns an error when a local path cannot be absolutized or the user home is unavailable.
pub fn source_from_reference(raw: &str, mode: Option<SourceMode>) -> Result<SourceEntry> {
    if let Some((owner, repo, reference, repo_path)) = parse_github_reference(raw) {
        let mut entry = SourceEntry {
            id: String::new(),
            source_type: SourceType::GitHub,
            mode: mode.unwrap_or(SourceMode::Collection),
            name: repo.clone(),
            label: title_case(&repo),
            exclude: Vec::new(),
            cache_ttl_hours: None,
            path: None,
            owner: Some(owner),
            repo: Some(repo),
            r#ref: reference,
            repo_path,
            extra: IndexMap::new(),
        };
        entry.id = derive_source_id(&entry);
        return Ok(entry);
    }
    let expanded = expand_home(raw)?;
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map_err(|error| SkillManagerError::io(".", error))?
            .join(expanded)
    };
    let normalized = portable_canonicalize(absolute);
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
        extra: IndexMap::new(),
    };
    entry.id = derive_source_id(&entry);
    Ok(entry)
}

fn portable_canonicalize(path: PathBuf) -> PathBuf {
    let canonical = path.canonicalize().unwrap_or(path);
    #[cfg(windows)]
    {
        let wide: Vec<u16> = canonical.as_os_str().encode_wide().collect();
        if let Some(rest) = wide.strip_prefix(VERBATIM_UNC_PREFIX) {
            let mut normal = vec![u16::from(b'\\'), u16::from(b'\\')];
            normal.extend_from_slice(rest);
            return PathBuf::from(OsString::from_wide(&normal));
        }
        if let Some(rest) = wide.strip_prefix(VERBATIM_PREFIX) {
            return PathBuf::from(OsString::from_wide(rest));
        }
    }
    canonical
}

fn expand_home(raw: &str) -> Result<PathBuf> {
    if raw == "~" || raw.starts_with("~/") || raw.starts_with("~\\") {
        let home = manager_home()?;
        if raw == "~" {
            return Ok(home);
        }
        return Ok(home.join(&raw[2..]));
    }
    Ok(PathBuf::from(raw))
}

/// Resolve skill-manager's home directory.
///
/// `SKILL_MANAGER_HOME` is an explicit automation/test override controlling
/// configuration, cache, `~` source expansion, and all built-in targets.
///
/// # Errors
///
/// Returns an error when neither the override nor the operating system supplies a home.
pub fn manager_home() -> Result<PathBuf> {
    if let Some(override_home) = std::env::var_os("SKILL_MANAGER_HOME")
        && !override_home.is_empty()
    {
        return Ok(PathBuf::from(override_home));
    }
    home::home_dir().ok_or_else(|| {
        SkillManagerError::InvalidInput("could not determine the user home directory".into())
    })
}

fn parse_github_reference(raw: &str) -> Option<(String, String, Option<String>, Option<String>)> {
    if let Ok(url) = Url::parse(raw) {
        if url.scheme() != "http" && url.scheme() != "https" {
            return None;
        }
        if !matches!(url.host_str(), Some("github.com" | "www.github.com")) {
            return None;
        }
        let parts: Vec<_> = url
            .path_segments()?
            .filter(|part| !part.is_empty())
            .collect();
        if parts.len() < 2 {
            return None;
        }
        if parts.get(2) == Some(&"tree") && parts.len() >= 4 {
            return Some((
                parts[0].to_owned(),
                parts[1].trim_end_matches(".git").to_owned(),
                Some(parts[3].to_owned()),
                (parts.len() > 4).then(|| parts[4..].join("/")),
            ));
        }
        return Some((
            parts[0].to_owned(),
            parts[1].trim_end_matches(".git").to_owned(),
            None,
            None,
        ));
    }
    if raw.contains("://") || raw.contains('\\') {
        return None;
    }
    let (owner, rest) = raw.split_once('/')?;
    if !valid_github_segment(owner) {
        return None;
    }
    let (repo_ref, repo_path) = rest
        .split_once('/')
        .map_or((rest, None), |(head, tail)| (head, Some(tail.to_owned())));
    let (repo, reference) = repo_ref
        .split_once(':')
        .map_or((repo_ref, None), |(name, value)| {
            (name, (!value.is_empty()).then(|| value.to_owned()))
        });
    if !valid_github_segment(repo) {
        return None;
    }
    Some((
        owner.to_owned(),
        repo.to_owned(),
        reference,
        repo_path.filter(|path| !path.is_empty()),
    ))
}

fn valid_github_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
}

/// Derive the Python-compatible canonical source ID.
#[must_use]
pub fn derive_source_id(source: &SourceEntry) -> String {
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
    let ascii = ensure_ascii(&serialized);
    let digest = Sha256::digest(ascii.as_bytes());
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
/// # Errors
///
/// Returns an ambiguity error when multiple sources share the selected label.
pub fn find_source_index(config: &Config, selector: &str) -> Result<Option<usize>> {
    let selector_folded = fold(selector);
    let direct = config.sources.iter().position(|source| {
        fold(&source.id) == selector_folded
            || fold(&source.name) == selector_folded
            || source
                .path
                .as_ref()
                .is_some_and(|path| fold(&path.to_string_lossy()) == selector_folded)
            || fold(&source_reference(source)) == selector_folded
    });
    if direct.is_some() {
        return Ok(direct);
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
    let defaults = builtin_targets(home);
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

fn builtin_targets(home: &Path) -> IndexMap<String, Target> {
    IndexMap::from([
        (
            "claude".into(),
            Target {
                name: "claude".into(),
                label: "Claude Code".into(),
                path: home.join(".claude").join("skills"),
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
                path: home.join(".agents").join("skills"),
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
                path: home.join(".gemini").join("antigravity").join("skills"),
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
    use indexmap::IndexMap;
    use serde_json::json;

    use super::{
        BuiltinTargetSettings, Config, ConfigRepository, FileConfigRepository, derive_source_id,
        ensure_ascii, find_source_index, is_builtin_name, migrate_v0, resolved_targets,
        source_from_reference, source_reference, validate_source,
    };
    use crate::domain::{SourceEntry, SourceMode, SourceType, TargetEntry};

    #[test]
    fn source_id_is_stable() {
        let source = source_from_reference("owner/repo:main/team", None)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(source.id, derive_source_id(&source));
        assert!(source.id.starts_with("src_"));
        assert_eq!(source.id.len(), 16);
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
        assert_eq!(loaded.config.schema_version, 1);
        assert_eq!(loaded.config.sources.len(), 1);
        assert!(
            home.path()
                .join(".skill-manager.config.json.v0.bak")
                .exists()
        );
    }

    #[test]
    fn rejects_non_integer_and_future_schema_without_writing() {
        for schema in [
            serde_json::json!("1"),
            serde_json::json!(-1),
            serde_json::json!(0.5),
            serde_json::Value::Null,
            serde_json::json!(2),
        ] {
            let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
            let path = home.path().join(".skill-manager.config.json");
            let bytes = serde_json::to_vec(&serde_json::json!({ "schema_version": schema }))
                .unwrap_or_else(|error| unreachable!("{error}"));
            std::fs::write(&path, &bytes).unwrap_or_else(|error| unreachable!("{error}"));
            let repository = FileConfigRepository::new(home.path());
            assert!(repository.load(false).is_err());
            assert_eq!(
                std::fs::read(&path).unwrap_or_else(|error| unreachable!("{error}")),
                bytes
            );
            assert!(
                !home
                    .path()
                    .join(".skill-manager.config.json.v0.bak")
                    .exists()
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
        let saved = std::fs::read_to_string(path).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!saved.contains("\"disabled\""));
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
        assert_eq!(loaded.active_path, current);
        assert!(loaded.warning.is_none());
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
    fn migration_is_memory_only_in_dry_run_and_conflicting_backup_is_refused() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let current = home.path().join(".skill-manager.config.json");
        let original = br#"{"schema_version":0,"sources":[]}"#;
        std::fs::write(&current, original).unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let loaded = repository
            .load(true)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(loaded.config.schema_version, 1);
        assert_eq!(
            std::fs::read(&current).unwrap_or_else(|error| unreachable!("{error}")),
            original
        );
        let backup = home.path().join(".skill-manager.config.json.v0.bak");
        assert!(!backup.exists());

        std::fs::write(&backup, "different").unwrap_or_else(|error| unreachable!("{error}"));
        assert!(repository.load(false).is_err());
        assert_eq!(
            std::fs::read(&current).unwrap_or_else(|error| unreachable!("{error}")),
            original
        );
    }

    #[test]
    fn source_references_cover_urls_shorthand_local_identity_and_lookup() {
        let shorthand = source_from_reference("owner/repo:feature/team/skills", None)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(shorthand.source_type, SourceType::GitHub);
        assert_eq!(shorthand.r#ref.as_deref(), Some("feature"));
        assert_eq!(shorthand.repo_path.as_deref(), Some("team/skills"));
        assert_eq!(
            source_reference(&shorthand),
            "owner/repo:feature/team/skills"
        );

        let url = source_from_reference(
            "https://github.com/owner/repo.git/tree/main/nested/skills",
            None,
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(url.owner.as_deref(), Some("owner"));
        assert_eq!(url.repo.as_deref(), Some("repo"));
        assert_eq!(url.r#ref.as_deref(), Some("main"));
        assert_eq!(url.repo_path.as_deref(), Some("nested/skills"));

        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let local = source_from_reference(&root.path().to_string_lossy(), None)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(local.source_type, SourceType::Local);
        let config = Config {
            sources: vec![shorthand.clone(), local.clone()],
            ..Config::default()
        };
        for selector in [
            shorthand.id.as_str(),
            shorthand.name.as_str(),
            "OWNER/REPO:FEATURE/TEAM/SKILLS",
        ] {
            assert_eq!(
                find_source_index(&config, selector)
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
                    .to_string_lossy()
            )
            .unwrap_or_else(|error| unreachable!("{error}")),
            Some(1)
        );
        assert_eq!(
            find_source_index(&config, "missing").unwrap_or_else(|error| unreachable!("{error}")),
            None
        );

        assert_eq!(ensure_ascii("é😀"), "\\u00e9\\ud83d\\ude00");
        assert!(is_builtin_name("CLAUDE"));
        assert!(!is_builtin_name("custom"));
    }

    #[test]
    fn source_lookup_accepts_unique_labels_and_rejects_ambiguous_labels() {
        let mut first = source_from_reference("owner/first", None)
            .unwrap_or_else(|error| unreachable!("{error}"));
        first.label = "Team Skills".into();
        let mut second = source_from_reference("owner/second", None)
            .unwrap_or_else(|error| unreachable!("{error}"));
        second.label = "Other Skills".into();
        let mut config = Config {
            sources: vec![first, second],
            ..Config::default()
        };
        assert_eq!(
            find_source_index(&config, "TEAM SKILLS")
                .unwrap_or_else(|error| unreachable!("{error}")),
            Some(0)
        );
        config.sources[1].label = "team skills".into();
        assert!(find_source_index(&config, "Team Skills").is_err());
        assert_eq!(
            find_source_index(&config, "first").unwrap_or_else(|error| unreachable!("{error}")),
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
        let mut source = source_from_reference("owner/repo", None)
            .unwrap_or_else(|error| unreachable!("{error}"));
        source.cache_ttl_hours = Some(-1);
        negative_ttl.sources.push(source);
        assert!(repository.save(&active, &negative_ttl).is_err());

        let mut duplicate = Config::default();
        let one = source_from_reference("owner/one", None)
            .unwrap_or_else(|error| unreachable!("{error}"));
        let mut two = source_from_reference("owner/two", None)
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
        let migrated = migrate_v0(&value).unwrap_or_else(|error| unreachable!("{error}"));
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
        let error = migrate_v0(&value)
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
            assert!(migrate_v0(&value).is_err(), "{value}");
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
                std::fs::read(&path).unwrap_or_else(|error| unreachable!("{error}")),
                bytes
            );
            assert!(
                !home
                    .path()
                    .join(".skill-manager.config.json.v0.bak")
                    .exists()
            );
        }
    }
}
