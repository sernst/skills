//! Mocked GitHub/cache behavior and archive-safety contracts.

#![allow(
    clippy::expect_used,
    reason = "Archive fixture construction failures are unrecoverable test harness failures."
)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use flate2::Compression;
use flate2::write::GzEncoder;
use indexmap::IndexMap;
use skill_manager::cache::{GitHubTransport, materialize_source};
use skill_manager::config::{ConfigRepository, FileConfigRepository};
use skill_manager::domain::{SourceEntry, SourceMode, SourceType};
use skill_manager::error::{Result, SkillManagerError};
use tar::{Builder, EntryType, Header};

struct ArchiveTransport {
    archive: PathBuf,
    downloads: AtomicUsize,
}

impl ArchiveTransport {
    fn new(archive: PathBuf) -> Self {
        Self {
            archive,
            downloads: AtomicUsize::new(0),
        }
    }
}

impl GitHubTransport for ArchiveTransport {
    fn default_branch(&self, _owner: &str, _repo: &str) -> Result<String> {
        Ok("main".into())
    }

    fn download_archive(
        &self,
        _owner: &str,
        _repo: &str,
        _reference: &str,
        destination: &Path,
    ) -> Result<()> {
        self.downloads.fetch_add(1, Ordering::SeqCst);
        fs::copy(&self.archive, destination)
            .map(|_| ())
            .map_err(|error| SkillManagerError::io(destination, error))
    }
}

struct FailingTransport;

impl GitHubTransport for FailingTransport {
    fn default_branch(&self, _owner: &str, _repo: &str) -> Result<String> {
        Ok("main".into())
    }

    fn download_archive(
        &self,
        owner: &str,
        repo: &str,
        _reference: &str,
        _destination: &Path,
    ) -> Result<()> {
        Err(SkillManagerError::GitHub {
            reference: format!("{owner}/{repo}"),
            message: "simulated network failure".into(),
        })
    }
}

fn github_source(id: &str, ttl: i64) -> SourceEntry {
    SourceEntry {
        id: id.into(),
        source_type: SourceType::GitHub,
        mode: SourceMode::Collection,
        name: "remote".into(),
        label: "Remote".into(),
        exclude: Vec::new(),
        cache_ttl_hours: Some(ttl),
        path: None,
        owner: Some("owner".into()),
        repo: Some("repo".into()),
        r#ref: None,
        repo_path: None,
        extra: IndexMap::new(),
    }
}

fn write_regular_archive(path: &Path, body: &[u8]) {
    let output = fs::File::create(path).expect("create archive");
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);
    let mut header = Header::new_gnu();
    header.set_size(u64::try_from(body.len()).expect("fixture length"));
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "repo-root/alpha/SKILL.md", body)
        .expect("append regular file");
    let encoder = archive.into_inner().expect("finish tar");
    encoder.finish().expect("finish gzip");
}

fn write_link_archive(path: &Path) {
    let output = fs::File::create(path).expect("create archive");
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_cksum();
    archive
        .append_link(
            &mut header,
            "repo-root/alpha/SKILL.md",
            Path::new("../../outside"),
        )
        .expect("append link");
    let encoder = archive.into_inner().expect("finish tar");
    encoder.finish().expect("finish gzip");
}

fn write_raw_header_archive(path: &Path, name: &str, size: u64) {
    assert!(name.len() <= 100, "raw fixture name must fit a tar header");
    let output = fs::File::create(path).expect("create archive");
    let mut encoder = GzEncoder::new(output, Compression::default());
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_size(size);
    header.set_mode(0o644);
    header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
    header.set_cksum();
    std::io::Write::write_all(&mut encoder, header.as_bytes()).expect("write raw header");
    encoder.finish().expect("finish gzip");
}

fn write_long_path_archive(path: &Path) {
    let output = fs::File::create(path).expect("create archive");
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);
    let long_path = format!("repo-root/{}/SKILL.md", "a".repeat(4_100));
    let mut header = Header::new_gnu();
    header.set_size(0);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, long_path, io::empty())
        .expect("append long path");
    let encoder = archive.into_inner().expect("finish tar");
    encoder.finish().expect("finish gzip");
}

fn write_many_entries_archive(path: &Path, entries: usize) {
    let output = fs::File::create(path).expect("create archive");
    let encoder = GzEncoder::new(output, Compression::fast());
    let mut archive = Builder::new(encoder);
    for _ in 0..entries {
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, "repo-root", io::empty())
            .expect("append directory entry");
    }
    let encoder = archive.into_inner().expect("finish tar");
    encoder.finish().expect("finish gzip");
}

