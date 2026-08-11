//! Typed errors exposed by the application boundary.

use std::path::PathBuf;

/// Failures returned by skill-manager operations.
#[derive(Debug, thiserror::Error)]
pub enum SkillManagerError {
    /// A command or configuration value is invalid.
    #[error("{0}")]
    InvalidInput(String),
    /// A requested item was not found.
    #[error("{kind} not found: {reference}")]
    NotFound {
        /// The kind of item that was requested.
        kind: &'static str,
        /// The user-supplied reference.
        reference: String,
    },
    /// A label selector identifies more than one configured source.
    #[error("ambiguous source label: {label}")]
    AmbiguousSourceLabel {
        /// The user-supplied label.
        label: String,
    },
    /// A bare `load`/`update` literal matched no source, directory, or skill.
    #[error(
        "no configured source, directory, or skill named \"{reference}\"; run `skill-manager ls` to see configured sources and discovered skills"
    )]
    NoSourceDirectoryOrSkill {
        /// The user-supplied literal operand.
        reference: String,
    },
    /// A filesystem operation failed.
    #[error("filesystem operation failed for {path}: {source}")]
    FileSystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Configuration JSON was malformed or incompatible.
    #[error("invalid configuration at {path}: {message}")]
    InvalidConfig {
        /// Configuration path.
        path: PathBuf,
        /// Validation failure.
        message: String,
    },
    /// A remote GitHub source could not be materialized.
    #[error("GitHub source {reference} failed: {message}")]
    GitHub {
        /// Safe source reference without credentials.
        reference: String,
        /// Transport or archive failure.
        message: String,
    },
    /// A lock could not be acquired in time.
    #[error("timed out waiting for {resource} lock after {seconds} seconds")]
    LockTimeout {
        /// Locked resource.
        resource: String,
        /// Timeout duration.
        seconds: u64,
    },
    /// An interactive decision is required in noninteractive mode.
    #[error("{0}")]
    InteractionRequired(String),
    /// An operation was cancelled by the user.
    #[error("cancelled")]
    Cancelled,
}

impl SkillManagerError {
    /// Construct a path-aware filesystem error.
    #[must_use]
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::FileSystem {
            path: path.into(),
            source,
        }
    }
}

/// Result type used throughout the crate.
pub type Result<T> = std::result::Result<T, SkillManagerError>;
