//! Application service and command orchestration.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::authorize::{Authorization, Authorizer, SelectionOption};
use crate::cache::{GitHubTransport, materialize_source};
use crate::cli::{
    Command, ConfigsAction, ConfigsArgs, CopyArgs, ImportArgs, RemoveArgs, ResolveArgs,
    ScopeSelection, SourceAction, SourceAddArgs, SourceAlternateArgs, SourceLocateArgs,
    SourceModeArg, SourceSelection, SourceSwapArgs, SourceUpdateArgs, StatusArgs, SyncArgs,
    TargetAction, TargetSelection,
};
use crate::config::{
    Config, ConfigBackup, ConfigRepository, FileConfigRepository, derive_salted_source_id,
    find_source_index, fold, is_builtin_name, location_from_reference, location_identity,
    location_reference, locations_equal, manager_home, normalize_target_template, paths_equal,
    portable_canonicalize, portable_path, resolved_targets, resolved_targets_for_scope,
    set_source_location, source_from_reference, source_location, source_reference,
};
use crate::domain::{
    ResolvedSource, Scope, ScopedTarget, SkillCandidate, SkillDiscovery, SourceEntry,
    SourceLocation, SourceMode, SourceType, Target, TargetEntry,
};
use crate::error::{Result, SkillManagerError};
use crate::event::{Level, Reporter};
use crate::plan::{
    DiffStat, GroupedUpdateEntry, PlanAction, creation_line, diff_directories, file_change_lines,
    grouped_update_table, totals_line,
};
use crate::prompt::Prompt;
use crate::review::{
    ChangePlan, Decision, DecisionOption, Destination, DestinationKind, OptionConsequence,
    PlanAuthorization, PlanRow, PlanSelection, PlannedAction, RenderStyle, ResultEntry,
    ResultMarker, colored, destination_label, location_of, location_text, plan_event_data,
    render_plan, result_footer,
};
use crate::skills::{
    PatternExpansion, deployed_skills, detect_skill_dirs, directories_equal, directory_files,
    discover_skills, expand_skill_patterns, is_fnmatch_operand, is_path_or_github_shaped,
    matches_patterns, skill_name, skill_state, split_sync_operands, validate_skill_name,
};
use crate::status::{
    DeploymentDetail, SkillLocation, SkillRow, SourceRow, display_width, join_columns, padded,
    separator, skill_table, source_table, status_summary_counts, status_summary_with_counts,
};
use crate::storage_migration::LayoutMigrationResult;
use crate::transaction::{TransactionHook, deploy_skill, import_skill, remove_skill};

/// One planned skill/target/scope deployment computed before any mutation.
struct SyncStep {
    candidate: SkillCandidate,
    target: Target,
    scope: Scope,
    destination: PathBuf,
    existed: bool,
    same: bool,
}

type TargetSpecificChangeDetails = Vec<(String, Vec<(String, Scope, String)>)>;

/// One deployed copy that differs from its source and can be imported.
#[derive(Clone)]
struct ImportCandidate {
    target: Target,
    scope: Scope,
    deployment: PathBuf,
    stat: DiffStat,
}

/// Scope roots and whether the current directory represents a real project.
struct ScopeContext {
    project_root: PathBuf,
    project_available: bool,
}

/// One target's inspected scopes for one `remove` skill: the resolved target
/// root at each scope actually inspected, populated only when the skill is
/// deployed there, alongside that exact deployment's own file count.
///
/// An explicit `--global`/`--project` inspects only its one scope; inference
/// inspects global always and project only when it is available. A cell with
/// both fields populated is the genuine branch point `remove` must surface
/// before asking anything. Deployments genuinely drift apart (a stale global
/// copy beside a refreshed project copy, say), so each scope's file count is
/// captured independently here rather than assumed equal across the cell —
/// nothing downstream may substitute one scope's count for the other's.
struct RemoveCell {
    target: Target,
    global_root: Option<PathBuf>,
    global_files: usize,
    project_root: Option<PathBuf>,
    project_files: usize,
}

/// Everywhere one skill is deployed, across every inspected target.
struct RemoveSkillPlan {
    identity: String,
    cells: Vec<RemoveCell>,
}

/// Which alternative resolves an ambiguous `remove` cell.
///
/// An unambiguous cell (exactly one scope populated) is removed under every
/// choice — the "N unambiguous deployments are removed in every option"
/// invariant proven against the Stage 1 fixture — so this only changes the
/// outcome where both scopes actually exist.
#[derive(Clone, Copy, Eq, PartialEq)]
enum RemoveScopeChoice {
    Project,
    Global,
    Both,
}

/// One resolved, apply-ready removal: exactly one skill at one target/scope.
struct RemoveApplyItem {
    skill: String,
    target: Target,
    scope: Scope,
    root: PathBuf,
}

#[derive(Clone, Copy)]
struct UpdatePlanOptions {
    implicit_targets: bool,
    dry_run: bool,
    confirmed: bool,
    context: UpdatePlanContext,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum UpdatePlanContext {
    Direct,
    ImportFollowUp,
}

/// Everything `update` needs after discovery to review, authorize, and apply.
///
/// `update` is the first command migrated onto the shared review pipeline, so
/// this bundle exists to keep that pipeline a single, testable call rather than
/// threading a dozen discovery outputs through it.
struct UpdateRun<'r> {
    args: &'r SyncArgs,
    steps: Vec<SyncStep>,
    target_names: Vec<String>,
    /// Skill names the user gave positionally, in the order they gave them.
    requested: Vec<String>,
    /// Folded skill names in the single order both review and apply follow.
    review_order: Vec<String>,
    glob_patterns: Vec<String>,
    has_targets: bool,
    positional_matched: bool,
    confirmed: bool,
}

/// Everything `load` needs after discovery to review, authorize, and apply.
///
/// Mirrors [`UpdateRun`] exactly, minus `requested`: `load` differs in that
/// every step shares one inferred-or-explicit scope decided up front, rather
/// than a per-step scope search across already-deployed targets, and it has
/// no per-named-skill "not deployed anywhere" message (installing is always
/// actionable, so an empty step list only ever means no targets exist).
struct LoadRun<'r> {
    args: &'r SyncArgs,
    steps: Vec<SyncStep>,
    target_names: Vec<String>,
    /// The single scope every step shares, decided before any step exists.
    scope: Scope,
    /// Folded skill names in the single order both review and apply follow.
    review_order: Vec<String>,
    glob_patterns: Vec<String>,
    has_targets: bool,
    positional_matched: bool,
    confirmed: bool,
}

/// Outcome converted to the executable exit code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    /// Command completed.
    Success,
    /// User declined a confirmation.
    Cancelled,
}

/// Dependencies and orchestration for one command invocation.
pub struct Application<'a, R, G, P, O, H> {
    repository: &'a R,
    github: &'a G,
    prompt: &'a mut P,
    reporter: &'a mut O,
    hook: &'a H,
    no_input: bool,
    home: PathBuf,
}

