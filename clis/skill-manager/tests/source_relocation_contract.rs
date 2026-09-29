//! Relocation selection, authorization, and application-level failure contracts.
#![allow(
    clippy::expect_used,
    reason = "Fixture failures are unrecoverable test setup errors."
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command as Process;
use serde_json::{Value, json};
use skill_manager::app::Application;
use skill_manager::cache::GitHubTransport;
use skill_manager::cli::{Command, SourceAction, SourceArgs, SourceLocateArgs};
use skill_manager::config::{Config, ConfigRepository, FileConfigRepository};
use skill_manager::error::{Result, SkillManagerError};
use skill_manager::event::{Level, Reporter};
use skill_manager::prompt::{Prompt, PromptChoice, PromptOutcome};
use skill_manager::relocation::{NoopRelocationHook, RelocationHook};
use skill_manager::transaction::NoopTransactionHook;

fn physical_tempdir() -> tempfile::TempDir {
    // macOS exposes its physical temporary directory through the linked /var alias.
    let root = skill_manager::config::portable_path(
        &fs::canonicalize(std::env::temp_dir()).expect("physical system temp root"),
    );
    tempfile::tempdir_in(root).expect("scratch home")
}

struct Fixture {
    home: tempfile::TempDir,
    repository: FileConfigRepository,
    source: PathBuf,
    destination: PathBuf,
    config: Config,
    before: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let home = physical_tempdir();
        let repository = FileConfigRepository::new(home.path());
        let source = home.path().join("source");
        let destination = home.path().join("destination");
        for (root, names) in [
            (&source, vec!["alpha", "beta"]),
            (&destination, vec!["alpha", "gamma"]),
        ] {
            for name in names {
                fs::create_dir_all(root.join(name)).expect("skill directory");
                fs::write(
                    root.join(name).join("SKILL.md"),
                    format!(
                        "---\nname: {name}\ndescription: test\n---\n{}",
                        root.display()
                    ),
                )
                .expect("skill content");
            }
        }
        fs::write(source.join("README.md"), "source root stays").expect("source readme");
        fs::write(destination.join("README.md"), "destination root stays")
            .expect("destination readme");
        fs::write(destination.join("alpha/extra.txt"), "prior extra file").expect("extra file");
        let config: Config = serde_json::from_value(json!({
            "schema_version": 2, "exclude": ["alpha", "beta"],
            "sources": [{"id":"physical", "name":"physical", "path":source, "exclude":["alpha", "beta"],
              "label":"Keep label", "custom_metadata":{"keep":true},
              "alternate":{"type":"github", "owner":"owner", "repo":"repo", "ref":"release/one", "branch_default":{"type":"branch", "name":"main"}}}]
        })).expect("configuration");
        repository
            .save(repository.config_path(), &config)
            .expect("persist fixture");
        let before = format!(
            "  {}\n\n",
            serde_json::to_string(&config).expect("serialize config")
        )
        .into_bytes();
        fs::write(repository.config_path(), &before).expect("exact prior config");
        Self {
            home,
            repository,
            source,
            destination,
            config,
            before,
        }
    }
    fn args(&self) -> SourceLocateArgs {
        SourceLocateArgs {
            source: "physical".into(),
            location: self.destination.display().to_string(),
            all: true,
            yes: true,
            ..SourceLocateArgs::default()
        }
    }
    fn run(
        &self,
        args: SourceLocateArgs,
        hook: &dyn RelocationHook,
        prompt: &mut Picker,
        reporter: &mut Recording,
        no_input: bool,
    ) -> Result<()> {
        Application::new(
            &self.repository,
            &NoNetwork,
            prompt,
            reporter,
            &NoopTransactionHook,
            no_input,
            self.home.path().to_path_buf(),
        )
        .with_relocation_hook(hook)
        .run(Command::Source(SourceArgs {
            action: SourceAction::Locate(args),
        }))
        .map(|_| ())
    }
    fn cli(&self) -> Process {
        let mut command = Process::cargo_bin("skill-manager").expect("binary");
        command
            .arg("--home")
            .arg(self.home.path())
            .current_dir(self.home.path());
        command
    }
}

fn image(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    skill_manager::skills::directory_files(root)
        .expect("safe files")
        .into_iter()
        .map(|(name, path)| (PathBuf::from(name), fs::read(path).expect("file bytes")))
        .collect()
}

