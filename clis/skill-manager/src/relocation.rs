//! A single durable batch boundary for local source copying and location changes.
//!
//! Lock order is configuration, then sorted destination target locks. This module
//! is invoked only after authorization; planning and cancellation perform no writes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{
    Config, ConfigRepository, ConfigWriteSession, acquire_lock, configuration_image, fold,
};
use crate::domain::{ResolvedSource, SourceType};
use crate::error::{Result, SkillManagerError};
use crate::fs_retry as fs;
use crate::skills::{detect_skill_dirs, skill_name, validate_skill_tree};
use crate::staging::{exists, reject_link, remove_tree};
use crate::transaction::{TransactionOutcome, copy_tree};

type TreeImage = BTreeMap<PathBuf, Option<String>>;

/// Reviewed physical skill mapping, independent of load/update exclusions.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RelocationCopy {
    /// Physical source skill.
    pub source: PathBuf,
    /// Exact destination, including direct single-skill mapping.
    pub destination: PathBuf,
    source_image: TreeImage,
    before: Option<TreeImage>,
}

impl RelocationCopy {
    /// Whether this selected skill would replace an existing complete directory.
    #[must_use]
    pub fn existed(&self) -> bool {
        self.before.is_some()
    }
    /// Whether the complete physical tree, including empty directories, is equal.
    #[must_use]
    pub fn unchanged(&self) -> bool {
        self.before.as_ref() == Some(&self.source_image)
    }
}

/// Enumerate every physical skill, independent of source/global exclusions.
/// # Errors
/// Returns an error for unavailable trees, links, or duplicate folded identities.
pub fn physical_names(source: &ResolvedSource) -> Result<Vec<String>> {
    safe_path(&source.path)?;
    let mut identities = BTreeSet::new();
    detect_skill_dirs(source)?
        .iter()
        .map(|path| {
            let name = skill_name(path)?;
            if !identities.insert(fold(&name)) {
                return Err(invalid("duplicate physical skill identities"));
            }
            Ok(name)
        })
        .collect()
}

/// A physical skill candidate whose destination has only been inspected shallowly.
#[derive(Clone, Debug)]
pub struct RelocationCandidate {
    /// Physical source tree.
    pub source: PathBuf,
    /// Exact eventual destination.
    pub destination: PathBuf,
    /// Whether any entry currently occupies that destination.
    pub existed: bool,
}

/// Preview candidates without traversing unselected destination entries.
/// # Errors
/// Rejects unsafe roots and duplicate source identities before selection.
pub fn candidates(source: &ResolvedSource, destination: &Path) -> Result<Vec<RelocationCandidate>> {
    let names = physical_names(source)?;
    if names.is_empty() {
        return Err(invalid("source contains no valid physical skills to copy"));
    }
    let root = projected(&source.path)?;
    let destination = projected(destination)?;
    if root.starts_with(&destination) || destination.starts_with(&root) {
        return Err(invalid("source and destination roots must not overlap"));
    }
    let single =
        source.entry.mode == crate::domain::SourceMode::Single || root.join("SKILL.md").is_file();
    names
        .into_iter()
        .map(|name| {
            let source = if single {
                root.clone()
            } else {
                root.join(&name)
            };
            let destination = if single {
                destination.clone()
            } else {
                destination.join(name)
            };
            let existed = exists(&destination)?;
            Ok(RelocationCandidate {
                source,
                destination,
                existed,
            })
        })
        .collect()
}

/// Read-only evidence retained from the authorized plan.
#[derive(Clone, Debug)]
pub struct RelocationPlan {
    /// Stable source identifier.
    pub source_id: String,
    /// Canonical original root.
    pub source: PathBuf,
    /// Canonical or canonically projected destination root.
    pub destination: PathBuf,
    /// All selected skill mappings, including unchanged selections.
    pub copies: Vec<RelocationCopy>,
    config_path: PathBuf,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
    next_config: Config,
    single: bool,
    protected_paths: Vec<PathBuf>,
}

/// Construct an exact copy selection without changing files or config.
///
/// # Errors
/// Rejects absent/unsafe roots, overlaps, ambiguous identities, and unknown names.
pub fn plan(
    config: &Config,
    config_path: &Path,
    before: Option<Vec<u8>>,
    source_id: &str,
    destination: &Path,
    names: &[String],
) -> Result<RelocationPlan> {
    let entry = config
        .sources
        .iter()
        .find(|entry| entry.id == source_id)
        .ok_or_else(|| invalid("source does not exist"))?;
    if entry.source_type != SourceType::Local {
        return Err(invalid("copying requires a local source"));
    }
    let original = entry
        .path
        .as_deref()
        .ok_or_else(|| invalid("local source has no path"))?;
    safe_path(original)?;
    safe_path(destination)?;
    let source =
        fs::canonicalize(original).map_err(|error| SkillManagerError::io(original, error))?;
    let destination = projected(destination)?;
    if source.starts_with(&destination) || destination.starts_with(&source) {
        return Err(invalid("source and destination roots must not overlap"));
    }
    let resolved = ResolvedSource {
        entry: entry.clone(),
        path: source.clone(),
        from_cache: false,
        temporary: None,
        cleanup_pending: None,
    };
    let physical = detect_skill_dirs(&resolved)?;
    if physical.is_empty() {
        return Err(invalid("source contains no valid physical skills to copy"));
    }
    let single = physical.len() == 1 && physical[0] == source;
    let mut candidates = BTreeMap::new();
    for path in physical {
        let name = skill_name(&path)?;
        if candidates.insert(fold(&name), (name, path)).is_some() {
            return Err(invalid(
                "source contains duplicate case-folded skill identities",
            ));
        }
    }
    let mut selected = BTreeSet::new();
    let mut mappings = Vec::new();
    for name in names {
        let key = fold(name);
        let (actual, source_path) = candidates
            .get(&key)
            .ok_or_else(|| invalid(format!("unknown physical skill: {name}")))?;
        if !selected.insert(key) {
            continue;
        }
        let target = if single {
            destination.clone()
        } else {
            destination.join(actual)
        };
        mappings.push((source_path.clone(), target));
    }
    let protected_paths = manager_protected_paths(config_path)?;
    for (_, target) in &mappings {
        reject_manager_overlap(target, &protected_paths)?;
    }
    let mut copies = Vec::new();
    for (source_path, target) in mappings {
        reject_fold_collision(&target)?;
        copies.push(RelocationCopy {
            source: source_path.clone(),
            source_image: tree_image(&source_path)?,
            before: optional_tree(&target)?,
            destination: target,
        });
    }
    let mut next_config = config.clone();
    let next = next_config
        .sources
        .iter_mut()
        .find(|entry| entry.id == source_id)
        .ok_or_else(|| invalid("source disappeared from configuration"))?;
    next.path = Some(crate::config::portable_path(&destination));
    let after = configuration_image(config_path, &next_config)?;
    Ok(RelocationPlan {
        source_id: source_id.into(),
        source,
        destination,
        copies,
        config_path: config_path.to_path_buf(),
        before,
        after,
        next_config,
        single,
        protected_paths,
    })
}