impl<'a, R, G, P, O, H> Application<'a, R, G, P, O, H>
where
    R: ConfigRepository,
    G: GitHubTransport,
    P: Prompt,
    O: Reporter,
    H: TransactionHook,
{
    /// Build an application service from narrow external ports.
    pub fn new(
        repository: &'a R,
        github: &'a G,
        prompt: &'a mut P,
        reporter: &'a mut O,
        hook: &'a H,
        no_input: bool,
        home: PathBuf,
    ) -> Self {
        Self {
            repository,
            github,
            prompt,
            reporter,
            hook,
            no_input,
            home,
        }
    }

    /// Execute one domain command.
    ///
    /// # Errors
    ///
    /// Returns a typed error when command validation, persistence, transport,
    /// prompting, reporting, or a filesystem operation fails.
    pub fn run(&mut self, command: Command) -> Result<RunOutcome> {
        self.validate_project_scope(&command)?;
        if let Command::Configs(args) = command {
            return self.run_configs(&args);
        }
        let dry_run = command_dry_run(&command);
        let loaded = self.repository.load(dry_run)?;
        self.emit_layout_migration(&loaded.layout_migration)?;
        let mut config = loaded.config;
        match command {
            Command::Load(args) => {
                if !self.run_sync(&config, &args.sync, false, args.yes)? {
                    return Ok(RunOutcome::Cancelled);
                }
            }
            Command::Update(args) => {
                if !self.run_sync(&config, &args.sync, true, args.yes)? {
                    return Ok(RunOutcome::Cancelled);
                }
            }
            Command::Import(args) => {
                if !self.run_import(&config, &args)? {
                    return Ok(RunOutcome::Cancelled);
                }
            }
            Command::Copy(args) => {
                if !self.run_copy(&config, &args)? {
                    return Ok(RunOutcome::Cancelled);
                }
            }
            Command::Remove(args) => {
                if !self.run_remove(&config, &args)? {
                    return Ok(RunOutcome::Cancelled);
                }
            }
            Command::Status(args) => {
                self.run_status(&config, &args)?;
            }
            Command::Resolve(args) => {
                self.run_resolve(&mut config, &loaded.active_path, &args)?;
            }
            Command::Source(args) => {
                self.run_source(&mut config, &loaded.active_path, args.action)?;
            }
            Command::Target(args) => {
                self.run_target(&mut config, &loaded.active_path, args.action)?;
            }
            Command::Configs(_) | Command::GenerateCompletions(_) | Command::GenerateMan(_) => {
                return Err(SkillManagerError::InvalidInput(
                    "generation commands must be handled at the executable boundary".into(),
                ));
            }
        }
        Ok(RunOutcome::Success)
    }

    /// Reject a project scope that would alias the global manager home.
    fn validate_project_scope(&self, command: &Command) -> Result<()> {
        let selection = match command {
            Command::Load(args) => Some(&args.sync.scope),
            Command::Update(args) => Some(&args.sync.scope),
            Command::Import(args) => Some(&args.scope),
            Command::Remove(args) => Some(&args.scope),
            Command::Status(args) => Some(&args.scope),
            _ => None,
        };
        if selection.is_some_and(|scope| scope.project)
            && !scope_context(&self.home)?.project_available
        {
            return Err(SkillManagerError::InvalidInput(
                "project scope is unavailable because the current directory is your global home; use --global or run this command from a project directory"
                    .into(),
            ));
        }
        Ok(())
    }

    fn run_configs(&mut self, args: &ConfigsArgs) -> Result<RunOutcome> {
        match &args.action {
            None if args.raw => {
                let migration = self.repository.migrate_layout()?;
                self.emit_raw_layout_migration(&migration)?;
                let bytes = self.repository.read_raw_or_create()?;
                self.reporter.raw(&bytes)?;
                Ok(RunOutcome::Success)
            }
            None => {
                let loaded = self.repository.load(false)?;
                self.emit_layout_migration(&loaded.layout_migration)?;
                self.show_config(&loaded.config, loaded.persisted)?;
                Ok(RunOutcome::Success)
            }
            Some(ConfigsAction::Reset(confirm)) => {
                let migration = self.repository.migrate_layout()?;
                self.emit_layout_migration(&migration)?;
                if !self.confirm_destructive("reset", confirm.yes)? {
                    return Ok(RunOutcome::Cancelled);
                }
                let backup = self.repository.reset_config()?;
                self.reporter.human(&format!(
                    "Reset configuration. Previous state saved as {}.",
                    backup.metadata.id
                ))?;
                self.reporter.event(
                    "config.reset",
                    Level::Info,
                    json!({
                        "path": self.repository.config_path(),
                        "backup_id": backup.metadata.id,
                        "backup_path": backup.raw_path,
                    }),
                )?;
                Ok(RunOutcome::Success)
            }
            Some(ConfigsAction::Restore(restore)) => {
                let migration = self.repository.migrate_layout()?;
                self.emit_layout_migration(&migration)?;
                if !self.confirm_destructive("restore", restore.yes)? {
                    return Ok(RunOutcome::Cancelled);
                }
                let outcome = self
                    .repository
                    .restore_config(restore.backup_id.as_deref())?;
                self.reporter.human(&format!(
                    "Restored configuration backup {}. Displaced state saved as {}.",
                    outcome.restored.metadata.id, outcome.displaced.metadata.id
                ))?;
                self.reporter.event(
                    "config.restored",
                    Level::Info,
                    json!({
                        "path": self.repository.config_path(),
                        "backup_id": outcome.restored.metadata.id,
                        "backup_path": outcome.restored.raw_path,
                        "displaced_backup_id": outcome.displaced.metadata.id,
                        "displaced_backup_path": outcome.displaced.raw_path,
                        "present": outcome.restored.metadata.present,
                    }),
                )?;
                Ok(RunOutcome::Success)
            }
        }
    }

    fn confirm_destructive(&mut self, action: &str, confirmed: bool) -> Result<bool> {
        if confirmed {
            return Ok(true);
        }
        if self.no_input {
            return Err(SkillManagerError::InteractionRequired(format!(
                "configs {action} is destructive; pass --yes in noninteractive mode"
            )));
        }
        let answer = self
            .prompt
            .exact_text(&format!("Type exactly 'yes' to {action} the configuration"))?;
        if answer == "yes" {
            return Ok(true);
        }
        self.report_cancelled(&format!("configs.{action}"))?;
        Ok(false)
    }

    /// Report one declined confirmation identically for every command.
    fn report_cancelled(&mut self, action: &str) -> Result<()> {
        self.reporter.human("Cancelled.")?;
        self.reporter.event(
            "command.cancelled",
            Level::Info,
            json!({ "action": action }),
        )
    }

    // Configuration display deliberately coordinates every human and machine section here.
    #[allow(clippy::too_many_lines)]
    fn show_config(&mut self, config: &Config, persisted: bool) -> Result<()> {
        let scope_context = scope_context(&self.home)?;
        let project_root = &scope_context.project_root;
        let global = resolved_targets_for_scope(config, &self.home, project_root, Scope::Global);
        let project = resolved_targets_for_scope(config, &self.home, project_root, Scope::Project);
        let targets = global
            .iter()
            .filter_map(|(name, global_target)| {
                project.get(name).map(|project_target| {
                    json!({
                        "name": name,
                        "label": global_target.target.label,
                        "template": global_target.template,
                        "enabled": global_target.target.enabled,
                        "builtin": global_target.target.builtin,
                        "legacy_override": global_target.target.legacy_override,
                        "global_path": global_target.target.path,
                        "project_path": project_target.target.path,
                    })
                })
            })
            .collect::<Vec<_>>();
        let backups = self.repository.list_backups()?;
        let backup_values = backups.iter().map(backup_data).collect::<Vec<_>>();

        let color = self.reporter.color_enabled();
        let verbose = self.reporter.verbose();
        self.reporter
            .human(&styled_heading("Configuration", color))?;
        self.reporter.human("")?;
        self.reporter.human(&format!(
            "  Config file   {} ({})",
            self.repository.config_path().display(),
            if persisted {
                "persisted"
            } else {
                "using defaults"
            }
        ))?;
        self.reporter.human(&format!(
            "  Storage       {}",
            self.repository.storage_root().display()
        ))?;
        self.reporter
            .human(&format!("  Global home   {}", self.home.display()))?;
        if scope_context.project_available {
            self.reporter
                .human(&format!("  Project       {}", project_root.display()))?;
        } else {
            self.reporter
                .human("  Project       unavailable — current directory is the global home")?;
        }
        let enabled_targets = global
            .values()
            .filter(|target| target.target.enabled)
            .count();
        self.reporter.human("")?;
        self.reporter.human(&format!(
            "  Schema v{} · {} · {} · {}",
            config.schema_version,
            counted_noun(config.sources.len(), "source"),
            enabled_target_label(enabled_targets),
            counted_noun(backups.len(), "backup")
        ))?;

        self.reporter.human("")?;
        self.reporter.human(&styled_heading("Sources", color))?;
        self.reporter.human("")?;
        if config.sources.is_empty() {
            self.reporter.human("  No sources configured.")?;
        } else {
            let source_rows = config
                .sources
                .iter()
                .map(|source| {
                    let display = if source.label.is_empty() || source.label == source.name {
                        source.name.clone()
                    } else {
                        format!("{} ({})", source.label, source.name)
                    };
                    let mut notes = Vec::new();
                    if !source.exclude.is_empty() {
                        notes.push(format!("excludes {}", source.exclude.join(", ")));
                    }
                    if let Some(alternate) = &source.alternate {
                        if verbose {
                            notes.push(format!("alternate: {}", location_reference(alternate)));
                        } else {
                            notes.push("alternate available".into());
                        }
                    }
                    let source_type = match source.source_type {
                        SourceType::Local => "local",
                        SourceType::GitHub => "GitHub",
                    };
                    let mut row = vec![display];
                    if verbose {
                        row.push(source.id.clone());
                    }
                    row.extend([
                        source_type.into(),
                        source_reference(source),
                        if notes.is_empty() {
                            "—".into()
                        } else {
                            notes.join("; ")
                        },
                    ]);
                    row
                })
                .collect::<Vec<_>>();
            let source_headers = if verbose {
                vec!["source", "id", "type", "location", "notes"]
            } else {
                vec!["source", "type", "location", "notes"]
            };
            for line in aligned_table(&source_headers, &source_rows) {
                self.reporter.human(&line)?;
            }
        }

        self.reporter.human("")?;
        self.reporter.human(&styled_heading("Targets", color))?;
        self.reporter.human("")?;
        let target_rows = global
            .iter()
            .filter_map(|(name, global_target)| {
                project.get(name).map(|project_target| {
                    let mut row = vec![
                        name.clone(),
                        if global_target.target.enabled {
                            "enabled".into()
                        } else {
                            "disabled".into()
                        },
                    ];
                    if verbose {
                        row.push(global_target.template.display().to_string());
                    }
                    row.push(global_target.target.path.display().to_string());
                    if scope_context.project_available {
                        row.push(project_target.target.path.display().to_string());
                    }
                    if verbose {
                        let mut notes = Vec::new();
                        if global_target.target.builtin {
                            notes.push("built-in");
                        }
                        if global_target.target.legacy_override {
                            notes.push("legacy override");
                        }
                        row.push(if notes.is_empty() {
                            "custom".into()
                        } else {
                            notes.join(", ")
                        });
                    }
                    row
                })
            })
            .collect::<Vec<_>>();
        let mut target_headers = vec!["target", "status"];
        if verbose {
            target_headers.push("template");
        }
        target_headers.push("global directory");
        if scope_context.project_available {
            target_headers.push("project directory");
        }
        if verbose {
            target_headers.push("notes");
        }
        for line in aligned_table_with_status(&target_headers, &target_rows, 1, color) {
            self.reporter.human(&line)?;
        }

        self.reporter.human("")?;
        self.reporter.human(&styled_heading("Backups", color))?;
        self.reporter.human("")?;
        if backups.is_empty() {
            self.reporter.human("  No configuration backups.")?;
        } else {
            let backup_rows = backups
                .iter()
                .map(|backup| {
                    let mut row = Vec::new();
                    if verbose {
                        row.push(backup.metadata.id.clone());
                    }
                    row.extend([
                        backup
                            .metadata
                            .created_at
                            .format("%Y-%m-%d %H:%M:%SZ")
                            .to_string(),
                        backup.metadata.reason.replace('-', " "),
                        if backup.metadata.valid {
                            "valid".into()
                        } else {
                            "invalid".into()
                        },
                    ]);
                    if verbose {
                        row.push(
                            backup
                                .metadata
                                .schema_version
                                .map_or_else(|| "unknown".into(), |version| format!("v{version}")),
                        );
                    }
                    row
                })
                .collect::<Vec<_>>();
            let backup_headers = if verbose {
                vec!["id", "created (UTC)", "reason", "status", "schema"]
            } else {
                vec!["created (UTC)", "reason", "status"]
            };
            let status_index = if verbose { 3 } else { 2 };
            for line in
                aligned_table_with_status(&backup_headers, &backup_rows, status_index, color)
            {
                self.reporter.human(&line)?;
            }
        }

        if verbose {
            self.reporter.human("")?;
            self.reporter
                .human(&styled_heading("Advanced settings", color))?;
            self.reporter.human("")?;
            for line in advanced_settings_lines(config) {
                self.reporter.human(&line)?;
            }
        }

        self.reporter.human("")?;
        if !verbose {
            self.reporter
                .human("Use --verbose for IDs, templates, alternates, and overrides.")?;
        }
        self.reporter
            .human("Use --raw for the exact configuration document.")?;
        self.reporter.event(
            "config.shown",
            Level::Info,
            json!({
                "path": self.repository.config_path(),
                "storage_root": self.repository.storage_root(),
                "home": self.home,
                "project_root": project_root,
                "persisted": persisted,
                "config": config,
                "targets": targets,
                "backups": backup_values,
            }),
        )
    }

    fn emit_layout_migration(&mut self, migration: &LayoutMigrationResult) -> Result<()> {
        for item in &migration.migrated {
            self.reporter.human(&format!(
                "Migrated {} from {} to {}.",
                item.component,
                item.from.display(),
                item.to.display()
            ))?;
            self.reporter.event(
                "config.migrated",
                Level::Info,
                json!({
                    "component": item.component,
                    "from": item.from,
                    "to": item.to,
                }),
            )?;
        }
        for warning in &migration.warnings {
            self.emit_message_diagnostic(warning)?;
        }
        Ok(())
    }

    fn emit_raw_layout_migration(&mut self, migration: &LayoutMigrationResult) -> Result<()> {
        for item in &migration.migrated {
            self.reporter.diagnostic(&format!(
                "Migrated {} from {} to {}.",
                item.component,
                item.from.display(),
                item.to.display()
            ))?;
        }
        for warning in &migration.warnings {
            self.reporter.diagnostic(&format!("Warning: {warning}"))?;
        }
        Ok(())
    }

    fn run_source(
        &mut self,
        config: &mut Config,
        active_path: &Path,
        action: SourceAction,
    ) -> Result<()> {
        match action {
            SourceAction::Add(args) => self.source_add(config, active_path, args),
            SourceAction::Remove(args) => {
                let selector = args.source.map_or_else(
                    || {
                        std::env::current_dir()
                            .map(|path| path.display().to_string())
                            .map_err(|error| SkillManagerError::io(".", error))
                    },
                    Ok,
                )?;
                let index = source_selector_index(config, &selector)?;
                let removed = config.sources.remove(index);
                self.repository.save(active_path, config)?;
                self.reporter.human(&format!(
                    "Removed source {} ({})",
                    removed.name,
                    source_reference(&removed)
                ))?;
                self.reporter
                    .event("source.removed", Level::Info, source_data(&removed))
            }
            SourceAction::List => {
                let rows = config
                    .sources
                    .iter()
                    .map(|source| SourceRow {
                        name: source.name.clone(),
                        label: source.label.clone(),
                        location: source_reference(source),
                        alternate: source.alternate.as_ref().map(location_reference),
                    })
                    .collect::<Vec<_>>();
                for line in source_table(&rows) {
                    self.reporter.human(&line)?;
                }
                for source in &config.sources {
                    self.reporter
                        .event("source.listed", Level::Info, source_data(source))?;
                }
                self.reporter.event(
                    "summary",
                    Level::Info,
                    json!({ "sources": config.sources.len() }),
                )
            }
            SourceAction::Update(args) => self.source_update(config, active_path, args),
            SourceAction::Locate(args) => self.source_locate(config, active_path, &args),
            SourceAction::Alternate(args) => self.source_alternate(config, active_path, args),
            SourceAction::Swap(args) => self.source_swap(config, active_path, &args),
        }
    }

    fn source_add(
        &mut self,
        config: &mut Config,
        active_path: &Path,
        args: SourceAddArgs,
    ) -> Result<()> {
        if args.cache_ttl_hours.is_some_and(|value| value < 0) {
            return Err(SkillManagerError::InvalidInput(
                "cache TTL must be zero or positive".into(),
            ));
        }
        let reference = args.source.map_or_else(
            || {
                std::env::current_dir()
                    .map(|path| path.display().to_string())
                    .map_err(|error| SkillManagerError::io(".", error))
            },
            Ok,
        )?;
        let mode = args.mode.map(|mode| match mode {
            SourceModeArg::Collection => SourceMode::Collection,
            SourceModeArg::Single => SourceMode::Single,
        });
        let mut source = source_from_reference(&reference, mode)?;
        let new_location = source_location(&source)?;
        if let Some(existing) = find_location_owner(config, &new_location, None) {
            return Err(SkillManagerError::InvalidInput(format!(
                "location is already configured by source '{}': {}",
                existing.name,
                location_reference(&new_location)
            )));
        }
        if config.sources.iter().any(|entry| entry.id == source.id) {
            for salt in 1_u64.. {
                let candidate = derive_salted_source_id(&source, salt);
                if config.sources.iter().all(|entry| entry.id != candidate) {
                    source.id = candidate;
                    break;
                }
            }
        }
        source.name = match args.name.or(args.source_name) {
            Some(name) if !name.trim().is_empty() => name,
            Some(_) => {
                return Err(SkillManagerError::InvalidInput(
                    "source name must not be blank".into(),
                ));
            }
            None if self.no_input => {
                return Err(SkillManagerError::InteractionRequired(
                    "source name is required in noninteractive mode; pass NAME or --name".into(),
                ));
            }
            None => self
                .prompt
                .text("Source name", Some(source.name.as_str()))?,
        };
        if config
            .sources
            .iter()
            .any(|entry| fold(&entry.name) == fold(&source.name))
        {
            return Err(SkillManagerError::InvalidInput(format!(
                "source name is already in use: {}",
                source.name
            )));
        }
        let default_label = title_case(&source.name);
        source.label = match args.label {
            Some(label) if !label.trim().is_empty() => label,
            Some(_) | None if self.no_input => default_label,
            Some(_) | None => self.prompt.text("Source Label", Some(&default_label))?,
        };
        source.exclude = normalized_patterns(args.exclude);
        source.cache_ttl_hours = args.cache_ttl_hours;
        config.sources.push(source.clone());
        self.repository.save(active_path, config)?;
        self.reporter.human(&format!(
            "Added source {} ({})",
            source.name,
            source_reference(&source)
        ))?;
        self.reporter
            .event("source.added", Level::Info, source_data(&source))
    }

    fn source_update(
        &mut self,
        config: &mut Config,
        active_path: &Path,
        args: SourceUpdateArgs,
    ) -> Result<()> {
        if args.cache_ttl_hours.is_some_and(|value| value < 0) {
            return Err(SkillManagerError::InvalidInput(
                "cache TTL must be zero or positive".into(),
            ));
        }
        let index = source_selector_index(config, &args.source)?;
        if let Some(name) = &args.name {
            if name.trim().is_empty() {
                return Err(SkillManagerError::InvalidInput(
                    "source name must not be blank".into(),
                ));
            }
            if config
                .sources
                .iter()
                .enumerate()
                .any(|(position, entry)| position != index && fold(&entry.name) == fold(name))
            {
                return Err(SkillManagerError::InvalidInput(format!(
                    "source name is already in use: {name}"
                )));
            }
        }
        let previous = config.sources.get(index).cloned().ok_or_else(|| {
            SkillManagerError::InvalidInput("source index changed unexpectedly".into())
        })?;
        let mut proposed = previous.clone();
        if let Some(location) = &args.location {
            let replacement = location_from_reference(location, proposed.mode)?;
            let active = source_location(&proposed)?;
            if !locations_equal(&active, &replacement) {
                if proposed
                    .alternate
                    .as_ref()
                    .is_some_and(|alternate| locations_equal(alternate, &replacement))
                {
                    return Err(SkillManagerError::InvalidInput(
                        "requested location is the saved alternate; use 'source swap'".into(),
                    ));
                }
                reject_location_collision(config, &replacement, index)?;
                set_source_location(&mut proposed, &replacement);
            }
        }
        if let Some(name) = args.name {
            proposed.name = name;
        }
        if let Some(label) = args.label {
            proposed.label = label;
        }
        if args.clear_exclude {
            proposed.exclude.clear();
        }
        for pattern in normalized_patterns(args.exclude) {
            if !proposed.exclude.iter().any(|existing| existing == &pattern) {
                proposed.exclude.push(pattern);
            }
        }
        if let Some(ttl) = args.cache_ttl_hours {
            proposed.cache_ttl_hours = Some(ttl);
        }
        let changed = proposed != previous;
        if changed {
            config.sources[index] = proposed.clone();
            self.repository.save(active_path, config)?;
        }
        self.reporter
            .human(&format!("Updated source {}", proposed.name))?;
        self.reporter.event(
            "source.updated",
            Level::Info,
            source_change_data(&proposed, &previous, changed),
        )
    }

    fn source_locate(
        &mut self,
        config: &mut Config,
        active_path: &Path,
        args: &SourceLocateArgs,
    ) -> Result<()> {
        let index = source_selector_index(config, &args.source)?;
        let previous = config.sources[index].clone();
        let replacement = location_from_reference(&args.location, previous.mode)?;
        let active = source_location(&previous)?;
        if locations_equal(&active, &replacement) {
            return self.reporter.event(
                "source.location-set",
                Level::Info,
                source_change_data(&previous, &previous, false),
            );
        }
        if previous
            .alternate
            .as_ref()
            .is_some_and(|alternate| locations_equal(alternate, &replacement))
        {
            return Err(SkillManagerError::InvalidInput(
                "requested location is the saved alternate; use 'source swap'".into(),
            ));
        }
        reject_location_collision(config, &replacement, index)?;
        let mut proposed = previous.clone();
        set_source_location(&mut proposed, &replacement);
        config.sources[index] = proposed.clone();
        self.repository.save(active_path, config)?;
        self.reporter.human(&format!(
            "Located source {} at {}",
            proposed.name,
            source_reference(&proposed)
        ))?;
        self.reporter.event(
            "source.location-set",
            Level::Info,
            source_change_data(&proposed, &previous, true),
        )
    }

    fn source_alternate(
        &mut self,
        config: &mut Config,
        active_path: &Path,
        args: SourceAlternateArgs,
    ) -> Result<()> {
        let index = source_selector_index(config, &args.source)?;
        let previous = config.sources[index].clone();
        let replacement = match (args.location, args.clear) {
            (Some(location), false) => Some(location_from_reference(&location, previous.mode)?),
            (None, true) => None,
            _ => {
                return Err(SkillManagerError::InvalidInput(
                    "source alternate requires exactly one of LOCATION or --clear".into(),
                ));
            }
        };
        let unchanged = match (&replacement, &previous.alternate) {
            (Some(replacement), Some(existing)) => locations_equal(replacement, existing),
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            let event = if replacement.is_some() {
                "source.alternate-set"
            } else {
                "source.alternate-cleared"
            };
            return self.reporter.event(
                event,
                Level::Info,
                source_change_data(&previous, &previous, false),
            );
        }
        if let Some(location) = &replacement {
            let active = source_location(&previous)?;
            if locations_equal(&active, location) {
                return Err(SkillManagerError::InvalidInput(
                    "alternate location must differ from the active location".into(),
                ));
            }
            reject_location_collision(config, location, index)?;
        }
        let mut proposed = previous.clone();
        proposed.alternate = replacement;
        config.sources[index] = proposed.clone();
        self.repository.save(active_path, config)?;
        let event = if proposed.alternate.is_some() {
            "source.alternate-set"
        } else {
            "source.alternate-cleared"
        };
        self.reporter
            .human(&format!("Updated alternate for source {}", proposed.name))?;
        self.reporter.event(
            event,
            Level::Info,
            source_change_data(&proposed, &previous, true),
        )
    }

    fn source_swap(
        &mut self,
        config: &mut Config,
        active_path: &Path,
        args: &SourceSwapArgs,
    ) -> Result<()> {
        let index = source_selector_index(config, &args.source)?;
        let previous = config.sources[index].clone();
        let alternate = previous.alternate.clone().ok_or_else(|| {
            SkillManagerError::InvalidInput(format!(
                "source '{}' has no alternate location to swap",
                previous.name
            ))
        })?;
        let active = source_location(&previous)?;
        let mut proposed = previous.clone();
        set_source_location(&mut proposed, &alternate);
        proposed.alternate = Some(active);
        config.sources[index] = proposed.clone();
        self.repository.save(active_path, config)?;
        self.reporter.human(&format!(
            "Swapped source {} to {}",
            proposed.name,
            source_reference(&proposed)
        ))?;
        self.reporter.event(
            "source.locations-swapped",
            Level::Info,
            source_change_data(&proposed, &previous, true),
        )
    }

    // Lifecycle policy is intentionally kept in one match so every target state
    // transition remains auditable together.
    #[allow(clippy::too_many_lines)]
    fn run_target(
        &mut self,
        config: &mut Config,
        active_path: &Path,
        action: TargetAction,
    ) -> Result<()> {
        match action {
            TargetAction::List => {
                for target in resolved_targets(config, &self.home).values() {
                    self.reporter.human(&format!(
                        "{}\t{}\t{}\t{}",
                        target.name,
                        target.label,
                        target.path.display(),
                        if target.enabled {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    ))?;
                    self.reporter.event(
                        "target.listed",
                        if target.legacy_override {
                            Level::Warning
                        } else {
                            Level::Info
                        },
                        target_data(target),
                    )?;
                }
                Ok(())
            }
            TargetAction::Add(args) => {
                if is_builtin_name(&args.name) {
                    return Err(SkillManagerError::InvalidInput(format!(
                        "custom target name is reserved: {}",
                        args.name
                    )));
                }
                if config
                    .targets
                    .keys()
                    .any(|name| fold(name) == fold(&args.name))
                {
                    return Err(SkillManagerError::InvalidInput(format!(
                        "target already exists: {}",
                        args.name
                    )));
                }
                let name = args.name;
                config.targets.insert(
                    name.clone(),
                    TargetEntry {
                        path: normalize_target_template(&args.path.to_string_lossy())?,
                        label: title_case(&name),
                        enabled: true,
                        extra: IndexMap::new(),
                    },
                );
                self.repository.save(active_path, config)?;
                self.emit_target_change(config, &name, "target.added", Level::Info)
            }
            TargetAction::Enable(args) => {
                set_target_enabled(config, &args.name, true)?;
                self.repository.save(active_path, config)?;
                self.emit_target_change(config, &args.name, "target.enabled", Level::Info)
            }
            TargetAction::Disable(args) => {
                set_target_enabled(config, &args.name, false)?;
                self.repository.save(active_path, config)?;
                self.emit_target_change(config, &args.name, "target.disabled", Level::Info)
            }
            TargetAction::SetPath(args) => {
                let path = normalize_target_template(&args.path.to_string_lossy())?;
                if let Some(entry) = find_named_mut(&mut config.targets, &args.name) {
                    entry.path = path;
                } else if let Some(entry) =
                    find_named_mut(&mut config.legacy_target_overrides, &args.name)
                {
                    entry.path = path;
                } else {
                    return Err(SkillManagerError::InvalidInput(format!(
                        "target set-path applies only to custom targets and legacy overrides: {}",
                        args.name
                    )));
                }
                self.repository.save(active_path, config)?;
                self.emit_target_change(config, &args.name, "target.path-set", Level::Info)
            }
            TargetAction::Remove(args) => {
                let custom_key = find_named_key(&config.targets, &args.name);
                let override_key = find_named_key(&config.legacy_target_overrides, &args.name);
                let level;
                if let Some(key) = custom_key {
                    config.targets.shift_remove(&key);
                    level = Level::Info;
                } else if let Some(key) = override_key {
                    config.legacy_target_overrides.shift_remove(&key);
                    level = Level::Warning;
                } else if is_builtin_name(&args.name) {
                    config.builtins.entry(fold(&args.name)).or_default().enabled = false;
                    level = Level::Warning;
                } else {
                    return Err(SkillManagerError::NotFound {
                        kind: "target",
                        reference: args.name,
                    });
                }
                self.repository.save(active_path, config)?;
                self.reporter.human("Target removed or disabled.")?;
                self.reporter
                    .event("target.removed", level, json!({ "name": args.name }))
            }
        }
    }

    fn emit_target_change(
        &mut self,
        config: &Config,
        name: &str,
        event: &str,
        level: Level,
    ) -> Result<()> {
        let targets = resolved_targets(config, &self.home);
        let target = targets
            .values()
            .find(|target| fold(&target.name) == fold(name))
            .ok_or_else(|| SkillManagerError::NotFound {
                kind: "target",
                reference: name.to_owned(),
            })?;
        self.reporter.human(&format!("{event}: {}", target.name))?;
        self.reporter.event(event, level, target_data(target))
    }

    // Sync is an orchestration boundary for discovery, scope inference, planning, and events.
    #[allow(clippy::too_many_lines)]
    fn run_sync(
        &mut self,
        config: &Config,
        args: &SyncArgs,
        update_only: bool,
        confirmed: bool,
    ) -> Result<bool> {
        let operands = split_sync_operands(&args.sources);
        let glob_patterns = operands.skill_patterns;

        // Pre-discovery pass: a literal operand is a definite source when it
        // already names a configured source or is shaped like a path/GitHub
        // reference. Everything else is deferred until skill discovery runs,
        // because only discovery can tell a bare skill name from a bare
        // directory name.
        let mut definite_sources = Vec::new();
        let mut deferred = Vec::new();
        for operand in &operands.sources {
            if configured_source_index(config, operand)?.is_some()
                || is_path_or_github_shaped(operand)
            {
                definite_sources.push(operand.clone());
            } else {
                deferred.push(operand.clone());
            }
        }

        let mut sources = self.resolve_sources(
            config,
            &definite_sources,
            &args.source_selection,
            args.refresh,
            args.dry_run,
        )?;
        // Collisions are not emitted from this preliminary discovery: it may
        // be discarded and replaced below once deferred bare words are
        // resolved, and emitting collisions from a discovery pass that gets
        // thrown away would report irrelevant warnings (or, when the final
        // discovery repeats the same collision, duplicate it).
        let mut discovery = discover_skills(&sources, &[], &config.exclude)?;
        let cwd = std::env::current_dir().map_err(|error| SkillManagerError::io(".", error))?;

        // Post-discovery pass: resolve every deferred bare word against the
        // skills just discovered. A discovered skill name wins over a
        // same-named CWD directory (with a warning); an unmatched bare word
        // that still names a CWD directory is queued for promotion; anything
        // else is provisionally unresolved until we know whether a directory
        // promotion (and its one permitted extra discovery pass) is coming.
        let (mut literal_skill_names, promoted_sources, provisional) =
            self.resolve_deferred_sync_operands(&deferred, &cwd, &discovery)?;
        if !promoted_sources.is_empty() {
            if definite_sources.is_empty() && literal_skill_names.is_empty() {
                sources = self.resolve_sources(
                    config,
                    &promoted_sources,
                    &args.source_selection,
                    args.refresh,
                    args.dry_run,
                )?;
            } else {
                for word in &promoted_sources {
                    let entry = configured_source_or_reference(config, word, None)?;
                    sources.push(materialize_source(
                        self.repository,
                        self.github,
                        &entry,
                        args.refresh,
                        args.dry_run,
                    )?);
                }
            }
            discovery = discover_skills(&sources, &[], &config.exclude)?;
            // Words that were neither a discovered skill nor a CWD directory
            // before promotion may only now resolve, since the directory
            // just promoted to a source can contain them (e.g. `load
            // plain-dir widget` where `plain-dir/widget/SKILL.md` exists).
            // This is the only place they get a second chance; anything
            // still unresolved after this single extra discovery pass is a
            // hard error.
            if !provisional.is_empty() {
                literal_skill_names.extend(self.resolve_provisional_sync_operands(
                    &provisional,
                    &cwd,
                    &discovery,
                )?);
            }
        }
        // Emit collisions exactly once, from the final discovery result.
        self.emit_collisions(&discovery.collisions)?;

        let target_templates = self.select_target_templates(config, &args.targets)?;
        let scope_context = scope_context(&self.home)?;
        let project_root = &scope_context.project_root;
        let load_scope = if update_only {
            None
        } else {
            Some(self.load_scope(&target_templates, &args.scope, project_root))
        };

        let candidate_names = discovery
            .winners
            .values()
            .map(|candidate| candidate.name.as_str())
            .collect::<Vec<_>>();
        // `expand_skill_patterns` now matches nothing for an empty pattern
        // list (see its doc comment), which is exactly what's wanted here:
        // this call site only ever means "narrow by glob pattern", never
        // "select everything". The separate "no operands: deploy everything"
        // opt-in lives below, in the `glob_patterns.is_empty() &&
        // literal_skill_names.is_empty()` branch, which reads straight from
        // `discovery.winners` instead of going through the expander.
        let expansion = expand_skill_patterns(&glob_patterns, candidate_names)?;
        self.emit_unmatched_patterns(&expansion.unmatched_patterns)?;
        // A literal skill name that resolved successfully must count as a
        // positional match on its own, even when an unrelated unmatched glob
        // pattern was also supplied: an unmatched glob only warns (it never
        // fails the whole invocation by itself), so it must not force a
        // non-zero exit after a valid literal already deployed something.
        let positional_matched = !literal_skill_names.is_empty()
            || glob_patterns.is_empty()
            || !expansion.matched.is_empty();
        // The names the user actually asked for, before folding, so a plan that
        // resolves to nothing can name the request back instead of a count.
        let mut requested = literal_skill_names.clone();
        requested.extend(expansion.matched.iter().cloned());
        let selected = if glob_patterns.is_empty() && literal_skill_names.is_empty() {
            // No positional selector narrowed skills at all: keep the
            // long-standing "select every discovered candidate" behavior
            // rather than accidentally tripping it when literal skill names
            // were supplied but happened not to widen `glob_patterns`.
            discovery.winners.keys().cloned().collect::<BTreeSet<_>>()
        } else {
            let mut set = expansion
                .matched
                .iter()
                .map(|name| fold(name))
                .collect::<BTreeSet<_>>();
            set.extend(literal_skill_names.iter().map(|name| fold(name)));
            set
        };

        let mut steps = Vec::new();
        for candidate in discovery.winners.values() {
            if !selected.contains(&fold(&candidate.name))
                || !matches_patterns(&candidate.name, &args.filters)?
            {
                continue;
            }
            for template in &target_templates {
                let scopes = if let Some(scope) = load_scope {
                    vec![scope]
                } else {
                    update_scopes(
                        template,
                        candidate,
                        &args.scope,
                        &self.home,
                        project_root,
                        scope_context.project_available,
                    )
                };
                for scope in scopes {
                    let target = scoped_target(template, scope, &self.home, project_root);
                    let destination = target.target.path.join(&candidate.name);
                    let existed = destination.is_dir();
                    if update_only && !existed {
                        continue;
                    }
                    let same = existed && directories_equal(&candidate.path, &destination)?;
                    steps.push(SyncStep {
                        candidate: candidate.clone(),
                        target: target.target,
                        scope,
                        destination,
                        existed,
                        same,
                    });
                }
            }
        }

        let target_names = target_templates
            .iter()
            .map(|target| target.target.name.clone())
            .collect::<Vec<_>>();
        // Review order and apply order are derived from one sequence so they
        // cannot drift: the CLI must never act in an order other than the one
        // it showed.
        let review_order = review_sequence(&steps, &requested);
        let mut steps = steps;
        let rank = |step: &SyncStep| {
            review_order
                .iter()
                .position(|key| *key == fold(&step.candidate.name))
                .unwrap_or(usize::MAX)
        };
        steps.sort_by_key(rank);
        let has_targets = !target_templates.is_empty();
        if update_only {
            return self.run_update(&UpdateRun {
                args,
                steps,
                target_names,
                requested,
                review_order,
                glob_patterns,
                has_targets,
                positional_matched,
                confirmed,
            });
        }
        let scope = load_scope.unwrap_or(Scope::Global);
        self.run_load(&LoadRun {
            args,
            steps,
            target_names,
            scope,
            review_order,
            glob_patterns,
            has_targets,
            positional_matched,
            confirmed,
        })
    }

    /// Emit the load/update summary event from one shared, auditable site.
    fn report_sync_summary(
        &mut self,
        action: &str,
        changed: usize,
        skipped: usize,
        dry_run: bool,
    ) -> Result<()> {
        self.reporter.event(
            "summary",
            Level::Info,
            json!({
                "action": action,
                "changed": changed,
                "skipped": skipped,
                "dry_run": dry_run
            }),
        )
    }

    /// Review, authorize, and apply the update plan.
    ///
    /// The complete plan is rendered before anything is asked, every render is
    /// significance gated, and cancelling names only the decisions that were
    /// inferred, so the next invocation is obvious rather than guessed.
    fn run_update(&mut self, run: &UpdateRun<'_>) -> Result<bool> {
        if !run.positional_matched {
            return Err(SkillManagerError::NotFound {
                kind: "skill matching positional pattern",
                reference: run.glob_patterns.join(", "),
            });
        }
        let implicit_targets = !run.args.targets.is_explicit();
        let actionable = run
            .steps
            .iter()
            .filter(|step| !step.same)
            .collect::<Vec<_>>();
        let style = self.render_style();
        let mut uniform_scope = None;
        if actionable.is_empty() {
            self.report_update_no_work(run, implicit_targets)?;
        } else {
            let plan = update_change_plan(
                &actionable,
                &run.target_names,
                run.args,
                &run.review_order,
                style,
                !run.args.dry_run && !run.confirmed && !self.no_input,
            )?;
            let view = plan.view();
            uniform_scope = view.uniform_scope();
            let label = destination_label(
                view.columns().len(),
                run.target_names.len(),
                !implicit_targets,
                "target",
            );
            let selection = PlanSelection {
                targets: run.target_names.clone(),
                targets_explicit: !implicit_targets,
                scope: uniform_scope,
                scope_explicit: run.args.scope.is_explicit(),
            };
            let revision = 0;
            let data = plan_event_data(
                &view,
                revision,
                run.args.dry_run,
                self.update_authorization(run),
                &selection,
            );
            let event = if revision == 0 {
                "plan"
            } else {
                "plan.updated"
            };
            self.reporter.event(event, Level::Info, data)?;
            for line in render_plan(&view, style) {
                self.reporter.human(&line)?;
            }
            self.reporter.human(&format!(
                "{} across {label}",
                counted_noun(view.actions(), "update")
            ))?;
            if run.args.dry_run {
                self.reporter.human("")?;
                self.reporter.human("Dry run — no changes were made.")?;
            } else if self.authorize_update(run, &label)? {
                self.reporter.human("")?;
            } else {
                self.report_update_cancelled(run, implicit_targets)?;
                return Ok(false);
            }
        }

        let mut changed = 0_usize;
        let mut skipped = 0_usize;
        self.apply_update_steps(run, uniform_scope, &mut changed, &mut skipped)?;
        if changed > 0 && !run.args.dry_run {
            self.reporter.human("")?;
            let footer = result_footer(
                &[
                    ResultEntry {
                        marker: ResultMarker::Completed,
                        count: changed,
                        description: format!(
                            "deployment{} updated",
                            if changed == 1 { "" } else { "s" }
                        ),
                    },
                    ResultEntry {
                        marker: ResultMarker::Unchanged,
                        count: skipped,
                        description: "unchanged".to_owned(),
                    },
                ],
                style,
            );
            self.reporter.human(&footer)?;
        }
        self.report_sync_summary("update", changed, skipped, run.args.dry_run)?;
        Ok(true)
    }

    /// Apply every planned step, reporting progress the plan already promised.
    fn apply_update_steps(
        &mut self,
        run: &UpdateRun<'_>,
        uniform_scope: Option<Scope>,
        changed: &mut usize,
        skipped: &mut usize,
    ) -> Result<()> {
        for step in &run.steps {
            if step.same {
                *skipped += 1;
                self.reporter.event(
                    "skill.skipped",
                    Level::Info,
                    skill_action_data(
                        &step.candidate,
                        &step.target,
                        Some(step.scope),
                        &step.destination,
                        run.args.dry_run,
                        "skipped",
                    ),
                )?;
                continue;
            }
            if !run.args.dry_run {
                deploy_skill(
                    &step.candidate.path,
                    &step.target.path,
                    self.repository.cache_root(),
                    self.hook,
                )?;
                // A uniform scope is already stated once above the plan, so
                // repeating it on every progress line would add no information.
                let scope = if uniform_scope.is_some() {
                    String::new()
                } else {
                    format!(" ({})", step.scope.as_str())
                };
                self.reporter.human(&format!(
                    "Updated {} -> {}{scope}",
                    step.candidate.name, step.target.name
                ))?;
            }
            *changed += 1;
            self.reporter.event(
                "skill.updated",
                Level::Info,
                skill_action_data(
                    &step.candidate,
                    &step.target,
                    Some(step.scope),
                    &step.destination,
                    run.args.dry_run,
                    "updated",
                ),
            )?;
        }
        Ok(())
    }

    /// Review, authorize, and apply the load plan.
    ///
    /// Mirrors [`Self::run_update`] exactly: the complete plan renders before
    /// anything is asked, new installs are distinguished from overwrites,
    /// already-identical deployments are hidden from the table and counted
    /// only in the footer, and cancelling names only the decisions that were
    /// inferred.
    #[allow(clippy::too_many_lines)]
    fn run_load(&mut self, run: &LoadRun<'_>) -> Result<bool> {
        if !run.positional_matched {
            return Err(SkillManagerError::NotFound {
                kind: "skill matching positional pattern",
                reference: run.glob_patterns.join(", "),
            });
        }
        let implicit_targets = !run.args.targets.is_explicit();
        let actionable = run
            .steps
            .iter()
            .filter(|step| !step.same)
            .collect::<Vec<_>>();
        let identical = run.steps.iter().filter(|step| step.same).count();
        let style = self.render_style();
        if actionable.is_empty() {
            self.report_load_no_work(run, implicit_targets)?;
        } else {
            let all_steps = run.steps.iter().collect::<Vec<_>>();
            let plan = load_change_plan(
                &all_steps,
                &run.target_names,
                run.args,
                run.scope,
                &run.review_order,
                style,
                !run.args.dry_run && !run.confirmed && !self.no_input,
            )?;
            let view = plan.view();
            let label = destination_label(
                view.columns().len(),
                run.target_names.len(),
                !implicit_targets,
                "target",
            );
            let selection = PlanSelection {
                targets: run.target_names.clone(),
                targets_explicit: !implicit_targets,
                scope: Some(run.scope),
                scope_explicit: run.args.scope.is_explicit(),
            };
            let revision = 0;
            let data = plan_event_data(
                &view,
                revision,
                run.args.dry_run,
                self.load_authorization(run),
                &selection,
            );
            let event = if revision == 0 {
                "plan"
            } else {
                "plan.updated"
            };
            self.reporter.event(event, Level::Info, data)?;
            for line in render_plan(&view, style) {
                self.reporter.human(&line)?;
            }
            self.reporter
                .human(&load_plan_footer(&actionable, identical, &label, style))?;
            if run.args.dry_run {
                self.reporter.human("")?;
                self.reporter.human("Dry run — no changes were made.")?;
            } else if self.authorize_load(run, &label)? {
                self.reporter.human("")?;
            } else {
                self.report_load_cancelled(run, implicit_targets)?;
                return Ok(false);
            }
        }

        let mut loaded = 0_usize;
        let mut overwritten = 0_usize;
        let mut skipped = 0_usize;
        self.apply_load_steps(run, &mut loaded, &mut overwritten, &mut skipped)?;
        let changed = loaded + overwritten;
        if changed > 0 && !run.args.dry_run {
            self.reporter.human("")?;
            let mut breakdown = Vec::new();
            if loaded > 0 {
                breakdown.push(format!("{loaded} loaded"));
            }
            if overwritten > 0 {
                breakdown.push(format!("{overwritten} overwritten"));
            }
            let mut description =
                format!("deployment{} changed", if changed == 1 { "" } else { "s" });
            if !breakdown.is_empty() {
                description = format!("{description} ({})", breakdown.join(", "));
            }
            let footer = result_footer(
                &[
                    ResultEntry {
                        marker: ResultMarker::Completed,
                        count: changed,
                        description,
                    },
                    ResultEntry {
                        marker: ResultMarker::Unchanged,
                        count: skipped,
                        description: "unchanged".to_owned(),
                    },
                ],
                style,
            );
            self.reporter.human(&footer)?;
        }
        self.report_sync_summary("load", changed, skipped, run.args.dry_run)?;
        Ok(true)
    }

    /// Apply every planned load step, reporting progress the plan already promised.
    fn apply_load_steps(
        &mut self,
        run: &LoadRun<'_>,
        loaded: &mut usize,
        overwritten: &mut usize,
        skipped: &mut usize,
    ) -> Result<()> {
        for step in &run.steps {
            if step.same {
                *skipped += 1;
                self.reporter.event(
                    "skill.skipped",
                    Level::Info,
                    skill_action_data(
                        &step.candidate,
                        &step.target,
                        Some(step.scope),
                        &step.destination,
                        run.args.dry_run,
                        "skipped",
                    ),
                )?;
                continue;
            }
            if !run.args.dry_run {
                deploy_skill(
                    &step.candidate.path,
                    &step.target.path,
                    self.repository.cache_root(),
                    self.hook,
                )?;
                // load's scope is decided once for the whole run, so the
                // progress line never needs a per-step scope suffix.
                let verb = if step.existed { "Overwrote" } else { "Loaded" };
                self.reporter.human(&format!(
                    "{verb} {} -> {}",
                    step.candidate.name, step.target.name
                ))?;
            }
            let action = if step.existed {
                *overwritten += 1;
                "overwritten"
            } else {
                *loaded += 1;
                "loaded"
            };
            self.reporter.event(
                "skill.loaded",
                Level::Info,
                skill_action_data(
                    &step.candidate,
                    &step.target,
                    Some(step.scope),
                    &step.destination,
                    run.args.dry_run,
                    action,
                ),
            )?;
        }
        Ok(())
    }

    /// Describe how this invocation authorizes its load plan.
    fn load_authorization(&self, run: &LoadRun<'_>) -> PlanAuthorization {
        let prompted = !run.args.dry_run && !run.confirmed && !self.no_input;
        let mode = if run.args.dry_run {
            "dry-run"
        } else if run.confirmed {
            "yes"
        } else if self.no_input {
            "noninteractive"
        } else {
            "prompt"
        };
        PlanAuthorization {
            kind: "binary",
            mode,
            default: prompted.then_some(true),
        }
    }

    /// Obtain consent for the rendered load plan.
    fn authorize_load(&mut self, run: &LoadRun<'_>, label: &str) -> Result<bool> {
        if run.confirmed {
            return Ok(true);
        }
        if self.no_input {
            // Machine mode keeps its established event-only contract; an
            // interactive-shaped stream still has to say yes explicitly.
            if self.reporter.is_json() {
                return Ok(true);
            }
            return Err(SkillManagerError::InteractionRequired(
                "applying this plan noninteractively requires --yes.".into(),
            ));
        }
        Ok(Authorizer::new(self.prompt)
            .confirm(&format!("Apply this load plan to {label}?"), false)?
            .is_approved())
    }

    /// Explain a declined load plan and how to change the next one.
    ///
    /// Flag teaching happens only here, on cancel — the rendered plan itself
    /// stays clean of any hint text.
    fn report_load_cancelled(&mut self, run: &LoadRun<'_>, implicit_targets: bool) -> Result<()> {
        self.report_cancelled("load")?;
        let inferred_scope = !run.args.scope.is_explicit();
        let hint = match (implicit_targets, inferred_scope) {
            (true, true) => Some(format!(
                "Hint: targets and scope were inferred. Re-run with {}, and --global or --project, to change this plan.",
                target_flag_hint(&run.target_names)
            )),
            (true, false) => Some(format!(
                "Hint: targets were inferred. Re-run with {} to change this plan.",
                target_flag_hint(&run.target_names)
            )),
            (false, true) => Some(
                "Hint: scope was inferred. Re-run with --global or --project to change this plan."
                    .to_owned(),
            ),
            (false, false) => None,
        };
        match hint {
            Some(line) => self.reporter.human(&line),
            None => Ok(()),
        }
    }

    /// State precisely why a load plan has nothing to do.
    fn report_load_no_work(&mut self, run: &LoadRun<'_>, implicit_targets: bool) -> Result<()> {
        let qualifier = if implicit_targets {
            "enabled"
        } else {
            "selected"
        };
        if run.steps.is_empty() {
            let message = if run.has_targets {
                "No installed skills matched this load.".to_owned()
            } else {
                format!("No {qualifier} targets are available for load.")
            };
            return self.reporter.human(&message);
        }
        let skills = run
            .steps
            .iter()
            .map(|step| step.candidate.name.as_str())
            .collect::<BTreeSet<_>>();
        let label = target_selection_label(run.target_names.len(), implicit_targets);
        let message = if skills.len() == 1 {
            format!(
                "{} is already identical across {label}.",
                skills
                    .iter()
                    .next()
                    .copied()
                    .unwrap_or("The selected skill")
            )
        } else {
            format!("All requested skills are already identical across {label}.")
        };
        self.reporter.human(&message)
    }

    /// Rendering vocabulary for this invocation's output stream.
    fn render_style(&self) -> RenderStyle {
        RenderStyle {
            symbols: self.reporter.is_interactive(),
            color: self.reporter.color_enabled(),
        }
    }

    /// Describe how this invocation authorizes its plan.
    fn update_authorization(&self, run: &UpdateRun<'_>) -> PlanAuthorization {
        let prompted = !run.args.dry_run && !run.confirmed && !self.no_input;
        let mode = if run.args.dry_run {
            "dry-run"
        } else if run.confirmed {
            "yes"
        } else if self.no_input {
            "noninteractive"
        } else {
            "prompt"
        };
        PlanAuthorization {
            kind: "binary",
            mode,
            default: prompted.then_some(true),
        }
    }

    /// Obtain consent for the rendered update plan.
    fn authorize_update(&mut self, run: &UpdateRun<'_>, label: &str) -> Result<bool> {
        if run.confirmed {
            return Ok(true);
        }
        if self.no_input {
            // Machine mode keeps its established event-only contract; an
            // interactive-shaped stream still has to say yes explicitly.
            if self.reporter.is_json() {
                return Ok(true);
            }
            return Err(SkillManagerError::InteractionRequired(
                "applying this plan noninteractively requires --yes.".into(),
            ));
        }
        Ok(Authorizer::new(self.prompt)
            .confirm(&format!("Apply this update plan to {label}?"), false)?
            .is_approved())
    }

    /// Explain a declined update plan and how to narrow the next one.
    fn report_update_cancelled(
        &mut self,
        run: &UpdateRun<'_>,
        implicit_targets: bool,
    ) -> Result<()> {
        self.report_cancelled("update")?;
        let inferred_scope = !run.args.scope.is_explicit();
        let hint = match (implicit_targets, inferred_scope) {
            (true, true) => Some(format!(
                "Hint: targets and deployed scopes were inferred. Re-run with {}, and --global or --project, to narrow this plan.",
                target_flag_hint(&run.target_names)
            )),
            (true, false) => Some(format!(
                "Hint: targets were inferred. Re-run with {} to narrow this plan.",
                target_flag_hint(&run.target_names)
            )),
            (false, true) => Some(
                "Hint: deployed scopes were inferred. Re-run with --global or --project to narrow this plan."
                    .to_owned(),
            ),
            (false, false) => None,
        };
        match hint {
            Some(line) => self.reporter.human(&line),
            None => Ok(()),
        }
    }

    /// State precisely why an update plan has nothing to do.
    fn report_update_no_work(&mut self, run: &UpdateRun<'_>, implicit_targets: bool) -> Result<()> {
        let qualifier = if implicit_targets {
            "enabled"
        } else {
            "selected"
        };
        if run.steps.is_empty() {
            let message = if run.has_targets {
                if run.args.filters.is_empty() && run.requested.len() == 1 {
                    format!(
                        "{} is not deployed to any {qualifier} target in {} scope.",
                        run.requested.first().map_or("", String::as_str),
                        scope_phrase(&run.args.scope)
                    )
                } else {
                    "No installed skills matched this update.".to_owned()
                }
            } else {
                format!("No {qualifier} targets are available for update.")
            };
            return self.reporter.human(&message);
        }
        let skills = run
            .steps
            .iter()
            .map(|step| step.candidate.name.as_str())
            .collect::<BTreeSet<_>>();
        let label = target_selection_label(run.target_names.len(), implicit_targets);
        let message = if skills.len() == 1 {
            format!(
                "{} is up to date across {label}.",
                skills
                    .iter()
                    .next()
                    .copied()
                    .unwrap_or("The selected skill")
            )
        } else {
            format!("All installed skills are up to date across {label}.")
        };
        self.reporter.human(&message)
    }

    /// Display the update plan and, when interactive, confirm it once.
    ///
    /// Machine mode keeps its existing event-only contract, so the plan is a
    /// human-output feature. A dry run and `--yes` both display the plan
    /// without prompting.
    fn confirm_update_plan(
        &mut self,
        steps: &[SyncStep],
        target_names: &[String],
        options: UpdatePlanOptions,
    ) -> Result<bool> {
        if self.reporter.is_json() || steps.is_empty() {
            return Ok(true);
        }
        let actionable = steps.iter().filter(|step| !step.same).collect::<Vec<_>>();
        if actionable.is_empty() {
            let skills = steps
                .iter()
                .map(|step| step.candidate.name.as_str())
                .collect::<BTreeSet<_>>();
            let message = if skills.len() == 1 {
                format!(
                    "{} is up to date across {}.",
                    skills
                        .iter()
                        .next()
                        .copied()
                        .unwrap_or("The selected skill"),
                    target_selection_label(target_names.len(), options.implicit_targets)
                )
            } else {
                format!(
                    "All installed skills are up to date across {}.",
                    target_selection_label(target_names.len(), options.implicit_targets)
                )
            };
            self.reporter.human(&message)?;
            return Ok(true);
        }

        let (entries, target_specific_details) = grouped_update_entries(&actionable, target_names)?;
        if options.context == UpdatePlanContext::ImportFollowUp {
            self.reporter.human("")?;
        }
        self.reporter.human(&styled_heading(
            "Update plan",
            self.reporter.color_enabled(),
        ))?;
        self.reporter.human("")?;
        for line in grouped_update_table(&entries, target_names, self.reporter.color_enabled()) {
            self.reporter.human(&line)?;
        }
        if !target_specific_details.is_empty() {
            self.reporter.human("")?;
            self.reporter.human("Target-specific changes")?;
            for (skill, deployments) in target_specific_details {
                self.reporter.human(&format!("  {skill}"))?;
                for (target, scope, totals) in deployments {
                    self.reporter
                        .human(&format!("    {target} · {}  {totals}", scope.as_str()))?;
                }
            }
        }
        self.reporter.human("")?;
        self.reporter.human(&format!(
            "{} across {}",
            counted_noun(actionable.len(), "update"),
            target_selection_label(target_names.len(), options.implicit_targets)
        ))?;
        if options.dry_run || options.confirmed || self.no_input {
            return Ok(true);
        }
        if self.prompt.confirm(
            &format!(
                "Apply this update plan to {}?",
                target_selection_label(target_names.len(), options.implicit_targets)
            ),
            true,
        )? {
            return Ok(true);
        }
        if options.context == UpdatePlanContext::Direct {
            self.report_cancelled("update")?;
        }
        Ok(false)
    }

    /// Review, authorize, and apply the copy plan.
    ///
    /// `copy` has one arbitrary path destination, so its plan is the shared
    /// model's degenerate-sentence case for a single matched skill and its
    /// ordinary table for more than one; no scope, no target selection, and no
    /// identical-hiding (unlike `load`, an overwrite is always shown because
    /// there is no existing-deployment concept to compare against ahead of
    /// the diff itself).
    #[allow(clippy::too_many_lines)]
    fn run_copy(&mut self, config: &Config, args: &CopyArgs) -> Result<bool> {
        let entry = configured_source_or_reference(config, &args.source, None)?;
        let resolved = materialize_source(
            self.repository,
            self.github,
            &entry,
            args.refresh,
            args.dry_run,
        )?;
        let discovery = discover_skills(&[resolved], &args.filters, &config.exclude)?;
        let destination = absolute_path(args.destination.clone())?;

        if discovery.winners.is_empty() {
            let message = if args.filters.is_empty() {
                format!("No skills found in {}.", args.source)
            } else {
                format!(
                    "No skills from {} matched {}.",
                    args.source,
                    copy_filter_clause(&args.filters)
                )
            };
            self.reporter.human(&message)?;
            self.emit_copy_summary(0, args.dry_run)?;
            return Ok(true);
        }

        // The same ordered collection builds the plan and drives the apply
        // loop below, so plan order and apply order cannot drift apart.
        let candidates = discovery.winners.values().collect::<Vec<_>>();
        let style = self.render_style();
        let prompting = !args.dry_run && !args.yes && !self.no_input;
        let plan = copy_change_plan(&candidates, &destination, prompting)?;
        let view = plan.view();
        let selection = PlanSelection {
            targets: Vec::new(),
            targets_explicit: true,
            scope: None,
            scope_explicit: true,
        };
        let revision = 0;
        let data = plan_event_data(
            &view,
            revision,
            args.dry_run,
            self.copy_authorization(args),
            &selection,
        );
        self.reporter.event("plan", Level::Info, data)?;
        for line in render_plan(&view, style) {
            self.reporter.human(&line)?;
        }
        self.reporter.human(&copy_plan_footer(&plan.rows, style))?;

        if args.dry_run {
            self.reporter.human("")?;
            self.reporter.human("Dry run — no changes were made.")?;
            self.emit_copy_summary(candidates.len(), args.dry_run)?;
            return Ok(true);
        }
        if !self.authorize_copy(args, candidates.len(), &destination)? {
            // Source, destination, and filtering are always explicit for
            // `copy`, so nothing was inferred and there is no hint to teach.
            self.report_cancelled("copy")?;
            return Ok(false);
        }
        self.reporter.human("")?;

        let target = Target {
            name: "copy".into(),
            label: "Copy destination".into(),
            path: destination.clone(),
            enabled: true,
            builtin: false,
            legacy_override: false,
        };
        let mut new_count = 0_usize;
        let mut overwritten = 0_usize;
        for candidate in &candidates {
            let output = target.path.join(&candidate.name);
            let existed = output.is_dir();
            deploy_skill(
                &candidate.path,
                &target.path,
                self.repository.cache_root(),
                self.hook,
            )?;
            let verb = if existed { "Overwrote" } else { "Copied" };
            self.reporter.human(&format!(
                "{verb} {} -> {}",
                candidate.name,
                output.display()
            ))?;
            let action = if existed {
                overwritten += 1;
                "overwritten"
            } else {
                new_count += 1;
                "copied"
            };
            self.reporter.event(
                "skill.copied",
                Level::Info,
                skill_action_data(candidate, &target, None, &output, args.dry_run, action),
            )?;
        }
        let changed = new_count + overwritten;
        self.reporter.human("")?;
        let mut breakdown = Vec::new();
        if new_count > 0 {
            breakdown.push(format!("{new_count} new"));
        }
        if overwritten > 0 {
            breakdown.push(format!("{overwritten} overwritten"));
        }
        let mut description = format!("skill{} copied", if changed == 1 { "" } else { "s" });
        if !breakdown.is_empty() {
            description = format!("{description} ({})", breakdown.join(", "));
        }
        let footer = result_footer(
            &[ResultEntry {
                marker: ResultMarker::Completed,
                count: changed,
                description,
            }],
            style,
        );
        self.reporter.human(&footer)?;
        self.emit_copy_summary(changed, args.dry_run)?;
        Ok(true)
    }

    /// Emit the shared `copy` `summary` payload shape from every exit path.
    fn emit_copy_summary(&mut self, copied: usize, dry_run: bool) -> Result<()> {
        self.reporter.event(
            "summary",
            Level::Info,
            json!({ "action": "copy", "copied": copied, "dry_run": dry_run }),
        )
    }

    /// Describe how this invocation authorizes its copy plan.
    fn copy_authorization(&self, args: &CopyArgs) -> PlanAuthorization {
        let prompted = !args.dry_run && !args.yes && !self.no_input;
        let mode = if args.dry_run {
            "dry-run"
        } else if args.yes {
            "yes"
        } else if self.no_input {
            "noninteractive"
        } else {
            "prompt"
        };
        PlanAuthorization {
            kind: "binary",
            mode,
            default: prompted.then_some(true),
        }
    }

    /// Obtain consent for the rendered copy plan.
    fn authorize_copy(
        &mut self,
        args: &CopyArgs,
        count: usize,
        destination: &Path,
    ) -> Result<bool> {
        if args.yes {
            return Ok(true);
        }
        if self.no_input {
            if self.reporter.is_json() {
                return Ok(true);
            }
            return Err(SkillManagerError::InteractionRequired(
                "applying this plan noninteractively requires --yes.".into(),
            ));
        }
        let prompt = if count == 1 {
            format!("Copy this skill to {}?", destination.display())
        } else {
            format!("Copy these {count} skills to {}?", destination.display())
        };
        Ok(Authorizer::new(self.prompt)
            .confirm(&prompt, false)?
            .is_approved())
    }

    // Import resolves one source, detects divergent deployments, plans, confirms,
    // and mirrors in a single ordered operation.
    #[allow(clippy::too_many_lines)]
    fn run_import(&mut self, config: &Config, args: &ImportArgs) -> Result<bool> {
        if is_fnmatch_operand(&args.skill) {
            return Err(SkillManagerError::InvalidInput(format!(
                "import selects exactly one skill and does not accept patterns: {}",
                args.skill
            )));
        }
        validate_skill_name(&args.skill)?;
        let sources = self.resolve_sources(
            config,
            &[],
            &SourceSelection::default(),
            false,
            args.dry_run,
        )?;
        let discovery = discover_skills(&sources, &[], &config.exclude)?;
        self.emit_collisions(&discovery.collisions)?;
        let candidate = discovery
            .winners
            .get(&fold(&args.skill))
            .cloned()
            .ok_or_else(|| SkillManagerError::NotFound {
                kind: "source skill",
                reference: args.skill.clone(),
            })?;
        // Detection compares deployments with the materialized source, so it runs
        // before the destination is resolved. Nothing to import must never ask
        // where an import would have been written.
        let target_templates = self.select_target_templates(config, &args.targets)?;
        let scope_context = scope_context(&self.home)?;
        let project_root = &scope_context.project_root;
        let inspected_scopes = available_scopes(&args.scope, scope_context.project_available);
        let mut detected = Vec::new();
        for template in &target_templates {
            for scope in &inspected_scopes {
                let target = scoped_target(template, *scope, &self.home, project_root);
                let deployment = target.target.path.join(&candidate.name);
                if !deployment.is_dir() || directories_equal(&candidate.path, &deployment)? {
                    continue;
                }
                detected.push((target.target, *scope, deployment));
            }
        }

        let mut imported = 0_usize;
        let mut skipped = 0_usize;
        if detected.is_empty() {
            skipped = 1;
            self.reporter.human(&format!(
                "Nothing to import: {} source is up to date with every selected deployment.",
                candidate.name
            ))?;
            self.reporter.event(
                "skill.import-skipped",
                Level::Info,
                skill_import_skipped_data(&candidate, args.dry_run),
            )?;
        } else {
            let Some(destination) = self.import_destination(&candidate)? else {
                return Ok(false);
            };
            let mut candidates = Vec::with_capacity(detected.len());
            for (target, scope, deployment) in detected {
                let stat = diff_directories(&destination, &deployment)?;
                candidates.push(ImportCandidate {
                    target,
                    scope,
                    deployment,
                    stat,
                });
            }
            let index = self.select_import_candidate(&candidate.name, &candidates)?;
            let selection = candidates.get(index).cloned().ok_or_else(|| {
                SkillManagerError::InvalidInput("import selection is out of range".into())
            })?;
            self.render_import_plan(&candidate, &selection, &destination)?;
            self.reporter.event(
                "skill.import-planned",
                Level::Info,
                skill_import_data(
                    &candidate,
                    &selection,
                    &destination,
                    args.dry_run,
                    "planned",
                ),
            )?;
            if !self.confirm_import(&candidate, &selection, args.dry_run, args.yes)? {
                return Ok(false);
            }
            if !args.dry_run && !args.yes && !self.no_input {
                self.reporter.human("")?;
            }
            if !args.dry_run {
                import_skill(
                    &selection.deployment,
                    &destination,
                    self.repository.cache_root(),
                    self.hook,
                )?;
            }
            imported = 1;
            let source_label = source_display_name(&candidate.source.entry);
            self.reporter.human(&format!(
                "Imported {} from {} · {} into {} (source){}.",
                candidate.name,
                selection.target.name,
                selection.scope.as_str(),
                source_label,
                if args.dry_run { " (dry-run)" } else { "" }
            ))?;
            self.reporter.event(
                "skill.imported",
                Level::Info,
                skill_import_data(
                    &candidate,
                    &selection,
                    &destination,
                    args.dry_run,
                    "imported",
                ),
            )?;
            if !args.dry_run && !self.no_input && !self.reporter.is_json() {
                self.offer_import_sync(config, &candidate, &selection, &destination)?;
            }
        }
        self.reporter.event(
            "summary",
            Level::Info,
            json!({
                "action": "import",
                "imported": imported,
                "skipped": skipped,
                "dry_run": args.dry_run
            }),
        )?;
        Ok(true)
    }

    /// Resolve the local source directory an import may overwrite.
    ///
    /// Returns `Ok(None)` when the user declines a GitHub-backed source's local
    /// alternate location.
    fn import_destination(&mut self, candidate: &SkillCandidate) -> Result<Option<PathBuf>> {
        let entry = &candidate.source.entry;
        if entry.source_type == SourceType::Local {
            return Ok(Some(portable_path(&candidate.path)));
        }
        let Some(SourceLocation::Local { path }) = entry.alternate.clone() else {
            return Err(SkillManagerError::InvalidInput(format!(
                "import writes to local source checkouts only; '{}' is GitHub-backed ({}) and has no local alternate location. Add one with: skill-manager source alternate {} <local-path>",
                entry.name,
                source_reference(entry),
                entry.name
            )));
        };
        let destination = portable_path(&if entry.mode == SourceMode::Single {
            path
        } else {
            path.join(&candidate.name)
        });
        if self.no_input {
            return Err(SkillManagerError::InteractionRequired(format!(
                "'{}' is GitHub-backed; importing into its local alternate requires interactive confirmation",
                entry.name
            )));
        }
        if self.prompt.confirm(
            &format!(
                "'{}' is GitHub-backed ({}); import into its local alternate instead?",
                entry.name,
                source_reference(entry)
            ),
            false,
        )? {
            return Ok(Some(destination));
        }
        self.report_cancelled("import")?;
        Ok(None)
    }

    /// Choose which divergent deployment supplies the imported content.
    fn select_import_candidate(
        &mut self,
        skill: &str,
        candidates: &[ImportCandidate],
    ) -> Result<usize> {
        if candidates.len() == 1 {
            return Ok(0);
        }
        if self.no_input {
            return Err(SkillManagerError::InteractionRequired(format!(
                "{} changed deployments of {skill} are importable; narrow the selection with --target, --global, or --project",
                candidates.len()
            )));
        }
        let choices = candidates
            .iter()
            .map(|item| {
                format!(
                    "{} · {}  {}",
                    item.target.name,
                    item.scope.as_str(),
                    totals_line(&item.stat)
                )
            })
            .collect::<Vec<_>>();
        self.prompt
            .choose(&format!("Choose the {skill} copy to import"), &choices)
    }

    /// Render the human-reviewable import plan.
    fn render_import_plan(
        &mut self,
        candidate: &SkillCandidate,
        selection: &ImportCandidate,
        destination: &Path,
    ) -> Result<()> {
        let color = self.reporter.color_enabled();
        let location = format!("{} · {}", selection.target.name, selection.scope.as_str());
        let source_label = format!("{} (source)", source_display_name(&candidate.source.entry));
        self.reporter.human("")?;
        self.reporter.human(&styled_heading(
            &format!("Import {}", candidate.name),
            color,
        ))?;
        self.reporter.human("")?;
        self.reporter.human(&format!("  From   {location}"))?;
        self.reporter.human(&format!("  Into   {source_label}"))?;
        if self.reporter.verbose() {
            self.reporter.human("")?;
            self.reporter.human(&format!(
                "  Deployment path   {}",
                portable_canonicalize(&selection.deployment).display()
            ))?;
            self.reporter.human(&format!(
                "  Source path       {}",
                portable_canonicalize(destination).display()
            ))?;
        }
        self.reporter.human("")?;
        self.reporter.human(&styled_heading("Changes", color))?;
        if !selection.stat.is_empty() {
            self.reporter.human("")?;
            for line in file_change_lines(&selection.stat, self.reporter.is_interactive(), color) {
                self.reporter.human(&line)?;
            }
        }
        self.reporter.human(&totals_line(&selection.stat))?;
        self.reporter.human("")
    }

    /// Confirm the destructive source overwrite unless it was pre-approved.
    fn confirm_import(
        &mut self,
        candidate: &SkillCandidate,
        selection: &ImportCandidate,
        dry_run: bool,
        confirmed: bool,
    ) -> Result<bool> {
        if dry_run || confirmed {
            return Ok(true);
        }
        if self.no_input {
            return Err(SkillManagerError::InteractionRequired(
                "import overwrites source content; pass --yes in noninteractive mode".into(),
            ));
        }
        if self.prompt.confirm(
            &format!(
                "Replace {} (source) with the {} · {} copy?",
                source_display_name(&candidate.source.entry),
                selection.target.name,
                selection.scope.as_str()
            ),
            false,
        )? {
            return Ok(true);
        }
        self.report_cancelled("import")?;
        Ok(false)
    }

    /// Offer to propagate freshly imported content to every other installed,
    /// enabled deployment that is now outdated.
    fn offer_import_sync(
        &mut self,
        config: &Config,
        candidate: &SkillCandidate,
        imported: &ImportCandidate,
        destination: &Path,
    ) -> Result<()> {
        let templates = self.select_target_templates(config, &TargetSelection::default())?;
        let target_names = templates
            .iter()
            .map(|target| target.target.name.clone())
            .collect::<Vec<_>>();
        let context = scope_context(&self.home)?;
        let scopes = available_scopes(&ScopeSelection::default(), context.project_available);
        let mut imported_candidate = candidate.clone();
        imported_candidate.path = destination.to_path_buf();
        let mut steps = Vec::new();
        for template in &templates {
            for scope in &scopes {
                if fold(&template.target.name) == fold(&imported.target.name)
                    && *scope == imported.scope
                {
                    continue;
                }
                let target = scoped_target(template, *scope, &self.home, &context.project_root);
                let deployment = target.target.path.join(&candidate.name);
                if !deployment.is_dir() || directories_equal(destination, &deployment)? {
                    continue;
                }
                steps.push(SyncStep {
                    candidate: imported_candidate.clone(),
                    target: target.target,
                    scope: *scope,
                    destination: deployment,
                    existed: true,
                    same: false,
                });
            }
        }
        if steps.is_empty() {
            return Ok(());
        }

        self.reporter.human("")?;
        let deployments = counted_noun(steps.len(), "other installed deployment");
        let verb = if steps.len() == 1 { "needs" } else { "need" };
        if !self.prompt.confirm(
            &format!("{deployments} {verb} this change. Review an update plan?"),
            true,
        )? {
            self.reporter
                .human("Other installed deployments were not updated.")?;
            return Ok(());
        }
        if !self.confirm_update_plan(
            &steps,
            &target_names,
            UpdatePlanOptions {
                implicit_targets: true,
                dry_run: false,
                confirmed: false,
                context: UpdatePlanContext::ImportFollowUp,
            },
        )? {
            self.reporter.human("")?;
            self.reporter
                .human("Imported successfully; other installed deployments were not updated.")?;
            return Ok(());
        }
        self.reporter.human("")?;
        for step in &steps {
            deploy_skill(
                &step.candidate.path,
                &step.target.path,
                self.repository.cache_root(),
                self.hook,
            )?;
            self.reporter.human(&format!(
                "Updated {} -> {} ({})",
                step.candidate.name,
                step.target.name,
                step.scope.as_str()
            ))?;
            self.reporter.event(
                "skill.updated",
                Level::Info,
                skill_action_data(
                    &step.candidate,
                    &step.target,
                    Some(step.scope),
                    &step.destination,
                    false,
                    "updated",
                ),
            )?;
        }
        Ok(())
    }

    /// Review, authorize, and apply the remove plan.
    ///
    /// Mirrors [`Self::run_update`] and [`Self::run_load`]: the complete plan
    /// — every skill, every destination it exists at, every file count —
    /// renders before anything is asked. Where a skill exists in both scopes
    /// and the caller did not say which, the plan itself is the question: a
    /// [`Decision`] with per-option blast radius, never a bare `[y/N]` count.
    #[allow(clippy::too_many_lines)]
    fn run_remove(&mut self, config: &Config, args: &RemoveArgs) -> Result<bool> {
        let target_templates = self.select_target_templates(config, &args.targets)?;
        let scope_context = scope_context(&self.home)?;
        let project_root = &scope_context.project_root;
        let inspected_scopes = available_scopes(&args.scope, scope_context.project_available);
        let mut deployed_names = BTreeMap::<String, String>::new();
        for template in &target_templates {
            for scope in &inspected_scopes {
                let scoped = scoped_target(template, *scope, &self.home, project_root);
                for (identity, name) in deployed_skills(&scoped.target.path)? {
                    deployed_names.entry(identity).or_insert(name);
                }
            }
        }
        let mut names = BTreeMap::<String, String>::new();
        let mut requested = Vec::<String>::new();
        let mut positional_patterns = Vec::new();
        if args.skills.is_empty() {
            let sources = self.resolve_sources(
                config,
                &[],
                &args.source_selection,
                args.refresh,
                args.dry_run,
            )?;
            let discovery = discover_skills(&sources, &args.filters, &config.exclude)?;
            for candidate in discovery.winners.values() {
                names.insert(fold(&candidate.name), candidate.name.clone());
            }
        } else {
            for raw in &args.skills {
                if is_fnmatch_operand(raw) {
                    positional_patterns.push(raw.clone());
                    continue;
                }
                let path = PathBuf::from(raw);
                if path.join("SKILL.md").is_file() {
                    let name = skill_name(&path)?;
                    if matches_patterns(&name, &args.filters)? {
                        names.insert(fold(&name), name.clone());
                        requested.push(name);
                    }
                } else if path.is_dir() {
                    let entry = source_from_reference(raw, Some(SourceMode::Collection))?;
                    let resolved = ResolvedSource {
                        path: entry.path.clone().ok_or_else(|| {
                            SkillManagerError::InvalidInput(format!(
                                "remove collection is not local: {raw}"
                            ))
                        })?,
                        entry,
                        from_cache: false,
                        temporary: None,
                    };
                    for skill in detect_skill_dirs(&resolved)? {
                        let name = skill_name(&skill)?;
                        if matches_patterns(&name, &args.filters)? {
                            names.insert(fold(&name), name.clone());
                            requested.push(name);
                        }
                    }
                } else {
                    validate_skill_name(raw)?;
                    if matches_patterns(raw, &args.filters)? {
                        names.insert(fold(raw), raw.clone());
                        requested.push(raw.clone());
                    }
                }
            }
        }
        // Captured before pattern expansion, mirroring `run_sync`'s
        // `positional_matched` formula: a literal name that already resolved
        // must count as a positional match on its own, even when an unrelated
        // pattern also given matches nothing.
        let literal_given = !requested.is_empty();
        // `positional_patterns` holds only genuine fnmatch operands (see the
        // `is_fnmatch_operand` split above); literal skill names never land
        // here. With `expand_skill_patterns` matching nothing for an empty
        // pattern list, this correctly expands to nothing extra when the
        // caller passed only literal names -- it must never widen to every
        // deployed skill. The `args.skills.is_empty()` branch above is the
        // sole, explicit "no operands: every discovered skill" opt-in.
        let expansion = expand_skill_patterns(
            &positional_patterns,
            deployed_names.values().map(String::as_str),
        )?;
        self.emit_unmatched_patterns(&expansion.unmatched_patterns)?;
        for name in &expansion.matched {
            if matches_patterns(name, &args.filters)? {
                names.insert(fold(name), name.clone());
                requested.push(name.clone());
            }
        }
        let positional_matched =
            literal_given || positional_patterns.is_empty() || !expansion.matched.is_empty();
        if !positional_matched {
            return Err(SkillManagerError::NotFound {
                kind: "deployed skill matching positional pattern",
                reference: positional_patterns.join(", "),
            });
        }

        let explicit = explicit_scope(&args.scope);
        let mut skill_plans = Vec::new();
        for name in names.values() {
            if let Some(skill_plan) = classify_remove_skill(
                name,
                &target_templates,
                explicit,
                scope_context.project_available,
                &self.home,
                project_root,
            )? {
                skill_plans.push(skill_plan);
            }
        }
        let discovered = skill_plans
            .iter()
            .map(|skill_plan| skill_plan.identity.clone())
            .collect::<Vec<_>>();
        let order = remove_review_order(&discovered, &requested);
        skill_plans.sort_by_key(|skill_plan| {
            order
                .iter()
                .position(|key| *key == fold(&skill_plan.identity))
                .unwrap_or(usize::MAX)
        });

        if skill_plans.is_empty() {
            self.report_remove_no_match(args, &requested, &target_templates)?;
            self.report_remove_summary(0, args.dry_run)?;
            return Ok(true);
        }

        let (unambiguous_total, ambiguous_total) = remove_ambiguity_counts(&skill_plans);
        let defers_to_branch = explicit.is_none() && !args.both && ambiguous_total > 0;
        let default_choice = if args.both {
            RemoveScopeChoice::Both
        } else if explicit == Some(Scope::Global) {
            RemoveScopeChoice::Global
        } else if explicit == Some(Scope::Project) {
            RemoveScopeChoice::Project
        } else {
            // Never exercised: reaching this branch means `ambiguous_total`
            // is zero, so every cell resolves independently of the choice.
            RemoveScopeChoice::Both
        };
        let target_names = target_templates
            .iter()
            .map(|template| template.target.name.clone())
            .collect::<Vec<_>>();
        let style = self.render_style();
        let prompting = !args.dry_run && !args.yes && !self.no_input;

        let (rows, mut resolved_items) =
            remove_plan_rows(&skill_plans, defers_to_branch, default_choice)?;
        let identities = skill_plans
            .iter()
            .map(|skill_plan| skill_plan.identity.clone())
            .collect::<Vec<_>>();
        let decisions =
            remove_scope_decisions(&skill_plans, unambiguous_total, ambiguous_total, args.both)?;
        let destinations = remove_destinations(&target_names);
        let body_heading = defers_to_branch.then(|| "Available deployments".to_owned());
        let plan = ChangePlan {
            command: "remove".to_owned(),
            plan_id: format!("remove:{}", identities.join(",")),
            heading: "Remove plan".to_owned(),
            metadata: Vec::new(),
            destinations,
            body_heading,
            metric_header: Some("files/deploy".to_owned()),
            detail_heading: "Destination-specific changes".to_owned(),
            connector: Some("from".to_owned()),
            rows,
            blocks: Vec::new(),
            decisions,
            prompting,
            distinguishes_overwrites: false,
        };
        let view = plan.view();
        let targets_explicit = args.targets.is_explicit();
        let label = destination_label(
            view.columns().len(),
            target_names.len(),
            targets_explicit,
            "target",
        );
        let selection = PlanSelection {
            targets: target_names.clone(),
            targets_explicit,
            scope: view.uniform_scope(),
            scope_explicit: args.scope.is_explicit(),
        };
        let authorization = self.remove_authorization(args, defers_to_branch, prompting);
        let data = plan_event_data(&view, 0, args.dry_run, authorization, &selection);
        self.reporter.event("plan", Level::Info, data)?;
        for line in render_plan(&view, style) {
            self.reporter.human(&line)?;
        }

        if defers_to_branch {
            if args.dry_run {
                let alternatives = plan
                    .decisions
                    .first()
                    .map_or(0, |decision| decision.options.len());
                self.reporter.human(&format!(
                    "Dry run — {alternatives} alternatives shown; no option selected and no changes were made."
                ))?;
                self.report_remove_summary(0, true)?;
                return Ok(true);
            }
            if args.yes || self.no_input {
                return Err(SkillManagerError::InteractionRequired(
                    "selected skills exist in both scopes; choose --project, --global, or --both before using --yes."
                        .into(),
                ));
            }
            let options = vec![
                SelectionOption::numbered(0, "Remove project copies", true),
                SelectionOption::numbered(1, "Remove global copies", true),
                SelectionOption::numbered(2, "Remove both copies", true),
            ];
            let choice = match Authorizer::new(self.prompt)
                .select("Select removal scope [1-3, c to cancel]", &options)?
            {
                Authorization::Cancelled => {
                    self.report_cancelled("remove")?;
                    return Ok(false);
                }
                Authorization::Approved(0) => RemoveScopeChoice::Project,
                Authorization::Approved(1) => RemoveScopeChoice::Global,
                Authorization::Approved(_) => RemoveScopeChoice::Both,
            };
            resolved_items = resolve_remove_apply_list(&skill_plans, choice);
            self.reporter.human("")?;
        } else {
            let (skills_total, files_total) = remove_totals_from_items(&resolved_items)?;
            self.reporter.human(&remove_plan_footer(
                view.actions(),
                &label,
                skills_total,
                files_total,
                style,
            ))?;
            if args.dry_run {
                self.reporter.human("")?;
                self.reporter.human("Dry run — no changes were made.")?;
            } else {
                let question = if view.actions() == 1 {
                    format!("Remove this deployment from {label}?")
                } else {
                    format!("Remove these {} deployments from {label}?", view.actions())
                };
                if self.authorize_remove(args, &question)? {
                    self.reporter.human("")?;
                } else {
                    self.report_remove_cancelled(
                        &target_names,
                        targets_explicit,
                        args.scope.is_explicit(),
                    )?;
                    return Ok(false);
                }
            }
        }

        // Captured before applying: the apply loop deletes the very
        // directories this walks to count files, so recomputing afterward
        // would silently read back zeros.
        let (result_skills_total, result_files_total) = remove_totals_from_items(&resolved_items)?;
        let removed = self.apply_remove_items(&resolved_items, args.dry_run)?;
        if removed > 0 && !args.dry_run {
            self.reporter.human("")?;
            let footer = result_footer(
                &[ResultEntry {
                    marker: ResultMarker::Completed,
                    count: removed,
                    description: format!(
                        "deployment{} removed ({}, {})",
                        if removed == 1 { "" } else { "s" },
                        counted_noun(result_skills_total, "skill"),
                        counted_noun(result_files_total, "file"),
                    ),
                }],
                style,
            );
            self.reporter.human(&footer)?;
        }
        self.report_remove_summary(removed, args.dry_run)?;
        Ok(true)
    }

    /// Emit the remove summary event from one shared, auditable site.
    fn report_remove_summary(&mut self, removed: usize, dry_run: bool) -> Result<()> {
        self.reporter.event(
            "summary",
            Level::Info,
            json!({ "action": "remove", "removed": removed, "dry_run": dry_run }),
        )
    }

    /// Apply every resolved removal, reporting progress the plan already
    /// promised.
    ///
    /// The event fires unconditionally (dry run included) so the machine
    /// stream stays complete; only the actual filesystem write and the human
    /// progress line are gated by `dry_run`, mirroring
    /// [`Self::apply_update_steps`].
    fn apply_remove_items(&mut self, items: &[RemoveApplyItem], dry_run: bool) -> Result<usize> {
        let mut removed = 0_usize;
        for item in items {
            let destination = item.root.join(&item.skill);
            if !dry_run {
                remove_skill(
                    &item.skill,
                    &item.root,
                    self.repository.cache_root(),
                    self.hook,
                )?;
                self.reporter.human(&format!(
                    "Removed {} from {} ({})",
                    item.skill,
                    item.target.name,
                    item.scope.as_str()
                ))?;
            }
            removed += 1;
            self.reporter.event(
                "skill.removed",
                Level::Info,
                json!({
                    "skill": item.skill,
                    "target": item.target.name,
                    "scope": item.scope,
                    "target_path": item.root,
                    "path": destination,
                    "action": "removed",
                    "dry_run": dry_run,
                }),
            )?;
        }
        Ok(removed)
    }

    /// Describe how this invocation authorizes its remove plan.
    fn remove_authorization(
        &self,
        args: &RemoveArgs,
        defers_to_branch: bool,
        prompting: bool,
    ) -> PlanAuthorization {
        let mode = if args.dry_run {
            "dry-run"
        } else if args.yes {
            "yes"
        } else if self.no_input {
            "noninteractive"
        } else {
            "prompt"
        };
        let kind = if defers_to_branch {
            "selection"
        } else {
            "binary"
        };
        PlanAuthorization {
            kind,
            mode,
            default: (kind == "binary")
                .then_some(prompting.then_some(false))
                .flatten(),
        }
    }

    /// Obtain consent for a resolved remove plan's plain `[y/N]` prompt.
    ///
    /// Unlike `load`/`update`/`copy`, `remove` never auto-authorizes a JSON
    /// stream under `--no-input`: removal is destructive and irreversible, so
    /// `--yes` is required regardless of output format.
    fn authorize_remove(&mut self, args: &RemoveArgs, question: &str) -> Result<bool> {
        if args.yes {
            return Ok(true);
        }
        if self.no_input {
            return Err(SkillManagerError::InteractionRequired(
                "applying this plan noninteractively requires --yes.".into(),
            ));
        }
        Ok(Authorizer::new(self.prompt)
            .confirm(question, true)?
            .is_approved())
    }

    /// Explain a declined remove plan and how to narrow the next one.
    ///
    /// Unlike the deferred-branch cancel (which never hints, because the
    /// numbered menu the user just answered already taught the scope
    /// decision), the plain `[y/N]` cancel teaches whichever of targets and
    /// scope were inferred rather than stated, mirroring
    /// `report_update_cancelled`/`report_load_cancelled`.
    fn report_remove_cancelled(
        &mut self,
        target_names: &[String],
        targets_explicit: bool,
        scope_explicit: bool,
    ) -> Result<()> {
        self.report_cancelled("remove")?;
        let hint = match (!targets_explicit, !scope_explicit) {
            (true, true) => Some(format!(
                "Hint: targets and deployed scopes were inferred. Re-run with {}, and --global or --project, to narrow this plan.",
                target_flag_hint(target_names)
            )),
            (true, false) => Some(format!(
                "Hint: targets were inferred. Re-run with {} to narrow this plan.",
                target_flag_hint(target_names)
            )),
            (false, true) => Some(
                "Hint: deployed scopes were inferred. Re-run with --global or --project to narrow this plan."
                    .to_owned(),
            ),
            (false, false) => None,
        };
        match hint {
            Some(line) => self.reporter.human(&line),
            None => Ok(()),
        }
    }

    /// State precisely why a remove plan has nothing to do.
    ///
    /// A single literal skill name with no other filters names the specific
    /// target and scope it was checked against, matching how narrowly the
    /// invocation asked; anything broader falls back to the generic message.
    fn report_remove_no_match(
        &mut self,
        args: &RemoveArgs,
        requested: &[String],
        target_templates: &[ScopedTarget],
    ) -> Result<()> {
        if args.filters.is_empty()
            && let [name] = requested
        {
            let scope_desc = match explicit_scope(&args.scope) {
                Some(scope) => format!("at {} scope", scope.as_str()),
                None => format!("in {} scope", scope_phrase(&args.scope)),
            };
            let target_desc = if args.targets.is_explicit() {
                match target_templates {
                    [only] => only.target.name.clone(),
                    _ => "any selected target".to_owned(),
                }
            } else {
                "any enabled target".to_owned()
            };
            return self.reporter.human(&format!(
                "{name} is not deployed to {target_desc} {scope_desc}."
            ));
        }
        self.reporter.human("No deployed skills matched.")
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Status discovery, deterministic row rendering, and aggregate counts form one cohesive read-only operation."
    )]
    fn run_status(&mut self, config: &Config, args: &StatusArgs) -> Result<()> {
        let sources =
            self.resolve_sources(config, &[], &args.source_selection, args.refresh, false)?;
        let (source_rows, source_names) = status_source_rows(config, &sources);
        self.reporter.human("Sources:")?;
        for line in source_table(&source_rows) {
            self.reporter.human(&line)?;
        }
        self.reporter.human("")?;
        let discovery = discover_skills(&sources, &[], &config.exclude)?;
        self.emit_collisions(&discovery.collisions)?;
        let target_templates = self.select_target_templates(config, &args.targets)?;
        let scope_context = scope_context(&self.home)?;
        let project_root = &scope_context.project_root;
        let inspected_scopes = available_scopes(&args.scope, scope_context.project_available);
        let mut names = BTreeMap::<String, String>::new();
        for (identity, candidate) in &discovery.winners {
            names.insert(identity.clone(), candidate.name.clone());
        }
        for template in &target_templates {
            for scope in &inspected_scopes {
                let target = scoped_target(template, *scope, &self.home, project_root);
                for (identity, name) in deployed_skills(&target.target.path)? {
                    names.entry(identity).or_insert(name);
                }
            }
        }
        let has_any_skills = !names.is_empty();
        let mut unmatched = Vec::new();
        for pattern in &args.filters {
            let one = std::slice::from_ref(pattern);
            let matched = names.iter().any(|(identity, name)| {
                status_matches(name, discovery.winners.get(identity), one).unwrap_or(false)
            });
            if !matched {
                unmatched.push(pattern.clone());
            }
        }
        self.emit_unmatched_patterns(&unmatched)?;
        let mut status_rows = Vec::new();
        for (identity, name) in names {
            let candidate = discovery.winners.get(&identity);
            if !status_matches(&name, candidate, &args.filters)?
                || !status_matches(&name, candidate, &args.option_filters)?
            {
                continue;
            }
            let mut states = IndexMap::new();
            let mut rendered_states = Vec::with_capacity(target_templates.len());
            let mut deployments = Vec::new();
            let mut target_scope_sets = Vec::<BTreeSet<Scope>>::new();
            let mut installed_global = false;
            let mut installed_project = false;
            let mut shadowed_global_divergent = false;
            for template in &target_templates {
                let observations = inspected_scopes
                    .iter()
                    .map(|scope| {
                        let target = scoped_target(template, *scope, &self.home, project_root);
                        let installed = target.target.path.join(&name).is_dir();
                        let state = skill_state(
                            candidate.map(|value| value.path.as_path()),
                            &target.target.path,
                            &name,
                        )?;
                        Ok((target, installed, state))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut installed_scopes = BTreeSet::new();
                for (target, installed, _) in &observations {
                    if *installed {
                        installed_scopes.insert(target.scope);
                        match target.scope {
                            Scope::Global => installed_global = true,
                            Scope::Project => installed_project = true,
                        }
                    }
                }
                if !installed_scopes.is_empty() {
                    target_scope_sets.push(installed_scopes);
                }
                let effective_index = observations
                    .iter()
                    .position(|(target, installed, _)| target.scope == Scope::Project && *installed)
                    .or_else(|| {
                        observations.iter().position(|(target, installed, _)| {
                            target.scope == Scope::Global && *installed
                        })
                    });
                let state_index = effective_index
                    .or_else(|| (!observations.is_empty()).then_some(observations.len() - 1));
                if let (Some(global), Some(project)) = (
                    observations
                        .iter()
                        .find(|(target, installed, _)| target.scope == Scope::Global && *installed),
                    observations.iter().find(|(target, installed, _)| {
                        target.scope == Scope::Project && *installed
                    }),
                ) {
                    let global_path = global.0.target.path.join(&name);
                    let project_path = project.0.target.path.join(&name);
                    if !directories_equal(&global_path, &project_path)? {
                        shadowed_global_divergent = true;
                    }
                }
                let effective_state = state_index
                    .and_then(|index| observations.get(index).map(|value| value.2))
                    .unwrap_or(crate::domain::SkillState::NotLoaded);
                states.insert(template.target.name.clone(), effective_state.as_str());
                rendered_states.push((template.target.name.clone(), effective_state));
                for (index, (target, installed, state)) in observations.into_iter().enumerate() {
                    deployments.push(DeploymentDetail {
                        target: target.target.name,
                        scope: target.scope,
                        path: target.target.path.join(&name),
                        installed,
                        state,
                        effective: Some(index) == effective_index,
                    });
                }
            }
            let location = match (installed_global, installed_project) {
                (true, false) => SkillLocation::Global,
                (false, true) => SkillLocation::Project,
                (true, true) => SkillLocation::Both,
                (false, false) => SkillLocation::None,
            };
            let mixed = target_scope_sets
                .first()
                .is_some_and(|first| target_scope_sets.iter().skip(1).any(|set| set != first));
            let source = candidate.map(|value| source_data(&value.source.entry));
            let source_name = candidate
                .and_then(|value| source_names.get(&value.source.entry.id))
                .cloned()
                .unwrap_or_else(|| "unknown".into());
            status_rows.push((
                SkillRow {
                    skill: name,
                    source: source_name,
                    targets: rendered_states,
                    location,
                    mixed,
                    shadowed_global_divergent,
                    deployments,
                },
                source,
                states,
            ));
        }
        if has_any_skills {
            let target_names = target_templates
                .iter()
                .map(|target| target.target.name.clone())
                .collect::<Vec<_>>();
            let rendered = status_rows
                .iter()
                .map(|(row, _, _)| row.clone())
                .collect::<Vec<_>>();
            for line in skill_table(
                &rendered,
                &target_names,
                self.reporter.is_interactive(),
                self.reporter.color_enabled(),
            ) {
                self.reporter.human(&line)?;
            }
        } else {
            self.reporter
                .human("No skills found in sources or deployed targets.")?;
        }
        if !args.filters.is_empty() && status_rows.is_empty() {
            return Err(SkillManagerError::NotFound {
                kind: "skill matching positional pattern",
                reference: args.filters.join(", "),
            });
        }
        for (row, source, states) in &status_rows {
            self.reporter.event(
                "status.row",
                Level::Info,
                json!({
                    "skill": row.skill,
                    "source": source,
                    "targets": states,
                    "location": row.location,
                    "mixed": row.mixed,
                    "shadowed_global_divergent": row.shadowed_global_divergent,
                    "deployments": row.deployments,
                }),
            )?;
        }
        if !status_rows.is_empty() {
            self.reporter.human("")?;
            let rendered = status_rows
                .iter()
                .map(|(row, _, _)| row.clone())
                .collect::<Vec<_>>();
            self.reporter.human(&status_summary_with_counts(
                &status_summary_counts(&rendered),
                self.reporter.is_interactive(),
                self.reporter.color_enabled(),
            ))?;
        }
        self.reporter.event(
            "summary",
            Level::Info,
            json!({ "action": "status", "skills": status_rows.len() }),
        )
    }

    // Resolve coordinates collision presentation, prompting, persistence, and event emission.
    #[allow(clippy::too_many_lines)]
    fn run_resolve(
        &mut self,
        config: &mut Config,
        active_path: &Path,
        args: &ResolveArgs,
    ) -> Result<()> {
        if let Some(preferred) = &args.prefer_source {
            let _configured = configured_source_index(config, preferred)?;
        }
        let sources =
            self.resolve_sources(config, &[], &args.source_selection, args.refresh, false)?;
        let discovery = discover_skills(&sources, &[], &config.exclude)?;
        let positional_patterns = args
            .skills
            .iter()
            .filter(|value| is_fnmatch_operand(value))
            .cloned()
            .collect::<Vec<_>>();
        // `ResolveArgs::skills` documents "omitted means every collision" as
        // an explicit contract. Opt in to that here instead of relying on
        // `expand_skill_patterns` to treat an empty pattern list as "select
        // everything" (it no longer does, and never implicitly should): a
        // bare literal skill name with no patterns must resolve to just that
        // name, not silently widen to every collision.
        let expansion = if args.skills.is_empty() {
            PatternExpansion {
                matched: discovery
                    .collisions
                    .values()
                    .filter_map(|candidates| {
                        candidates.first().map(|candidate| candidate.name.clone())
                    })
                    .collect(),
                unmatched_patterns: Vec::new(),
            }
        } else {
            expand_skill_patterns(
                &positional_patterns,
                discovery.collisions.values().filter_map(|candidates| {
                    candidates.first().map(|candidate| candidate.name.as_str())
                }),
            )?
        };
        self.emit_unmatched_patterns(&expansion.unmatched_patterns)?;
        let mut selected: BTreeSet<String> = args
            .skills
            .iter()
            .filter(|value| !is_fnmatch_operand(value))
            .map(|value| fold(value))
            .collect();
        selected.extend(expansion.matched.iter().map(|value| fold(value)));
        let mut resolved_count = 0_usize;
        for (identity, candidates) in discovery.collisions {
            if !selected.is_empty() && !selected.contains(&identity) {
                continue;
            }
            let winner_index = if let Some(preferred) = &args.prefer_source {
                candidates
                    .iter()
                    .position(|candidate| source_matches(&candidate.source.entry, preferred))
                    .ok_or_else(|| {
                        SkillManagerError::InvalidInput(format!(
                            "preferred source {preferred:?} is not a candidate for {}",
                            candidates[0].name
                        ))
                    })?
            } else if self.no_input {
                return Err(SkillManagerError::InteractionRequired(
                    "resolve requires --prefer-source in noninteractive mode".into(),
                ));
            } else {
                let choices: Vec<_> = candidates
                    .iter()
                    .map(|candidate| {
                        format!(
                            "{} ({})",
                            candidate.source.entry.label,
                            source_reference(&candidate.source.entry)
                        )
                    })
                    .collect();
                self.prompt.choose(
                    &format!("Choose source for {}", candidates[0].name),
                    &choices,
                )?
            };
            let skill = candidates[0].name.clone();
            for (index, candidate) in candidates.iter().enumerate() {
                if index == winner_index {
                    continue;
                }
                if let Some(config_index) = config
                    .sources
                    .iter()
                    .position(|entry| entry.id == candidate.source.entry.id)
                {
                    let entry = config.sources.get_mut(config_index).ok_or_else(|| {
                        SkillManagerError::InvalidInput("source index changed unexpectedly".into())
                    })?;
                    if !entry.exclude.iter().any(|value| fold(value) == identity) {
                        entry.exclude.push(skill.clone());
                    }
                } else {
                    self.reporter.diagnostic(&format!(
                        "Warning: cannot persist an exclusion for temporary source {}",
                        source_reference(&candidate.source.entry)
                    ))?;
                }
            }
            self.reporter.event(
                "collision.resolved",
                Level::Info,
                json!({
                    "skill": skill,
                    "preferred_source": source_data(&candidates[winner_index].source.entry)
                }),
            )?;
            resolved_count += 1;
        }
        if !positional_patterns.is_empty() && resolved_count == 0 {
            return Err(SkillManagerError::NotFound {
                kind: "collision matching positional pattern",
                reference: positional_patterns.join(", "),
            });
        }
        if resolved_count > 0 {
            self.repository.save(active_path, config)?;
        }
        self.reporter.event(
            "summary",
            Level::Info,
            json!({ "action": "resolve", "resolved": resolved_count }),
        )
    }

    fn resolve_sources(
        &mut self,
        config: &Config,
        explicit: &[String],
        selection: &crate::cli::SourceSelection,
        refresh: bool,
        dry_run: bool,
    ) -> Result<Vec<ResolvedSource>> {
        let mut entries = Vec::new();
        if !explicit.is_empty() {
            for reference in explicit {
                entries.push(configured_source_or_reference(config, reference, None)?);
            }
        } else if selection.cd_only {
            entries.push(source_from_reference(
                &std::env::current_dir()
                    .map_err(|error| SkillManagerError::io(".", error))?
                    .display()
                    .to_string(),
                None,
            )?);
        } else {
            entries.extend(config.sources.clone());
            if selection.cd {
                let cwd = source_from_reference(
                    &std::env::current_dir()
                        .map_err(|error| SkillManagerError::io(".", error))?
                        .display()
                        .to_string(),
                    None,
                )?;
                if !entries.iter().any(|entry| entry.id == cwd.id) {
                    entries.push(cwd);
                }
            }
        }
        let mut resolved = Vec::with_capacity(entries.len());
        for entry in entries {
            resolved.push(materialize_source(
                self.repository,
                self.github,
                &entry,
                refresh,
                dry_run,
            )?);
        }
        Ok(resolved)
    }

    fn select_target_templates(
        &mut self,
        config: &Config,
        selection: &TargetSelection,
    ) -> Result<Vec<ScopedTarget>> {
        let project_root = current_project_root()?;
        let all = resolved_targets_for_scope(config, &self.home, &project_root, Scope::Global);
        let mut explicit_names = BTreeSet::new();
        for requested in &selection.target_names {
            let target = all
                .values()
                .find(|target| fold(&target.target.name) == fold(requested))
                .ok_or_else(|| SkillManagerError::NotFound {
                    kind: "target",
                    reference: requested.clone(),
                })?;
            explicit_names.insert(fold(&target.target.name));
        }
        for (requested, enabled) in [
            ("claude", selection.claude),
            ("shared", selection.shared),
            ("antigravity", selection.antigravity),
        ] {
            if enabled
                && !explicit_names.contains(requested)
                && all
                    .get(requested)
                    .is_some_and(|target| !target.target.enabled)
            {
                return Err(SkillManagerError::InvalidInput(format!(
                    "target '{requested}' is disabled; use --target {requested} to override"
                )));
            }
        }
        let mut selected = Vec::new();
        for target in all.values() {
            let wanted = explicit_names.contains(&fold(&target.target.name))
                || selection.all_targets && target.target.enabled
                || selection.claude && target.target.name == "claude"
                || selection.shared && target.target.name == "shared"
                || selection.antigravity && target.target.name == "antigravity";
            if wanted {
                selected.push(target.clone());
            }
        }
        if selection.is_explicit() {
            return Ok(selected);
        }
        selected.extend(all.values().filter(|target| target.target.enabled).cloned());
        Ok(selected)
    }

    fn load_scope(
        &self,
        targets: &[ScopedTarget],
        selection: &ScopeSelection,
        project_root: &Path,
    ) -> Scope {
        if let Some(scope) = explicit_scope(selection) {
            return scope;
        }
        if !project_scope_available(&self.home, project_root) {
            return Scope::Global;
        }
        let project_default = targets.iter().any(|target| {
            target
                .template
                .components()
                .next()
                .is_some_and(|component| project_root.join(component.as_os_str()).is_dir())
        });
        if project_default {
            Scope::Project
        } else {
            Scope::Global
        }
    }

    fn emit_unmatched_patterns(&mut self, patterns: &[String]) -> Result<()> {
        for pattern in patterns {
            let message = format!("skill pattern matched nothing: {pattern}");
            self.emit_pattern_diagnostic(&message, pattern)?;
        }
        Ok(())
    }

    /// Emit the shared message-only `diagnostic` warning shape, on both the
    /// human and NDJSON channels.
    fn emit_message_diagnostic(&mut self, message: &str) -> Result<()> {
        self.reporter.diagnostic(&format!("Warning: {message}"))?;
        self.reporter
            .event("diagnostic", Level::Warning, json!({ "message": message }))
    }

    /// Emit the shared `diagnostic` warning shape carrying a `message` and a
    /// `pattern`/operand field, on both the human and NDJSON channels.
    fn emit_pattern_diagnostic(&mut self, message: &str, pattern: &str) -> Result<()> {
        self.reporter.diagnostic(&format!("Warning: {message}"))?;
        self.reporter.event(
            "diagnostic",
            Level::Warning,
            json!({ "message": message, "pattern": pattern }),
        )
    }

    /// Check `word` against the skills just discovered. If it names a
    /// discovered skill (case-insensitively), that skill wins; when a
    /// same-named directory also exists under `cwd`, a warning names the
    /// ambiguity and points at `./word` to force the directory
    /// interpretation instead. Shared by both the preliminary
    /// (`resolve_deferred_sync_operands`) and post-promotion
    /// (`resolve_provisional_sync_operands`) resolution passes so the
    /// ambiguity rule cannot drift between them.
    ///
    /// # Errors
    ///
    /// Returns an error only if the ambiguity diagnostic cannot be emitted.
    fn resolve_skill_word_with_ambiguity_warning(
        &mut self,
        word: &str,
        cwd: &Path,
        discovery: &SkillDiscovery,
    ) -> Result<Option<String>> {
        let Some(candidate) = discovery.winners.get(&fold(word)) else {
            return Ok(None);
        };
        let name = candidate.name.clone();
        if cwd.join(word).is_dir() {
            let message = format!(
                "\"{word}\" matches both a discovered skill and a directory in the current working directory; the skill was selected. Use \"./{word}\" to select the directory as a source instead."
            );
            self.emit_message_diagnostic(&message)?;
        }
        Ok(Some(name))
    }

    /// Resolve `load`/`update` bare words left unclassified before discovery.
    ///
    /// Each deferred word is checked, in order, against the skills just
    /// discovered and then against the current working directory:
    ///
    /// 1. A discovered skill name (case-insensitively) selects that skill,
    ///    applying the ambiguity warning via
    ///    [`Self::resolve_skill_word_with_ambiguity_warning`] when a
    ///    same-named CWD directory also exists.
    /// 2. Otherwise, an existing CWD directory is returned so the caller can
    ///    promote it to a source and re-resolve once, preserving the
    ///    historical bare-relative-directory behavior.
    /// 3. Otherwise, the word is provisionally unresolved: it may still
    ///    resolve once a promoted directory's skills are discovered (see
    ///    [`Self::resolve_provisional_sync_operands`]). When no word in the
    ///    batch triggered a directory promotion, there is no further chance
    ///    for these words to resolve, so they hard-error immediately here,
    ///    exactly as before this refinement.
    ///
    /// # Errors
    ///
    /// Returns an error when a diagnostic cannot be emitted, or (when no
    /// directory was promoted) a deferred word matches no source,
    /// directory, or skill.
    fn resolve_deferred_sync_operands(
        &mut self,
        deferred: &[String],
        cwd: &Path,
        discovery: &SkillDiscovery,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        let mut literal_skill_names = Vec::new();
        let mut promoted_sources = Vec::new();
        let mut provisional = Vec::new();
        if deferred.is_empty() {
            return Ok((literal_skill_names, promoted_sources, provisional));
        }
        for word in deferred {
            if let Some(name) =
                self.resolve_skill_word_with_ambiguity_warning(word, cwd, discovery)?
            {
                literal_skill_names.push(name);
            } else if cwd.join(word).is_dir() {
                promoted_sources.push(word.clone());
            } else {
                provisional.push(word.clone());
            }
        }
        // No directory is being promoted, so there is no second discovery
        // pass coming: any word that is still unresolved must hard-error now.
        if promoted_sources.is_empty()
            && let Some(word) = provisional.first()
        {
            return Err(SkillManagerError::NoSourceDirectoryOrSkill {
                reference: word.clone(),
            });
        }
        Ok((literal_skill_names, promoted_sources, provisional))
    }

    /// Resolve deferred words left provisionally unresolved by
    /// [`Self::resolve_deferred_sync_operands`] against the final discovery
    /// that followed a directory promotion, applying the same
    /// discovered-skill-vs-CWD-directory ambiguity check (via
    /// [`Self::resolve_skill_word_with_ambiguity_warning`]) as the
    /// preliminary pass. A word still unmatched is a hard error, identical
    /// in shape to the immediate-error path above.
    ///
    /// # Errors
    ///
    /// Returns an error when a diagnostic cannot be emitted, or a
    /// provisional word still matches no discovered skill after the final
    /// discovery pass.
    fn resolve_provisional_sync_operands(
        &mut self,
        provisional: &[String],
        cwd: &Path,
        discovery: &SkillDiscovery,
    ) -> Result<Vec<String>> {
        let mut literal_skill_names = Vec::new();
        for word in provisional {
            match self.resolve_skill_word_with_ambiguity_warning(word, cwd, discovery)? {
                Some(name) => literal_skill_names.push(name),
                None => {
                    return Err(SkillManagerError::NoSourceDirectoryOrSkill {
                        reference: word.clone(),
                    });
                }
            }
        }
        Ok(literal_skill_names)
    }

    fn emit_collisions(
        &mut self,
        collisions: &IndexMap<String, Vec<SkillCandidate>>,
    ) -> Result<()> {
        for candidates in collisions.values() {
            let winner = &candidates[0];
            self.reporter.diagnostic(&format!(
                "Warning: {} is supplied by {} sources; using {}",
                winner.name,
                candidates.len(),
                winner.source.entry.name
            ))?;
            self.reporter.event(
                "collision.detected",
                Level::Warning,
                json!({
                    "skill": winner.name,
                    "winner": source_data(&winner.source.entry),
                    "candidates": candidates
                        .iter()
                        .map(|candidate| source_data(&candidate.source.entry))
                        .collect::<Vec<_>>()
                }),
            )?;
        }
        Ok(())
    }
}

fn command_dry_run(command: &Command) -> bool {
    match command {
        Command::Load(args) => args.sync.dry_run,
        Command::Update(args) => args.sync.dry_run,
        Command::Import(args) => args.dry_run,
        Command::Copy(args) => args.dry_run,
        Command::Remove(args) => args.dry_run,
        _ => false,
    }
}

fn current_project_root() -> Result<PathBuf> {
    std::env::current_dir().map_err(|error| SkillManagerError::io(".", error))
}

fn scope_context(home: &Path) -> Result<ScopeContext> {
    let project_root = current_project_root()?;
    Ok(ScopeContext {
        project_available: project_scope_available(home, &project_root),
        project_root,
    })
}

fn project_scope_available(home: &Path, project_root: &Path) -> bool {
    !paths_equal(home, project_root)
}

fn available_scopes(selection: &ScopeSelection, project_available: bool) -> Vec<Scope> {
    explicit_scope(selection).map_or_else(
        || {
            if project_available {
                vec![Scope::Global, Scope::Project]
            } else {
                vec![Scope::Global]
            }
        },
        |scope| vec![scope],
    )
}

fn explicit_scope(selection: &ScopeSelection) -> Option<Scope> {
    if selection.project {
        Some(Scope::Project)
    } else if selection.global {
        Some(Scope::Global)
    } else {
        None
    }
}

fn scoped_target(
    template: &ScopedTarget,
    scope: Scope,
    home: &Path,
    project_root: &Path,
) -> ScopedTarget {
    let mut target = template.target.clone();
    target.path = scope.root(home, project_root).join(&template.template);
    ScopedTarget {
        target,
        template: template.template.clone(),
        scope,
    }
}

fn update_scopes(
    template: &ScopedTarget,
    candidate: &SkillCandidate,
    selection: &ScopeSelection,
    home: &Path,
    project_root: &Path,
    project_available: bool,
) -> Vec<Scope> {
    if let Some(scope) = explicit_scope(selection) {
        let target = scoped_target(template, scope, home, project_root);
        return target
            .target
            .path
            .join(&candidate.name)
            .is_dir()
            .then_some(scope)
            .into_iter()
            .collect();
    }
    let mut scopes = Vec::with_capacity(2);
    let global = scoped_target(template, Scope::Global, home, project_root);
    if global.target.path.join(&candidate.name).is_dir() {
        scopes.push(Scope::Global);
    }
    if project_available {
        let project = scoped_target(template, Scope::Project, home, project_root);
        if project.target.path.join(&candidate.name).is_dir() {
            scopes.push(Scope::Project);
        }
    }
    scopes
}

/// Build the shared change plan for one `update` invocation.
///
/// Every enabled target contributes both scope destinations even when only one
/// is planned: significance gating, not this builder, decides which survive, so
/// the same construction works unchanged when `load`, `remove`, `copy`, and
/// `import` migrate onto it.
/// The single skill order both plan review and apply follow.
///
/// Names the user gave positionally come first, in the order they gave them;
/// everything else keeps discovery order. Deriving one sequence and ranking
/// both the plan rows and the apply steps from it is what makes it impossible
/// for the CLI to act in an order other than the one it rendered.
fn review_sequence(steps: &[SyncStep], requested: &[String]) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    let mut push = |key: String| {
        if !order.contains(&key) {
            order.push(key);
        }
    };
    let present = steps
        .iter()
        .map(|step| fold(&step.candidate.name))
        .collect::<BTreeSet<_>>();
    for name in requested {
        let key = fold(name);
        if present.contains(&key) {
            push(key);
        }
    }
    for step in steps {
        push(fold(&step.candidate.name));
    }
    order
}

fn update_change_plan(
    actionable: &[&SyncStep],
    target_names: &[String],
    args: &SyncArgs,
    review_order: &[String],
    style: RenderStyle,
    prompting: bool,
) -> Result<ChangePlan> {
    let mut destinations = Vec::new();
    for name in target_names {
        for scope in [Scope::Global, Scope::Project] {
            destinations.push(Destination {
                id: destination_id(name, scope),
                column: name.clone(),
                label: format!("{name} · {}", scope.as_str()),
                kind: DestinationKind::Deployment {
                    target: name.clone(),
                    scope,
                },
                path: None,
            });
        }
    }

    let mut grouped = BTreeMap::<String, Vec<&SyncStep>>::new();
    for step in actionable {
        grouped
            .entry(fold(&step.candidate.name))
            .or_default()
            .push(step);
    }
    let order = review_order
        .iter()
        .filter(|key| grouped.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();

    let mut rows = Vec::with_capacity(order.len());
    let mut identities = Vec::with_capacity(order.len());
    for key in &order {
        let Some(steps) = grouped.get(key) else {
            continue;
        };
        let mut actions = Vec::with_capacity(steps.len());
        for step in steps {
            let stat = diff_directories(&step.destination, &step.candidate.path)?;
            actions.push(PlannedAction {
                destination: destination_id(&step.target.name, step.scope),
                action: PlanAction::Update,
                existed: step.existed,
                description: totals_line(&stat),
                stat,
            });
        }
        let identity = steps
            .first()
            .map(|step| step.candidate.name.clone())
            .unwrap_or_default();
        identities.push(identity.clone());
        rows.push(PlanRow {
            identity,
            actions,
            ..PlanRow::default()
        });
    }

    let scopes = actionable
        .iter()
        .map(|step| step.scope)
        .collect::<BTreeSet<_>>();
    let mut metadata = Vec::new();
    if let [scope] = scopes.iter().copied().collect::<Vec<_>>().as_slice()
        && !args.scope.is_explicit()
        && let Some(location) = review_location(*scope)
    {
        metadata.push((
            "Scope".to_owned(),
            format!("{} (inferred)", location_text(location, style.symbols)),
        ));
    }

    Ok(ChangePlan {
        command: "update".to_owned(),
        plan_id: format!("update:{}", identities.join(",")),
        heading: "Update plan".to_owned(),
        metadata,
        destinations,
        body_heading: None,
        metric_header: None,
        detail_heading: "Destination-specific changes".to_owned(),
        connector: Some("->".to_owned()),
        rows,
        blocks: Vec::new(),
        decisions: Vec::new(),
        prompting,
        distinguishes_overwrites: false,
    })
}

/// Build the load plan: new installs and overwrites side by side. Every
/// requested step is present as a machine-visible entry — including an
/// already-identical deployment, kept as a [`PlanAction::Skip`] action so the
/// structured `plan` event stays complete — but a row whose every action is
/// such a skip is dormant: [`PlanView::visible_rows`] hides it from the
/// table, from column significance, and from progress lines, and it is
/// counted only in the footer.
fn load_change_plan(
    steps: &[&SyncStep],
    target_names: &[String],
    args: &SyncArgs,
    scope: Scope,
    review_order: &[String],
    style: RenderStyle,
    prompting: bool,
) -> Result<ChangePlan> {
    let mut destinations = Vec::new();
    for name in target_names {
        for candidate_scope in [Scope::Global, Scope::Project] {
            destinations.push(Destination {
                id: destination_id(name, candidate_scope),
                column: name.clone(),
                label: format!("{name} · {}", candidate_scope.as_str()),
                kind: DestinationKind::Deployment {
                    target: name.clone(),
                    scope: candidate_scope,
                },
                path: None,
            });
        }
    }
    // Every step (whether actionable or already identical) knows its own
    // resolved target root, so the destination list can report `path` for
    // every id a step actually touches without re-resolving anything. This
    // is the target's root directory, not the skill-specific deployment
    // path within it: a `Destination` is one write location shared by every
    // row, not a per-skill subpath.
    for step in steps {
        let id = destination_id(&step.target.name, step.scope);
        if let Some(destination) = destinations.iter_mut().find(|d| d.id == id) {
            destination.path = Some(step.target.path.clone());
        }
    }

    let mut grouped = BTreeMap::<String, Vec<&SyncStep>>::new();
    for step in steps {
        grouped
            .entry(fold(&step.candidate.name))
            .or_default()
            .push(step);
    }
    let order = review_order
        .iter()
        .filter(|key| grouped.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();

    let mut rows = Vec::with_capacity(order.len());
    let mut identities = Vec::with_capacity(order.len());
    for key in &order {
        let Some(steps) = grouped.get(key) else {
            continue;
        };
        let mut actions = Vec::with_capacity(steps.len());
        for step in steps {
            let stat = diff_directories(&step.destination, &step.candidate.path)?;
            let (action, description) = if step.same {
                (PlanAction::Skip, String::new())
            } else if step.existed {
                (PlanAction::Update, totals_line(&stat))
            } else {
                (PlanAction::Load, creation_line("deployment", &stat))
            };
            actions.push(PlannedAction {
                destination: destination_id(&step.target.name, step.scope),
                action,
                existed: step.existed,
                description,
                stat,
            });
        }
        let identity = steps
            .first()
            .map(|step| step.candidate.name.clone())
            .unwrap_or_default();
        let provenance = steps
            .first()
            .map(|step| source_display_name(&step.candidate.source.entry).to_owned());
        identities.push(identity.clone());
        rows.push(PlanRow {
            identity,
            provenance,
            actions,
            ..PlanRow::default()
        });
    }

    let mut metadata = Vec::new();
    if !args.scope.is_explicit()
        && let Some(location) = review_location(scope)
    {
        metadata.push((
            "Scope".to_owned(),
            format!("{} (inferred)", location_text(location, style.symbols)),
        ));
    }

    Ok(ChangePlan {
        command: "load".to_owned(),
        plan_id: format!("load:{}", identities.join(",")),
        heading: "Load plan".to_owned(),
        metadata,
        destinations,
        body_heading: None,
        metric_header: None,
        detail_heading: "Destination-specific changes".to_owned(),
        connector: Some("->".to_owned()),
        rows,
        blocks: Vec::new(),
        decisions: Vec::new(),
        prompting,
        distinguishes_overwrites: true,
    })
}

/// The load plan footer: total actionable changes, then nonzero-only new,
/// overwrite, and already-identical clauses.
///
/// The leading `+`/`↑`/`✓` glyphs are a TTY-only convenience like every other
/// compact symbol in this plan; a redirected stream drops them rather than
/// substituting a second, redundant word (the count and category noun already
/// read as plain English on their own).
fn load_plan_footer(
    actionable: &[&SyncStep],
    identical: usize,
    label: &str,
    style: RenderStyle,
) -> String {
    let new_count = actionable.iter().filter(|step| !step.existed).count();
    let overwrite_count = actionable.iter().filter(|step| step.existed).count();
    let clause = |symbol: &str, count: usize, noun: &str, code: Option<u8>| {
        let text = if style.symbols {
            format!("{symbol} {count} {noun}")
        } else {
            format!("{count} {noun}")
        };
        colored(&text, code, style.color)
    };
    let mut clauses = Vec::new();
    if new_count > 0 {
        clauses.push(clause("+", new_count, "new", PlanAction::Load.color_code()));
    }
    if overwrite_count > 0 {
        clauses.push(clause(
            "↑",
            overwrite_count,
            "overwrite",
            PlanAction::Update.color_code(),
        ));
    }
    if identical > 0 {
        clauses.push(clause(
            "✓",
            identical,
            "already identical",
            PlanAction::Skip.color_code(),
        ));
    }
    format!(
        "{} across {label}: {}",
        counted_noun(actionable.len(), "change"),
        clauses.join(", ")
    )
}

fn destination_id(target: &str, scope: Scope) -> String {
    format!("{}:{}", fold(target), scope.as_str())
}

/// Build the copy plan: one arbitrary path destination shared by every row.
///
/// Every row references the same single destination id, so the shared
/// [`render_body`](crate::review) machinery collapses a single matched skill
/// to the degenerate sentence and renders two or more as a table whose only
/// destination column is literally named `action` — the destination path
/// itself is stated once in the metadata line instead of being repeated.
fn copy_change_plan(
    candidates: &[&SkillCandidate],
    destination: &Path,
    prompting: bool,
) -> Result<ChangePlan> {
    let destination_id = "action".to_owned();
    let destinations = vec![Destination {
        id: destination_id.clone(),
        column: "action".to_owned(),
        label: "action".to_owned(),
        kind: DestinationKind::Path,
        path: Some(destination.to_path_buf()),
    }];

    let mut rows = Vec::with_capacity(candidates.len());
    let mut identities = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let output = destination.join(&candidate.name);
        let existed = output.is_dir();
        let stat = diff_directories(&output, &candidate.path)?;
        let (action, description) = if existed {
            (PlanAction::Update, totals_line(&stat))
        } else {
            (PlanAction::Copy, creation_line("copy", &stat))
        };
        identities.push(candidate.name.clone());
        rows.push(PlanRow {
            identity: candidate.name.clone(),
            actions: vec![PlannedAction {
                destination: destination_id.clone(),
                action,
                existed,
                description,
                stat,
            }],
            ..PlanRow::default()
        });
    }

    Ok(ChangePlan {
        command: "copy".to_owned(),
        plan_id: format!("copy:{}", identities.join(",")),
        heading: "Copy plan".to_owned(),
        metadata: vec![("Destination".to_owned(), destination.display().to_string())],
        destinations,
        body_heading: None,
        metric_header: None,
        detail_heading: "Destination-specific changes".to_owned(),
        connector: None,
        rows,
        blocks: Vec::new(),
        decisions: Vec::new(),
        prompting,
        distinguishes_overwrites: true,
    })
}

/// The copy plan footer: total changes to the one destination, then
/// nonzero-only new and overwrite clauses. Unlike `load`, copy has no
/// already-identical clause: there is no existing-deployment concept to
/// compare against ahead of the diff, so an unchanged copy is out of scope.
fn copy_plan_footer(rows: &[PlanRow], style: RenderStyle) -> String {
    let existed = |row: &PlanRow| row.actions.first().is_some_and(|action| action.existed);
    let new_count = rows.iter().filter(|row| !existed(row)).count();
    let overwrite_count = rows.iter().filter(|row| existed(row)).count();
    let clause = |symbol: &str, count: usize, noun: &str, code: Option<u8>| {
        let text = if style.symbols {
            format!("{symbol} {count} {noun}")
        } else {
            format!("{count} {noun}")
        };
        colored(&text, code, style.color)
    };
    let mut clauses = Vec::new();
    if new_count > 0 {
        clauses.push(clause("+", new_count, "new", PlanAction::Copy.color_code()));
    }
    if overwrite_count > 0 {
        clauses.push(clause(
            "↑",
            overwrite_count,
            "overwrite",
            PlanAction::Update.color_code(),
        ));
    }
    format!(
        "{} to 1 destination: {}",
        counted_noun(rows.len(), "change"),
        clauses.join(", ")
    )
}

/// Render `--filter` clauses the way the "no match" message quotes them back.
fn copy_filter_clause(filters: &[String]) -> String {
    filters
        .iter()
        .map(|pattern| format!("--filter \"{pattern}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

fn review_location(scope: Scope) -> Option<SkillLocation> {
    location_of(&BTreeSet::from([scope]))
}

/// Classify one skill's deployments into `remove`'s per-target cells.
///
/// An explicit scope inspects only its own field; inference always checks
/// global and checks project only when it is available (mirroring
/// [`update_scopes`]). A skill with no populated cell anywhere is not
/// deployed and is silently omitted — the caller decides what "nothing
/// matched" means.
fn classify_remove_skill(
    name: &str,
    target_templates: &[ScopedTarget],
    explicit: Option<Scope>,
    project_available: bool,
    home: &Path,
    project_root: &Path,
) -> Result<Option<RemoveSkillPlan>> {
    let mut cells = Vec::with_capacity(target_templates.len());
    for template in target_templates {
        let mut global_root = None;
        let mut project_root_path = None;
        match explicit {
            Some(Scope::Global) => {
                let resolved = scoped_target(template, Scope::Global, home, project_root);
                if resolved.target.path.join(name).is_dir() {
                    global_root = Some(resolved.target.path);
                }
            }
            Some(Scope::Project) => {
                let resolved = scoped_target(template, Scope::Project, home, project_root);
                if resolved.target.path.join(name).is_dir() {
                    project_root_path = Some(resolved.target.path);
                }
            }
            None => {
                let global = scoped_target(template, Scope::Global, home, project_root);
                if global.target.path.join(name).is_dir() {
                    global_root = Some(global.target.path);
                }
                if project_available {
                    let project = scoped_target(template, Scope::Project, home, project_root);
                    if project.target.path.join(name).is_dir() {
                        project_root_path = Some(project.target.path);
                    }
                }
            }
        }
        if global_root.is_none() && project_root_path.is_none() {
            continue;
        }
        // Each present scope's file count is measured from its own
        // deployment, never borrowed from the other scope: they can and do
        // drift apart, and a representative count here would silently
        // understate or overstate whichever scope the user actually picks.
        let global_files = match &global_root {
            Some(root) => directory_files(&root.join(name))?.len(),
            None => 0,
        };
        let project_files = match &project_root_path {
            Some(root) => directory_files(&root.join(name))?.len(),
            None => 0,
        };
        cells.push(RemoveCell {
            target: template.target.clone(),
            global_root,
            global_files,
            project_root: project_root_path,
            project_files,
        });
    }
    if cells.is_empty() {
        return Ok(None);
    }
    Ok(Some(RemoveSkillPlan {
        identity: name.to_owned(),
        cells,
    }))
}

/// The single skill order both plan review and apply follow for `remove`.
///
/// Mirrors [`review_sequence`]'s two-phase technique — requested names first
/// in the order they were given, then everything else in discovery order —
/// over `remove`'s own per-skill classification instead of a flat
/// [`SyncStep`] list, since a removal row aggregates cells rather than one
/// step per destination.
fn remove_review_order(discovered: &[String], requested: &[String]) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    let mut push = |key: String| {
        if !order.contains(&key) {
            order.push(key);
        }
    };
    let present = discovered
        .iter()
        .map(|name| fold(name))
        .collect::<BTreeSet<_>>();
    for name in requested {
        let key = fold(name);
        if present.contains(&key) {
            push(key);
        }
    }
    for name in discovered {
        push(fold(name));
    }
    order
}

/// Count how many cells are unambiguous (only one scope present) vs
/// ambiguous (both scopes present) across every skill's cells.
///
/// Used only to decide whether a real branch exists and how many deployments
/// the preamble reports as removed under every option regardless of choice.
/// Each option's own deployment and file totals are computed separately by
/// [`remove_choice_totals`], directly from [`resolve_remove_apply_list`], so
/// they can never drift from what applying that option would actually do.
fn remove_ambiguity_counts(skill_plans: &[RemoveSkillPlan]) -> (usize, usize) {
    let mut unambiguous_total = 0_usize;
    let mut ambiguous_total = 0_usize;
    for skill_plan in skill_plans {
        for cell in &skill_plan.cells {
            match (cell.global_root.is_some(), cell.project_root.is_some()) {
                (true, true) => ambiguous_total += 1,
                (true, false) | (false, true) => unambiguous_total += 1,
                (false, false) => {}
            }
        }
    }
    (unambiguous_total, ambiguous_total)
}

/// One row's file-count hint: the shared count when every present
/// deployment agrees, or an explicit `min-max` range the moment they don't,
/// so this informational column can never imply a false agreement between
/// deployments that have actually drifted apart.
fn remove_metric_display(counts: &[usize]) -> String {
    let min = counts.iter().copied().min().unwrap_or(0);
    let max = counts.iter().copied().max().unwrap_or(0);
    if min == max {
        min.to_string()
    } else {
        format!("{min}-{max}")
    }
}

/// The scopes one cell resolves to under one [`RemoveScopeChoice`].
///
/// An unambiguous cell (only one scope populated) ignores the choice
/// entirely and resolves to whichever scope exists — proven against the
/// Stage 1 fixture's "N unambiguous deployments are removed in every option."
fn resolved_cell_scopes(cell: &RemoveCell, choice: RemoveScopeChoice) -> Vec<Scope> {
    if cell.global_root.is_some() && cell.project_root.is_some() {
        return match choice {
            RemoveScopeChoice::Project => vec![Scope::Project],
            RemoveScopeChoice::Global => vec![Scope::Global],
            RemoveScopeChoice::Both => vec![Scope::Global, Scope::Project],
        };
    }
    let mut scopes = Vec::with_capacity(1);
    if cell.global_root.is_some() {
        scopes.push(Scope::Global);
    }
    if cell.project_root.is_some() {
        scopes.push(Scope::Project);
    }
    scopes
}

/// Flatten every skill's cells into the final ordered, apply-ready list.
///
/// This is the single source of truth for both the resolved row actions
/// (built at render time) and the apply loop (built once the branch, if any,
/// is resolved): both call this with the same `choice`, so plan order and
/// apply order cannot drift apart.
fn resolve_remove_apply_list(
    skill_plans: &[RemoveSkillPlan],
    choice: RemoveScopeChoice,
) -> Vec<RemoveApplyItem> {
    let mut items = Vec::new();
    for skill_plan in skill_plans {
        for cell in &skill_plan.cells {
            for scope in resolved_cell_scopes(cell, choice) {
                let root = match scope {
                    Scope::Global => cell.global_root.clone(),
                    Scope::Project => cell.project_root.clone(),
                }
                .unwrap_or_default();
                items.push(RemoveApplyItem {
                    skill: skill_plan.identity.clone(),
                    target: cell.target.clone(),
                    scope,
                    root,
                });
            }
        }
    }
    items
}

/// Build the destination grid for `remove`: every enabled target contributes
/// both scope destinations, exactly mirroring [`update_change_plan`] and
/// [`load_change_plan`] — significance gating, not this builder, decides
/// which survive rendering.
fn remove_destinations(target_names: &[String]) -> Vec<Destination> {
    let mut destinations = Vec::with_capacity(target_names.len() * 2);
    for name in target_names {
        for scope in [Scope::Global, Scope::Project] {
            destinations.push(Destination {
                id: destination_id(name, scope),
                column: name.clone(),
                label: format!("{name} · {}", scope.as_str()),
                kind: DestinationKind::Deployment {
                    target: name.clone(),
                    scope,
                },
                path: None,
            });
        }
    }
    destinations
}

/// Build `remove`'s rows: pure availability while the scope branch is still
/// open, or concrete [`PlannedAction::Remove`] actions once it is resolved
/// (whether by explicit scope, `--both`, or an inference that never actually
/// branched). Also returns the flat apply list for the resolved case; the
/// deferred case builds its apply list only after interactive selection.
///
/// The single-row, single-action, no-availability case is `remove`'s
/// degenerate sentence (shared [`render_body`](crate::review) mechanism): the
/// table's bare file count reads oddly as prose, so this mutates that one row
/// to say `"N files"` instead, matching the `ux-guidelines.md` example
/// (`− managing-skills from claude: 3 files`).
fn remove_plan_rows(
    skill_plans: &[RemoveSkillPlan],
    defers_to_branch: bool,
    choice: RemoveScopeChoice,
) -> Result<(Vec<PlanRow>, Vec<RemoveApplyItem>)> {
    if defers_to_branch {
        let mut rows = Vec::with_capacity(skill_plans.len());
        for skill_plan in skill_plans {
            let mut availability = Vec::new();
            let mut counts = Vec::new();
            for cell in &skill_plan.cells {
                if cell.global_root.is_some() {
                    availability.push(destination_id(&cell.target.name, Scope::Global));
                    counts.push(cell.global_files);
                }
                if cell.project_root.is_some() {
                    availability.push(destination_id(&cell.target.name, Scope::Project));
                    counts.push(cell.project_files);
                }
            }
            rows.push(PlanRow {
                identity: skill_plan.identity.clone(),
                metric: Some(remove_metric_display(&counts)),
                availability,
                ..PlanRow::default()
            });
        }
        return Ok((rows, Vec::new()));
    }

    let mut rows = Vec::with_capacity(skill_plans.len());
    let mut items = Vec::new();
    for skill_plan in skill_plans {
        let mut actions = Vec::new();
        let mut counts = Vec::new();
        for cell in &skill_plan.cells {
            for scope in resolved_cell_scopes(cell, choice) {
                let root = match scope {
                    Scope::Global => cell.global_root.clone(),
                    Scope::Project => cell.project_root.clone(),
                }
                .unwrap_or_default();
                let stat = diff_directories(&root.join(&skill_plan.identity), Path::new(""))?;
                counts.push(stat.files_changed());
                actions.push(PlannedAction {
                    destination: destination_id(&cell.target.name, scope),
                    action: PlanAction::Remove,
                    existed: true,
                    description: String::new(),
                    stat,
                });
                items.push(RemoveApplyItem {
                    skill: skill_plan.identity.clone(),
                    target: cell.target.clone(),
                    scope,
                    root,
                });
            }
        }
        rows.push(PlanRow {
            identity: skill_plan.identity.clone(),
            metric: Some(remove_metric_display(&counts)),
            actions,
            ..PlanRow::default()
        });
    }
    if let [row] = rows.as_mut_slice()
        && let [action] = row.actions.as_mut_slice()
        && row.availability.is_empty()
        && let Some(metric) = row.metric.take()
    {
        let count = metric.parse::<usize>().unwrap_or(0);
        action.description = counted_noun(count, "file");
    }
    Ok((rows, items))
}

/// Sum the skill and file counts of a resolved, apply-ready removal list.
///
/// Recomputes each item's diff rather than reusing a row's cached
/// [`PlannedAction::stat`] because the deferred (branch) case has no row
/// actions yet at the point this is needed — the interactive selection
/// resolves the apply list directly without a second render.
fn remove_totals_from_items(items: &[RemoveApplyItem]) -> Result<(usize, usize)> {
    let skills = items
        .iter()
        .map(|item| fold(&item.skill))
        .collect::<BTreeSet<_>>()
        .len();
    let mut files = 0_usize;
    for item in items {
        let stat = diff_directories(&item.root.join(&item.skill), Path::new(""))?;
        files += stat.files_changed();
    }
    Ok((skills, files))
}

/// Total deployments and files one scope choice would actually remove.
///
/// Resolves the exact apply list [`resolve_remove_apply_list`] would hand to
/// [`App::apply_remove_items`] for this choice and diffs each item for real,
/// rather than combining a per-skill representative count across cells. This
/// is what keeps a branch option's advertised blast radius from drifting
/// away from what selecting it actually deletes when deployments across
/// scopes have genuinely diverged.
fn remove_choice_totals(
    skill_plans: &[RemoveSkillPlan],
    choice: RemoveScopeChoice,
) -> Result<(usize, usize)> {
    let items = resolve_remove_apply_list(skill_plans, choice);
    let deployments = items.len();
    let mut files = 0_usize;
    for item in &items {
        let stat = diff_directories(&item.root.join(&item.skill), Path::new(""))?;
        files += stat.files_changed();
    }
    Ok((deployments, files))
}

/// Build one removal alternative whose blast radius is too wide to enumerate
/// per destination, so it travels as typed aggregate totals.
fn remove_scope_option(
    id: &str,
    token: &str,
    label: String,
    deployments: usize,
    files: usize,
) -> DecisionOption {
    let mut totals = vec![("deployments".to_owned(), deployments as u64)];
    let effect = if files > 0 {
        totals.push(("files".to_owned(), files as u64));
        format!("− {deployments} deployments, {files} files")
    } else {
        format!("− {deployments} deployments")
    };
    DecisionOption {
        id: id.to_owned(),
        token: token.to_owned(),
        label,
        effect: Some(effect),
        effect_color: PlanAction::Remove.color_code(),
        consequence: OptionConsequence {
            operation: Some(PlanAction::Remove),
            totals,
            ..OptionConsequence::default()
        },
        ..DecisionOption::default()
    }
}

/// Build `remove`'s `removal_scope` decision when a real branch exists.
///
/// Unambiguous deployments are removed by every option, so the preamble and
/// each label's "where both exist" suffix appear only when there are any —
/// confirmed against the Stage 1 fixtures, which show both forms. `--both`
/// pre-resolves the dimension to `"both"`, which still carries every
/// option's typed consequence in the structured event but is filtered out of
/// every render by [`PlanView::decisions`](crate::review::PlanView::decisions).
///
/// Each option's deployment and file totals come from
/// [`remove_choice_totals`], which resolves and diffs that exact choice's
/// real apply list — never from multiplying a shared per-skill count across
/// cells, which silently misreports blast radius the moment a skill's
/// deployments have drifted apart across scopes.
fn remove_scope_decisions(
    skill_plans: &[RemoveSkillPlan],
    unambiguous_total: usize,
    ambiguous_total: usize,
    both: bool,
) -> Result<Vec<Decision>> {
    if ambiguous_total == 0 {
        return Ok(Vec::new());
    }
    let preamble = (unambiguous_total > 0).then(|| {
        format!(
            "{} are removed in every option.",
            counted_noun(unambiguous_total, "unambiguous deployment")
        )
    });
    let suffix = if unambiguous_total > 0 {
        " where both exist"
    } else {
        ""
    };
    let (project_deployments, project_files) =
        remove_choice_totals(skill_plans, RemoveScopeChoice::Project)?;
    let (global_deployments, global_files) =
        remove_choice_totals(skill_plans, RemoveScopeChoice::Global)?;
    let (both_deployments, both_files) =
        remove_choice_totals(skill_plans, RemoveScopeChoice::Both)?;
    let options = vec![
        remove_scope_option(
            "project",
            "1",
            format!("Remove project copies{suffix}"),
            project_deployments,
            project_files,
        ),
        remove_scope_option(
            "global",
            "2",
            format!("Remove global copies{suffix}"),
            global_deployments,
            global_files,
        ),
        remove_scope_option(
            "both",
            "3",
            format!("Remove both copies{suffix}"),
            both_deployments,
            both_files,
        ),
    ];
    Ok(vec![Decision {
        id: "removal_scope".to_owned(),
        preamble,
        prompt: "Select removal scope".to_owned(),
        options,
        resolved: both.then(|| "both".to_owned()),
        ..Decision::default()
    }])
}

/// The resolved remove plan footer: total removals, then the skill and file
/// counts they cover.
fn remove_plan_footer(
    actions: usize,
    label: &str,
    skills: usize,
    files: usize,
    style: RenderStyle,
) -> String {
    let clause_text = if style.symbols {
        format!("− {actions} remove")
    } else {
        format!("{actions} remove")
    };
    let clause = colored(&clause_text, PlanAction::Remove.color_code(), style.color);
    format!(
        "{} across {label}: {clause}; {}, {}",
        counted_noun(actions, "deployment removal"),
        counted_noun(skills, "skill"),
        counted_noun(files, "file"),
    )
}

/// Describe the scope a plan searched, for a plan that found nothing.
fn scope_phrase(scope: &ScopeSelection) -> &'static str {
    match (scope.global, scope.project) {
        (true, false) => "global",
        (false, true) => "project",
        _ => "global or project",
    }
}

/// Render the target flags that would make an inferred target set explicit.
fn target_flag_hint(target_names: &[String]) -> String {
    let mut flags = target_names
        .iter()
        .filter(|name| is_builtin_name(name))
        .map(|name| format!("--{name}"))
        .collect::<Vec<_>>();
    flags.push("--all".to_owned());
    format!("{}, or --target NAME", flags.join(", "))
}

fn grouped_update_entries(
    actionable: &[&SyncStep],
    target_names: &[String],
) -> Result<(Vec<GroupedUpdateEntry>, TargetSpecificChangeDetails)> {
    let mut grouped = BTreeMap::<String, Vec<&SyncStep>>::new();
    for step in actionable {
        grouped
            .entry(fold(&step.candidate.name))
            .or_default()
            .push(step);
    }
    let mut entries = Vec::with_capacity(grouped.len());
    let mut target_specific_details = Vec::new();
    for steps in grouped.values() {
        let mut changes = BTreeSet::new();
        let mut scopes = vec![Vec::new(); target_names.len()];
        let mut deployment_changes = Vec::new();
        for step in steps {
            let totals = totals_line(&diff_directories(&step.destination, &step.candidate.path)?);
            changes.insert(totals.clone());
            deployment_changes.push((step.target.name.clone(), step.scope, totals));
            if let Some(index) = target_names
                .iter()
                .position(|target| fold(target) == fold(&step.target.name))
            {
                scopes[index].push(step.scope);
            }
        }
        let target_scopes = scopes
            .into_iter()
            .map(|scopes| match scopes.as_slice() {
                [] => None,
                [Scope::Global] => Some("global".into()),
                [Scope::Project] => Some("project".into()),
                _ => Some("both".into()),
            })
            .collect();
        if changes.len() > 1 {
            target_specific_details.push((steps[0].candidate.name.clone(), deployment_changes));
        }
        entries.push(GroupedUpdateEntry {
            skill: steps[0].candidate.name.clone(),
            change: if changes.len() == 1 {
                changes.into_iter().next().unwrap_or_default()
            } else {
                format!("{} target-specific changes", changes.len())
            },
            target_scopes,
        });
    }
    Ok((entries, target_specific_details))
}

fn styled_heading(text: &str, color: bool) -> String {
    if color {
        format!("\u{1b}[1;36m{text}\u{1b}[0m")
    } else {
        text.to_owned()
    }
}

fn enabled_target_label(count: usize) -> String {
    counted_noun(count, "enabled target")
}

fn target_selection_label(count: usize, implicit: bool) -> String {
    if implicit {
        enabled_target_label(count)
    } else {
        counted_noun(count, "selected target")
    }
}

fn counted_noun(count: usize, singular: &str) -> String {
    format!("{count} {singular}{}", if count == 1 { "" } else { "s" })
}

fn aligned_table(headers: &[&str], rows: &[Vec<String>]) -> Vec<String> {
    let mut widths = headers
        .iter()
        .map(|header| display_width(header))
        .collect::<Vec<_>>();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(display_width(cell));
        }
    }
    let header = headers
        .iter()
        .enumerate()
        .map(|(index, header)| padded(header, widths[index]))
        .collect::<Vec<_>>();
    let mut lines = vec![join_columns(&header), separator(&widths)];
    lines.extend(rows.iter().map(|row| {
        let columns = row
            .iter()
            .enumerate()
            .map(|(index, cell)| padded(cell, widths[index]))
            .collect::<Vec<_>>();
        join_columns(&columns)
    }));
    lines
}

fn aligned_table_with_status(
    headers: &[&str],
    rows: &[Vec<String>],
    status_index: usize,
    color: bool,
) -> Vec<String> {
    let mut widths = headers
        .iter()
        .map(|header| display_width(header))
        .collect::<Vec<_>>();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(display_width(cell));
        }
    }
    let header = headers
        .iter()
        .enumerate()
        .map(|(index, header)| padded(header, widths[index]))
        .collect::<Vec<_>>();
    let mut lines = vec![join_columns(&header), separator(&widths)];
    lines.extend(rows.iter().map(|row| {
        let columns = row
            .iter()
            .enumerate()
            .map(|(index, cell)| {
                let padding = " ".repeat(widths[index].saturating_sub(display_width(cell)));
                if index == status_index && color {
                    let code = match cell.as_str() {
                        "enabled" | "valid" => Some(32),
                        "disabled" => Some(2),
                        "invalid" => Some(31),
                        _ => None,
                    };
                    code.map_or_else(
                        || format!("{cell}{padding}"),
                        |code| format!("\u{1b}[{code}m{cell}\u{1b}[0m{padding}"),
                    )
                } else {
                    format!("{cell}{padding}")
                }
            })
            .collect::<Vec<_>>();
        join_columns(&columns)
    }));
    lines
}

