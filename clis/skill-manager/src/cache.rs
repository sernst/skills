//! GitHub transport, bounded archive extraction, and persistent source caching.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize};
use tar::Archive;

use crate::config::{ConfigRepository, DEFAULT_CACHE_TTL_HOURS, acquire_lock, source_reference};
use crate::domain::{ResolvedSource, SourceEntry, SourceType};
use crate::error::{Result, SkillManagerError};

const MAX_COMPRESSED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const MAX_ARCHIVE_PATH_BYTES: usize = 4096;
const MAX_COMPONENT_UNITS: usize = 255;

/// Metadata persisted beside one remote source cache.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CacheMetadata {
    /// RFC3339 download completion time.
    pub fetched_at: DateTime<Utc>,
    /// Exact branch, tag, or commit downloaded.
    pub resolved_ref: String,
    /// Normalized GitHub owner associated with the cached content.
    pub owner: String,
    /// Normalized GitHub repository associated with the cached content.
    pub repo: String,
    /// Configured ref, or null when the default branch was requested.
    #[serde(rename = "ref")]
    pub source_ref: Option<String>,
    /// Configured repository subpath.
    pub repo_path: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CacheSwapState {
    Prepared,
    OldMoved,
    Committed,
}

#[derive(Debug, Deserialize, Serialize)]
struct CacheJournal {
    state: CacheSwapState,
    destination: PathBuf,
    backup: PathBuf,
    staging_root: PathBuf,
}

/// Time boundary used by cache freshness and metadata tests.
pub trait Clock {
    /// Return the current UTC instant.
    fn now(&self) -> DateTime<Utc>;
}

/// Production UTC clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// HTTP boundary used by GitHub materialization.
pub trait GitHubTransport {
    /// Resolve a repository default branch.
    ///
    /// # Errors
    ///
    /// Returns an error when transport or response validation fails.
    fn default_branch(&self, owner: &str, repo: &str) -> Result<String>;
    /// Download a repository tarball into `destination`.
    ///
    /// # Errors
    ///
    /// Returns an error on transport, size-limit, or filesystem failure.
    fn download_archive(
        &self,
        owner: &str,
        repo: &str,
        reference: &str,
        destination: &Path,
    ) -> Result<()>;
}

/// Blocking rustls GitHub transport with bounded retries and timeouts.
pub struct ReqwestGitHubTransport {
    client: Client,
    token: Option<String>,
    api_base: String,
    codeload_base: String,
}

impl ReqwestGitHubTransport {
    /// Create a production GitHub transport.
    ///
    /// # Errors
    ///
    /// Returns an error when the bounded HTTP client cannot be constructed.
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_mins(2))
            .user_agent(concat!("skill-manager/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| SkillManagerError::GitHub {
                reference: "github.com".into(),
                message: error.to_string(),
            })?;
        let token = std::env::var("GITHUB_TOKEN")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| {
                std::env::var("GH_TOKEN")
                    .ok()
                    .filter(|value| !value.is_empty())
            });
        Ok(Self {
            client,
            token,
            api_base: "https://api.github.com".into(),
            codeload_base: "https://codeload.github.com".into(),
        })
    }

    fn request(&self, url: &str) -> reqwest::blocking::RequestBuilder {
        let request = self
            .client
            .get(url)
            .header("Accept", "application/vnd.github+json");
        if let Some(token) = &self.token {
            request.header("Authorization", format!("Bearer {token}"))
        } else {
            request
        }
    }

    fn send_with_retry(&self, url: &str) -> std::result::Result<Response, reqwest::Error> {
        let mut attempt = 0_u8;
        loop {
            attempt += 1;
            match self.request(url).send() {
                Ok(response)
                    if response.status().is_success()
                        || !is_transient_status(response.status())
                        || attempt >= 3 =>
                {
                    return Ok(response);
                }
                Ok(_response) if attempt < 3 => thread::sleep(Duration::from_millis(
                    100_u64.saturating_mul(1_u64 << attempt),
                )),
                Err(error) if attempt >= 3 || !error.is_timeout() && !error.is_connect() => {
                    return Err(error);
                }
                Err(_error) => thread::sleep(Duration::from_millis(
                    100_u64.saturating_mul(1_u64 << attempt),
                )),
                Ok(response) => return Ok(response),
            }
        }
    }
}

impl GitHubTransport for ReqwestGitHubTransport {
    fn default_branch(&self, owner: &str, repo: &str) -> Result<String> {
        let reference = format!("{owner}/{repo}");
        let url = format!("{}/repos/{owner}/{repo}", self.api_base);
        let response = self
            .send_with_retry(&url)
            .and_then(Response::error_for_status)
            .map_err(|error| SkillManagerError::GitHub {
                reference: reference.clone(),
                message: error.to_string(),
            })?;
        let value: serde_json::Value =
            response.json().map_err(|error| SkillManagerError::GitHub {
                reference,
                message: error.to_string(),
            })?;
        value
            .get("default_branch")
            .and_then(serde_json::Value::as_str)
            .filter(|branch| !branch.is_empty())
            .map(ToOwned::to_owned)
            .ok_or_else(|| SkillManagerError::GitHub {
                reference: format!("{owner}/{repo}"),
                message: "GitHub response omitted default_branch".into(),
            })
    }

