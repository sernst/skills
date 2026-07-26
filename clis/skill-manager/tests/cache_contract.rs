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

fn write_scoped_archive_with_link(path: &Path, link_path: &str) {
    let output = fs::File::create(path).expect("create archive");
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);

    let body = b"# selected";
    let mut file_header = Header::new_gnu();
    file_header.set_size(u64::try_from(body.len()).expect("fixture length"));
    file_header.set_mode(0o644);
    file_header.set_cksum();
    archive
        .append_data(
            &mut file_header,
            "repo-root/skills/productivity/alpha/SKILL.md",
            body.as_slice(),
        )
        .expect("append selected file");

    let mut link_header = Header::new_gnu();
    link_header.set_entry_type(EntryType::Symlink);
    link_header.set_size(0);
    link_header.set_mode(0o777);
    link_header.set_cksum();
    archive
        .append_link(
            &mut link_header,
            format!("repo-root/{link_path}"),
            Path::new("../../outside"),
        )
        .expect("append link");

    let encoder = archive.into_inner().expect("finish tar");
    encoder.finish().expect("finish gzip");
}

fn write_scoped_archive_with_pax(path: &Path, entry_type: EntryType) {
    let output = fs::File::create(path).expect("create archive");
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);

    let metadata = b"25 comment=fixture-value\n";
    let mut pax_header = Header::new_gnu();
    pax_header.set_entry_type(entry_type);
    pax_header.set_size(u64::try_from(metadata.len()).expect("fixture length"));
    pax_header.set_mode(0o644);
    pax_header.set_cksum();
    archive
        .append_data(&mut pax_header, "pax_header", metadata.as_slice())
        .expect("append PAX header");

    let body = b"# selected";
    let mut file_header = Header::new_gnu();
    file_header.set_size(u64::try_from(body.len()).expect("fixture length"));
    file_header.set_mode(0o644);
    file_header.set_cksum();
    archive
        .append_data(
            &mut file_header,
            "repo-root/skills/productivity/alpha/SKILL.md",
            body.as_slice(),
        )
        .expect("append selected file");

    let encoder = archive.into_inner().expect("finish tar");
    encoder.finish().expect("finish gzip");
}

fn write_long_global_pax_path_archive(path: &Path) {
    let output = fs::File::create(path).expect("create archive");
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::XGlobalHeader);
    header.set_size(0);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "p".repeat(4_100), io::empty())
        .expect("append long global PAX path");
    let encoder = archive.into_inner().expect("finish tar");
    encoder.finish().expect("finish gzip");
}

fn write_scoped_archive_with_gnu_longname(path: &Path) -> PathBuf {
    let output = fs::File::create(path).expect("create archive");
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);
    let long_name = format!("{}.md", "a".repeat(120));
    let relative = PathBuf::from("alpha").join(&long_name);
    let archive_path = Path::new("repo-root/skills/productivity").join(&relative);
    let body = b"long path";
    let mut header = Header::new_gnu();
    header.set_size(u64::try_from(body.len()).expect("fixture length"));
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, archive_path, body.as_slice())
        .expect("append GNU longname file");
    let encoder = archive.into_inner().expect("finish tar");
    encoder.finish().expect("finish gzip");
    relative
}

fn write_raw_header_archive(path: &Path, name: &str, size: u64) {
    write_raw_typed_header_archive(path, name, size, EntryType::Regular);
}

fn write_raw_typed_header_archive(path: &Path, name: &str, size: u64, entry_type: EntryType) {
    assert!(name.len() <= 100, "raw fixture name must fit a tar header");
    let output = fs::File::create(path).expect("create archive");
    let mut encoder = GzEncoder::new(output, Compression::default());
    let mut header = Header::new_gnu();
    header.set_entry_type(entry_type);
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

fn write_many_entries_with_hidden_metadata(path: &Path) {
    let output = fs::File::create(path).expect("create archive");
    let encoder = GzEncoder::new(output, Compression::fast());
    let mut archive = Builder::new(encoder);
    for _ in 0..99_999 {
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, "repo-root", io::empty())
            .expect("append directory entry");
    }
    let metadata = b"25 comment=fixture-value\n";
    let mut pax_header = Header::new_gnu();
    pax_header.set_entry_type(EntryType::XHeader);
    pax_header.set_size(u64::try_from(metadata.len()).expect("fixture length"));
    pax_header.set_mode(0o644);
    pax_header.set_cksum();
    archive
        .append_data(&mut pax_header, "pax_header", metadata.as_slice())
        .expect("append hidden local PAX header");
    let mut file_header = Header::new_gnu();
    file_header.set_size(0);
    file_header.set_mode(0o644);
    file_header.set_cksum();
    archive
        .append_data(&mut file_header, "repo-root/alpha/SKILL.md", io::empty())
        .expect("append logical file");
    let encoder = archive.into_inner().expect("finish tar");
    encoder.finish().expect("finish gzip");
}