fn advanced_settings_lines(config: &Config) -> Vec<String> {
    let mut lines = vec!["Global exclusions".into()];
    if config.exclude.is_empty() {
        lines.push("  none".into());
    } else {
        lines.extend(config.exclude.iter().map(|pattern| format!("  {pattern}")));
    }

    lines.push(String::new());
    lines.push("Built-in settings".into());
    if config.builtins.is_empty() {
        lines.push("  none".into());
    } else {
        for (name, settings) in &config.builtins {
            lines.push(format!("  {name}"));
            let mut values = vec![("enabled".into(), settings.enabled.to_string())];
            flatten_map(&settings.extra, &mut values);
            lines.extend(indented_key_values(&values, 4));
        }
    }

    lines.push(String::new());
    lines.push("Legacy target overrides".into());
    if config.legacy_target_overrides.is_empty() {
        lines.push("  none".into());
    } else {
        for (name, target) in &config.legacy_target_overrides {
            lines.push(format!("  {name}"));
            let mut values = vec![
                ("path".into(), target.path.display().to_string()),
                (
                    "label".into(),
                    if target.label.is_empty() {
                        "none".into()
                    } else {
                        target.label.clone()
                    },
                ),
                ("enabled".into(), target.enabled.to_string()),
            ];
            flatten_map(&target.extra, &mut values);
            lines.extend(indented_key_values(&values, 4));
        }
    }

    lines.push(String::new());
    lines.push("Extension fields".into());
    if config.extra.is_empty() {
        lines.push("  none".into());
    } else {
        let mut values = Vec::new();
        flatten_map(&config.extra, &mut values);
        lines.extend(indented_key_values(&values, 2));
    }
    lines
}