/// Testable precommit and housekeeping boundaries; no success is emitted here.
pub trait RelocationHook {
    /// Immediately before exclusive workspace creation.
    /// # Errors
    /// Injected failures leave concurrent data untouched.
    fn before_workspace(&self, _path: &Path) -> Result<()> {
        Ok(())
    }
    /// Immediately before exclusive destination ancestor creation.
    /// # Errors
    /// Failures roll back only directories already created by this batch.
    fn before_directory(&self, _path: &Path) -> Result<()> {
        Ok(())
    }
    /// Before the numbered placement (zero-based).
    /// # Errors
    /// An injected error rolls back the entire batch.
    fn before_placement(&self, _index: usize) -> Result<()> {
        Ok(())
    }
    /// After all placements and immediately before config installation.
    /// # Errors
    /// An injected error rolls back the entire batch.
    fn before_config(&self) -> Result<()> {
        Ok(())
    }
    /// After configuration installation, before the durable commit record.
    /// # Errors
    /// An injected error restores configuration and every destination.
    fn before_commit(&self) -> Result<()> {
        Ok(())
    }
    /// After durable commitment, before housekeeping.
    /// # Errors
    /// An injected error preserves commitment and returns a cleanup warning.
    fn before_cleanup(&self) -> Result<()> {
        Ok(())
    }
}

