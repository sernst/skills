//! Proves `--home` relocates every command's home resolution away from
//! `HOME`/`USERPROFILE`/`SKILL_MANAGER_HOME`, and that it outranks those
//! environment variables. This is the regression guard for the incident that
//! motivated the `--home` flag: a prior manual validation run touched the
//! real user home and had to be undone by hand.
//!
//! The universal guarantee lives in two tests that share one enumeration of
//! the clap command tree:
//!
//! - `every_command_leaf_is_known_or_explicitly_skipped` is the fail-closed
//!   structural check: every LEAF clap subcommand (a command with no
//!   subcommands of its own — a dispatch-only group like `source` or
//!   `target` is traversed into, never treated as a leaf itself) must
//!   either have synthetic operands registered for it in `known_leaf_args`,
//!   or a justified entry in `SKIP_LIST`. A newly added subcommand that is
//!   neither fails this test immediately, naming itself, until a future
//!   change handles it deliberately.
//! - `no_command_leaf_ever_reads_or_writes_the_decoy_home` is the
//!   behavioral check: it actually invokes every non-skipped leaf (plus one
//!   deliberately added non-leaf invocation, bare `configs`; see
//!   `additional_invocations`) against a decoy directory standing in for
//!   the OS home, and proves the decoy is never read (no sentinel marker
//!   leaks into stdout/stderr) or written (a byte-for-byte snapshot of the
//!   decoy is identical before and after).

#![allow(
    clippy::expect_used,
    reason = "Test fixture construction and missing test binaries are unrecoverable harness failures."
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use clap::CommandFactory;
use serde_json::json;
use skill_manager::cli::Cli;
use skill_manager::config::{Config, resolved_targets};
use tempfile::TempDir;

/// Directory entries recursively, or an empty vector for a missing directory
/// (a directory that was never created is trivially "empty" for our purposes).
fn entries(path: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(read) = fs::read_dir(path) else {
        return found;
    };
    for entry in read {
        let entry = entry.expect("read directory entry");
        let child = entry.path();
        found.push(child.clone());
        if child.is_dir() {
            found.extend(entries(&child));
        }
    }
    found
}

/// A recursive, path-and-content snapshot of a directory tree: `None` for a
/// directory, `Some(bytes)` for a file's exact contents, keyed by the path
/// relative to `root`. Two snapshots taken before and after a command runs
/// are compared for exact equality, which is a stronger and simpler
/// assertion than separately enumerating every possible kind of unwanted
/// read or write (renamed files, truncated files, new empty directories,
/// touched mtimes with unchanged content are all still just "not equal" or
/// correctly "equal").
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut map = BTreeMap::new();
    snapshot_into(root, root, &mut map);
    map
}

fn snapshot_into(root: &Path, current: &Path, map: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
    let Ok(read) = fs::read_dir(current) else {
        return;
    };
    for entry in read {
        let entry = entry.expect("read decoy directory entry");
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .expect("decoy entry nested under decoy root")
            .to_path_buf();
        if path.is_dir() {
            map.insert(relative, None);
            snapshot_into(root, &path, map);
        } else {
            let bytes = fs::read(&path).expect("read decoy file contents");
            map.insert(relative, Some(bytes));
        }
    }
}

/// Distinguishable marker text seeded into the decoy standing in for the OS
/// home; any command output containing this string, or any change to the
/// decoy's snapshot, proves a command read from or wrote to the decoy
/// instead of (or in addition to) `--home`.
const DECOY_MARKER: &str = "DECOY-HOME-LEAK-MARKER-0f3c8a";