struct NoNetwork;
impl GitHubTransport for NoNetwork {
    fn default_branch(&self, _: &str, _: &str) -> Result<String> {
        Err(failure("unexpected network"))
    }
    fn validate_branch(&self, _: &str, _: &str, _: &str) -> Result<()> {
        Err(failure("unexpected network"))
    }
    fn download_archive(&self, _: &str, _: &str, _: &str, _: &Path) -> Result<()> {
        Err(failure("unexpected network"))
    }
}
fn failure(message: &str) -> SkillManagerError {
    SkillManagerError::InvalidInput(message.into())
}

#[derive(Default)]
struct Picker {
    selection: Option<Vec<usize>>,
    choices: Vec<PromptChoice>,
    approve: bool,
}
impl Prompt for Picker {
    fn confirm(&mut self, _: &str, default: bool) -> Result<bool> {
        assert!(!default);
        Ok(self.approve)
    }
    fn text(&mut self, _: &str, _: Option<&str>) -> Result<String> {
        Err(failure("unexpected text prompt"))
    }
    fn choose(&mut self, _: &str, _: &[String]) -> Result<usize> {
        Err(failure("unexpected choice"))
    }
    fn select_many(
        &mut self,
        _: &str,
        choices: &[PromptChoice],
    ) -> Result<PromptOutcome<Vec<usize>>> {
        self.choices = choices.to_vec();
        Ok(self
            .selection
            .clone()
            .map_or(PromptOutcome::Cancelled, PromptOutcome::Submitted))
    }
}

#[derive(Default)]
struct Recording {
    events: Vec<(String, Value)>,
    lines: Vec<String>,
    json: bool,
}
impl Reporter for Recording {
    fn event(&mut self, event: &str, _: Level, data: Value) -> Result<()> {
        self.events.push((event.into(), data));
        Ok(())
    }
    fn human(&mut self, text: &str) -> Result<()> {
        self.lines.push(text.into());
        Ok(())
    }
    fn diagnostic(&mut self, text: &str) -> Result<()> {
        self.lines.push(text.into());
        Ok(())
    }
    fn is_json(&self) -> bool {
        self.json
    }
}

#[test]
fn application_config_failure_after_both_placements_restores_everything_and_reports_no_success() {
    struct FailConfig<'a>(&'a Fixture);
    impl RelocationHook for FailConfig<'_> {
        fn before_config(&self) -> Result<()> {
            assert!(self.0.destination.join("beta/SKILL.md").exists());
            assert!(!self.0.destination.join("alpha/extra.txt").exists());
            Err(failure("config install failed after both placements"))
        }
    }
    let fixture = Fixture::new();
    let original_source = image(&fixture.source);
    let original_destination = image(&fixture.destination);
    let mut reporter = Recording::default();
    let result = fixture.run(
        fixture.args(),
        &FailConfig(&fixture),
        &mut Picker::default(),
        &mut reporter,
        true,
    );
    assert!(result.is_err());
    assert_eq!(
        fs::read(fixture.repository.config_path()).expect("config"),
        fixture.before
    );
    assert_eq!(image(&fixture.source), original_source);
    assert_eq!(image(&fixture.destination), original_destination);
    assert!(reporter.events.iter().any(|(name, _)| name == "plan"));
    assert!(!reporter.events.iter().any(|(name, _)| matches!(
        name.as_str(),
        "source.location-set" | "summary" | "skill.copied"
    )));
}

#[test]
fn interactive_editor_defaults_missing_only_and_empty_selection_changes_only_configuration() {
    for selection in [vec![1], vec![]] {
        let fixture = Fixture::new();
        let destination_before = image(&fixture.destination);
        let mut picker = Picker {
            selection: Some(selection.clone()),
            approve: true,
            ..Picker::default()
        };
        let mut reporter = Recording::default();
        let args = SourceLocateArgs {
            all: false,
            yes: false,
            ..fixture.args()
        };
        fixture
            .run(args, &NoopRelocationHook, &mut picker, &mut reporter, false)
            .expect("interactive relocation");
        assert_eq!(
            picker
                .choices
                .iter()
                .map(|choice| (choice.label.as_str(), choice.selected))
                .collect::<Vec<_>>(),
            [("alpha", false), ("beta", true)]
        );
        assert!(fixture.destination.join("alpha/extra.txt").exists());
        assert_eq!(
            fixture.destination.join("beta").exists(),
            !selection.is_empty()
        );
        if selection.is_empty() {
            assert_eq!(image(&fixture.destination), destination_before);
            assert!(
                reporter
                    .lines
                    .iter()
                    .any(|line| line.contains("location only; no skill directories"))
            );
        }
        let stored = fixture
            .repository
            .load(false)
            .expect("stored config")
            .config;
        assert_eq!(
            stored.sources[0].alternate,
            fixture.config.sources[0].alternate
        );
        assert_eq!(stored.sources[0].extra, fixture.config.sources[0].extra);
    }
}