fn flatten_map(values: &IndexMap<String, Value>, output: &mut Vec<(String, String)>) {
    for (key, value) in values {
        flatten_value(key, value, output);
    }
}

fn flatten_value(prefix: &str, value: &Value, output: &mut Vec<(String, String)>) {
    match value {
        Value::Object(values) if values.is_empty() => output.push((prefix.into(), "empty".into())),
        Value::Object(values) => {
            for (key, value) in values {
                flatten_value(&format!("{prefix}.{key}"), value, output);
            }
        }
        Value::Array(values) if values.is_empty() => output.push((prefix.into(), "none".into())),
        Value::Array(values) if values.iter().all(Value::is_string) => output.push((
            prefix.into(),
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        )),
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                flatten_value(&format!("{prefix}[{}]", index + 1), value, output);
            }
        }
        Value::Null => output.push((prefix.into(), "none".into())),
        Value::Bool(value) => output.push((prefix.into(), value.to_string())),
        Value::Number(value) => output.push((prefix.into(), value.to_string())),
        Value::String(value) => output.push((prefix.into(), value.clone())),
    }
}

fn indented_key_values(values: &[(String, String)], indent: usize) -> Vec<String> {
    let width = values
        .iter()
        .map(|(key, _)| display_width(key))
        .max()
        .unwrap_or_default();
    let prefix = " ".repeat(indent);
    values
        .iter()
        .map(|(key, value)| format!("{prefix}{}  {value}", padded(key, width)))
        .collect()
}

