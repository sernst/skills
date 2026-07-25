//! Core domain types independent of user interface and persistence.

use std::path::PathBuf;
use std::sync::Arc;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Stable source identifier.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(pub String);

/// Supported source storage kinds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    /// A local filesystem source.
    #[default]
    Local,
    /// A source downloaded from a GitHub repository archive.
    GitHub,
}

/// How skills are laid out below a source root.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceMode {
    /// Immediate child directories containing `SKILL.md` are skills.
    #[default]
    Collection,
    /// The source root itself is one skill.
    Single,
}

/// Persisted source definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceEntry {
    /// Stable identifier.
    #[serde(default)]
    pub id: String,
    /// Storage kind.
    #[serde(default, rename = "type")]
    pub source_type: SourceType,
    /// Layout below the materialized root.
    #[serde(default)]
    pub mode: SourceMode,
    /// Unique command-facing name.
    #[serde(default)]
    pub name: String,
    /// Human-facing label.
    #[serde(default)]
    pub label: String,
    /// Per-source exclusion patterns.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Optional cache lifetime override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_ttl_hours: Option<i64>,
    /// Local source path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// GitHub owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// GitHub repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Git ref; omitted to resolve the default branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    /// Path within the GitHub repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_path: Option<String>,
    /// Unknown fields preserved across configuration updates.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_json::Value>,
}

/// A source materialized into the local filesystem.
#[derive(Clone, Debug)]
pub struct ResolvedSource {
    /// Persisted definition.
    pub entry: SourceEntry,
    /// Local root from which discovery runs.
    pub path: PathBuf,
    /// Whether the root belongs to the persistent remote cache.
    pub from_cache: bool,
    /// Keeps invocation-scoped materialization alive without persistent cache writes.
    pub temporary: Option<Arc<tempfile::TempDir>>,
}

/// A discovered source skill.
#[derive(Clone, Debug)]
pub struct SkillCandidate {
    /// Portable skill name.
    pub name: String,
    /// Directory containing `SKILL.md`.
    pub path: PathBuf,
    /// Source that supplied the candidate.
    pub source: ResolvedSource,
}

/// Deterministic discovery output.
#[derive(Clone, Debug, Default)]
pub struct SkillDiscovery {
    /// First-source-wins skill candidates keyed by folded identity.
    pub winners: IndexMap<String, SkillCandidate>,
    /// All candidates for collided identities, winner first.
    pub collisions: IndexMap<String, Vec<SkillCandidate>>,
}

/// Persisted deployment target.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetEntry {
    /// Target directory.
    pub path: PathBuf,
    /// Human-readable label.
    #[serde(default)]
    pub label: String,
    /// Whether implicit target selection includes this target.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Unknown fields preserved across configuration updates.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_json::Value>,
}

const fn default_true() -> bool {
    true
}

/// Runtime target with its stable name.
#[derive(Clone, Debug)]
pub struct Target {
    /// Stable command-facing name.
    pub name: String,
    /// Human-readable label.
    pub label: String,
    /// Target directory.
    pub path: PathBuf,
    /// Whether implicit selection includes the target.
    pub enabled: bool,
    /// Whether this is a built-in target.
    pub builtin: bool,
    /// Whether a migrated legacy definition overrides a built-in.
    pub legacy_override: bool,
}

/// State of a source skill relative to a deployment target.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillState {
    /// Source and deployment have identical regular files.
    UpToDate,
    /// A deployment exists but differs from the source.
    NeedsUpdate,
    /// The source exists but is not deployed.
    NotLoaded,
    /// A deployment has no corresponding source.
    NoConnection,
}

impl SkillState {
    /// Stable machine/human label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UpToDate => "up-to-date",
            Self::NeedsUpdate => "needs-update",
            Self::NotLoaded => "not-loaded",
            Self::NoConnection => "no-connection",
        }
    }
}