#[test]
fn cancellation_and_resolved_dry_run_leave_config_and_destination_exact() {
    for dry_run in [false, true] {
        let fixture = Fixture::new();
        let destination_before = image(&fixture.destination);
        let mut reporter = Recording::default();
        let args = SourceLocateArgs {
            yes: false,
            dry_run,
            ..fixture.args()
        };
        fixture
            .run(
                args,
                &NoopRelocationHook,
                &mut Picker::default(),
                &mut reporter,
                false,
            )
            .expect("cancel or dry run");
        assert_eq!(image(&fixture.destination), destination_before);
        assert_eq!(
            fs::read(fixture.repository.config_path()).expect("config"),
            fixture.before
        );
        assert!(
            !reporter
                .events
                .iter()
                .any(|(name, _)| name == "source.location-set")
        );
        if dry_run {
            assert_eq!(
                reporter.lines.iter().rev().take(2).collect::<Vec<_>>(),
                ["Dry run — no changes were made.", ""]
            );
        }
    }
}

#[test]
fn resolved_relocation_json_and_human_previews_are_complete() {
    for no_copy in [false, true] {
        let fixture = Fixture::new();
        let mut command = fixture.cli();
        command
            .args(["--json", "source", "locate", "physical"])
            .arg(&fixture.destination)
            .arg("--dry-run");
        if no_copy {
            command.arg("--no-copy");
        } else {
            command.arg("--all");
        }
        let output = command.output().expect("JSON preview");
        assert!(output.status.success());
        let events = String::from_utf8(output.stdout)
            .expect("JSON")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("event"))
            .collect::<Vec<_>>();
        let plan = &events
            .iter()
            .find(|event| event["event"] == "plan")
            .expect("plan")["data"];
        assert_eq!(plan["source"], json!({"id":"physical", "name":"physical"}));
        assert_eq!(
            plan["from"],
            skill_manager::config::portable_path(&fixture.source)
                .display()
                .to_string()
        );
        assert_eq!(
            plan["to"],
            skill_manager::config::portable_path(&fixture.destination)
                .display()
                .to_string()
        );
        assert_eq!(
            plan["configuration_effect"]["operation"],
            "set-source-location"
        );
        assert_eq!(plan["configuration_effect"]["from"], plan["from"]);
        assert_eq!(plan["configuration_effect"]["to"], plan["to"]);
        assert!(
            plan["effect"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        assert_eq!(plan["originals_retained"], true);
        assert_eq!(
            plan["entries"].as_array().expect("entries").is_empty(),
            no_copy
        );

        let fixture = Fixture::new();
        let mut command = fixture.cli();
        command
            .args(["source", "locate", "physical"])
            .arg(&fixture.destination)
            .arg("--dry-run");
        if no_copy {
            command.arg("--no-copy");
        } else {
            command.arg("--all");
        }
        command
            .assert()
            .success()
            .stdout(predicates::str::ends_with(
                "\nDry run — no changes were made.\n",
            ));
    }
}

#[test]
fn machine_modes_require_resolved_selection_and_explicit_yes() {
    for (all, yes) in [(false, true), (true, false)] {
        let fixture = Fixture::new();
        let mut reporter = Recording {
            json: true,
            ..Recording::default()
        };
        assert!(
            fixture
                .run(
                    SourceLocateArgs {
                        all,
                        yes,
                        ..fixture.args()
                    },
                    &NoopRelocationHook,
                    &mut Picker::default(),
                    &mut reporter,
                    true
                )
                .is_err()
        );
        assert_eq!(
            fs::read(fixture.repository.config_path()).expect("config"),
            fixture.before
        );
        assert!(!fixture.destination.join("beta").exists());
    }
}

#[test]
fn no_copy_accepts_missing_old_path_and_does_not_create_deep_destination() {
    let fixture = Fixture::new();
    let mut config = fixture.config.clone();
    config.sources[0].path = Some(fixture.home.path().join("gone"));
    fixture
        .repository
        .save(fixture.repository.config_path(), &config)
        .expect("missing old source");
    let destination = fixture.home.path().join("missing/deep/destination");
    let args = SourceLocateArgs {
        location: destination.display().to_string(),
        all: false,
        no_copy: true,
        ..fixture.args()
    };
    fixture
        .run(
            args,
            &NoopRelocationHook,
            &mut Picker::default(),
            &mut Recording::default(),
            true,
        )
        .expect("configuration only");
    assert!(!fixture.home.path().join("missing").exists());
    assert_eq!(
        fixture
            .repository
            .load(false)
            .expect("config")
            .config
            .sources[0]
            .path,
        Some(destination)
    );
}

#[test]
fn cli_copy_missing_and_recipe_explicit_replacements_have_the_same_selection_rules() {
    let fixture = Fixture::new();
    fixture
        .cli()
        .args(["--json", "source", "locate", "physical"])
        .arg(&fixture.destination)
        .args(["--copy", "--yes"])
        .assert()
        .success();
    assert!(fixture.destination.join("alpha/extra.txt").exists());
    assert!(fixture.destination.join("beta/SKILL.md").exists());

    let fixture = Fixture::new();
    let recipe = json!({"command":"source.locate", "source":"physical", "location":fixture.destination, "skill":"alpha", "filter":"b*", "yes":true});
    fixture
        .cli()
        .arg(format!("--json={recipe}"))
        .assert()
        .success();
    assert!(!fixture.destination.join("alpha/extra.txt").exists());
    assert!(fixture.destination.join("beta/SKILL.md").exists());
    assert!(fixture.destination.join("gamma/SKILL.md").exists());
}

#[test]
fn explicit_partial_selection_does_not_traverse_unselected_destination_entries() {
    let fixture = Fixture::new();
    let destination = fixture.home.path().join("partial");
    fs::create_dir(&destination).expect("partial destination");
    fs::write(destination.join("alpha"), "keep unselected file").expect("unselected file");
    fixture
        .cli()
        .args(["source", "locate", "physical"])
        .arg(&destination)
        .args(["--skill", "beta", "--yes"])
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(destination.join("alpha")).expect("preserved"),
        "keep unselected file"
    );
    assert!(destination.join("beta/SKILL.md").exists());
}