/// Production boundaries without failure injection.
pub struct NoopRelocationHook;
impl RelocationHook for NoopRelocationHook {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Phase {
    Staging,
    Applying,
    Rollback,
    Committed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Intent {
    Pending,
    Backup,
    Place,
    Placed,
    Restore,
    Restored,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Placement {
    copy: RelocationCopy,
    stage: PathBuf,
    backup: PathBuf,
    intent: Intent,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CreatedDirectory {
    path: PathBuf,
    owned: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Journal {
    version: u8,
    source_id: String,
    source: PathBuf,
    destination: PathBuf,
    config_path: PathBuf,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
    phase: Phase,
    workspace: PathBuf,
    workspace_owned: bool,
    created_directories: Vec<CreatedDirectory>,
    placements: Vec<Placement>,
}

/// Apply one already authorized relocation, returning only after durable commit.
///
/// # Errors
/// Any precommit failure rolls back the whole batch. Failed rollback retains the
/// exact journal and reports recovery pending. The caller keeps its old Config
/// until this function returns a committed outcome.
pub fn apply(
    repository: &dyn ConfigRepository,
    plan: &RelocationPlan,
    hook: &dyn RelocationHook,
) -> Result<TransactionOutcome<Config>> {
    safe_path(&plan.config_path)?;
    safe_path(repository.cache_root())?;
    let mut session = repository.begin_write(&plan.config_path)?;
    let target_root = if plan.single {
        parent(&plan.destination)?
    } else {
        &plan.destination
    };
    let key = hex::encode(Sha256::digest(target_root.to_string_lossy().as_bytes()));
    let lock_path = repository
        .cache_root()
        .join(".locks")
        .join(format!("target-{}.lock", &key[..24]));
    safe_path(&lock_path)?;
    let _target_lock = acquire_lock(
        &lock_path,
        "relocation destination",
        Duration::from_secs(10),
    )?;
    let (journal_path, workspace) = journal_paths(&plan.destination)?;
    safe_path(&journal_path)?;
    safe_path(&workspace)?;
    if exists(&journal_path)? {
        return Err(invalid(format!(
            "relocation recovery required at {}; recover this authorized destination batch before replanning",
            journal_path.display()
        )));
    }
    if exists(&workspace)? {
        return Err(invalid(format!(
            "unowned relocation directory {}; inspect and move it aside",
            workspace.display()
        )));
    }
    if session.image()? != plan.before {
        return Err(invalid(
            "configuration changed after the relocation plan; replan before applying",
        ));
    }
    recheck(plan)?;
    let mut journal = Journal {
        version: 1,
        source_id: plan.source_id.clone(),
        source: plan.source.clone(),
        destination: plan.destination.clone(),
        config_path: plan.config_path.clone(),
        before: plan.before.clone(),
        after: plan.after.clone(),
        phase: Phase::Staging,
        workspace: workspace.clone(),
        workspace_owned: false,
        created_directories: missing_directories(if plan.single {
            parent(&plan.destination)?
        } else {
            &plan.destination
        })?
        .into_iter()
        .map(|path| CreatedDirectory { path, owned: false })
        .collect(),
        placements: plan
            .copies
            .iter()
            .filter(|copy| copy.before.as_ref() != Some(&copy.source_image))
            .enumerate()
            .map(|(index, copy)| Placement {
                copy: copy.clone(),
                stage: workspace.join(format!("stage-{index}")),
                backup: workspace.join(format!("backup-{index}")),
                intent: Intent::Pending,
            })
            .collect(),
    };
    write_journal(&journal_path, &journal)?;
    let result = execute(&journal_path, &mut journal, session.as_mut(), plan, hook);
    if let Err(error) = result {
        return match rollback(&journal_path, &mut journal, session.as_mut()) {
            Ok(()) => Err(error),
            Err(recovery) => Err(invalid(format!(
                "{error}; relocation rollback pending: {recovery}; exact recovery journal: {}",
                journal_path.display()
            ))),
        };
    }
    let cleanup_pending = hook
        .before_cleanup()
        .and_then(|()| cleanup(&journal_path, &journal))
        .err()
        .map(|error| {
            format!(
                "relocation committed; cleanup pending: {error}; exact recovery journal: {}",
                journal_path.display()
            )
        });
    Ok(TransactionOutcome {
        value: plan.next_config.clone(),
        cleanup_pending,
    })
}

fn execute(
    path: &Path,
    journal: &mut Journal,
    session: &mut dyn ConfigWriteSession,
    plan: &RelocationPlan,
    hook: &dyn RelocationHook,
) -> Result<()> {
    hook.before_workspace(&journal.workspace)?;
    fs::create_dir(&journal.workspace)
        .map_err(|error| SkillManagerError::io(&journal.workspace, error))?;
    journal.workspace_owned = true;
    write_journal(path, journal)?;
    for placement in &journal.placements {
        safe_path(&placement.copy.source)?;
        copy_tree(&placement.copy.source, &placement.stage)?;
        validate_skill_tree(&placement.stage)?;
        if tree_image(&placement.stage)? != placement.copy.source_image {
            return Err(invalid("source changed while staging the relocation"));
        }
        sync_tree(&placement.stage)?;
    }
    recheck(plan)?;
    journal.phase = Phase::Applying;
    write_journal(path, journal)?;
    for index in 0..journal.created_directories.len() {
        let directory = &journal.created_directories[index].path;
        hook.before_directory(directory)?;
        safe_path(directory)?;
        fs::create_dir(directory).map_err(|error| SkillManagerError::io(directory, error))?;
        journal.created_directories[index].owned = true;
        write_journal(path, journal)?;
    }
    for index in 0..journal.placements.len() {
        hook.before_placement(index)?;
        let copy = &journal.placements[index].copy;
        safe_path(&copy.destination)?;
        reject_fold_collision(&copy.destination)?;
        if optional_tree(&copy.destination)? != copy.before {
            return Err(invalid("destination changed at the placement boundary"));
        }
        if copy.before.is_some() {
            journal.placements[index].intent = Intent::Backup;
            write_journal(path, journal)?;
            let item = &journal.placements[index];
            rename(&item.copy.destination, &item.backup)?;
        }
        journal.placements[index].intent = Intent::Place;
        write_journal(path, journal)?;
        let item = &journal.placements[index];
        safe_path(&item.copy.destination)?;
        if exists(&item.copy.destination)? {
            return Err(invalid("destination appeared before placement"));
        }
        rename(&item.stage, &item.copy.destination)?;
        journal.placements[index].intent = Intent::Placed;
        write_journal(path, journal)?;
    }
    hook.before_config()?;
    if session.image()? != journal.before {
        return Err(invalid("configuration changed before installation"));
    }
    session.install(Some(&journal.after))?;
    hook.before_commit()?;
    // Never set the in-memory state to Committed until persistence succeeds.
    let mut committed = journal.clone();
    committed.phase = Phase::Committed;
    write_journal(path, &committed)?;
    journal.phase = Phase::Committed;
    Ok(())
}

fn rollback(
    path: &Path,
    journal: &mut Journal,
    session: &mut dyn ConfigWriteSession,
) -> Result<()> {
    journal.phase = Phase::Rollback;
    write_journal(path, journal)?;
    let current = session.image()?;
    if current != journal.before {
        if current.as_deref() != Some(journal.after.as_slice()) {
            return Err(invalid(
                "configuration diverged from both recorded images; refusing to overwrite it",
            ));
        }
        session.install(journal.before.as_deref())?;
    }
    for index in (0..journal.placements.len()).rev() {
        let item = &journal.placements[index];
        if matches!(item.intent, Intent::Pending | Intent::Restored) {
            continue;
        }
        let backup_exists = exists(&item.backup)?;
        let current = optional_tree(&item.copy.destination)?;
        if !backup_exists && current == item.copy.before {
            journal.placements[index].intent = Intent::Restored;
            write_journal(path, journal)?;
            continue;
        }
        let partially_removed = item.intent == Intent::Restore
            && current.as_ref().is_some_and(|image| {
                image
                    .iter()
                    .all(|(path, hash)| item.copy.source_image.get(path) == Some(hash))
            });
        if current.is_some()
            && current.as_ref() != Some(&item.copy.source_image)
            && !partially_removed
        {
            return Err(invalid(format!(
                "destination diverged during rollback: {}",
                item.copy.destination.display()
            )));
        }
        if item.copy.before.is_some() && !backup_exists {
            return Err(invalid(
                "original destination backup is missing; retaining journal",
            ));
        }
        journal.placements[index].intent = Intent::Restore;
        write_journal(path, journal)?;
        let item = &journal.placements[index];
        if current.is_some() {
            remove_tree(&item.copy.destination)?;
        }
        if backup_exists {
            if optional_tree(&item.backup)? != item.copy.before {
                return Err(invalid(
                    "original destination backup changed; retaining journal",
                ));
            }
            rename(&item.backup, &item.copy.destination)?;
        }
        journal.placements[index].intent = Intent::Restored;
        write_journal(path, journal)?;
    }
    let mut ambiguous_directories = Vec::new();
    for directory in journal.created_directories.iter().rev() {
        if !directory.owned {
            if exists(&directory.path)? {
                ambiguous_directories.push(directory.path.display().to_string());
            }
            continue;
        }
        safe_path(&directory.path)?;
        match fs::remove_dir(&directory.path) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => return Err(SkillManagerError::io(&directory.path, error)),
        }
    }
    if !ambiguous_directories.is_empty() {
        return Err(invalid(format!(
            "directory ownership was not durably established; preserving for inspection: {}",
            ambiguous_directories.join(", ")
        )));
    }
    cleanup(path, journal)
}

fn cleanup(path: &Path, journal: &Journal) -> Result<()> {
    safe_path(&journal.workspace)?;
    if journal.workspace_owned {
        remove_tree(&journal.workspace)?;
    } else if exists(&journal.workspace)? {
        return Err(invalid(format!(
            "workspace ownership was not durably established; preserving {} for inspection",
            journal.workspace.display()
        )));
    }
    fs::remove_file(path).map_err(|error| SkillManagerError::io(path, error))
}

/// Recover the exact prior batch for an explicitly authorized destination.
///
/// Call before rediscovering/replanning only after authorization. This is an
/// internal application seam, not a startup sweep or a separate recovery CLI.
/// Committed journals only clean their owned scratch data; subsequent config or
/// destination edits are never rolled back.
///
/// # Errors
/// Rejects invalid ownership, conflicting configuration, blocked restoration,
/// or cleanup; the journal remains until the pending work completes.
pub fn recover_authorized(
    repository: &dyn ConfigRepository,
    config_path: &Path,
    destination: &Path,
) -> Result<()> {
    let destination = projected(destination)?;
    let (path, workspace) = journal_paths(&destination)?;
    safe_path(&path)?;
    if !exists(&path)? {
        return Ok(());
    }
    let mut session = repository.begin_write(config_path)?;
    let bytes = fs::read(&path).map_err(|error| SkillManagerError::io(&path, error))?;
    let mut journal: Journal =
        serde_json::from_slice(&bytes).map_err(|error| invalid(error.to_string()))?;
    if journal.version != 1
        || journal.destination != destination
        || journal.workspace != workspace
        || journal.config_path != config_path
        || journal.source == destination
        || journal.source.starts_with(&destination)
        || destination.starts_with(&journal.source)
    {
        return Err(invalid("relocation journal has inconsistent ownership"));
    }
    let single = journal
        .placements
        .iter()
        .any(|item| item.copy.destination == destination);
    if single && journal.placements.len() != 1 {
        return Err(invalid("invalid single-skill relocation journal"));
    }
    let mut identities = BTreeSet::new();
    let mut directory_parent = parent(&workspace)?;
    for directory in &journal.created_directories {
        if directory.path.parent() != Some(directory_parent)
            || !destination.starts_with(&directory.path)
            || (single && directory.path == destination)
        {
            return Err(invalid(
                "relocation journal contains an unowned created directory",
            ));
        }
        safe_path(&directory.path)?;
        directory_parent = &directory.path;
    }
    for (index, item) in journal.placements.iter().enumerate() {
        if item.stage != workspace.join(format!("stage-{index}"))
            || item.backup != workspace.join(format!("backup-{index}"))
            || (!single && item.copy.destination.parent() != Some(destination.as_path()))
            || (!single && item.copy.source.parent() != Some(journal.source.as_path()))
            || (single && item.copy.source != journal.source)
            || !identities.insert(fold(&item.copy.destination.to_string_lossy()))
        {
            return Err(invalid("relocation journal contains an unowned mapping"));
        }
        safe_path(&item.copy.destination)?;
        safe_path(&item.stage)?;
        safe_path(&item.backup)?;
    }
    let target = if single {
        parent(&destination)?
    } else {
        &destination
    };
    let key = hex::encode(Sha256::digest(target.to_string_lossy().as_bytes()));
    let lock_path = repository
        .cache_root()
        .join(".locks")
        .join(format!("target-{}.lock", &key[..24]));
    safe_path(&lock_path)?;
    let _target_lock = acquire_lock(&lock_path, "relocation recovery", Duration::from_secs(10))?;
    if journal.phase == Phase::Committed {
        cleanup(&path, &journal)
    } else {
        rollback(&path, &mut journal, session.as_mut())
    }
}

fn recheck(plan: &RelocationPlan) -> Result<()> {
    safe_path(&plan.source)?;
    safe_path(&plan.destination)?;
    if projected(&plan.destination)? != plan.destination {
        return Err(invalid("destination identity changed after planning"));
    }
    for copy in &plan.copies {
        safe_path(&copy.source)?;
        reject_manager_overlap(&copy.destination, &plan.protected_paths)?;
        reject_fold_collision(&copy.destination)?;
        if tree_image(&copy.source)? != copy.source_image
            || optional_tree(&copy.destination)? != copy.before
        {
            return Err(invalid(
                "source or destination changed after the relocation plan",
            ));
        }
    }
    Ok(())
}

fn manager_protected_paths(config_path: &Path) -> Result<Vec<PathBuf>> {
    let storage_root = parent(config_path)?;
    [
        config_path.to_path_buf(),
        storage_root.join("cache"),
        storage_root.join("backups"),
        storage_root.join("locks"),
    ]
    .into_iter()
    .map(|path| projected_entry(&path))
    .collect()
}

fn reject_manager_overlap(destination: &Path, protected_paths: &[PathBuf]) -> Result<()> {
    let destination = projected_entry(destination)?;
    if protected_paths
        .iter()
        .any(|protected| destination.starts_with(protected) || protected.starts_with(&destination))
    {
        return Err(invalid(format!(
            "relocation destination overlaps active skill-manager state: {}",
            destination.display()
        )));
    }
    Ok(())
}

fn projected_entry(path: &Path) -> Result<PathBuf> {
    safe_path(path)?;
    if exists(path)? {
        return fs::canonicalize(path).map_err(|error| SkillManagerError::io(path, error));
    }
    let canonical = projected_entry(parent(path)?)?;
    Ok(canonical.join(
        path.file_name()
            .ok_or_else(|| invalid("path has no name"))?,
    ))
}

fn journal_paths(destination: &Path) -> Result<(PathBuf, PathBuf)> {
    let key = hex::encode(Sha256::digest(destination.to_string_lossy().as_bytes()));
    let stem = format!(".skill-manager-relocation-{}", &key[..24]);
    let mut found = None;
    let mut nearest = None;
    for ancestor in parent(destination)?.ancestors() {
        safe_path(ancestor)?;
        if exists(ancestor)? {
            if nearest.is_none() {
                nearest = Some(ancestor);
            }
            let journal = ancestor.join(format!("{stem}.json"));
            if exists(&journal)? {
                if found.is_some() {
                    return Err(invalid(
                        "multiple journals claim this relocation destination; inspect exact ownership before retrying",
                    ));
                }
                found = Some((journal, ancestor.join(&stem)));
            }
        }
    }
    if let Some(paths) = found {
        return Ok(paths);
    }
    let ancestor = nearest.ok_or_else(|| invalid("destination has no existing ancestor"))?;
    Ok((ancestor.join(format!("{stem}.json")), ancestor.join(stem)))
}

fn write_journal(path: &Path, journal: &Journal) -> Result<()> {
    safe_path(path)?;
    let bytes = serde_json::to_vec(journal).map_err(|error| invalid(error.to_string()))?;
    fs::atomic_write(path, &bytes).map_err(|error| SkillManagerError::io(path, error))
}

fn tree_image(root: &Path) -> Result<TreeImage> {
    safe_path(root)?;
    if !root.is_dir() {
        return Err(invalid(format!(
            "destination is not a directory: {}",
            root.display()
        )));
    }
    let mut result = TreeImage::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| invalid(error.to_string()))?;
        safe_path(entry.path())?;
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|error| invalid(error.to_string()))?;
        let hash = if entry.file_type().is_dir() {
            None
        } else if entry.file_type().is_file() {
            let bytes = fs::read(entry.path())
                .map_err(|error| SkillManagerError::io(entry.path(), error))?;
            Some(hex::encode(Sha256::digest(bytes)))
        } else {
            return Err(invalid("tree contains an unsupported entry"));
        };
        result.insert(relative.to_path_buf(), hash);
    }
    Ok(result)
}

fn optional_tree(path: &Path) -> Result<Option<TreeImage>> {
    safe_path(path)?;
    if exists(path)? {
        tree_image(path).map(Some)
    } else {
        Ok(None)
    }
}

fn sync_tree(root: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| invalid(error.to_string()))?;
        if entry.file_type().is_file() {
            let file = fs::retry(|| {
                fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(entry.path())
            })
            .map_err(|error| SkillManagerError::io(entry.path(), error))?;
            fs::retry(|| file.sync_all())
                .map_err(|error| SkillManagerError::io(entry.path(), error))?;
        }
    }
    Ok(())
}