fn backup_data(backup: &ConfigBackup) -> Value {
    json!({
        "id": backup.metadata.id,
        "created_at": backup.metadata.created_at,
        "reason": backup.metadata.reason,
        "original_path": backup.metadata.original_path,
        "present": backup.metadata.present,
        "schema_version": backup.metadata.schema_version,
        "valid": backup.metadata.valid,
        "raw_path": backup.raw_path,
    })
}

fn source_data(source: &SourceEntry) -> Value {
    json!({
        "source": source_reference(source),
        "source_id": source.id,
        "source_name": source.name,
        "source_label": source.label,
        "source_type": source.source_type,
        "mode": source.mode,
        "alternate": source.alternate.as_ref().map(location_data)
    })
}

fn source_display_name(source: &SourceEntry) -> &str {
    if source.label.is_empty() {
        &source.name
    } else {
        &source.label
    }
}

fn location_data(location: &SourceLocation) -> Value {
    let source_type = match location {
        SourceLocation::Local { .. } => SourceType::Local,
        SourceLocation::GitHub { .. } => SourceType::GitHub,
    };
    json!({
        "source": location_reference(location),
        "source_type": source_type
    })
}

fn source_snapshot(source: &SourceEntry) -> Value {
    json!({
        "source": source_reference(source),
        "source_type": source.source_type,
        "alternate": source.alternate.as_ref().map(location_data)
    })
}