#[test]
fn invalid_names_patterns_and_conflicting_recipe_selectors_never_mutate() {
    for flags in [
        vec!["--skill", "unknown"],
        vec!["--filter", "nothing-*"],
        vec!["--copy", "--no-copy"],
    ] {
        let fixture = Fixture::new();
        fixture
            .cli()
            .args(["source", "locate", "physical"])
            .arg(&fixture.destination)
            .args(flags)
            .arg("--yes")
            .assert()
            .failure();
        assert_eq!(
            fs::read(fixture.repository.config_path()).expect("config"),
            fixture.before
        );
    }
    let fixture = Fixture::new();
    let recipe = json!({"command":"source.locate", "source":"physical", "location":fixture.destination, "copy":true, "no_copy":true, "yes":true});
    fixture
        .cli()
        .arg(format!("--json={recipe}"))
        .assert()
        .failure();
    assert_eq!(
        fs::read(fixture.repository.config_path()).expect("config"),
        fixture.before
    );
}

#[test]
fn unresolved_json_dry_run_contains_candidate_effects_and_changes_nothing() {
    let fixture = Fixture::new();
    let destination_before = image(&fixture.destination);
    let output = fixture
        .cli()
        .args(["--json", "source", "locate", "physical"])
        .arg(&fixture.destination)
        .arg("--dry-run")
        .output()
        .expect("preview");
    assert!(output.status.success());
    let events = String::from_utf8(output.stdout)
        .expect("json")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("event"))
        .collect::<Vec<_>>();
    let candidates = events
        .iter()
        .filter(|event| event["event"] == "source.relocation-candidate")
        .collect::<Vec<_>>();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0]["data"]["skill"], "alpha");
    assert_eq!(candidates[0]["data"]["default_selected"], false);
    assert_eq!(candidates[1]["data"]["skill"], "beta");
    assert_eq!(candidates[1]["data"]["default_selected"], true);
    assert!(
        events
            .iter()
            .all(|event| event["event"] != "plan" && event["event"] != "source.location-set")
    );
    assert_eq!(image(&fixture.destination), destination_before);
    assert_eq!(
        fs::read(fixture.repository.config_path()).expect("config"),
        fixture.before
    );
}