fn safe_path(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(invalid(format!(
            "relocation requires an absolute path without parent traversal: {}",
            path.display()
        )));
    }
    for ancestor in path.ancestors() {
        reject_link(ancestor)?;
        #[cfg(windows)]
        if let Ok(metadata) = fs::symlink_metadata(ancestor) {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(invalid(format!(
                    "refusing reparse path: {}",
                    ancestor.display()
                )));
            }
        }
    }
    Ok(())
}

/// Inspect the lexical copy operand before source-reference normalization can
/// erase a linked ancestor. Parent components are resolved only after checking
/// each preceding component, so a junction followed by `..` is still rejected.
pub(crate) fn validate_copy_operand(path: &Path) -> Result<()> {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                prefix.pop();
            }
            std::path::Component::CurDir => {}
            other => prefix.push(other.as_os_str()),
        }
        if prefix.is_absolute() {
            safe_path(&prefix)?;
        }
    }
    Ok(())
}

fn projected(path: &Path) -> Result<PathBuf> {
    safe_path(path)?;
    if exists(path)? {
        if !path.is_dir() {
            return Err(invalid("relocation destination must be a directory"));
        }
        return fs::canonicalize(path).map_err(|error| SkillManagerError::io(path, error));
    }
    let canonical = projected(parent(path)?)?;
    Ok(canonical.join(
        path.file_name()
            .ok_or_else(|| invalid("destination has no name"))?,
    ))
}