    fn download_archive(
        &self,
        owner: &str,
        repo: &str,
        reference: &str,
        destination: &Path,
    ) -> Result<()> {
        let source = format!("{owner}/{repo}:{reference}");
        let encoded_ref: String =
            url::form_urlencoded::byte_serialize(reference.as_bytes()).collect();
        let url = format!("{}/{owner}/{repo}/tar.gz/{encoded_ref}", self.codeload_base);
        let mut response = self
            .send_with_retry(&url)
            .and_then(Response::error_for_status)
            .map_err(|error| SkillManagerError::GitHub {
                reference: source.clone(),
                message: error.to_string(),
            })?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_COMPRESSED_BYTES)
        {
            return Err(SkillManagerError::GitHub {
                reference: source,
                message: format!("archive exceeds {MAX_COMPRESSED_BYTES} compressed bytes"),
            });
        }
        let mut output =
            File::create(destination).map_err(|error| SkillManagerError::io(destination, error))?;
        let copied = std::io::copy(
            &mut response.by_ref().take(MAX_COMPRESSED_BYTES + 1),
            &mut output,
        )
        .map_err(|error| SkillManagerError::GitHub {
            reference: source.clone(),
            message: error.to_string(),
        })?;
        if copied > MAX_COMPRESSED_BYTES {
            return Err(SkillManagerError::GitHub {
                reference: source,
                message: format!("archive exceeds {MAX_COMPRESSED_BYTES} compressed bytes"),
            });
        }
        output
            .sync_all()
            .map_err(|error| SkillManagerError::io(destination, error))
    }
}

fn is_transient_status(status: StatusCode) -> bool {
    status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
}

/// Resolve a local or GitHub source into a usable local root.
///
/// # Errors
///
/// Returns an error for invalid source data, failed transport, unsafe archives,
/// lock contention, or cache filesystem failures.
pub fn materialize_source<R: ConfigRepository, G: GitHubTransport>(
    repository: &R,
    github: &G,
    source: &SourceEntry,
    refresh: bool,
    dry_run: bool,
) -> Result<ResolvedSource> {
    materialize_source_with_clock(repository, github, &SystemClock, source, refresh, dry_run)
}

/// Resolve a source using an injected clock.
///
/// # Errors
///
/// Returns the same validation, transport, archive, lock, and filesystem
/// failures as [`materialize_source`].
pub fn materialize_source_with_clock<R: ConfigRepository, G: GitHubTransport, C: Clock>(
    repository: &R,
    github: &G,
    clock: &C,
    source: &SourceEntry,
    refresh: bool,
    dry_run: bool,
) -> Result<ResolvedSource> {
    match source.source_type {
        SourceType::Local => Ok(ResolvedSource {
            entry: source.clone(),
            path: source.path.clone().ok_or_else(|| {
                SkillManagerError::InvalidInput(format!(
                    "local source '{}' has no path",
                    source.name
                ))
            })?,
            from_cache: false,
            temporary: None,
        }),
        SourceType::GitHub => {
            materialize_github(repository, github, clock, source, refresh, dry_run)
        }
    }
}