#[test]
fn unresolved_human_dry_run_ends_with_standard_conclusion() {
    let fixture = Fixture::new();
    fixture
        .cli()
        .args(["source", "locate", "physical"])
        .arg(&fixture.destination)
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicates::str::ends_with(
            "\nDry run — no changes were made.\n",
        ));
}

#[test]
fn single_skill_copy_rejects_manager_state_before_destination_traversal() {
    let home = physical_tempdir();
    let repository = FileConfigRepository::new(home.path());
    let source = home.path().join("single");
    fs::create_dir(&source).expect("single source");
    fs::write(
        source.join("SKILL.md"),
        "---\nname: single\ndescription: test\n---\n",
    )
    .expect("skill");
    let config: Config = serde_json::from_value(json!({
        "schema_version": 2,
        "sources": [{"id":"single", "name":"single", "mode":"single", "path":source}]
    }))
    .expect("config");
    repository
        .save(repository.config_path(), &config)
        .expect("save config");
    let config_before = fs::read(repository.config_path()).expect("config image");
    let source_before = image(&source);
    let marker = repository.storage_root().join("ordinary.txt");
    fs::write(&marker, "manager state").expect("marker");

    let mut command = Process::cargo_bin("skill-manager").expect("binary");
    command
        .arg("--home")
        .arg(home.path())
        .current_dir(home.path())
        .args(["source", "locate", "single"])
        .arg(repository.storage_root())
        .args(["--all", "--yes"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "relocation destination overlaps active skill-manager state",
        ));
    assert_eq!(
        fs::read(repository.config_path()).expect("config"),
        config_before
    );
    assert_eq!(image(&source), source_before);
    assert_eq!(fs::read_to_string(marker).expect("marker"), "manager state");
}

#[test]
fn relocation_help_completion_and_packaged_manual_contain_selection_flags() {
    let fixture = Fixture::new();
    let help = fixture
        .cli()
        .args(["source", "locate", "--help"])
        .output()
        .expect("help");
    let help = String::from_utf8(help.stdout).expect("help UTF8");
    for flag in [
        "--copy",
        "--no-copy",
        "--missing",
        "--all",
        "--skill",
        "--filter",
        "--include",
        "--exclude",
        "--dry-run",
        "--yes",
    ] {
        assert!(help.contains(flag), "missing {flag}");
    }
    let completions = fixture
        .cli()
        .args(["generate-completions", "--shell", "powershell"])
        .output()
        .expect("completions");
    let completions = String::from_utf8(completions.stdout).expect("completion UTF8");
    assert!(completions.contains("source;locate"));
    assert!(completions.contains("--no-copy"));
    let manual = fixture.home.path().join("skill-manager.1");
    fixture
        .cli()
        .args(["generate-man", "--output"])
        .arg(&manual)
        .assert()
        .success();
    let manual = fs::read_to_string(manual).expect("manual");
    assert!(manual.contains("SOURCE RELOCATION"));
    assert!(manual.contains("no\\-copy"));
    assert!(manual.contains("missing"));
}

#[test]
fn exclusions_apply_last_and_empty_set_creates_no_destination() {
    let fixture = Fixture::new();
    let destination = fixture.home.path().join("new/deep/skills");
    fixture
        .cli()
        .args(["source", "locate", "physical"])
        .arg(&destination)
        .args(["--all", "--exclude", "*", "--yes"])
        .assert()
        .success();
    assert!(!fixture.home.path().join("new").exists());
    let fixture = Fixture::new();
    fixture
        .cli()
        .args(["source", "locate", "physical"])
        .arg(&fixture.destination)
        .args([
            "--missing",
            "--skill",
            "alpha",
            "--exclude",
            "beta",
            "--yes",
        ])
        .assert()
        .success();
    assert!(!fixture.destination.join("alpha/extra.txt").exists());
    assert!(!fixture.destination.join("beta").exists());
}