fn reject_fold_collision(path: &Path) -> Result<()> {
    let parent = parent(path)?;
    if !exists(parent)? {
        return Ok(());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("non-UTF8 destination name"))?;
    for entry in fs::read_dir(parent).map_err(|error| SkillManagerError::io(parent, error))? {
        let entry = entry.map_err(|error| SkillManagerError::io(parent, error))?;
        let actual = entry.file_name();
        let actual = actual
            .to_str()
            .ok_or_else(|| invalid("non-UTF8 destination entry"))?;
        if actual != name && fold(actual) == fold(name) {
            return Err(invalid(format!(
                "case-folded destination collision: {name} and {actual}"
            )));
        }
    }
    Ok(())
}

fn missing_directories(root: &Path) -> Result<Vec<PathBuf>> {
    let mut directories = Vec::new();
    for ancestor in root.ancestors() {
        safe_path(ancestor)?;
        if exists(ancestor)? {
            break;
        }
        directories.push(ancestor.to_path_buf());
    }
    directories.reverse();
    Ok(directories)
}

/// Read-only pending-batch disclosure for the application authorization phase.
/// # Errors
/// Returns an error for ambiguous journals or malformed destination ownership.
pub fn pending_recovery(destination: &Path) -> Result<Option<(PathBuf, String)>> {
    let destination = projected(destination)?;
    let (path, _) = journal_paths(&destination)?;
    if !exists(&path)? {
        return Ok(None);
    }
    safe_path(&path)?;
    let bytes = fs::read(&path).map_err(|error| SkillManagerError::io(&path, error))?;
    let journal: Journal =
        serde_json::from_slice(&bytes).map_err(|error| invalid(error.to_string()))?;
    if journal.version != 1 || journal.destination != destination {
        return Err(invalid("invalid pending relocation ownership"));
    }
    let effect = if journal.phase == Phase::Committed {
        format!(
            "Clean only owned staging and backups at {}; preserve later configuration and destination edits.",
            journal.workspace.display()
        )
    } else {
        let destinations = journal
            .placements
            .iter()
            .map(|item| {
                format!(
                    "{} ({})",
                    item.copy.destination.display(),
                    if item.copy.before.is_some() {
                        "restore previous directory"
                    } else {
                        "restore absence"
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "Restore the exact prior configuration at {} if its current image matches this batch; restore selected destinations: {destinations}. Clean owned staging and backups at {}. Preserve source originals.",
            journal.config_path.display(),
            journal.workspace.display()
        )
    };
    Ok(Some((path, effect)))
}

fn parent(path: &Path) -> Result<&Path> {
    path.parent().ok_or_else(|| invalid("path has no parent"))
}
fn rename(from: &Path, to: &Path) -> Result<()> {
    safe_path(from)?;
    safe_path(to)?;
    fs::rename(from, to).map_err(|error| SkillManagerError::io(to, error))
}
fn invalid(message: impl Into<String>) -> SkillManagerError {
    SkillManagerError::InvalidInput(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FileConfigRepository;

    struct Fixture {
        home: tempfile::TempDir,
        repository: FileConfigRepository,
        config: Config,
        source: PathBuf,
        destination: PathBuf,
        before: Vec<u8>,
        original_source: TreeImage,
        original_destination: TreeImage,
    }

    impl Fixture {
        fn new() -> Result<Self> {
            let home = tempfile::tempdir().map_err(|error| invalid(error.to_string()))?;
            let repository = FileConfigRepository::new(home.path());
            let source = home.path().join("source");
            let destination = home.path().join("destination");
            for (root, names) in [
                (&source, vec!["alpha", "beta"]),
                (&destination, vec!["alpha", "gamma"]),
            ] {
                for name in names {
                    let directory = root.join(name);
                    fs::create_dir_all(&directory)
                        .map_err(|error| SkillManagerError::io(&directory, error))?;
                    let content = format!(
                        "---\nname: {name}\ndescription: physical test skill\n---\n{}\n",
                        root.display()
                    );
                    fs::write(directory.join("SKILL.md"), content)
                        .map_err(|error| invalid(error.to_string()))?;
                }
            }
            fs::write(destination.join("alpha/extra.txt"), "prior extra file")
                .map_err(|error| invalid(error.to_string()))?;
            fs::write(destination.join("README.md"), "destination root file")
                .map_err(|error| invalid(error.to_string()))?;
            fs::write(source.join("README.md"), "do not copy root files")
                .map_err(|error| invalid(error.to_string()))?;
            let config: Config = serde_json::from_value(serde_json::json!({
                "schema_version": 2, "exclude": ["alpha", "beta"],
                "sources": [{ "id": "physical", "name": "physical", "path": source,
                    "exclude": ["alpha", "beta"], "custom_metadata": {"keep": true} }]
            }))
            .map_err(|error| invalid(error.to_string()))?;
            repository.save(repository.config_path(), &config)?;
            // Preserve intentionally noncanonical formatting to prove exact-byte rollback.
            let before = format!(
                "  {}\n\n",
                serde_json::to_string(&config).map_err(|error| invalid(error.to_string()))?
            )
            .into_bytes();
            fs::write(repository.config_path(), &before)
                .map_err(|error| invalid(error.to_string()))?;
            let original_source = tree_image(&source)?;
            let original_destination = tree_image(&destination)?;
            Ok(Self {
                home,
                repository,
                config,
                source,
                destination,
                before,
                original_source,
                original_destination,
            })
        }
        fn plan(&self) -> Result<RelocationPlan> {
            plan(
                &self.config,
                self.repository.config_path(),
                Some(self.before.clone()),
                "physical",
                &self.destination,
                &["alpha".into(), "beta".into()],
            )
        }
        fn assert_rolled_back(&self) -> Result<()> {
            assert_eq!(
                fs::read(self.repository.config_path())
                    .map_err(|error| invalid(error.to_string()))?,
                self.before
            );
            assert_eq!(tree_image(&self.destination)?, self.original_destination);
            assert_eq!(tree_image(&self.source)?, self.original_source);
            assert!(!self.destination.join("beta").exists());
            assert!(self.destination.join("alpha/extra.txt").exists());
            assert!(!journal_paths(&projected(&self.destination)?)?.0.exists());
            Ok(())
        }
    }

    struct FailConfig<'a>(&'a Fixture);
    impl RelocationHook for FailConfig<'_> {
        fn before_config(&self) -> Result<()> {
            assert!(self.0.destination.join("beta/SKILL.md").exists());
            assert!(!self.0.destination.join("alpha/extra.txt").exists());
            Err(invalid(
                "injected config installation failure after both placements",
            ))
        }
    }

    #[test]
    fn configuration_failure_restores_exact_whole_batch_and_returns_no_committed_result()
    -> Result<()> {
        let fixture = Fixture::new()?;
        let plan = fixture.plan()?;
        assert_eq!(
            plan.copies.len(),
            2,
            "global and source exclusions must not hide physical skills"
        );
        let result = apply(&fixture.repository, &plan, &FailConfig(&fixture));
        assert!(result.is_err());
        fixture.assert_rolled_back()
    }

    #[test]
    fn success_replaces_entire_alpha_creates_beta_and_preserves_other_data() -> Result<()> {
        let fixture = Fixture::new()?;
        let plan = fixture.plan()?;
        let outcome = apply(&fixture.repository, &plan, &NoopRelocationHook)?;
        assert!(outcome.cleanup_pending.is_none());
        assert_eq!(
            outcome.value.sources[0].path.as_deref(),
            Some(crate::config::portable_path(&plan.destination).as_path())
        );
        assert_eq!(
            outcome.value.sources[0].extra,
            fixture.config.sources[0].extra
        );
        assert_eq!(
            tree_image(&fixture.destination.join("alpha"))?,
            tree_image(&fixture.source.join("alpha"))?
        );
        assert_eq!(
            tree_image(&fixture.destination.join("beta"))?,
            tree_image(&fixture.source.join("beta"))?
        );
        assert!(!fixture.destination.join("alpha/extra.txt").exists());
        assert_eq!(tree_image(&fixture.source)?, fixture.original_source);
        assert_eq!(
            fs::read(fixture.destination.join("README.md"))
                .map_err(|error| invalid(error.to_string()))?,
            b"destination root file"
        );
        for (name, image) in &fixture.original_destination {
            if name.starts_with("gamma") {
                assert_eq!(tree_image(&fixture.destination)?.get(name), Some(image));
            }
        }
        assert_eq!(
            fs::read(fixture.repository.config_path())
                .map_err(|error| invalid(error.to_string()))?,
            plan.after
        );
        Ok(())
    }

    #[test]
    fn second_placement_failure_restores_first_replacement() -> Result<()> {
        struct FailSecond;
        impl RelocationHook for FailSecond {
            fn before_placement(&self, index: usize) -> Result<()> {
                if index == 1 {
                    Err(invalid("injected second placement failure"))
                } else {
                    Ok(())
                }
            }
        }
        let fixture = Fixture::new()?;
        assert!(apply(&fixture.repository, &fixture.plan()?, &FailSecond).is_err());
        fixture.assert_rolled_back()
    }

    #[test]
    fn failed_commit_record_restores_config_even_after_installation() -> Result<()> {
        struct FailCommit;
        impl RelocationHook for FailCommit {
            fn before_commit(&self) -> Result<()> {
                Err(invalid("injected commit record failure"))
            }
        }
        let fixture = Fixture::new()?;
        assert!(apply(&fixture.repository, &fixture.plan()?, &FailCommit).is_err());
        fixture.assert_rolled_back()
    }

    #[cfg(windows)]
    #[test]
    fn real_config_install_sharing_failure_restores_both_destinations() -> Result<()> {
        use std::cell::RefCell;
        use std::os::windows::fs::OpenOptionsExt;
        struct HoldConfig<'a> {
            fixture: &'a Fixture,
            held: RefCell<Option<std::fs::File>>,
        }
        impl RelocationHook for HoldConfig<'_> {
            fn before_config(&self) -> Result<()> {
                assert!(self.fixture.destination.join("beta/SKILL.md").exists());
                assert!(!self.fixture.destination.join("alpha/extra.txt").exists());
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(1)
                    .open(self.fixture.repository.config_path())
                    .map_err(|error| invalid(error.to_string()))?;
                *self.held.borrow_mut() = Some(file);
                Ok(())
            }
        }
        let fixture = Fixture::new()?;
        let hook = HoldConfig {
            fixture: &fixture,
            held: RefCell::new(None),
        };
        let result = apply(&fixture.repository, &fixture.plan()?, &hook);
        assert!(result.is_err());
        assert!(
            hook.held.borrow().is_some(),
            "must reach actual config installation after both placements"
        );
        drop(hook);
        fixture.assert_rolled_back()
    }

    #[cfg(windows)]
    #[test]
    fn blocked_batch_rollback_retains_journal_and_recovers_after_handle_release() -> Result<()> {
        use std::cell::RefCell;
        use std::os::windows::fs::OpenOptionsExt;
        struct BlockRollback<'a> {
            fixture: &'a Fixture,
            held: RefCell<Option<std::fs::File>>,
        }
        impl RelocationHook for BlockRollback<'_> {
            fn before_config(&self) -> Result<()> {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(1)
                    .open(self.fixture.destination.join("beta/SKILL.md"))
                    .map_err(|error| invalid(error.to_string()))?;
                *self.held.borrow_mut() = Some(file);
                Err(invalid(
                    "fail before config with beta locked against rollback",
                ))
            }
        }
        let fixture = Fixture::new()?;
        let hook = BlockRollback {
            fixture: &fixture,
            held: RefCell::new(None),
        };
        let error = apply(&fixture.repository, &fixture.plan()?, &hook)
            .err()
            .ok_or_else(|| invalid("expected blocked rollback"))?;
        assert!(error.to_string().contains("rollback pending"));
        let journal_path = journal_paths(&projected(&fixture.destination)?)?.0;
        assert!(journal_path.exists());
        assert_eq!(
            fs::read(fixture.repository.config_path())
                .map_err(|error| invalid(error.to_string()))?,
            fixture.before
        );
        assert_eq!(tree_image(&fixture.source)?, fixture.original_source);
        drop(hook);
        recover_authorized(
            &fixture.repository,
            fixture.repository.config_path(),
            &fixture.destination,
        )?;
        fixture.assert_rolled_back()
    }

    #[test]
    fn configuration_drift_refuses_destination_writes() -> Result<()> {
        let fixture = Fixture::new()?;
        let plan = fixture.plan()?;
        fs::write(
            fixture.repository.config_path(),
            b"concurrent user configuration",
        )
        .map_err(|error| invalid(error.to_string()))?;
        let result = apply(&fixture.repository, &plan, &NoopRelocationHook);
        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("configuration changed"))
        );
        assert_eq!(
            tree_image(&fixture.destination)?,
            fixture.original_destination
        );
        assert_eq!(
            fs::read(fixture.repository.config_path())
                .map_err(|error| invalid(error.to_string()))?,
            b"concurrent user configuration"
        );
        Ok(())
    }

    #[test]
    fn committed_cleanup_recovery_preserves_later_config_and_destination_edits() -> Result<()> {
        struct DeferCleanup;
        impl RelocationHook for DeferCleanup {
            fn before_cleanup(&self) -> Result<()> {
                Err(invalid("held cleanup"))
            }
        }
        let fixture = Fixture::new()?;
        let outcome = apply(&fixture.repository, &fixture.plan()?, &DeferCleanup)?;
        assert!(outcome.cleanup_pending.is_some());
        fs::write(fixture.repository.config_path(), b"later user config")
            .map_err(|error| invalid(error.to_string()))?;
        fs::write(fixture.destination.join("alpha/later.txt"), b"later edit")
            .map_err(|error| invalid(error.to_string()))?;
        recover_authorized(
            &fixture.repository,
            fixture.repository.config_path(),
            &fixture.destination,
        )?;
        assert!(fixture.destination.join("alpha/later.txt").exists());
        assert_eq!(
            fs::read(fixture.repository.config_path())
                .map_err(|error| invalid(error.to_string()))?,
            b"later user config"
        );
        Ok(())
    }

    #[test]
    fn deep_missing_destination_parents_are_owned_and_rolled_back() -> Result<()> {
        struct FailCommit;
        impl RelocationHook for FailCommit {
            fn before_commit(&self) -> Result<()> {
                Err(invalid("fail after config install"))
            }
        }
        let fixture = Fixture::new()?;
        let destination = fixture.home.path().join("missing/parents/skills");
        let plan = plan(
            &fixture.config,
            fixture.repository.config_path(),
            Some(fixture.before.clone()),
            "physical",
            &destination,
            &["alpha".into(), "beta".into()],
        )?;
        assert!(apply(&fixture.repository, &plan, &FailCommit).is_err());
        assert!(!fixture.home.path().join("missing").exists());
        fixture.assert_rolled_back()?;
        apply(&fixture.repository, &plan, &NoopRelocationHook)?;
        assert!(destination.join("alpha/SKILL.md").exists());
        assert!(destination.join("beta/SKILL.md").exists());
        Ok(())
    }

    #[test]
    fn deep_parent_committed_recovery_finds_original_ancestor_journal() -> Result<()> {
        struct DeferCleanup;
        impl RelocationHook for DeferCleanup {
            fn before_cleanup(&self) -> Result<()> {
                Err(invalid("defer owned cleanup"))
            }
        }
        let fixture = Fixture::new()?;
        let destination = fixture.home.path().join("missing/parents/skills");
        let plan = plan(
            &fixture.config,
            fixture.repository.config_path(),
            Some(fixture.before.clone()),
            "physical",
            &destination,
            &["beta".into()],
        )?;
        let expected = journal_paths(&plan.destination)?.0;
        let outcome = apply(&fixture.repository, &plan, &DeferCleanup)?;
        assert!(outcome.cleanup_pending.is_some());
        assert_eq!(journal_paths(&plan.destination)?.0, expected);
        recover_authorized(
            &fixture.repository,
            fixture.repository.config_path(),
            &destination,
        )?;
        assert!(!expected.exists());
        assert!(destination.join("beta/SKILL.md").exists());
        Ok(())
    }

    #[test]
    fn single_skill_is_copied_directly_and_overlap_and_file_targets_are_rejected() -> Result<()> {
        let fixture = Fixture::new()?;
        for destination in [
            &fixture.source,
            &fixture.source.join("nested"),
            fixture.home.path(),
        ] {
            assert!(
                plan(
                    &fixture.config,
                    fixture.repository.config_path(),
                    Some(fixture.before.clone()),
                    "physical",
                    destination,
                    &["alpha".into()]
                )
                .is_err()
            );
        }
        let target = fixture.home.path().join("file");
        fs::write(&target, b"preserve").map_err(|error| invalid(error.to_string()))?;
        assert!(
            plan(
                &fixture.config,
                fixture.repository.config_path(),
                Some(fixture.before.clone()),
                "physical",
                &target,
                &["alpha".into()]
            )
            .is_err()
        );
        let mut config = fixture.config.clone();
        config.sources[0].path = Some(fixture.source.join("alpha"));
        config.sources[0].mode = crate::domain::SourceMode::Single;
        fixture
            .repository
            .save(fixture.repository.config_path(), &config)?;
        let before = fs::read(fixture.repository.config_path())
            .map_err(|error| invalid(error.to_string()))?;
        let destination = fixture.home.path().join("new/renamed");
        let plan = plan(
            &config,
            fixture.repository.config_path(),
            Some(before),
            "physical",
            &destination,
            &["alpha".into()],
        )?;
        apply(&fixture.repository, &plan, &NoopRelocationHook)?;
        assert!(destination.join("SKILL.md").exists());
        assert!(!destination.join("alpha").exists());
        assert_eq!(tree_image(&fixture.source)?, fixture.original_source);
        Ok(())
    }

    #[test]
    fn selected_case_collision_is_rejected_but_unselected_file_is_preserved() -> Result<()> {
        let fixture = Fixture::new()?;
        let target = fixture.home.path().join("partial");
        fs::create_dir(&target).map_err(|error| invalid(error.to_string()))?;
        fs::write(target.join("ALPHA"), b"unselected data")
            .map_err(|error| invalid(error.to_string()))?;
        assert!(
            plan(
                &fixture.config,
                fixture.repository.config_path(),
                Some(fixture.before.clone()),
                "physical",
                &target,
                &["alpha".into()]
            )
            .is_err()
        );
        let plan = plan(
            &fixture.config,
            fixture.repository.config_path(),
            Some(fixture.before.clone()),
            "physical",
            &target,
            &["beta".into()],
        )?;
        apply(&fixture.repository, &plan, &NoopRelocationHook)?;
        assert_eq!(
            fs::read(target.join("ALPHA")).map_err(|error| invalid(error.to_string()))?,
            b"unselected data"
        );
        assert!(target.join("beta/SKILL.md").exists());
        Ok(())
    }

    #[test]
    fn concurrently_created_workspace_is_never_claimed_or_deleted() -> Result<()> {
        struct OccupyWorkspace;
        impl RelocationHook for OccupyWorkspace {
            fn before_workspace(&self, path: &Path) -> Result<()> {
                fs::create_dir(path).map_err(|error| invalid(error.to_string()))?;
                fs::write(path.join("unowned-sentinel"), b"keep")
                    .map_err(|error| invalid(error.to_string()))?;
                Ok(())
            }
        }
        let fixture = Fixture::new()?;
        let plan = fixture.plan()?;
        let (journal, workspace) = journal_paths(&plan.destination)?;
        assert!(apply(&fixture.repository, &plan, &OccupyWorkspace).is_err());
        assert_eq!(
            fs::read(workspace.join("unowned-sentinel"))
                .map_err(|error| invalid(error.to_string()))?,
            b"keep"
        );
        assert_eq!(
            tree_image(&fixture.destination)?,
            fixture.original_destination
        );
        assert_eq!(
            fs::read(fixture.repository.config_path())
                .map_err(|error| invalid(error.to_string()))?,
            fixture.before
        );
        assert!(journal.exists());
        assert!(
            recover_authorized(
                &fixture.repository,
                fixture.repository.config_path(),
                &plan.destination
            )
            .is_err()
        );
        assert!(workspace.join("unowned-sentinel").exists());
        Ok(())
    }

    #[test]
    fn concurrently_created_empty_destination_parent_is_preserved() -> Result<()> {
        struct OccupyParent;
        impl RelocationHook for OccupyParent {
            fn before_directory(&self, path: &Path) -> Result<()> {
                fs::create_dir(path).map_err(|error| invalid(error.to_string()))?;
                Ok(())
            }
        }
        let fixture = Fixture::new()?;
        let destination = fixture.home.path().join("unowned-parent/nested/skills");
        let plan = plan(
            &fixture.config,
            fixture.repository.config_path(),
            Some(fixture.before.clone()),
            "physical",
            &destination,
            &["beta".into()],
        )?;
        let result = apply(&fixture.repository, &plan, &OccupyParent);
        assert!(result.err().is_some_and(|error| {
            error
                .to_string()
                .contains("ownership was not durably established")
        }));
        assert!(fixture.home.path().join("unowned-parent").is_dir());
        assert!(!fixture.home.path().join("unowned-parent/nested").exists());
        assert_eq!(
            fs::read(fixture.repository.config_path())
                .map_err(|error| invalid(error.to_string()))?,
            fixture.before
        );
        Ok(())
    }
}