fn materialize_github<R: ConfigRepository, G: GitHubTransport, C: Clock>(
    repository: &R,
    github: &G,
    clock: &C,
    source: &SourceEntry,
    refresh: bool,
    dry_run: bool,
) -> Result<ResolvedSource> {
    let source_ref = source_reference(source);
    let owner = source.owner.as_deref().ok_or_else(|| {
        SkillManagerError::InvalidInput(format!("GitHub source {source_ref} has no owner"))
    })?;
    let repo = source.repo.as_deref().ok_or_else(|| {
        SkillManagerError::InvalidInput(format!("GitHub source {source_ref} has no repository"))
    })?;
    let source_cache = repository.cache_root().join(&source.id);
    let content = source_cache.join("content");
    let metadata_path = source_cache.join("metadata.json");
    let ttl = source.cache_ttl_hours.unwrap_or(DEFAULT_CACHE_TTL_HOURS);
    if ttl < 0 {
        return Err(SkillManagerError::InvalidInput(format!(
            "source {source_ref} has negative cache TTL"
        )));
    }
    let swap_paths = cache_swap_paths(&source_cache)?;
    if dry_run {
        if !swap_paths.journal.exists()
            && cache_can_be_reused(clock, &content, &metadata_path, source, ttl, refresh)
        {
            return resolved_cached(source, &content);
        }
        let temporary = Arc::new(
            tempfile::tempdir().map_err(|error| SkillManagerError::io("<temporary>", error))?,
        );
        let reference = source
            .r#ref
            .clone()
            .map_or_else(|| github.default_branch(owner, repo), Ok)?;
        let archive = temporary.path().join("source.tar.gz");
        let extracted = temporary.path().join("content");
        github.download_archive(owner, repo, &reference, &archive)?;
        extract_archive(
            &archive,
            &extracted,
            source.repo_path.as_deref(),
            &source_ref,
        )?;
        let selected = select_repo_path(&extracted, source)?;
        return Ok(ResolvedSource {
            entry: source.clone(),
            path: selected,
            from_cache: false,
            temporary: Some(temporary),
        });
    }

    let lock_path = repository
        .cache_root()
        .join(".locks")
        .join(format!("source-{}.lock", source.id));
    let _lock = acquire_lock(&lock_path, &source_ref, Duration::from_secs(10))?;
    recover_cache_swap(&source_cache, &swap_paths.backup, &swap_paths.journal)?;
    // Recovery runs before reuse, and another process may have refreshed while
    // this process waited for the source lock.
    if cache_can_be_reused(clock, &content, &metadata_path, source, ttl, refresh) {
        return resolved_cached(source, &content);
    }
    fs::create_dir_all(repository.cache_root())
        .map_err(|error| SkillManagerError::io(repository.cache_root(), error))?;
    let staging = tempfile::Builder::new()
        .prefix(&format!(".{}.stage-", source.id))
        .tempdir_in(repository.cache_root())
        .map_err(|error| SkillManagerError::io(repository.cache_root(), error))?;
    let reference = source
        .r#ref
        .clone()
        .map_or_else(|| github.default_branch(owner, repo), Ok)?;
    let archive = staging.path().join("source.tar.gz");
    let new_content = staging.path().join("content");
    github.download_archive(owner, repo, &reference, &archive)?;
    extract_archive(
        &archive,
        &new_content,
        source.repo_path.as_deref(),
        &source_ref,
    )?;
    let selected = select_repo_path(&new_content, source)?;
    if !selected.exists() {
        return Err(SkillManagerError::GitHub {
            reference: source_ref,
            message: "configured repository path does not exist".into(),
        });
    }
    let staged_cache = staging.path().join("cache");
    fs::create_dir(&staged_cache).map_err(|error| SkillManagerError::io(&staged_cache, error))?;
    fs::rename(&new_content, staged_cache.join("content"))
        .map_err(|error| SkillManagerError::io(&new_content, error))?;
    write_metadata(
        &staged_cache.join("metadata.json"),
        &CacheMetadata {
            fetched_at: clock.now(),
            resolved_ref: reference,
            owner: owner.to_ascii_lowercase(),
            repo: repo.to_ascii_lowercase(),
            source_ref: source.r#ref.clone(),
            repo_path: source.repo_path.clone(),
        },
    )?;
    swap_cache(&source_cache, &staged_cache, staging.path())?;
    resolved_cached(source, &content)
}

fn cache_can_be_reused<C: Clock>(
    clock: &C,
    content: &Path,
    metadata_path: &Path,
    source: &SourceEntry,
    ttl: i64,
    refresh: bool,
) -> bool {
    !refresh
        && ttl > 0
        && content.is_dir()
        && read_metadata(metadata_path).as_ref().is_some_and(|value| {
            cache_is_fresh(clock, value, ttl) && cache_identity_matches(value, source)
        })
}

fn cache_identity_matches(metadata: &CacheMetadata, source: &SourceEntry) -> bool {
    metadata.owner
        == source
            .owner
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
        && metadata.repo
            == source
                .repo
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase()
        && metadata.source_ref == source.r#ref
        && metadata.repo_path == source.repo_path
}

fn cache_is_fresh<C: Clock>(clock: &C, metadata: &CacheMetadata, ttl: i64) -> bool {
    clock
        .now()
        .signed_duration_since(metadata.fetched_at)
        .num_seconds()
        < ttl.saturating_mul(3600)
}

fn resolved_cached(source: &SourceEntry, content: &Path) -> Result<ResolvedSource> {
    let path = select_repo_path(content, source)?;
    if !path.exists() {
        return Err(SkillManagerError::GitHub {
            reference: source_reference(source),
            message: format!("repository path does not exist: {}", path.display()),
        });
    }
    Ok(ResolvedSource {
        entry: source.clone(),
        path,
        from_cache: true,
        temporary: None,
    })
}

fn select_repo_path(content: &Path, source: &SourceEntry) -> Result<PathBuf> {
    let Some(repo_path) = source.repo_path.as_deref() else {
        return Ok(content.to_path_buf());
    };
    let relative = validate_relative_path(Path::new(repo_path), &source_reference(source))?;
    Ok(content.join(relative))
}