/// Seed the decoy directory standing in for the OS home with recognizable,
/// structurally valid content that a command could plausibly read or
/// clobber if it resolved the OS home instead of `--home`:
///
/// - `.skill-manager/config.json` with one source whose name/label is
///   [`DECOY_MARKER`], so a read-side bug (e.g. `configs`/`status`
///   rendering the stored configuration) leaks the marker into output.
/// - one skill directory containing `SKILL.md` with [`DECOY_MARKER`] as its
///   content, under every built-in target's real relative path (currently
///   `.claude/skills`, `.agents/skills`, and `.gemini/antigravity/skills`),
///   so a read-side bug in deployment discovery (e.g. `status`/`load`/
///   `resolve` scanning already-deployed skills) leaks it the same way.
///   The relative paths are derived from [`resolved_targets`] rather than
///   hardcoded, so seeding stays in sync with the real built-in layout
///   instead of silently drifting from it.
fn seed_decoy(decoy: &Path) {
    let storage_root = decoy.join(".skill-manager");
    fs::create_dir_all(&storage_root).expect("create decoy storage root");
    // The path need not exist: local source locations are lexically
    // normalized when canonicalization fails, so a marker path under the
    // decoy is sufficient to build a structurally valid source.
    let marker_source_path = decoy.join("marker-source-should-never-be-read");
    let config = json!({
        "schema_version": 2,
        "sources": [{
            "id": "decoy-marker-source",
            "type": "local",
            "mode": "collection",
            "name": DECOY_MARKER,
            "label": DECOY_MARKER,
            "path": marker_source_path,
        }],
        "targets": {},
        "legacy_target_overrides": {},
        "builtins": {},
        "exclude": []
    });
    let bytes = serde_json::to_vec_pretty(&config).expect("serialize decoy marker config");
    fs::write(storage_root.join("config.json"), bytes).expect("write decoy marker config");

    for target in resolved_targets(&Config::default(), decoy).values() {
        let skill_dir = target.path.join("marker-skill");
        fs::create_dir_all(&skill_dir).expect("create decoy marker skill directory");
        fs::write(skill_dir.join("SKILL.md"), DECOY_MARKER).expect("write decoy marker skill");
    }
}

/// Build an invocation with the OS-home environment seams pointed at
/// `decoy` and the real scratch home passed explicitly via `--home`.
///
/// `SKILL_MANAGER_HOME` is unset outright (not merely pointed elsewhere):
/// the `--home` > `SKILL_MANAGER_HOME` > OS-home precedence is proven
/// separately by
/// `home_flag_outranks_skill_manager_home_env_var_which_outranks_os_home`
/// below. This helper exists solely to prove the OS home itself is never
/// touched, so any bug that skipped `--home` entirely must fall all the way
/// through to `home::home_dir()`, which is what actually resolves the OS
/// home. Checked against the vendored `home` crate source (`home-0.5.12`):
/// on Windows it consults only `USERPROFILE` (falling back to the CRT/
/// `SHGetKnownFolderPath` profile lookup if that is unset or empty —
/// `HOMEDRIVE`/`HOMEPATH` are not consulted at all); on Unix it consults
/// only `HOME` (via `std::env::home_dir`). Setting both `HOME` and
/// `USERPROFILE` to the decoy therefore covers home resolution on every
/// platform this test suite runs on.
fn command_with_decoy_home(decoy: &Path, home: &Path, cwd: &Path) -> Command {
    let mut command = Command::cargo_bin("skill-manager").expect("test binary");
    command
        .current_dir(cwd)
        .env("HOME", decoy)
        .env("USERPROFILE", decoy)
        .env_remove("SKILL_MANAGER_HOME")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env("NO_COLOR", "1")
        .args(["--home"])
        .arg(home)
        .arg("--no-input");
    command
}

/// Recursively enumerate every LEAF subcommand from the clap command tree: a
/// leaf has no subcommands of its own. A dispatch-only group (`source`,
/// `target`, `configs`) is traversed into rather than treated as a leaf
/// itself, so this covers e.g. `source add`/`source remove` individually,
/// not merely `source`. Deriving this from the clap tree, rather than a
/// hand-maintained list, is what makes coverage automatic for subcommands
/// added in the future: a new leaf shows up here without anyone needing to
/// remember to add it.
fn command_leaves() -> Vec<Vec<String>> {
    let mut leaves = Vec::new();
    let mut prefix = Vec::new();
    collect_leaves(&Cli::command(), &mut prefix, &mut leaves);
    leaves
}

fn collect_leaves(
    command: &clap::Command,
    prefix: &mut Vec<String>,
    leaves: &mut Vec<Vec<String>>,
) {
    let mut subcommands = command.get_subcommands().peekable();
    if subcommands.peek().is_none() {
        leaves.push(prefix.clone());
        return;
    }
    for subcommand in subcommands {
        prefix.push(subcommand.get_name().to_owned());
        collect_leaves(subcommand, prefix, leaves);
        prefix.pop();
    }
}

/// Leaves that cannot be safely or meaningfully exercised by this contract,
/// each entry needing its own justification. Kept empty today: every
/// current leaf can be driven with local, network-free synthetic operands
/// (see `known_leaf_args`). The list exists so a future leaf that genuinely
/// cannot be exercised (for example, one that unavoidably calls out to the
/// network with no offline mode) has an explicit, auditable, commented home
/// instead of being silently dropped from coverage. Every entry here is
/// checked against the live clap tree by
/// `every_command_leaf_is_known_or_explicitly_skipped`, which fails if an
/// entry no longer matches any real leaf (a stale skip is exactly as
/// dangerous as a missing one: both hide a command from this guarantee).
const SKIP_LIST: &[&[&str]] = &[];

