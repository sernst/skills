//! Bounded retries of individual filesystem primitives, never whole transactions.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use std::fs::{File, Metadata, OpenOptions, Permissions, ReadDir};

const DELAYS: [u64; 5] = [25, 50, 100, 200, 400];

/// Retry an immediately attempted primitive for at most 775ms of backoff.
///
/// # Errors
/// Returns the final original I/O error without changing its OS code.
pub fn retry<T>(operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    retry_with(operation, false, std::thread::sleep)
}

fn retry_with<T>(
    mut operation: impl FnMut() -> io::Result<T>,
    removal: bool,
    mut sleep: impl FnMut(Duration),
) -> io::Result<T> {
    let mut delays = DELAYS.into_iter();
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                if !transient(&error, removal) {
                    return Err(error);
                }
                let Some(delay) = delays.next() else {
                    return Err(error);
                };
                sleep(Duration::from_millis(delay));
            }
        }
    }
}

fn transient(error: &io::Error, removal: bool) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
    ) {
        return true;
    }
    #[cfg(windows)]
    {
        // Windows can report access denied while a sharing handle is closing.
        // Ordinary permission failures still retain their exact error after the bound.
        matches!(error.raw_os_error(), Some(5 | 32 | 33 | 170))
            || (removal && error.raw_os_error() == Some(145))
    }
    #[cfg(not(windows))]
    {
        let _ = removal;
        false
    }
}

macro_rules! path_operation {
    ($name:ident, $result:ty) => {
        #[doc = concat!("Retry `std::fs::", stringify!($name), "` on transient I/O errors.")]
        ///
        /// # Errors
        /// Returns the original final filesystem error.
        pub fn $name(path: impl AsRef<Path>) -> io::Result<$result> {
            retry(|| std::fs::$name(path.as_ref()))
        }
    };
}

path_operation!(create_dir, ());
path_operation!(create_dir_all, ());
path_operation!(metadata, Metadata);
path_operation!(symlink_metadata, Metadata);
path_operation!(canonicalize, PathBuf);
path_operation!(read_link, PathBuf);
path_operation!(read_dir, ReadDir);

/// Retry a rename under its caller's transaction lock.
///
/// # Errors
/// Returns the original final error; the caller reconciles transaction state.
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    retry(|| std::fs::rename(from.as_ref(), to.as_ref()))
}

macro_rules! remove_operation {
    ($name:ident) => {
        #[doc = concat!("Retry `std::fs::", stringify!($name), "`, including Windows pending deletion.")]
        ///
        /// # Errors
        /// Returns the original final error. Missing paths are not implicitly success.
        pub fn $name(path: impl AsRef<Path>) -> io::Result<()> {
            retry_with(|| std::fs::$name(path.as_ref()), true, std::thread::sleep)
        }
    };
}
remove_operation!(remove_dir);
remove_operation!(remove_dir_all);
remove_operation!(remove_file);

/// Open a file with bounded transient retries.
///
/// # Errors
/// Returns the original final open error.
pub fn open(path: impl AsRef<Path>) -> io::Result<File> {
    retry(|| File::open(path.as_ref()))
}

/// Read a file without replaying successful reads.
///
/// # Errors
/// Returns the original final read or open error.
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let mut file = Retrying(open(path)?);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(original_error)?;
    Ok(bytes)
}

/// Read UTF-8 text with bounded retries.
///
/// # Errors
/// Returns an I/O error or invalid UTF-8 error.
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    String::from_utf8(read(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Write bytes, retrying individual writes without replaying already-written bytes.
///
/// # Errors
/// Returns the original final open or write error.
pub fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    let file = retry(|| File::create(path.as_ref()))?;
    Retrying(file)
        .write_all(bytes.as_ref())
        .map_err(original_error)
}

/// Copy bytes without restarting a partially completed copy.
///
/// # Errors
/// Returns the original final I/O error.
pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    let source = open(from.as_ref())?;
    let permissions = retry(|| source.metadata())?.permissions();
    let destination = retry(|| File::create(to.as_ref()))?;
    let length =
        io::copy(&mut Retrying(source), &mut Retrying(&destination)).map_err(original_error)?;
    retry(|| destination.set_permissions(permissions.clone()))?;
    Ok(length)
}

/// Set permissions with bounded retries.
///
/// # Errors
/// Returns the original final error.
pub fn set_permissions(path: impl AsRef<Path>, permissions: &Permissions) -> io::Result<()> {
    retry(|| std::fs::set_permissions(path.as_ref(), permissions.clone()))
}