fn read_metadata(path: &Path) -> Option<CacheMetadata> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_metadata(path: &Path, metadata: &CacheMetadata) -> Result<()> {
    let mut data = serde_json::to_vec_pretty(metadata)
        .map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
    data.push(b'\n');
    fs::write(path, data).map_err(|error| SkillManagerError::io(path, error))
}

struct CacheSwapPaths {
    backup: PathBuf,
    journal: PathBuf,
}

fn cache_swap_paths(destination: &Path) -> Result<CacheSwapPaths> {
    let parent = destination.parent().ok_or_else(|| {
        SkillManagerError::InvalidInput(format!(
            "cache destination has no parent: {}",
            destination.display()
        ))
    })?;
    let backup = parent.join(format!(
        ".{}.backup",
        destination
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("source")
    ));
    let journal = parent.join(format!(
        ".{}.journal.json",
        destination
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("source")
    ));
    Ok(CacheSwapPaths { backup, journal })
}

fn swap_cache(destination: &Path, staged: &Path, staging_root: &Path) -> Result<()> {
    let paths = cache_swap_paths(destination)?;
    recover_cache_swap(destination, &paths.backup, &paths.journal)?;
    let mut journal = CacheJournal {
        state: CacheSwapState::Prepared,
        destination: destination.to_path_buf(),
        backup: paths.backup.clone(),
        staging_root: staging_root.to_path_buf(),
    };
    write_cache_journal(&paths.journal, &journal)?;
    if destination.exists() {
        fs::rename(destination, &paths.backup)
            .map_err(|error| SkillManagerError::io(destination, error))?;
        journal.state = CacheSwapState::OldMoved;
        write_cache_journal(&paths.journal, &journal)?;
    }
    if let Err(error) = fs::rename(staged, destination) {
        if paths.backup.exists() && !destination.exists() {
            let _rollback = fs::rename(&paths.backup, destination);
        }
        let _recovery = recover_cache_swap(destination, &paths.backup, &paths.journal);
        return Err(SkillManagerError::io(staged, error));
    }
    journal.state = CacheSwapState::Committed;
    write_cache_journal(&paths.journal, &journal)?;
    if paths.backup.exists() {
        fs::remove_dir_all(&paths.backup)
            .map_err(|error| SkillManagerError::io(&paths.backup, error))?;
    }
    cleanup_cache_staging(staging_root, destination)?;
    fs::remove_file(&paths.journal).map_err(|error| SkillManagerError::io(&paths.journal, error))
}

fn write_cache_journal(path: &Path, journal: &CacheJournal) -> Result<()> {
    let mut data = serde_json::to_vec(journal)
        .map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
    data.push(b'\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|error| SkillManagerError::io(path, error))?;
    file.write_all(&data)
        .and_then(|()| file.sync_all())
        .map_err(|error| SkillManagerError::io(path, error))
}

fn recover_cache_swap(destination: &Path, backup: &Path, journal: &Path) -> Result<()> {
    if !journal.exists() {
        if backup.exists() {
            fs::remove_dir_all(backup).map_err(|error| SkillManagerError::io(backup, error))?;
        }
        return Ok(());
    }
    let data = fs::read(journal).map_err(|error| SkillManagerError::io(journal, error))?;
    let record: CacheJournal = serde_json::from_slice(&data).map_err(|error| {
        SkillManagerError::InvalidInput(format!(
            "cache journal {} is invalid: {error}",
            journal.display()
        ))
    })?;
    if record.destination != destination || record.backup != backup {
        return Err(SkillManagerError::InvalidInput(format!(
            "cache journal {} names unexpected transaction paths",
            journal.display()
        )));
    }
    if !matches!(record.state, CacheSwapState::Committed)
        && backup.exists()
        && !destination.exists()
    {
        fs::rename(backup, destination).map_err(|error| SkillManagerError::io(backup, error))?;
    } else if backup.exists() {
        fs::remove_dir_all(backup).map_err(|error| SkillManagerError::io(backup, error))?;
    }
    cleanup_cache_staging(&record.staging_root, destination)?;
    fs::remove_file(journal).map_err(|error| SkillManagerError::io(journal, error))
}

fn cleanup_cache_staging(staging_root: &Path, destination: &Path) -> Result<()> {
    if !staging_root.exists() {
        return Ok(());
    }
    let parent = destination.parent().ok_or_else(|| {
        SkillManagerError::InvalidInput(format!(
            "cache destination has no parent: {}",
            destination.display()
        ))
    })?;
    let source_name = destination
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("source");
    let expected_prefix = format!(".{source_name}.stage-");
    let safe = staging_root.parent() == Some(parent)
        && staging_root
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| name.starts_with(&expected_prefix));
    if !safe {
        return Err(SkillManagerError::InvalidInput(format!(
            "cache journal names an unsafe staging path: {}",
            staging_root.display()
        )));
    }
    fs::remove_dir_all(staging_root).map_err(|error| SkillManagerError::io(staging_root, error))
}