fn is_skipped(leaf: &[String]) -> bool {
    SKIP_LIST
        .iter()
        .any(|skip| skip.len() == leaf.len() && skip.iter().zip(leaf).all(|(a, b)| *a == b))
}

/// Synthetic, safe operands for `leaf` (a full command path such as
/// `["source", "add"]`) that let it run past clap's own argument parsing
/// and reach actual home-resolution code, without touching the network or
/// anything outside `home`/the process working directory. Returns `None`
/// for a leaf this function does not yet know about, which is the fail-
/// closed signal consumed by `every_command_leaf_is_known_or_explicitly_skipped`.
///
/// Every operand here is deliberately local and non-existent-by-name (a
/// path under `home`, or a source/target name that is not configured), so
/// each invocation legitimately fails at the business-logic layer (source
/// not found, target not found, and so on) rather than succeeding — that is
/// fine and expected. What matters for this contract is that the command
/// reaches and exercises home resolution at all, not that it succeeds.
/// Target path operands are plain relative names, never joined with `home`:
/// `normalize_target_template` (see `config.rs`) rejects absolute target
/// path templates outright, so an absolute path would merely prove clap
/// parsing succeeded without ever reaching that validation.
fn known_leaf_args(leaf: &[String], home: &Path) -> Option<Vec<String>> {
    let parts: Vec<&str> = leaf.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["load" | "update" | "remove"] => Some(vec!["--dry-run".to_owned()]),
        ["import"] => Some(vec!["nonexistent-skill".to_owned(), "--dry-run".to_owned()]),
        ["copy"] => Some(vec![
            "nonexistent-source".to_owned(),
            home.join("copy-destination").display().to_string(),
            "--dry-run".to_owned(),
        ]),
        ["resolve"] => Some(vec!["nonexistent-skill".to_owned()]),
        ["status"] | ["source" | "target", "list"] => Some(Vec::new()),
        ["source", "add"] => Some(vec![
            home.join("synthetic-source").display().to_string(),
            "synthetic-source-name".to_owned(),
        ]),
        ["source", "remove" | "swap"] => Some(vec!["nonexistent-source".to_owned()]),
        ["source", "update"] => Some(vec![
            "nonexistent-source".to_owned(),
            "--label".to_owned(),
            "Synthetic Label".to_owned(),
        ]),
        ["source", "locate"] => Some(vec![
            "nonexistent-source".to_owned(),
            home.join("synthetic-location").display().to_string(),
        ]),
        ["source", "alternate"] => Some(vec![
            "nonexistent-source".to_owned(),
            home.join("synthetic-alternate").display().to_string(),
        ]),
        ["target", "add"] => Some(vec![
            "synthetic-target".to_owned(),
            "synthetic-target-dir".to_owned(),
        ]),
        ["target", "enable" | "disable" | "remove"] => Some(vec!["nonexistent-target".to_owned()]),
        ["target", "set-path"] => Some(vec![
            "nonexistent-target".to_owned(),
            "synthetic-target-dir-2".to_owned(),
        ]),
        ["configs", "reset" | "restore"] => Some(vec!["--yes".to_owned()]),
        ["generate-completions"] => Some(vec!["--shell".to_owned(), "bash".to_owned()]),
        ["generate-man"] => Some(vec![
            "--output".to_owned(),
            home.join("man")
                .join("skill-manager.1")
                .display()
                .to_string(),
        ]),
        _ => None,
    }
}

/// One deliberately added invocation that is not a clap LEAF (bare
/// `configs` dispatches into an optional subcommand, so `configs` itself
/// has subcommand children in the tree and is excluded from
/// `command_leaves`) but is behaviorally distinct from every leaf, and was
/// the concrete deficiency motivating this rewrite: bare `configs` reads
/// and renders the stored configuration, so a read-side bypass there is
/// exactly as dangerous as one in `status` (already a leaf).
///
/// Bare invocation of the root command itself is deliberately NOT added
/// here: `main.rs` maps an absent subcommand to `Command::Status` before
/// dispatch even happens, so it is provably identical to the already-
/// covered `status` leaf and adding it would just be a redundant iteration.
fn additional_invocations() -> Vec<(Vec<String>, Vec<String>)> {
    vec![(vec!["configs".to_owned()], Vec::new())]
}