fn source_change_data(current: &SourceEntry, previous: &SourceEntry, changed: bool) -> Value {
    let mut data = source_data(current);
    if let Some(object) = data.as_object_mut() {
        object.insert("changed".into(), json!(changed));
        object.insert("previous".into(), source_snapshot(previous));
    }
    data
}

fn source_selector_index(config: &Config, selector: &str) -> Result<usize> {
    if let Some(index) = configured_source_index(config, selector)? {
        return Ok(index);
    }
    Err(SkillManagerError::NotFound {
        kind: "source",
        reference: selector.to_owned(),
    })
}

fn configured_source_index(config: &Config, selector: &str) -> Result<Option<usize>> {
    if let Some(index) = find_source_index(config, selector)? {
        return Ok(Some(index));
    }
    if let Ok(candidate) = location_from_reference(selector, SourceMode::Collection)
        && let Some(source) = config.sources.iter().find(|source| {
            source
                .alternate
                .as_ref()
                .is_some_and(|alternate| locations_equal(alternate, &candidate))
        })
    {
        return Err(SkillManagerError::InvalidInput(format!(
            "alternate locations are not source selectors; use '{}' or '{}'",
            source.name, source.id
        )));
    }
    Ok(None)
}

fn configured_source_or_reference(
    config: &Config,
    reference: &str,
    mode: Option<SourceMode>,
) -> Result<SourceEntry> {
    if let Some(index) = configured_source_index(config, reference)? {
        return config.sources.get(index).cloned().ok_or_else(|| {
            SkillManagerError::InvalidInput("source index changed unexpectedly".into())
        });
    }
    source_from_reference(reference, mode)
}