#[test]
fn github_to_local_remains_configuration_only_without_network() {
    let fixture = Fixture::new();
    let mut config = fixture.config.clone();
    let alternate = config.sources[0].alternate.take().expect("remote");
    skill_manager::config::set_source_location(&mut config.sources[0], &alternate);
    fixture
        .repository
        .save(fixture.repository.config_path(), &config)
        .expect("remote active");
    let destination_before = image(&fixture.destination);
    fixture
        .run(
            SourceLocateArgs {
                all: false,
                ..fixture.args()
            },
            &NoopRelocationHook,
            &mut Picker::default(),
            &mut Recording::default(),
            true,
        )
        .expect("location change");
    assert_eq!(image(&fixture.destination), destination_before);
}

#[cfg(windows)]
#[test]
fn linked_destination_operand_and_selected_reparse_tree_are_rejected_but_unselected_link_survives()
{
    let fixture = Fixture::new();
    let link = fixture.home.path().join("destination-link");
    let result = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&fixture.destination)
        .output()
        .expect("junction");
    assert!(result.status.success());
    fixture
        .cli()
        .args(["source", "locate", "physical"])
        .arg(&link)
        .args(["--all", "--yes"])
        .assert()
        .failure();
    let partial = fixture.home.path().join("partial-links");
    fs::create_dir(&partial).expect("partial");
    let alpha_link = partial.join("alpha");
    let result = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&alpha_link)
        .arg(fixture.source.join("alpha"))
        .output()
        .expect("skill junction");
    assert!(result.status.success());
    fixture
        .cli()
        .args(["source", "locate", "physical"])
        .arg(&partial)
        .args(["--skill", "alpha", "--yes"])
        .assert()
        .failure();
    fixture
        .cli()
        .args(["source", "locate", "physical"])
        .arg(&partial)
        .args(["--skill", "beta", "--yes"])
        .assert()
        .success();
    assert!(alpha_link.join("SKILL.md").exists());
    assert!(partial.join("beta/SKILL.md").exists());
    fs::remove_dir(alpha_link).expect("unlink fixture junction");
    fs::remove_dir(link).expect("unlink fixture destination");
}

#[cfg(windows)]
#[test]
fn pending_rollback_is_previewed_without_writes_then_recovers_after_authorization() {
    use std::cell::RefCell;
    use std::os::windows::fs::OpenOptionsExt;
    struct BlockRollback<'a> {
        fixture: &'a Fixture,
        held: RefCell<Option<fs::File>>,
    }
    impl RelocationHook for BlockRollback<'_> {
        fn before_config(&self) -> Result<()> {
            *self.held.borrow_mut() = Some(
                fs::OpenOptions::new()
                    .read(true)
                    .share_mode(1)
                    .open(self.fixture.destination.join("beta/SKILL.md"))
                    .expect("hold beta"),
            );
            Err(failure("config unavailable"))
        }
    }
    let fixture = Fixture::new();
    let hook = BlockRollback {
        fixture: &fixture,
        held: RefCell::new(None),
    };
    assert!(
        fixture
            .run(
                fixture.args(),
                &hook,
                &mut Picker::default(),
                &mut Recording::default(),
                true
            )
            .is_err()
    );
    let pending = image(&fixture.destination);
    let mut preview = Recording::default();
    fixture
        .run(
            SourceLocateArgs {
                dry_run: true,
                ..fixture.args()
            },
            &NoopRelocationHook,
            &mut Picker::default(),
            &mut preview,
            true,
        )
        .expect("recovery preview");
    assert!(
        preview
            .lines
            .iter()
            .any(|line| line.contains("Pending relocation recovery plan"))
    );
    assert_eq!(
        preview.lines.iter().rev().take(2).collect::<Vec<_>>(),
        ["Dry run — no changes were made.", ""]
    );
    let recovery = &preview
        .events
        .iter()
        .find(|(name, _)| name == "plan")
        .expect("recovery plan")
        .1;
    assert_eq!(
        recovery["source"],
        json!({"id":"physical", "name":"physical"})
    );
    assert!(recovery["recovery"]["journal"].as_str().is_some());
    assert!(recovery["recovery"]["effect"].as_str().is_some());
    assert_eq!(
        recovery["recovery"]["next"],
        "prepare-and-review-fresh-relocation-plan"
    );
    assert_eq!(image(&fixture.destination), pending);
    drop(hook);
    fixture
        .run(
            fixture.args(),
            &NoopRelocationHook,
            &mut Picker::default(),
            &mut Recording::default(),
            true,
        )
        .expect("recover and apply freshly reviewed batch");
    assert!(!fixture.destination.join("alpha/extra.txt").exists());
    assert!(fixture.destination.join("beta/SKILL.md").exists());
}
