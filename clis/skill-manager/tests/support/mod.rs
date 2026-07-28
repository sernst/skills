//! Shared helpers for integration-test path assertions.

use std::io;
use std::path::{Path, PathBuf};

/// Canonicalize a fixture path while removing Windows-only verbatim prefixes.
///
/// The application serializes ordinary Windows paths, while `std::fs` may
/// return verbatim paths from `canonicalize`.  Tests compare path identities,
/// not those platform-specific spellings.
pub fn portable_canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let canonical = path.as_ref().canonicalize()?;
    #[cfg(windows)]
    {
        let text = canonical.to_string_lossy();
        if let Some(path) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{path}")));
        }
        if let Some(path) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(path));
        }
    }
    Ok(canonical)
}