/// Atomically replace a file, retaining the same staged file across persist retries.
///
/// # Errors
/// Returns the final I/O error, including an explicit staging path if cleanup fails.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file has no parent"))?;
    create_dir_all(parent)?;
    let temporary = retry(|| tempfile::NamedTempFile::new_in(parent))?;
    let staged_path = temporary.path().to_path_buf();
    let mut staged = Some(temporary);
    let prepared = {
        let file = staged
            .as_mut()
            .ok_or_else(|| io::Error::other("missing staged file"))?;
        Retrying(file.as_file_mut())
            .write_all(bytes)
            .map_err(original_error)
            .and_then(|()| retry(|| file.as_file().sync_all()))
    };
    let result = prepared.and_then(|()| {
        retry(|| {
            let file = staged
                .take()
                .ok_or_else(|| io::Error::other("missing staged file"))?;
            match file.persist(path) {
                Ok(_) => Ok(()),
                Err(error) => {
                    staged = Some(error.file);
                    Err(error.error)
                }
            }
        })
    });
    if let Some(file) = staged {
        return cleanup_failed_write(file, &staged_path, result, |path| remove_file(path));
    }
    result
}

fn cleanup_failed_write(
    mut file: tempfile::NamedTempFile,
    path: &Path,
    result: io::Result<()>,
    remove: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    // This transfer cannot fail, unlike keep(). Drop closes the file handle
    // without silently deleting the path, which remains explicitly owned here.
    file.disable_cleanup(true);
    drop(file);
    if let Err(cleanup) = remove(path) {
        return Err(io::Error::new(
            cleanup.kind(),
            format!(
                "{}; temporary-file cleanup pending at {}: {cleanup}",
                result
                    .err()
                    .map_or_else(|| "write failed".to_owned(), |error| error.to_string()),
                path.display()
            ),
        ));
    }
    result
}

/// Adapter which retries only a failed read/write primitive, preserving stream position.
pub(crate) struct Retrying<T>(pub(crate) T);

/// Standard stream helpers automatically retry Interrupted. Keep exhaustion
/// non-retryable inside those helpers, retaining the exact original error.
#[derive(Debug)]
struct InterruptedExhausted(io::Error);

impl std::fmt::Display for InterruptedExhausted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for InterruptedExhausted {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

fn stream_error(error: io::Error) -> io::Error {
    if error.kind() == io::ErrorKind::Interrupted {
        io::Error::other(InterruptedExhausted(error))
    } else {
        error
    }
}

fn original_error(error: io::Error) -> io::Error {
    match error.downcast::<InterruptedExhausted>() {
        Ok(InterruptedExhausted(original)) => original,
        Err(error) => error,
    }
}

/// Invocation-scoped scratch directory with bounded cleanup at the final owner drop.
/// Its path comes directly from exclusive temporary-directory creation, never a scan.
#[derive(Debug)]
pub struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    /// Create a disposable system-temp directory without writing manager state.
    ///
    /// # Errors
    /// Returns the original final temporary-directory creation error.
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            path: retry(tempfile::tempdir)?.keep(),
        })
    }

    /// Directory kept alive by this owner.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        if let Err(error) = remove_dir_all(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            // Final Arc ownership can end after command reporting. Keep this on
            // stderr so NDJSON stdout and its final summary remain unchanged.
            eprintln!(
                "Warning: temporary source cleanup pending at {}: {error}; inspect this invocation-owned scratch directory and remove it when the handle is released",
                self.path.display()
            );
        }
    }
}

impl<T: Read> Read for Retrying<T> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        retry(|| self.0.read(buffer)).map_err(stream_error)
    }
}

