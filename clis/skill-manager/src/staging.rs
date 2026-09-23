//! Exact-path receipts for scratch directories used under an existing resource lock.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, SkillManagerError};
use crate::fs_retry as fs;

#[derive(Deserialize, Serialize)]
struct Receipt {
    version: u8,
    directory: PathBuf,
}

/// Reject links, including Windows junctions, without following them.
pub(crate) fn reject_link(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            let linked = {
                use std::os::windows::fs::FileTypeExt;
                let kind = metadata.file_type();
                kind.is_symlink() || kind.is_symlink_dir() || kind.is_symlink_file()
            };
            #[cfg(not(windows))]
            let linked = metadata.file_type().is_symlink();
            if linked {
                return Err(SkillManagerError::InvalidInput(format!(
                    "refusing cleanup through a linked manager path: {}",
                    path.display()
                )));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SkillManagerError::io(path, error)),
    }
}

pub(crate) fn remove_tree(path: &Path) -> Result<()> {
    reject_link(path)?;
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SkillManagerError::io(path, error)),
    }
}

pub(crate) fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(SkillManagerError::io(path, error)),
    }
}

/// A durable receipt is written before creating/populating a disposable directory.
/// The caller holds the corresponding config, migration, source, or target lock.
pub(crate) fn with_directory<T>(
    parent: &Path,
    key: &str,
    operation: impl FnOnce(&Path) -> Result<T>,
) -> Result<T> {
    let directory = parent.join(format!(".skill-manager-{key}-stage"));
    let receipt = parent.join(format!(".skill-manager-{key}-cleanup.json"));
    fs::create_dir_all(parent).map_err(|error| SkillManagerError::io(parent, error))?;
    reject_link(parent)?;
    reject_link(&receipt)?;
    match fs::read(&receipt) {
        Ok(bytes) => {
            let record: Receipt = serde_json::from_slice(&bytes).map_err(|error| {
                SkillManagerError::InvalidInput(format!(
                    "invalid cleanup receipt {}: {error}",
                    receipt.display()
                ))
            })?;
            if record.version != 1 || record.directory != directory {
                return Err(SkillManagerError::InvalidInput(format!(
                    "cleanup receipt {} names an unexpected directory",
                    receipt.display()
                )));
            }
            remove_tree(&directory)?;
            fs::remove_file(&receipt).map_err(|error| SkillManagerError::io(&receipt, error))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(SkillManagerError::io(&receipt, error)),
    }
    if exists(&directory)? {
        return Err(SkillManagerError::InvalidInput(format!(
            "unowned staging directory {}; inspect and move it aside before retrying; no cleanup receipt exists",
            directory.display()
        )));
    }
    let bytes = serde_json::to_vec(&Receipt {
        version: 1,
        directory: directory.clone(),
    })
    .map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
    fs::atomic_write(&receipt, &bytes).map_err(|error| SkillManagerError::io(&receipt, error))?;
    fs::create_dir(&directory).map_err(|error| SkillManagerError::io(&directory, error))?;
    let result = operation(&directory);
    let cleanup = remove_tree(&directory).and_then(|()| {
        fs::remove_file(&receipt).map_err(|error| SkillManagerError::io(&receipt, error))
    });
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (result, Err(cleanup)) => Err(SkillManagerError::InvalidInput(format!(
            "{}; staging cleanup pending at {} (receipt {}); retry this operation to recover: {cleanup}",
            result.err().map_or_else(
                || "staged work completed".to_owned(),
                |error| error.to_string()
            ),
            directory.display(),
            receipt.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_preparation_cleans_exact_owned_directory() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let result: Result<()> = with_directory(root.path(), "test", |path| {
            fs::write(path.join("partial"), "bytes")
                .unwrap_or_else(|error| unreachable!("{error}"));
            Err(SkillManagerError::InvalidInput("preparation failed".into()))
        });
        assert!(result.is_err());
        assert_eq!(fs::read_dir(root.path()).map(Iterator::count).ok(), Some(0));
    }

    #[test]
    fn interrupted_receipt_recovers_but_unowned_paths_are_preserved() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let directory = root.path().join(".skill-manager-test-stage");
        let receipt = root.path().join(".skill-manager-test-cleanup.json");
        fs::create_dir(&directory).unwrap_or_else(|error| unreachable!("{error}"));
        fs::write(directory.join("partial"), "keep until owned")
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(with_directory(root.path(), "test", |_| Ok(())).is_err());
        assert!(directory.join("partial").exists());
        let bytes = serde_json::to_vec(&Receipt {
            version: 1,
            directory: directory.clone(),
        })
        .unwrap_or_else(|error| unreachable!("{error}"));
        fs::write(&receipt, bytes).unwrap_or_else(|error| unreachable!("{error}"));
        with_directory(root.path(), "test", |path| {
            assert!(!path.join("partial").exists());
            Ok(())
        })
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!directory.exists());
        assert!(!receipt.exists());
    }

    #[test]
    fn hostile_receipt_never_deletes_an_unrelated_directory() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let outside = root.path().join("unrelated");
        fs::create_dir(&outside).unwrap_or_else(|error| unreachable!("{error}"));
        fs::write(outside.join("keep"), "safe").unwrap_or_else(|error| unreachable!("{error}"));
        let receipt = root.path().join(".skill-manager-test-cleanup.json");
        fs::write(
            &receipt,
            serde_json::to_vec(&Receipt {
                version: 1,
                directory: outside.clone(),
            })
            .unwrap_or_else(|error| unreachable!("{error}")),
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(with_directory(root.path(), "test", |_| Ok(())).is_err());
        assert!(outside.join("keep").exists());
        fs::write(&receipt, "corrupt").unwrap_or_else(|error| unreachable!("{error}"));
        assert!(with_directory(root.path(), "test", |_| Ok(())).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_lasting_handle_retains_a_receipt_for_next_operation() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let mut held = None;
        let result: Result<()> = with_directory(root.path(), "test", |path| {
            let file = path.join("locked");
            fs::write(&file, "bytes").unwrap_or_else(|error| unreachable!("{error}"));
            held = Some(
                std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(&file)
                    .unwrap_or_else(|error| unreachable!("{error}")),
            );
            Err(SkillManagerError::InvalidInput("preparation failed".into()))
        });
        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("cleanup pending"))
        );
        assert!(
            root.path()
                .join(".skill-manager-test-cleanup.json")
                .exists()
        );
        drop(held);
        with_directory(root.path(), "test", |_| Ok(()))
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(fs::read_dir(root.path()).map(Iterator::count).ok(), Some(0));
    }
}