fn extract_archive(
    archive_path: &Path,
    destination: &Path,
    repo_path: Option<&str>,
    source: &str,
) -> Result<()> {
    validate_raw_archive(archive_path, source)?;
    fs::create_dir_all(destination).map_err(|error| SkillManagerError::io(destination, error))?;
    let selected_path = repo_path
        .map(|path| validate_relative_path(Path::new(path), source))
        .transpose()?;
    let archive_file =
        File::open(archive_path).map_err(|error| SkillManagerError::io(archive_path, error))?;
    let decoder = GzDecoder::new(archive_file);
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| SkillManagerError::GitHub {
            reference: source.to_owned(),
            message: error.to_string(),
        })?;
    for entry_result in entries {
        let mut entry = entry_result.map_err(|error| SkillManagerError::GitHub {
            reference: source.to_owned(),
            message: error.to_string(),
        })?;
        let entry_type = entry.header().entry_type();
        let relative = archive_entry_relative_path(&entry, source)?;
        if entry_type.is_pax_global_extensions() {
            continue;
        }
        if relative.is_none() && !entry_type.is_file() && !entry_type.is_dir() {
            return archive_error(source, "archive root is a link or special entry");
        }
        let Some(relative) = relative else {
            continue;
        };
        let is_selected = selected_path.as_ref().is_none_or(|selected| {
            relative.starts_with(selected) || selected.starts_with(&relative)
        });
        if !entry_type.is_file() && !entry_type.is_dir() {
            if is_selected {
                return archive_error(
                    source,
                    format!(
                        "archive contains a link or special entry in the selected source path: {}",
                        relative.display()
                    ),
                );
            }
            continue;
        }
        if !is_selected {
            continue;
        }
        let output = destination.join(relative);
        if entry_type.is_dir() {
            fs::create_dir_all(&output).map_err(|error| SkillManagerError::io(&output, error))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|error| SkillManagerError::io(parent, error))?;
        }
        let mut file =
            File::create(&output).map_err(|error| SkillManagerError::io(&output, error))?;
        let copied = std::io::copy(&mut entry.by_ref().take(MAX_FILE_BYTES + 1), &mut file)
            .map_err(|error| SkillManagerError::GitHub {
                reference: source.to_owned(),
                message: error.to_string(),
            })?;
        if copied > MAX_FILE_BYTES {
            return archive_error(source, "archive file exceeds per-file limit");
        }
        preserve_executable_permission(&entry, &output)?;
    }
    Ok(())
}

fn validate_raw_archive(archive_path: &Path, source: &str) -> Result<()> {
    let archive_file =
        File::open(archive_path).map_err(|error| SkillManagerError::io(archive_path, error))?;
    let decoder = GzDecoder::new(archive_file);
    let mut archive = Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| SkillManagerError::GitHub {
            reference: source.to_owned(),
            message: error.to_string(),
        })?
        .raw(true);
    let mut count = 0_usize;
    let mut expanded = 0_u64;
    for entry_result in entries {
        count += 1;
        if count > MAX_ENTRIES {
            return archive_error(source, format!("archive exceeds {MAX_ENTRIES} entries"));
        }
        let entry = entry_result.map_err(|error| SkillManagerError::GitHub {
            reference: source.to_owned(),
            message: error.to_string(),
        })?;
        validate_raw_archive_entry_path(&entry, source)?;
        account_expanded_size(&entry, &mut expanded, source)?;
    }
    Ok(())
}

fn validate_raw_archive_entry_path<R: Read>(entry: &tar::Entry<'_, R>, source: &str) -> Result<()> {
    let entry_type = entry.header().entry_type();
    let raw_path = entry.path().map_err(|error| SkillManagerError::GitHub {
        reference: source.to_owned(),
        message: error.to_string(),
    })?;
    if (entry_type.is_gnu_longname() || entry_type.is_gnu_longlink())
        && raw_path == Path::new("././@LongLink")
    {
        return Ok(());
    }
    validate_archive_path(&raw_path, source).map(|_| ())
}

fn archive_entry_relative_path<R: Read>(
    entry: &tar::Entry<'_, R>,
    source: &str,
) -> Result<Option<PathBuf>> {
    let validated_raw = validated_archive_entry_path(entry, source)?;
    let stripped: PathBuf = validated_raw.components().skip(1).collect();
    if stripped.as_os_str().is_empty() {
        Ok(None)
    } else {
        validate_relative_path(&stripped, source).map(Some)
    }
}