fn find_location_owner<'a>(
    config: &'a Config,
    location: &SourceLocation,
    except_index: Option<usize>,
) -> Option<&'a SourceEntry> {
    let identity = location_identity(location);
    config
        .sources
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != except_index)
        .find_map(|(_, source)| {
            let active_matches =
                source_location(source).is_ok_and(|active| location_identity(&active) == identity);
            let alternate_matches = source
                .alternate
                .as_ref()
                .is_some_and(|alternate| location_identity(alternate) == identity);
            (active_matches || alternate_matches).then_some(source)
        })
}

fn reject_location_collision(
    config: &Config,
    location: &SourceLocation,
    source_index: usize,
) -> Result<()> {
    if let Some(existing) = find_location_owner(config, location, Some(source_index)) {
        return Err(SkillManagerError::InvalidInput(format!(
            "location is already configured by source '{}': {}",
            existing.name,
            location_reference(location)
        )));
    }
    Ok(())
}

fn status_source_rows(
    config: &Config,
    sources: &[ResolvedSource],
) -> (Vec<SourceRow>, IndexMap<String, String>) {
    let current_directory = std::env::current_dir()
        .ok()
        .map(|path| path.canonicalize().unwrap_or(path));
    let mut used_names = BTreeSet::new();
    let mut names = IndexMap::new();
    let mut rows = Vec::with_capacity(sources.len());

    for source in sources {
        let configured = config
            .sources
            .iter()
            .any(|configured| configured.id == source.entry.id);
        let is_current_directory = !configured
            && current_directory.as_ref().is_some_and(|cwd| {
                source
                    .entry
                    .path
                    .as_ref()
                    .map(|path| path.canonicalize().unwrap_or_else(|_| path.clone()))
                    .is_some_and(|path| path == *cwd)
            });
        let base = if is_current_directory {
            "cwd".into()
        } else if !source.entry.name.is_empty() {
            source.entry.name.clone()
        } else if !source.entry.label.is_empty() {
            source.entry.label.clone()
        } else {
            source_reference(&source.entry)
        };
        let mut name = base.clone();
        let mut suffix = 2_usize;
        while !used_names.insert(fold(&name)) {
            name = format!("{base}#{suffix}");
            suffix += 1;
        }
        let label = if is_current_directory {
            "Current directory".into()
        } else if source.entry.label.is_empty() {
            name.clone()
        } else {
            source.entry.label.clone()
        };

        names.insert(source.entry.id.clone(), name.clone());
        rows.push(SourceRow {
            name,
            label,
            location: source_reference(&source.entry),
            alternate: source.entry.alternate.as_ref().map(location_reference),
        });
    }

    (rows, names)
}

fn target_data(target: &Target) -> Value {
    json!({
        "name": target.name,
        "label": target.label,
        "path": target.path,
        "enabled": target.enabled,
        "builtin": target.builtin,
        "legacy_override": target.legacy_override
    })
}

fn skill_action_data(
    candidate: &SkillCandidate,
    target: &Target,
    scope: Option<Scope>,
    destination: &Path,
    dry_run: bool,
    action: &str,
) -> Value {
    let mut data = source_data(&candidate.source.entry);
    if let Some(object) = data.as_object_mut() {
        object.insert("skill".into(), json!(candidate.name));
        object.insert("path".into(), json!(candidate.path));
        object.insert("target".into(), json!(target.name));
        object.insert("target_path".into(), json!(target.path));
        object.insert("destination".into(), json!(destination));
        object.insert("dry_run".into(), json!(dry_run));
        object.insert("action".into(), json!(action));
        if let Some(scope) = scope {
            object.insert("scope".into(), json!(scope));
        }
    }
    data
}

fn skill_import_data(
    candidate: &SkillCandidate,
    selection: &ImportCandidate,
    destination: &Path,
    dry_run: bool,
    action: &str,
) -> Value {
    let mut data = source_data(&candidate.source.entry);
    if let Some(object) = data.as_object_mut() {
        object.insert("skill".into(), json!(candidate.name));
        object.insert("path".into(), json!(candidate.path));
        object.insert("target".into(), json!(selection.target.name));
        object.insert("target_path".into(), json!(selection.target.path));
        object.insert("scope".into(), json!(selection.scope));
        object.insert("deployment".into(), json!(selection.deployment));
        object.insert("destination".into(), json!(destination));
        object.insert(
            "files_changed".into(),
            json!(selection.stat.files_changed()),
        );
        object.insert("insertions".into(), json!(selection.stat.insertions()));
        object.insert("deletions".into(), json!(selection.stat.deletions()));
        object.insert("action".into(), json!(action));
        object.insert("dry_run".into(), json!(dry_run));
    }
    data
}

fn skill_import_skipped_data(candidate: &SkillCandidate, dry_run: bool) -> Value {
    let mut data = source_data(&candidate.source.entry);
    if let Some(object) = data.as_object_mut() {
        object.insert("skill".into(), json!(candidate.name));
        object.insert("path".into(), json!(candidate.path));
        object.insert("action".into(), json!("skipped"));
        object.insert("dry_run".into(), json!(dry_run));
    }
    data
}

fn set_target_enabled(config: &mut Config, name: &str, enabled: bool) -> Result<()> {
    if let Some(entry) = find_named_mut(&mut config.targets, name) {
        entry.enabled = enabled;
        return Ok(());
    }
    if let Some(entry) = find_named_mut(&mut config.legacy_target_overrides, name) {
        entry.enabled = enabled;
        return Ok(());
    }
    if is_builtin_name(name) {
        config.builtins.entry(fold(name)).or_default().enabled = enabled;
        return Ok(());
    }
    Err(SkillManagerError::NotFound {
        kind: "target",
        reference: name.to_owned(),
    })
}

fn find_named_key<T>(entries: &IndexMap<String, T>, name: &str) -> Option<String> {
    entries.keys().find(|key| fold(key) == fold(name)).cloned()
}

fn find_named_mut<'a, T>(entries: &'a mut IndexMap<String, T>, name: &str) -> Option<&'a mut T> {
    let key = find_named_key(entries, name)?;
    entries.get_mut(&key)
}

fn normalized_patterns(patterns: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    for pattern in patterns {
        if !pattern.trim().is_empty() && !result.iter().any(|value| value == &pattern) {
            result.push(pattern);
        }
    }
    result
}

fn source_matches(source: &SourceEntry, selector: &str) -> bool {
    if [source.id.as_str(), source.name.as_str()]
        .iter()
        .any(|value| fold(value) == fold(selector))
    {
        return true;
    }
    location_from_reference(selector, source.mode).is_ok_and(|location| {
        source_location(source).is_ok_and(|active| locations_equal(&active, &location))
    })
}

fn status_matches(
    skill: &str,
    candidate: Option<&SkillCandidate>,
    filters: &[String],
) -> Result<bool> {
    if filters.is_empty() {
        return Ok(true);
    }
    if matches_patterns(skill, filters)? {
        return Ok(true);
    }
    let Some(value) = candidate else {
        return Ok(false);
    };
    Ok(matches_patterns(&value.source.entry.name, filters)?
        || matches_patterns(&value.source.entry.label, filters)?)
}

fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|error| SkillManagerError::io(".", error))?
            .join(path)
    };
    Ok(canonicalize_existing_ancestor(&absolute))
}