/// `generate-completions` and `generate-man` are handled entirely in
/// `main.rs` before the repository is constructed, so they never create
/// `<home>/.skill-manager` — that is expected, not a gap in coverage. Every
/// other command reaches the shared `FileConfigRepository::load` seam
/// before dispatching to its own logic (see `Application::run` in
/// `app.rs`), which creates `<home>/.skill-manager/locks/` on first touch
/// regardless of whether the command subsequently succeeds.
fn reaches_repository(label: &str) -> bool {
    !matches!(label, "generate-completions" | "generate-man")
}

/// Assert one invocation's isolation properties: the decoy is never read
/// (no marker in output) or written (byte-identical snapshot), the command
/// actually reached home resolution instead of merely failing clap's
/// argument parsing, and (for the two generation commands, which have no
/// legitimate reason to fail with these minimal arguments) that it
/// succeeded.
fn assert_isolated(
    label: &str,
    output: &Output,
    decoy: &Path,
    before: &BTreeMap<PathBuf, Option<Vec<u8>>>,
    home: &Path,
    cwd: &Path,
) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if matches!(label, "generate-completions" | "generate-man") {
        assert!(
            output.status.success(),
            "`{label}` was expected to succeed with its minimal extra args (it runs entirely \
             before home resolution); stderr: {stderr}"
        );
    }

    assert!(
        !stdout.contains(DECOY_MARKER) && !stderr.contains(DECOY_MARKER),
        "`{label}` leaked the decoy's marker into its output, proving it read the decoy \
         instead of (or in addition to) --home:\nstdout: {stdout}\nstderr: {stderr}"
    );

    let clap_rejected_before_running =
        output.status.code() == Some(2) && stderr.contains("required arguments were not provided");
    assert!(
        !clap_rejected_before_running,
        "`{label}` was rejected by clap before any repository code ran (missing required \
         operand, exit code 2). This leaf needs safe, no-op synthetic operands registered in \
         `known_leaf_args` in tests/home_isolation_contract.rs so this iteration actually \
         exercises home resolution instead of failing at argument parsing. stderr:\n{stderr}"
    );

    if reaches_repository(label) {
        let home_state = home.join(".skill-manager");
        assert!(
            home_state.exists(),
            "`{label}` reaches the repository but left no observable state under --home ({}); \
             expected `.skill-manager` to be created there",
            home_state.display()
        );
    }

    let after = snapshot(decoy);
    assert_eq!(
        &after, before,
        "`{label}` changed the decoy directory standing in for the OS home; the decoy must \
         never be read or written when --home is supplied"
    );

    let cwd_entries = entries(cwd);
    assert!(
        cwd_entries.is_empty(),
        "`{label}` wrote outside --home into the working directory: {cwd_entries:?}"
    );
}

/// Fail-closed structural guarantee: every leaf enumerated from the live
/// clap tree must be either registered in `known_leaf_args` (meaning it
/// will actually be exercised below) or explicitly, non-stale-ly present in
/// `SKIP_LIST`. A subcommand added to the CLI in the future — at any
/// nesting depth — that is neither trips this test immediately, naming
/// itself, until a human deliberately handles it one way or the other.
#[test]
fn every_command_leaf_is_known_or_explicitly_skipped() {
    let leaves = command_leaves();
    assert!(
        !leaves.is_empty(),
        "clap command tree enumeration produced no leaves; enumeration is broken"
    );

    let placeholder_home = PathBuf::from("placeholder-home-for-structural-check-only");
    for leaf in &leaves {
        if is_skipped(leaf) {
            continue;
        }
        assert!(
            known_leaf_args(leaf, &placeholder_home).is_some(),
            "command leaf `{}` is new or otherwise unhandled by this contract test: register \
             synthetic operands for it in `known_leaf_args`, or a justified entry in \
             `SKIP_LIST`, in tests/home_isolation_contract.rs. Until then this leaf's home \
             isolation is NOT verified by this test.",
            leaf.join(" ")
        );
    }

    for skip in SKIP_LIST {
        let skip_path: Vec<String> = skip.iter().map(|part| (*part).to_owned()).collect();
        assert!(
            leaves.contains(&skip_path),
            "SKIP_LIST entry `{}` no longer matches any command leaf in the clap tree; remove \
             the stale entry from tests/home_isolation_contract.rs",
            skip.join(" ")
        );
    }
}