fn validated_archive_entry_path<R: Read>(
    entry: &tar::Entry<'_, R>,
    source: &str,
) -> Result<PathBuf> {
    let raw_path = entry.path().map_err(|error| SkillManagerError::GitHub {
        reference: source.to_owned(),
        message: error.to_string(),
    })?;
    validate_archive_path(&raw_path, source)
}

fn validate_archive_path(path: &Path, source: &str) -> Result<PathBuf> {
    if path.as_os_str().to_string_lossy().len() > MAX_ARCHIVE_PATH_BYTES {
        return archive_error(source, "archive path exceeds portable limit");
    }
    validate_relative_path(path, source)
}

fn account_expanded_size<R: Read>(
    entry: &tar::Entry<'_, R>,
    expanded: &mut u64,
    source: &str,
) -> Result<()> {
    let size = entry
        .header()
        .size()
        .map_err(|error| SkillManagerError::GitHub {
            reference: source.to_owned(),
            message: error.to_string(),
        })?;
    if entry.header().entry_type().is_file() && size > MAX_FILE_BYTES {
        return archive_error(
            source,
            format!("archive file exceeds {MAX_FILE_BYTES} bytes"),
        );
    }
    *expanded = expanded.saturating_add(size);
    if *expanded > MAX_EXPANDED_BYTES {
        return archive_error(
            source,
            format!("archive exceeds {MAX_EXPANDED_BYTES} expanded bytes"),
        );
    }
    Ok(())
}

fn validate_relative_path(path: &Path, source: &str) -> Result<PathBuf> {
    let mut validated = PathBuf::new();
    for component in path.components() {
        let Component::Normal(name) = component else {
            return archive_error(source, "path contains traversal or an absolute component");
        };
        if name.to_string_lossy().len() > MAX_COMPONENT_UNITS {
            return archive_error(source, "path component exceeds portable byte limit");
        }
        let units = name.to_string_lossy().encode_utf16().count();
        if units > MAX_COMPONENT_UNITS {
            return archive_error(source, "path component exceeds portable limit");
        }
        validated.push(name);
    }
    Ok(validated)
}