/// Canonicalize the longest existing ancestor of `path` component by
/// component, and rejoin the unresolved tail literally, rather than
/// lexically collapsing `..` ourselves.
///
/// `Path::canonicalize` requires the whole path to exist, which fails for
/// the common `copy` destination case of a directory being created for the
/// first time. Naively falling back to lexical `..` collapse is unsound once
/// a symlinked ancestor is involved: `link/../destination` is not
/// necessarily `link`'s sibling once `link` resolves elsewhere, so
/// collapsing it ourselves before the symlink is resolved can silently
/// redirect a write to the wrong directory.
///
/// It is not enough to canonicalize a whole prefix that still contains a
/// trailing `..`: on Windows, the Win32 path APIs collapse `..` lexically as
/// part of turning a path into its NT form, before a reparse point later in
/// that same string is followed, so `canonicalize("link/..")` can still
/// return the symlink's own parent rather than its target's parent. Instead,
/// each component is resolved in turn — a `Normal` component is joined and
/// canonicalized (following any symlink at that step) as long as it exists,
/// and a `..` is popped from the already-resolved path rather than folded
/// into the string being canonicalized — so `..` is always applied after any
/// symlink at that position has been followed. Once a component does not
/// exist, resolution stops and the remaining tail — which by definition
/// contains no symlinks, because nothing there exists yet — is appended
/// unresolved.
fn canonicalize_existing_ancestor(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return portable_path(&canonical);
    }
    let mut resolved = PathBuf::new();
    let mut resolving = true;
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                resolved.push(component.as_os_str());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir if resolving => {
                resolved.pop();
            }
            std::path::Component::Normal(name) if resolving => {
                let candidate = resolved.join(name);
                if let Ok(canonical) = candidate.canonicalize() {
                    resolved = canonical;
                } else {
                    resolving = false;
                    resolved.push(name);
                }
            }
            // Once an ancestor is missing, nothing further in the path can
            // exist on disk, so there is no symlink left to resolve: append
            // the remaining components literally.
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::Normal(name) => resolved.push(name),
        }
    }
    portable_path(&resolved)
}

fn title_case(value: &str) -> String {
    value
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Create the production repository and discover its home path together.
///
/// # Errors
///
/// Returns an error when the operating system does not provide a user home.
pub fn production_repository() -> Result<(FileConfigRepository, PathBuf)> {
    let home = manager_home()?;
    Ok((FileConfigRepository::new(home.clone()), home))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::PathBuf;

    use indexmap::IndexMap;

    use super::{
        Application, RunOutcome, absolute_path, command_dry_run, find_named_key,
        normalized_patterns, set_target_enabled, skill_action_data, source_data, source_matches,
        status_matches, target_data, title_case,
    };
    use crate::cache::GitHubTransport;
    use crate::cli::{
        Command, CopyArgs, ImportArgs, LoadArgs, RemoveArgs, SourceAction, SourceAddArgs,
        SourceArgs, SourceModeArg, SourceRemoveArgs, SourceUpdateArgs, StatusArgs, SyncArgs,
        TargetAction, TargetArgs, TargetNameArgs, TargetPathArgs, UpdateArgs,
    };
    use crate::config::{
        Config, FileConfigRepository, portable_canonicalize, resolved_targets,
        source_from_reference,
    };
    use crate::domain::{ResolvedSource, Scope, SkillCandidate, SkillDiscovery, TargetEntry};
    use crate::error::{Result, SkillManagerError};
    use crate::event::{Level, Reporter};
    use crate::prompt::Prompt;
    use crate::transaction::NoopTransactionHook;

    struct NoNetwork;

    impl GitHubTransport for NoNetwork {
        fn default_branch(&self, _owner: &str, _repo: &str) -> Result<String> {
            Err(SkillManagerError::InvalidInput(
                "network must not be used".into(),
            ))
        }

        fn download_archive(
            &self,
            _owner: &str,
            _repo: &str,
            _reference: &str,
            _destination: &std::path::Path,
        ) -> Result<()> {
            Err(SkillManagerError::InvalidInput(
                "network must not be used".into(),
            ))
        }
    }

    #[derive(Default)]
    struct TestPrompt {
        texts: VecDeque<String>,
    }

    impl Prompt for TestPrompt {
        fn confirm(&mut self, _message: &str, default: bool) -> Result<bool> {
            Ok(default)
        }

        fn text(&mut self, _message: &str, default: Option<&str>) -> Result<String> {
            Ok(self
                .texts
                .pop_front()
                .or_else(|| default.map(ToOwned::to_owned))
                .unwrap_or_default())
        }

        fn choose(&mut self, _message: &str, choices: &[String]) -> Result<usize> {
            if choices.is_empty() {
                Err(SkillManagerError::InvalidInput("no choices".into()))
            } else {
                Ok(0)
            }
        }
    }

    #[derive(Default)]
    struct RecordingReporter {
        events: Vec<String>,
        human: Vec<String>,
        diagnostics: Vec<String>,
    }

    impl Reporter for RecordingReporter {
        fn event(&mut self, event: &str, _level: Level, _data: serde_json::Value) -> Result<()> {
            self.events.push(event.into());
            Ok(())
        }

        fn human(&mut self, text: &str) -> Result<()> {
            self.human.push(text.into());
            Ok(())
        }

        fn diagnostic(&mut self, text: &str) -> Result<()> {
            self.diagnostics.push(text.into());
            Ok(())
        }

        fn is_json(&self) -> bool {
            false
        }
    }

    #[test]
    fn dry_run_detection_and_pattern_normalization_cover_command_families() {
        let sync = SyncArgs {
            dry_run: true,
            ..SyncArgs::default()
        };
        assert!(command_dry_run(&Command::Load(LoadArgs {
            sync,
            yes: false,
        })));
        assert!(command_dry_run(&Command::Update(UpdateArgs {
            sync: SyncArgs {
                dry_run: true,
                ..SyncArgs::default()
            },
            yes: false,
        })));
        assert!(command_dry_run(&Command::Import(ImportArgs {
            skill: "alpha".into(),
            dry_run: true,
            ..ImportArgs::default()
        })));
        assert!(command_dry_run(&Command::Copy(CopyArgs {
            source: "source".into(),
            destination: PathBuf::from("target"),
            filters: Vec::new(),
            dry_run: true,
            refresh: false,
            yes: false,
        })));
        let remove = RemoveArgs {
            dry_run: true,
            ..RemoveArgs::default()
        };
        assert!(command_dry_run(&Command::Remove(remove)));
        assert!(!command_dry_run(&Command::Status(StatusArgs::default())));

        assert_eq!(
            normalized_patterns(vec![
                String::new(),
                "a*".into(),
                "a*".into(),
                "  ".into(),
                "b?".into(),
            ]),
            ["a*", "b?"]
        );
    }

    #[test]
    fn target_enablement_is_case_folded_across_custom_legacy_and_builtin_entries() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let mut config = Config::default();
        config.targets.insert(
            "Custom".into(),
            TargetEntry {
                path: root.path().join("custom"),
                label: String::new(),
                enabled: true,
                extra: IndexMap::new(),
            },
        );
        config.legacy_target_overrides.insert(
            "Claude".into(),
            TargetEntry {
                path: root.path().join("legacy"),
                label: String::new(),
                enabled: true,
                extra: IndexMap::new(),
            },
        );
        set_target_enabled(&mut config, "CUSTOM", false)
            .unwrap_or_else(|error| unreachable!("{error}"));
        set_target_enabled(&mut config, "claude", false)
            .unwrap_or_else(|error| unreachable!("{error}"));
        set_target_enabled(&mut config, "shared", false)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(!config.targets["Custom"].enabled);
        assert!(!config.legacy_target_overrides["Claude"].enabled);
        assert!(!config.builtins["shared"].enabled);
        assert!(set_target_enabled(&mut config, "missing", true).is_err());
        assert_eq!(
            find_named_key(&config.targets, "cUsToM").as_deref(),
            Some("Custom")
        );
    }

    #[test]
    fn status_filtering_matches_skill_source_name_and_label() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let skill = root.path().join("demo-skill");
        std::fs::create_dir(&skill).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::write(skill.join("SKILL.md"), "# Demo")
            .unwrap_or_else(|error| unreachable!("{error}"));
        let mut entry = source_from_reference("owner/repository", None)
            .unwrap_or_else(|error| unreachable!("{error}"));
        entry.name = "primary-source".into();
        entry.label = "Primary Collection".into();
        let candidate = SkillCandidate {
            name: "demo-skill".into(),
            path: skill,
            source: ResolvedSource {
                entry: entry.clone(),
                path: root.path().to_path_buf(),
                from_cache: false,
                temporary: None,
            },
        };
        assert!(source_matches(&entry, "PRIMARY-SOURCE"));
        assert!(source_matches(&entry, "OWNER/REPOSITORY"));
        assert!(!source_matches(&entry, "secondary"));
        assert!(
            status_matches("demo-skill", Some(&candidate), &[])
                .unwrap_or_else(|error| unreachable!("{error}"))
        );
        for pattern in ["demo-*", "primary-*", "primary collection"] {
            assert!(
                status_matches("demo-skill", Some(&candidate), &[pattern.into()])
                    .unwrap_or_else(|error| unreachable!("{error}")),
                "{pattern}"
            );
        }
        assert!(
            !status_matches("orphan", None, &["primary-*".into()])
                .unwrap_or_else(|error| unreachable!("{error}"))
        );
    }

    #[test]
    fn event_payload_helpers_preserve_provenance_and_target_state() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let entry = source_from_reference("owner/repository:main/team", None)
            .unwrap_or_else(|error| unreachable!("{error}"));
        let candidate = SkillCandidate {
            name: "demo".into(),
            path: root.path().join("demo"),
            source: ResolvedSource {
                entry: entry.clone(),
                path: root.path().to_path_buf(),
                from_cache: true,
                temporary: None,
            },
        };
        let target = resolved_targets(&Config::default(), root.path())
            .shift_remove("claude")
            .unwrap_or_else(|| unreachable!("builtin target"));
        let source_payload = source_data(&entry);
        assert_eq!(source_payload["source_id"], entry.id);
        assert_eq!(source_payload["source"], "owner/repository:main/team");
        let target_payload = target_data(&target);
        assert_eq!(target_payload["builtin"], true);
        assert_eq!(target_payload["legacy_override"], false);
        let destination = target.path.join("demo");
        let action = skill_action_data(
            &candidate,
            &target,
            Some(Scope::Global),
            &destination,
            true,
            "loaded",
        );
        assert_eq!(action["skill"], "demo");
        assert_eq!(action["target"], "claude");
        assert_eq!(action["destination"], serde_json::json!(destination));
        assert_eq!(action["dry_run"], true);
        assert_eq!(action["action"], "loaded");
        assert_eq!(action["scope"], "global");
    }

    #[test]
    fn path_and_title_helpers_handle_absolute_relative_and_separator_cases() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            absolute_path(root.path().to_path_buf())
                .unwrap_or_else(|error| unreachable!("{error}")),
            portable_canonicalize(root.path())
        );
        assert!(
            !absolute_path(root.path().to_path_buf())
                .unwrap_or_else(|error| unreachable!("{error}"))
                .to_string_lossy()
                .contains(r"\\?\"),
            "absolute_path must not leak Windows verbatim path spellings"
        );
        assert!(
            absolute_path(PathBuf::from("relative"))
                .unwrap_or_else(|error| unreachable!("{error}"))
                .is_absolute()
        );
        assert_eq!(title_case("one-two_three"), "One Two Three");
        assert_eq!(title_case("--"), "");
    }

    #[test]
    fn absolute_path_resolves_a_nonexistent_destination_whose_parent_does_not_exist() {
        let sandbox = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let parent = sandbox.path().join("nested").join("home");
        std::fs::create_dir_all(&parent).unwrap_or_else(|error| unreachable!("{error}"));
        let destination = parent.join("brand-new-child");
        let resolved =
            absolute_path(destination.clone()).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            resolved,
            portable_canonicalize(&parent).join("brand-new-child")
        );
        assert!(
            !resolved.to_string_lossy().contains(r"\\?\"),
            "absolute_path must not leak Windows verbatim path spellings for missing destinations"
        );
    }

    #[test]
    fn absolute_path_resolves_a_missing_destination_through_a_symlinked_ancestor_correctly() {
        let sandbox = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let real_root = sandbox.path().join("real-root");
        let inner = real_root.join("inner");
        std::fs::create_dir_all(&inner).unwrap_or_else(|error| unreachable!("{error}"));
        let alias = sandbox.path().join("alias");

        #[cfg(unix)]
        let linked = {
            std::os::unix::fs::symlink(&inner, &alias)
                .unwrap_or_else(|error| unreachable!("{error}"));
            true
        };
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&inner, &alias).is_ok();

        if !linked {
            // Creating a directory symlink without elevated privileges is not
            // possible in every CI environment; skip rather than fail.
            return;
        }

        // `alias` resolves to `real-root/inner`, so `alias/../destination`
        // must resolve to `real-root/destination`, not to a lexical strip of
        // `alias`'s own parent (`sandbox/destination`).
        let requested = alias.join("..").join("destination");
        let resolved = absolute_path(requested).unwrap_or_else(|error| unreachable!("{error}"));
        let expected = portable_canonicalize(&real_root).join("destination");
        assert_eq!(
            resolved, expected,
            "a symlinked ancestor must be resolved before applying '..', not lexically stripped"
        );
        assert_ne!(
            resolved,
            portable_canonicalize(sandbox.path()).join("destination"),
            "must not silently redirect the write to the symlink's own parent directory"
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "One stateful source lifecycle keeps identity and duplicate checks in sequence."
    )]
    fn application_source_lifecycle_covers_interactive_and_error_branches() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let first = home.path().join("first");
        let second = home.path().join("second");
        for path in [&first, &second] {
            std::fs::create_dir_all(path).unwrap_or_else(|error| unreachable!("{error}"));
        }
        let repository = FileConfigRepository::new(home.path());
        let network = NoNetwork;
        let hook = NoopTransactionHook;
        let mut prompt = TestPrompt {
            texts: VecDeque::from(["prompted-source".into()]),
        };
        let mut reporter = RecordingReporter::default();
        let mut app = Application::new(
            &repository,
            &network,
            &mut prompt,
            &mut reporter,
            &hook,
            false,
            home.path().to_path_buf(),
        );

        let outcome = app
            .run(Command::Source(SourceArgs {
                action: SourceAction::Add(SourceAddArgs {
                    source: Some(first.to_string_lossy().into_owned()),
                    source_name: None,
                    name: None,
                    label: None,
                    exclude: vec!["draft-*".into(), "draft-*".into(), String::new()],
                    mode: Some(SourceModeArg::Collection),
                    cache_ttl_hours: Some(0),
                }),
            }))
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(outcome, RunOutcome::Success);
        assert!(app.reporter.events.contains(&"source.added".into()));

        app.run(Command::Source(SourceArgs {
            action: SourceAction::Add(SourceAddArgs {
                source: Some(second.to_string_lossy().into_owned()),
                source_name: Some("second-source".into()),
                name: None,
                label: Some("Second Label".into()),
                exclude: Vec::new(),
                mode: None,
                cache_ttl_hours: None,
            }),
        }))
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            app.run(Command::Source(SourceArgs {
                action: SourceAction::Add(SourceAddArgs {
                    source: Some(second.to_string_lossy().into_owned()),
                    source_name: Some("duplicate".into()),
                    name: None,
                    label: None,
                    exclude: Vec::new(),
                    mode: None,
                    cache_ttl_hours: None,
                }),
            }))
            .is_err()
        );
        app.run(Command::Source(SourceArgs {
            action: SourceAction::List,
        }))
        .unwrap_or_else(|error| unreachable!("{error}"));
        app.run(Command::Source(SourceArgs {
            action: SourceAction::Update(SourceUpdateArgs {
                source: "Prompted Source".into(),
                name: Some("renamed".into()),
                location: None,
                label: Some("Renamed Label".into()),
                exclude: vec!["private-*".into(), "private-*".into()],
                clear_exclude: true,
                cache_ttl_hours: Some(2),
            }),
        }))
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            app.run(Command::Source(SourceArgs {
                action: SourceAction::Update(SourceUpdateArgs {
                    source: "renamed".into(),
                    name: Some("second-source".into()),
                    location: None,
                    label: None,
                    exclude: Vec::new(),
                    clear_exclude: false,
                    cache_ttl_hours: None,
                }),
            }))
            .is_err()
        );
        assert!(
            app.run(Command::Source(SourceArgs {
                action: SourceAction::Update(SourceUpdateArgs {
                    source: "renamed".into(),
                    name: None,
                    location: None,
                    label: None,
                    exclude: Vec::new(),
                    clear_exclude: false,
                    cache_ttl_hours: Some(-1),
                }),
            }))
            .is_err()
        );
        app.run(Command::Source(SourceArgs {
            action: SourceAction::Remove(SourceRemoveArgs {
                source: Some("Renamed Label".into()),
            }),
        }))
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            app.run(Command::Source(SourceArgs {
                action: SourceAction::Remove(SourceRemoveArgs {
                    source: Some("missing".into()),
                }),
            }))
            .is_err()
        );
    }

    /// `resolve_provisional_sync_operands` resolves bare words against the
    /// FINAL discovery that follows a directory promotion (see
    /// `run_sync`'s two-phase sequencing). This directly exercises that the
    /// same discovered-skill-vs-CWD-directory ambiguity check applied by the
    /// preliminary resolver (`resolve_deferred_sync_operands`) is also
    /// applied here, via the shared `resolve_skill_word_with_ambiguity_warning`
    /// helper. A synthetic `cwd` and hand-built `SkillDiscovery` are used
    /// (rather than driving this through the full CLI, which cannot express
    /// this exact combination: a bare word that only resolves to a skill
    /// after a *different* directory operand is promoted, while a
    /// same-named directory unrelated to that promotion also exists
    /// directly under the CWD) so the ambiguity condition is deterministic
    /// and isolated from global process state. Before the fix, this
    /// resolver selected the skill without ever rechecking `cwd`, so no
    /// diagnostic would have been recorded here.
    #[test]
    fn provisional_resolution_still_warns_when_a_same_named_cwd_directory_exists() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let network = NoNetwork;
        let hook = NoopTransactionHook;
        let mut prompt = TestPrompt::default();
        let mut reporter = RecordingReporter::default();
        let mut app = Application::new(
            &repository,
            &network,
            &mut prompt,
            &mut reporter,
            &hook,
            false,
            home.path().to_path_buf(),
        );

        // A directory named exactly like the provisionally-resolved skill
        // exists directly under the synthetic CWD, distinct from wherever
        // the skill itself actually lives.
        let cwd = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::create_dir_all(cwd.path().join("widget"))
            .unwrap_or_else(|error| unreachable!("{error}"));

        let entry = source_from_reference("owner/repository", None)
            .unwrap_or_else(|error| unreachable!("{error}"));
        let mut discovery = SkillDiscovery::default();
        discovery.winners.insert(
            "widget".into(),
            SkillCandidate {
                name: "widget".into(),
                path: home.path().join("plain-dir").join("widget"),
                source: ResolvedSource {
                    entry,
                    path: home.path().join("plain-dir"),
                    from_cache: false,
                    temporary: None,
                },
            },
        );

        let resolved = app
            .resolve_provisional_sync_operands(&["widget".into()], cwd.path(), &discovery)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(resolved, vec!["widget".to_string()]);
        assert!(
            app.reporter.events.contains(&"diagnostic".to_string()),
            "the ambiguity must emit a diagnostic event: {:?}",
            app.reporter.events
        );
        assert!(
            app.reporter.diagnostics.iter().any(|line| line
                .contains("matches both a discovered skill and a directory")
                && line.contains("./widget")),
            "the diagnostic must name the ambiguity and point at ./widget: {:?}",
            app.reporter.diagnostics
        );
    }

    /// Regression companion to the ambiguity test above: a provisionally
    /// resolved word that has NO same-named CWD directory must select the
    /// skill without emitting any ambiguity diagnostic.
    #[test]
    fn provisional_resolution_does_not_warn_without_a_same_named_cwd_directory() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let network = NoNetwork;
        let hook = NoopTransactionHook;
        let mut prompt = TestPrompt::default();
        let mut reporter = RecordingReporter::default();
        let mut app = Application::new(
            &repository,
            &network,
            &mut prompt,
            &mut reporter,
            &hook,
            false,
            home.path().to_path_buf(),
        );

        // No "widget" directory exists directly under this synthetic CWD.
        let cwd = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));

        let entry = source_from_reference("owner/repository", None)
            .unwrap_or_else(|error| unreachable!("{error}"));
        let mut discovery = SkillDiscovery::default();
        discovery.winners.insert(
            "widget".into(),
            SkillCandidate {
                name: "widget".into(),
                path: home.path().join("plain-dir").join("widget"),
                source: ResolvedSource {
                    entry,
                    path: home.path().join("plain-dir"),
                    from_cache: false,
                    temporary: None,
                },
            },
        );

        let resolved = app
            .resolve_provisional_sync_operands(&["widget".into()], cwd.path(), &discovery)
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(resolved, vec!["widget".to_string()]);
        assert!(
            app.reporter.diagnostics.is_empty(),
            "no ambiguity exists, so no diagnostic should be emitted: {:?}",
            app.reporter.diagnostics
        );
    }

    #[test]
    fn application_target_lifecycle_covers_custom_builtin_and_error_branches() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let network = NoNetwork;
        let hook = NoopTransactionHook;
        let mut prompt = TestPrompt::default();
        let mut reporter = RecordingReporter::default();
        let mut app = Application::new(
            &repository,
            &network,
            &mut prompt,
            &mut reporter,
            &hook,
            false,
            home.path().to_path_buf(),
        );
        assert!(
            app.run(Command::Target(TargetArgs {
                action: TargetAction::Add(TargetPathArgs {
                    name: "claude".into(),
                    path: home.path().join("reserved"),
                }),
            }))
            .is_err()
        );
        app.run(Command::Target(TargetArgs {
            action: TargetAction::Add(TargetPathArgs {
                name: "custom-target".into(),
                path: PathBuf::from(".custom").join("skills"),
            }),
        }))
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            app.run(Command::Target(TargetArgs {
                action: TargetAction::Add(TargetPathArgs {
                    name: "CUSTOM-TARGET".into(),
                    path: PathBuf::from(".duplicate").join("skills"),
                }),
            }))
            .is_err()
        );
        for action in [
            TargetAction::Disable(TargetNameArgs {
                name: "custom-target".into(),
            }),
            TargetAction::Enable(TargetNameArgs {
                name: "custom-target".into(),
            }),
        ] {
            app.run(Command::Target(TargetArgs { action }))
                .unwrap_or_else(|error| unreachable!("{error}"));
        }
        app.run(Command::Target(TargetArgs {
            action: TargetAction::SetPath(TargetPathArgs {
                name: "custom-target".into(),
                path: PathBuf::from(".custom-new").join("skills"),
            }),
        }))
        .unwrap_or_else(|error| unreachable!("{error}"));
        app.run(Command::Target(TargetArgs {
            action: TargetAction::List,
        }))
        .unwrap_or_else(|error| unreachable!("{error}"));
        app.run(Command::Target(TargetArgs {
            action: TargetAction::Remove(TargetNameArgs {
                name: "custom-target".into(),
            }),
        }))
        .unwrap_or_else(|error| unreachable!("{error}"));
        app.run(Command::Target(TargetArgs {
            action: TargetAction::Remove(TargetNameArgs {
                name: "shared".into(),
            }),
        }))
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            app.run(Command::Target(TargetArgs {
                action: TargetAction::Remove(TargetNameArgs {
                    name: "missing".into(),
                }),
            }))
            .is_err()
        );
    }

    #[test]
    fn noninteractive_source_add_requires_a_name_and_rejects_invalid_values() {
        let home = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let source = home.path().join("source");
        std::fs::create_dir(&source).unwrap_or_else(|error| unreachable!("{error}"));
        let repository = FileConfigRepository::new(home.path());
        let network = NoNetwork;
        let hook = NoopTransactionHook;
        let mut prompt = TestPrompt::default();
        let mut reporter = RecordingReporter::default();
        let mut app = Application::new(
            &repository,
            &network,
            &mut prompt,
            &mut reporter,
            &hook,
            true,
            home.path().to_path_buf(),
        );
        for (name, ttl) in [
            (None, None),
            (Some(" ".into()), None),
            (Some("valid".into()), Some(-1)),
        ] {
            assert!(
                app.run(Command::Source(SourceArgs {
                    action: SourceAction::Add(SourceAddArgs {
                        source: Some(source.to_string_lossy().into_owned()),
                        source_name: name,
                        name: None,
                        label: None,
                        exclude: Vec::new(),
                        mode: None,
                        cache_ttl_hours: ttl,
                    }),
                }))
                .is_err()
            );
        }
    }
}