impl<T: Write> Write for Retrying<T> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        retry(|| self.0.write(buffer)).map_err(stream_error)
    }
    fn flush(&mut self) -> io::Result<()> {
        retry(|| self.0.flush()).map_err(stream_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct InterruptedStream {
        attempts: usize,
        bytes: Vec<u8>,
    }

    impl Read for InterruptedStream {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.attempts += 1;
            if self.attempts == 1 {
                buffer[0] = b'x';
                Ok(1)
            } else {
                Err(io::Error::new(io::ErrorKind::Interrupted, "held read"))
            }
        }
    }

    impl Write for InterruptedStream {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.attempts += 1;
            if self.attempts == 1 {
                self.bytes.push(buffer[0]);
                Ok(1)
            } else {
                Err(io::Error::new(io::ErrorKind::Interrupted, "held write"))
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn interrupted_high_level_streams_stop_after_one_budget_without_replaying_progress() {
        let mut source = InterruptedStream::default();
        let mut bytes = Vec::new();
        let error = Retrying(&mut source)
            .read_to_end(&mut bytes)
            .map_err(original_error)
            .err()
            .unwrap_or_else(|| unreachable!("interrupted read must stop"));
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(error.to_string(), "held read");
        assert_eq!(source.attempts, 7);
        assert_eq!(bytes, b"x");

        let mut destination = InterruptedStream::default();
        let error = Retrying(&mut destination)
            .write_all(b"xy")
            .map_err(original_error)
            .err()
            .unwrap_or_else(|| unreachable!("interrupted write must stop"));
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(destination.attempts, 7);
        assert_eq!(destination.bytes, b"x");

        let mut source = InterruptedStream::default();
        let mut copied = Vec::new();
        let error = io::copy(&mut Retrying(&mut source), &mut copied)
            .map_err(original_error)
            .err()
            .unwrap_or_else(|| unreachable!("interrupted copy must stop"));
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(source.attempts, 7);
        assert_eq!(copied, b"x");
    }

    #[test]
    fn compressed_stream_does_not_restart_exhausted_read_budget() {
        let mut source = InterruptedStream {
            attempts: 1,
            bytes: Vec::new(),
        };
        let mut decoder = flate2::read::GzDecoder::new(Retrying(&mut source));
        let error = io::copy(&mut decoder, &mut Vec::new())
            .err()
            .unwrap_or_else(|| unreachable!("interrupted archive must stop"));
        assert_eq!(error.to_string(), "held read");
        assert_eq!(source.attempts, 7);
    }

    #[test]
    fn failed_atomic_write_retains_original_error_and_exact_owned_cleanup_path() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let file = tempfile::NamedTempFile::new_in(directory.path())
            .unwrap_or_else(|error| unreachable!("{error}"));
        let path = file.path().to_path_buf();
        let result = cleanup_failed_write(
            file,
            &path,
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "original persist failure",
            )),
            |_| Err(io::Error::new(io::ErrorKind::WouldBlock, "held cleanup")),
        );
        let message = result
            .err()
            .unwrap_or_else(|| unreachable!("cleanup must report retained file"))
            .to_string();
        assert!(message.contains("original persist failure"));
        assert!(message.contains("held cleanup"));
        assert!(message.contains(&path.display().to_string()));
        assert!(path.exists(), "drop must not silently remove retained file");
        remove_file(path).unwrap_or_else(|error| unreachable!("{error}"));
    }

    #[test]
    fn first_success_and_permanent_errors_never_sleep() {
        let mut sleeps = Vec::new();
        assert_eq!(
            retry_with(|| Ok(7), false, |d| sleeps.push(d)).ok(),
            Some(7)
        );
        for kind in [
            io::ErrorKind::NotFound,
            io::ErrorKind::AlreadyExists,
            io::ErrorKind::InvalidInput,
        ] {
            let result: io::Result<()> = retry_with(|| Err(kind.into()), false, |d| sleeps.push(d));
            assert_eq!(result.err().map(|e| e.kind()), Some(kind));
        }
        assert!(sleeps.is_empty());
    }

    #[test]
    fn retries_are_bounded_and_preserve_the_final_error() {
        let mut attempts = 0;
        let mut sleeps = Vec::new();
        let result: io::Result<()> = retry_with(
            || {
                attempts += 1;
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("attempt {attempts}"),
                ))
            },
            false,
            |d| sleeps.push(d),
        );
        assert_eq!(attempts, 6);
        assert_eq!(sleeps.iter().sum::<Duration>(), Duration::from_millis(775));
        assert_eq!(
            result.err().map(|e| e.to_string()).as_deref(),
            Some("attempt 6")
        );
    }

    #[test]
    fn transient_then_success_stops_immediately() {
        let mut attempts = 0;
        let mut sleeps = Vec::new();
        let result = retry_with(
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(io::ErrorKind::Interrupted.into())
                } else {
                    Ok(42)
                }
            },
            false,
            |d| sleeps.push(d),
        );
        assert_eq!(result.ok(), Some(42));
        assert_eq!(
            sleeps,
            [Duration::from_millis(25), Duration::from_millis(50)]
        );
    }
}