#[cfg(unix)]
fn preserve_executable_permission<R: Read>(entry: &tar::Entry<'_, R>, output: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
    fs::set_permissions(output, fs::Permissions::from_mode(mode))
        .map_err(|error| SkillManagerError::io(output, error))
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn preserve_executable_permission<R: Read>(
    _entry: &tar::Entry<'_, R>,
    _output: &Path,
) -> Result<()> {
    Ok(())
}

fn archive_error<T>(source: &str, message: impl Into<String>) -> Result<T> {
    Err(SkillManagerError::GitHub {
        reference: source.to_owned(),
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::path::Path;
    use std::thread;
    use std::time::Duration;

    use chrono::{DateTime, TimeZone, Utc};
    use reqwest::StatusCode;

    use super::{
        CacheJournal, CacheMetadata, CacheSwapState, Clock, GitHubTransport, MAX_COMPRESSED_BYTES,
        ReqwestGitHubTransport, cache_is_fresh, cache_swap_paths, is_transient_status,
        read_metadata, recover_cache_swap, resolved_cached, select_repo_path, swap_cache,
        validate_relative_path, write_cache_journal, write_metadata,
    };
    use crate::config::source_from_reference;

    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    struct MockResponse {
        status: &'static str,
        body: &'static str,
        content_length: Option<u64>,
    }

    fn mock_server(responses: Vec<MockResponse>) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener =
            TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| unreachable!("{error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| unreachable!("{error}"));
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) = listener
                    .accept()
                    .unwrap_or_else(|error| unreachable!("{error}"));
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap_or_else(|error| unreachable!("{error}"));
                let mut bytes = Vec::new();
                let mut buffer = [0_u8; 1024];
                loop {
                    let count = stream
                        .read(&mut buffer)
                        .unwrap_or_else(|error| unreachable!("{error}"));
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..count]);
                    if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                requests.push(String::from_utf8_lossy(&bytes).into_owned());
                let length = response
                    .content_length
                    .unwrap_or(response.body.len() as u64);
                let wire = format!(
                    "HTTP/1.1 {}\r\nContent-Length: {length}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                    response.status, response.body
                );
                stream
                    .write_all(wire.as_bytes())
                    .unwrap_or_else(|error| unreachable!("{error}"));
            }
            requests
        });
        (format!("http://{address}"), handle)
    }

    fn test_transport(base: &str, token: Option<&str>) -> ReqwestGitHubTransport {
        ReqwestGitHubTransport {
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap_or_else(|error| unreachable!("{error}")),
            token: token.map(ToOwned::to_owned),
            api_base: base.into(),
            codeload_base: base.into(),
        }
    }

    #[test]
    fn raw_archive_paths_reject_traversal_and_absolute_roots() {
        assert!(validate_relative_path(Path::new("../repo-root/skill"), "fixture").is_err());
        assert!(validate_relative_path(Path::new("repo-root/../../outside"), "fixture").is_err());
        assert!(validate_relative_path(Path::new("/repo-root/skill"), "fixture").is_err());
    }

    #[test]
    fn cache_journal_restores_backup_and_cleans_staging() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let destination = root.path().join("src_example");
        let paths = cache_swap_paths(&destination).unwrap_or_else(|error| unreachable!("{error}"));
        fs::create_dir(&paths.backup).unwrap_or_else(|error| unreachable!("{error}"));
        fs::write(paths.backup.join("old"), "old").unwrap_or_else(|error| unreachable!("{error}"));
        let staging = root.path().join(".src_example.stage-interrupted");
        fs::create_dir(&staging).unwrap_or_else(|error| unreachable!("{error}"));
        write_cache_journal(
            &paths.journal,
            &CacheJournal {
                state: CacheSwapState::OldMoved,
                destination: destination.clone(),
                backup: paths.backup.clone(),
                staging_root: staging.clone(),
            },
        )
        .unwrap_or_else(|error| unreachable!("{error}"));

        recover_cache_swap(&destination, &paths.backup, &paths.journal)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(destination.join("old").is_file());
        assert!(!staging.exists());
        assert!(!paths.journal.exists());
    }

    #[test]
    fn freshness_metadata_and_status_classification_are_strict() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let path = root.path().join("metadata.json");
        let fetched_at = Utc
            .with_ymd_and_hms(2026, 7, 25, 12, 0, 0)
            .single()
            .unwrap_or_else(|| unreachable!("valid timestamp"));
        let metadata = CacheMetadata {
            fetched_at,
            resolved_ref: "abc123".into(),
            owner: "owner".into(),
            repo: "repo".into(),
            source_ref: None,
            repo_path: None,
        };
        write_metadata(&path, &metadata).unwrap_or_else(|error| unreachable!("{error}"));
        let decoded = read_metadata(&path).unwrap_or_else(|| unreachable!("valid metadata"));
        assert_eq!(decoded.resolved_ref, "abc123");
        assert!(cache_is_fresh(
            &FixedClock(fetched_at + chrono::Duration::minutes(59)),
            &decoded,
            1
        ));
        assert!(!cache_is_fresh(
            &FixedClock(fetched_at + chrono::Duration::hours(1)),
            &decoded,
            1
        ));
        fs::write(&path, "{broken").unwrap_or_else(|error| unreachable!("{error}"));
        assert!(read_metadata(&path).is_none());
        assert!(read_metadata(&root.path().join("missing")).is_none());

        for status in [
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert!(is_transient_status(status));
        }
        assert!(!is_transient_status(StatusCode::NOT_FOUND));
    }

    #[test]
    fn repo_subpaths_are_validated_and_cached_paths_must_exist() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let content = root.path().join("content");
        fs::create_dir_all(content.join("team")).unwrap_or_else(|error| unreachable!("{error}"));
        let mut source = source_from_reference("owner/repo:main/team", None, root.path())
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            select_repo_path(&content, &source).unwrap_or_else(|error| unreachable!("{error}")),
            content.join("team")
        );
        let resolved =
            resolved_cached(&source, &content).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(resolved.from_cache);
        assert_eq!(resolved.path, content.join("team"));

        source.repo_path = Some("../outside".into());
        assert!(select_repo_path(&content, &source).is_err());
        source.repo_path = Some("missing".into());
        assert!(resolved_cached(&source, &content).is_err());
    }

    #[test]
    fn cache_swap_covers_new_replacement_and_committed_recovery() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let destination = root.path().join("src_example");
        let staging_root = root.path().join(".src_example.stage-first");
        let staged = staging_root.join("cache");
        fs::create_dir_all(&staged).unwrap_or_else(|error| unreachable!("{error}"));
        fs::write(staged.join("new"), "one").unwrap_or_else(|error| unreachable!("{error}"));
        swap_cache(&destination, &staged, &staging_root)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            fs::read_to_string(destination.join("new"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            "one"
        );

        let replacement_root = root.path().join(".src_example.stage-second");
        let replacement = replacement_root.join("cache");
        fs::create_dir_all(&replacement).unwrap_or_else(|error| unreachable!("{error}"));
        fs::write(replacement.join("new"), "two").unwrap_or_else(|error| unreachable!("{error}"));
        swap_cache(&destination, &replacement, &replacement_root)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            fs::read_to_string(destination.join("new"))
                .unwrap_or_else(|error| unreachable!("{error}")),
            "two"
        );

        let paths = cache_swap_paths(&destination).unwrap_or_else(|error| unreachable!("{error}"));
        fs::create_dir(&paths.backup).unwrap_or_else(|error| unreachable!("{error}"));
        let abandoned = root.path().join(".src_example.stage-committed");
        fs::create_dir(&abandoned).unwrap_or_else(|error| unreachable!("{error}"));
        write_cache_journal(
            &paths.journal,
            &CacheJournal {
                state: CacheSwapState::Committed,
                destination: destination.clone(),
                backup: paths.backup.clone(),
                staging_root: abandoned.clone(),
            },
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        recover_cache_swap(&destination, &paths.backup, &paths.journal)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!paths.backup.exists());
        assert!(!abandoned.exists());
    }

    #[test]
    fn recovery_rejects_corrupt_paths_and_cleans_orphan_backup_without_journal() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let destination = root.path().join("src_example");
        let paths = cache_swap_paths(&destination).unwrap_or_else(|error| unreachable!("{error}"));
        fs::create_dir(&paths.backup).unwrap_or_else(|error| unreachable!("{error}"));
        recover_cache_swap(&destination, &paths.backup, &paths.journal)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!paths.backup.exists());

        fs::write(&paths.journal, "{broken").unwrap_or_else(|error| unreachable!("{error}"));
        assert!(recover_cache_swap(&destination, &paths.backup, &paths.journal).is_err());

        let unsafe_staging = root.path().join("unrelated");
        fs::create_dir(&unsafe_staging).unwrap_or_else(|error| unreachable!("{error}"));
        write_cache_journal(
            &paths.journal,
            &CacheJournal {
                state: CacheSwapState::Prepared,
                destination: destination.clone(),
                backup: paths.backup.clone(),
                staging_root: unsafe_staging,
            },
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(recover_cache_swap(&destination, &paths.backup, &paths.journal).is_err());

        let safe_staging = root.path().join(".src_example.stage-mismatch");
        fs::create_dir(&safe_staging).unwrap_or_else(|error| unreachable!("{error}"));
        write_cache_journal(
            &paths.journal,
            &CacheJournal {
                state: CacheSwapState::Prepared,
                destination: root.path().join("other"),
                backup: paths.backup.clone(),
                staging_root: safe_staging,
            },
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(recover_cache_swap(&destination, &paths.backup, &paths.journal).is_err());
    }

    #[test]
    fn reqwest_transport_retries_transient_statuses_and_sends_authentication() {
        let (base, handle) = mock_server(vec![
            MockResponse {
                status: "500 Internal Server Error",
                body: "{}",
                content_length: None,
            },
            MockResponse {
                status: "429 Too Many Requests",
                body: "{}",
                content_length: None,
            },
            MockResponse {
                status: "200 OK",
                body: r#"{"default_branch":"main"}"#,
                content_length: None,
            },
        ]);
        let transport = test_transport(&base, Some("secret-token"));
        assert_eq!(
            transport
                .default_branch("owner", "repo")
                .unwrap_or_else(|error| unreachable!("{error}")),
            "main"
        );
        let requests = handle.join().unwrap_or_else(|_| unreachable!("server"));
        assert_eq!(requests.len(), 3);
        assert!(requests.iter().all(|request| {
            request.contains("GET /repos/owner/repo HTTP/1.1")
                && request.contains("authorization: Bearer secret-token")
        }));
    }

    #[test]
    fn reqwest_transport_downloads_encoded_refs_and_validates_responses() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let archive = root.path().join("archive.tar.gz");
        let (base, handle) = mock_server(vec![MockResponse {
            status: "200 OK",
            body: "archive-content",
            content_length: None,
        }]);
        test_transport(&base, None)
            .download_archive("owner", "repo", "feature/one", &archive)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            fs::read_to_string(&archive).unwrap_or_else(|error| unreachable!("{error}")),
            "archive-content"
        );
        let requests = handle.join().unwrap_or_else(|_| unreachable!("server"));
        assert!(requests[0].contains("GET /owner/repo/tar.gz/feature%2Fone HTTP/1.1"));
        assert!(!requests[0].contains("authorization:"));

        for response in [
            MockResponse {
                status: "404 Not Found",
                body: "{}",
                content_length: None,
            },
            MockResponse {
                status: "200 OK",
                body: "{}",
                content_length: None,
            },
        ] {
            let (base, handle) = mock_server(vec![response]);
            assert!(
                test_transport(&base, None)
                    .default_branch("owner", "repo")
                    .is_err()
            );
            let _requests = handle.join().unwrap_or_else(|_| unreachable!("server"));
        }

        let (base, handle) = mock_server(vec![MockResponse {
            status: "200 OK",
            body: "",
            content_length: Some(MAX_COMPRESSED_BYTES + 1),
        }]);
        assert!(
            test_transport(&base, None)
                .download_archive("owner", "repo", "main", &archive)
                .is_err()
        );
        let _requests = handle.join().unwrap_or_else(|_| unreachable!("server"));

        ReqwestGitHubTransport::new().unwrap_or_else(|error| unreachable!("{error}"));
    }
}