#[test]
fn remote_cache_is_reused_and_failed_refresh_preserves_it() {
    let home = tempfile::tempdir().expect("temporary home");
    let archive_path = home.path().join("source.tar.gz");
    write_regular_archive(&archive_path, b"# cached");
    let repository = FileConfigRepository::new(home.path());
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
    let repository = FileConfigRepository::new(home.path());
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
    let repository = FileConfigRepository::new(home.path());
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
fn scoped_source_ignores_unrelated_link_but_rejects_links_in_or_above_scope() {
    let home = tempfile::tempdir().expect("temporary home");
    let repository = FileConfigRepository::new(home.path());

    let unrelated = home.path().join("unrelated-link.tar.gz");
    write_scoped_archive_with_link(&unrelated, "AGENTS.md");
    let mut source = github_source("scoped", 24);
    source.repo_path = Some("skills/productivity".into());
    let resolved = materialize_source(
        &repository,
        &ArchiveTransport::new(unrelated),
        &source,
        false,
        false,
    )
    .expect("unrelated repository link must not block a scoped source");
    assert_eq!(
        fs::read_to_string(resolved.path.join("alpha/SKILL.md")).expect("read selected skill"),
        "# selected"
    );

    for (id, entry_type) in [
        ("global-pax", EntryType::XGlobalHeader),
        ("local-pax", EntryType::XHeader),
    ] {
        let with_pax = home.path().join(format!("{id}.tar.gz"));
        write_scoped_archive_with_pax(&with_pax, entry_type);
        let mut pax_source = github_source(id, 24);
        pax_source.repo_path = Some("skills/productivity".into());
        let pax_resolved = materialize_source(
            &repository,
            &ArchiveTransport::new(with_pax),
            &pax_source,
            false,
            false,
        )
        .expect("PAX metadata must not be materialized as a source entry");
        assert_eq!(
            fs::read_to_string(pax_resolved.path.join("alpha/SKILL.md"))
                .expect("read PAX archive skill"),
            "# selected"
        );
    }

    let with_longname = home.path().join("gnu-longname.tar.gz");
    let long_relative = write_scoped_archive_with_gnu_longname(&with_longname);
    let mut longname_source = github_source("gnu-longname", 24);
    longname_source.repo_path = Some("skills/productivity".into());
    let longname_resolved = materialize_source(
        &repository,
        &ArchiveTransport::new(with_longname),
        &longname_source,
        false,
        false,
    )
    .expect("bounded GNU longname metadata must resolve normally");
    assert_eq!(
        fs::read_to_string(longname_resolved.path.join(long_relative))
            .expect("read GNU longname file"),
        "long path"
    );

    for (id, link_path) in [
        ("link-inside-scope", "skills/productivity/leak"),
        ("link-above-scope", "skills"),
        ("link-at-archive-root", ""),
    ] {
        let archive = home.path().join(format!("{id}.tar.gz"));
        write_scoped_archive_with_link(&archive, link_path);
        let mut linked_source = github_source(id, 24);
        linked_source.repo_path = Some("skills/productivity".into());
        let result = materialize_source(
            &repository,
            &ArchiveTransport::new(archive),
            &linked_source,
            false,
            false,
        );
        assert!(
            result
                .expect_err("link intersecting selected path must fail")
                .to_string()
                .contains("link or special entry"),
            "{id}"
        );
        assert!(!repository.cache_root().join(id).join("content").exists());
    }
}

#[test]
fn absolute_traversal_oversized_and_long_archive_paths_are_rejected() {
    let home = tempfile::tempdir().expect("temporary home");
    let repository = FileConfigRepository::new(home.path());
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

    for (id, entry_type) in [
        ("global-pax-bomb", EntryType::XGlobalHeader),
        ("local-pax-bomb", EntryType::XHeader),
        ("gnu-longname-bomb", EntryType::GNULongName),
        ("gnu-longlink-bomb", EntryType::GNULongLink),
    ] {
        let metadata_bomb = home.path().join(format!("{id}.tar.gz"));
        write_raw_typed_header_archive(
            &metadata_bomb,
            "metadata_header",
            1024 * 1024 * 1024 + 1,
            entry_type,
        );
        let result = materialize_source(
            &repository,
            &ArchiveTransport::new(metadata_bomb),
            &github_source(id, 24),
            false,
            false,
        );
        assert!(
            result
                .expect_err("expanded metadata limit must fail")
                .to_string()
                .contains("expanded bytes"),
            "{id}"
        );
    }

    let traversal_pax = home.path().join("traversal-pax.tar.gz");
    write_raw_typed_header_archive(
        &traversal_pax,
        "../pax_global_header",
        0,
        EntryType::XGlobalHeader,
    );
    let result = materialize_source(
        &repository,
        &ArchiveTransport::new(traversal_pax),
        &github_source("traversal-pax", 24),
        false,
        false,
    );
    assert!(result.is_err(), "traversing PAX header path must fail");

    let long_pax = home.path().join("long-pax.tar.gz");
    write_long_global_pax_path_archive(&long_pax);
    let result = materialize_source(
        &repository,
        &ArchiveTransport::new(long_pax),
        &github_source("long-pax", 24),
        false,
        false,
    );
    assert!(result.is_err(), "over-limit PAX header path must fail");
}

#[test]
fn archive_entry_count_limit_is_enforced() {
    let home = tempfile::tempdir().expect("temporary home");
    let archive = home.path().join("many.tar.gz");
    write_many_entries_archive(&archive, 100_001);
    let repository = FileConfigRepository::new(home.path());
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

    let hidden = home.path().join("hidden-metadata.tar.gz");
    write_many_entries_with_hidden_metadata(&hidden);
    let result = materialize_source(
        &repository,
        &ArchiveTransport::new(hidden),
        &github_source("hidden-metadata", 24),
        false,
        false,
    );
    assert!(
        result
            .expect_err("hidden metadata must count as an entry")
            .to_string()
            .contains("100000 entries")
    );
}