#[test]
fn remote_cache_is_reused_and_failed_refresh_preserves_it() {
    let home = tempfile::tempdir().expect("temporary home");
    let archive_path = home.path().join("source.tar.gz");
    write_regular_archive(&archive_path, b"# cached");
    let repository = FileConfigRepository::new(home.path().to_path_buf());
    let transport = ArchiveTransport::new(archive_path);
    let source = github_source("src_cached", 24);

    let initial = materialize_source(&repository, &transport, &source, false, false)
        .expect("initial materialization");
    assert!(initial.from_cache);
    assert_eq!(
        fs::read_to_string(initial.path.join("alpha").join("SKILL.md")).expect("read cached skill"),
        "# cached"
    );
    assert_eq!(transport.downloads.load(Ordering::SeqCst), 1);

    let reused = materialize_source(&repository, &transport, &source, false, false)
        .expect("reuse materialization");
    assert!(reused.from_cache);
    assert_eq!(transport.downloads.load(Ordering::SeqCst), 1);

    let refresh = materialize_source(&repository, &FailingTransport, &source, true, false);
    assert!(refresh.is_err());
    assert_eq!(
        fs::read_to_string(
            repository
                .cache_root()
                .join("src_cached/content/alpha/SKILL.md")
        )
        .expect("old cache survives"),
        "# cached"
    );
}

#[test]
fn zero_ttl_refreshes_and_dry_run_uses_only_temporary_storage() {
    let home = tempfile::tempdir().expect("temporary home");
    let archive_path = home.path().join("source.tar.gz");
    write_regular_archive(&archive_path, b"# remote");
    let repository = FileConfigRepository::new(home.path().to_path_buf());
    let transport = ArchiveTransport::new(archive_path);

    let zero_ttl = github_source("src_zero", 0);
    materialize_source(&repository, &transport, &zero_ttl, false, false)
        .expect("initial zero-TTL materialization");
    materialize_source(&repository, &transport, &zero_ttl, false, false).expect("zero TTL refresh");
    assert_eq!(transport.downloads.load(Ordering::SeqCst), 2);

    let dry = github_source("src_dry", 24);
    let resolved = materialize_source(&repository, &transport, &dry, false, true)
        .expect("dry-run materialization");
    assert!(resolved.temporary.is_some());
    assert!(resolved.path.join("alpha").join("SKILL.md").is_file());
    assert!(!repository.cache_root().join("src_dry").exists());
}

#[test]
fn negative_ttl_and_archive_links_are_rejected() {
    let home = tempfile::tempdir().expect("temporary home");
    let regular = home.path().join("regular.tar.gz");
    write_regular_archive(&regular, b"# remote");
    let repository = FileConfigRepository::new(home.path().to_path_buf());
    let regular_transport = ArchiveTransport::new(regular);

    let invalid_ttl = materialize_source(
        &repository,
        &regular_transport,
        &github_source("negative", -1),
        false,
        false,
    );
    assert!(
        invalid_ttl
            .expect_err("negative TTL must fail")
            .to_string()
            .contains("negative cache TTL")
    );
    assert_eq!(regular_transport.downloads.load(Ordering::SeqCst), 0);

    let linked = home.path().join("linked.tar.gz");
    write_link_archive(&linked);
    let linked_transport = ArchiveTransport::new(linked);
    let result = materialize_source(
        &repository,
        &linked_transport,
        &github_source("linked", 24),
        false,
        false,
    );
    assert!(
        result
            .expect_err("link archive must fail")
            .to_string()
            .contains("link or special entry")
    );
    assert!(!repository.cache_root().join("linked/content").exists());
}

#[test]
fn absolute_traversal_oversized_and_long_archive_paths_are_rejected() {
    let home = tempfile::tempdir().expect("temporary home");
    let repository = FileConfigRepository::new(home.path().to_path_buf());
    let fixtures = [
        ("absolute", "/outside", 0_u64),
        ("traversal", "repo-root/../../outside", 0),
        ("oversized", "repo-root/huge.bin", 256 * 1024 * 1024 + 1),
    ];
    for (id, name, size) in fixtures {
        let archive = home.path().join(format!("{id}.tar.gz"));
        write_raw_header_archive(&archive, name, size);
        let result = materialize_source(
            &repository,
            &ArchiveTransport::new(archive),
            &github_source(id, 24),
            false,
            false,
        );
        assert!(result.is_err(), "{id} archive entry must be rejected");
        assert!(!repository.cache_root().join(id).join("content").exists());
    }

    let long = home.path().join("long.tar.gz");
    write_long_path_archive(&long);
    let result = materialize_source(
        &repository,
        &ArchiveTransport::new(long),
        &github_source("long", 24),
        false,
        false,
    );
    assert!(result.is_err(), "over-limit archive path must be rejected");
}

#[test]
fn archive_entry_count_limit_is_enforced() {
    let home = tempfile::tempdir().expect("temporary home");
    let archive = home.path().join("many.tar.gz");
    write_many_entries_archive(&archive, 100_001);
    let repository = FileConfigRepository::new(home.path().to_path_buf());
    let result = materialize_source(
        &repository,
        &ArchiveTransport::new(archive),
        &github_source("many", 24),
        false,
        false,
    );
    assert!(
        result
            .expect_err("entry overflow must fail")
            .to_string()
            .contains("100000 entries")
    );
}