/// Behavioral guarantee: actually invoke every non-skipped leaf (plus the
/// deliberately added non-leaf `configs` invocation) against a decoy
/// standing in for the OS home, and prove the decoy is never read or
/// written when `--home` is supplied.
#[test]
fn no_command_leaf_ever_reads_or_writes_the_decoy_home() {
    let leaves = command_leaves();
    let mut exercised = 0_usize;

    for leaf in &leaves {
        if is_skipped(leaf) {
            continue;
        }
        let decoy = TempDir::new().expect("decoy temp dir");
        let home = TempDir::new().expect("home temp dir");
        let cwd = TempDir::new().expect("cwd temp dir");
        seed_decoy(decoy.path());
        let before = snapshot(decoy.path());

        let args = known_leaf_args(leaf, home.path()).unwrap_or_else(|| {
            unreachable!(
                "leaf `{}` has no known args even though \
                 `every_command_leaf_is_known_or_explicitly_skipped` should have failed first",
                leaf.join(" ")
            )
        });
        let mut command = command_with_decoy_home(decoy.path(), home.path(), cwd.path());
        for part in leaf {
            command.arg(part);
        }
        for arg in &args {
            command.arg(arg);
        }
        let output = command.output().expect("run command leaf");
        exercised += 1;

        assert_isolated(
            &leaf.join(" "),
            &output,
            decoy.path(),
            &before,
            home.path(),
            cwd.path(),
        );
    }

    for (path, args) in additional_invocations() {
        let decoy = TempDir::new().expect("decoy temp dir");
        let home = TempDir::new().expect("home temp dir");
        let cwd = TempDir::new().expect("cwd temp dir");
        seed_decoy(decoy.path());
        let before = snapshot(decoy.path());

        let mut command = command_with_decoy_home(decoy.path(), home.path(), cwd.path());
        for part in &path {
            command.arg(part);
        }
        for arg in &args {
            command.arg(arg);
        }
        let output = command.output().expect("run additional invocation");
        exercised += 1;

        assert_isolated(
            &path.join(" "),
            &output,
            decoy.path(),
            &before,
            home.path(),
            cwd.path(),
        );
    }

    assert!(
        exercised > 0,
        "no command invocations were exercised; enumeration or skip-list logic is broken"
    );
}

#[test]
fn home_flag_outranks_skill_manager_home_env_var_which_outranks_os_home() {
    // `--home` beats `SKILL_MANAGER_HOME`: point the env var at one temp dir
    // and `--home` at another; state must land under `--home`'s directory
    // only.
    let env_home = TempDir::new().expect("env override temp dir");
    let flag_home = TempDir::new().expect("flag override temp dir");
    let cwd = TempDir::new().expect("cwd temp dir");

    let mut command = Command::cargo_bin("skill-manager").expect("test binary");
    command
        .current_dir(cwd.path())
        .env("SKILL_MANAGER_HOME", env_home.path())
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env("NO_COLOR", "1")
        .args(["--home"])
        .arg(flag_home.path())
        .args(["--no-input", "status"]);
    let _output = command.output().expect("run status");

    assert!(
        flag_home.path().join(".skill-manager").exists(),
        "--home did not receive the configuration store"
    );
    assert!(
        entries(env_home.path()).is_empty(),
        "SKILL_MANAGER_HOME was used even though --home was also supplied: {:?}",
        entries(env_home.path())
    );

    // `SKILL_MANAGER_HOME` beats the OS home: without `--home`, wire only the
    // env var plus a sentinel HOME/USERPROFILE and confirm state lands under
    // the env var's directory, not the sentinel.
    let sentinel = TempDir::new().expect("sentinel temp dir");
    let env_only_home = TempDir::new().expect("env-only temp dir");
    let cwd2 = TempDir::new().expect("second cwd temp dir");

    let mut env_only_command = Command::cargo_bin("skill-manager").expect("test binary");
    env_only_command
        .current_dir(cwd2.path())
        .env("HOME", sentinel.path())
        .env("USERPROFILE", sentinel.path())
        .env("SKILL_MANAGER_HOME", env_only_home.path())
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env("NO_COLOR", "1")
        .args(["--no-input", "status"]);
    let _output = env_only_command.output().expect("run status");

    assert!(
        env_only_home.path().join(".skill-manager").exists(),
        "SKILL_MANAGER_HOME did not receive the configuration store"
    );
    assert!(
        entries(sentinel.path()).is_empty(),
        "the OS home sentinel was used even though SKILL_MANAGER_HOME was also supplied: {:?}",
        entries(sentinel.path())
    );
}
