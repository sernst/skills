//! Application service and command orchestration.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_json::{Value, json};

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
    ResolvedSource, Scope, ScopedTarget, SkillCandidate, SourceEntry, SourceLocation, SourceMode,
    SourceType, Target, TargetEntry,
};
use crate::error::{Result, SkillManagerError};
use crate::event::{Level, Reporter};
use crate::plan::{
    DiffStat, GroupedUpdateEntry, diff_directories, file_change_lines, grouped_update_table,
    totals_line,
};
use crate::prompt::Prompt;
use crate::skills::{
    deployed_skills, detect_skill_dirs, directories_equal, discover_skills, expand_skill_patterns,
    is_fnmatch_operand, matches_patterns, skill_name, skill_state, split_sync_operands,
    validate_skill_name,
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
                self.run_sync(&config, &args, false, true)?;
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
                self.run_copy(&config, &args)?;
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
            Command::Load(args) => Some(&args.scope),
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
            self.reporter.diagnostic(&format!("Warning: {warning}"))?;
            self.reporter
                .event("diagnostic", Level::Warning, json!({ "message": warning }))?;
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
        let sources = self.resolve_sources(
            config,
            &operands.sources,
            &args.source_selection,
            args.refresh,
            args.dry_run,
        )?;
        let discovery = discover_skills(&sources, &[], &config.exclude)?;
        self.emit_collisions(&discovery.collisions)?;
        let target_templates =
            self.select_target_templates(config, &args.targets, !update_only, args.dry_run)?;
        let scope_context = scope_context(&self.home)?;
        let project_root = &scope_context.project_root;
        let load_scope = if update_only {
            None
        } else {
            Some(self.load_scope(config, &target_templates, &args.scope, project_root)?)
        };

        let candidate_names = discovery
            .winners
            .values()
            .map(|candidate| candidate.name.as_str())
            .collect::<Vec<_>>();
        let expansion = expand_skill_patterns(&operands.skill_patterns, candidate_names)?;
        self.emit_unmatched_patterns(&expansion.unmatched_patterns)?;
        let positional_matched =
            operands.skill_patterns.is_empty() || !expansion.matched.is_empty();
        let selected = expansion
            .matched
            .iter()
            .map(|name| fold(name))
            .collect::<BTreeSet<_>>();

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
        let implicit_targets = !args.targets.is_explicit();
        if update_only && steps.is_empty() && positional_matched && !self.reporter.is_json() {
            let message = if target_templates.is_empty() {
                if implicit_targets {
                    "No enabled targets are available for update."
                } else {
                    "No selected targets are available for update."
                }
            } else {
                "No installed skills matched this update."
            };
            self.reporter.human(message)?;
        }
        if update_only
            && !self.confirm_update_plan(
                &steps,
                &target_names,
                UpdatePlanOptions {
                    implicit_targets,
                    dry_run: args.dry_run,
                    confirmed,
                    context: UpdatePlanContext::Direct,
                },
            )?
        {
            return Ok(false);
        }

        if update_only && !self.reporter.is_json() && steps.iter().any(|step| !step.same) {
            self.reporter.human("")?;
        }

        let mut changed = 0_usize;
        let mut skipped = 0_usize;
        for step in &steps {
            if step.same {
                skipped += 1;
                self.reporter.event(
                    "skill.skipped",
                    Level::Info,
                    skill_action_data(
                        &step.candidate,
                        &step.target,
                        Some(step.scope),
                        &step.destination,
                        args.dry_run,
                        "skipped",
                    ),
                )?;
                continue;
            }
            if !args.dry_run {
                deploy_skill(
                    &step.candidate.path,
                    &step.target.path,
                    self.repository.cache_root(),
                    self.hook,
                )?;
            }
            changed += 1;
            self.reporter.human(&format!(
                "{} {} -> {} ({}){}",
                if update_only { "Updated" } else { "Loaded" },
                step.candidate.name,
                step.target.name,
                step.scope.as_str(),
                if args.dry_run { " (dry-run)" } else { "" }
            ))?;
            let action = if update_only {
                "updated"
            } else if step.existed {
                "overwritten"
            } else {
                "loaded"
            };
            self.reporter.event(
                if update_only {
                    "skill.updated"
                } else {
                    "skill.loaded"
                },
                Level::Info,
                skill_action_data(
                    &step.candidate,
                    &step.target,
                    Some(step.scope),
                    &step.destination,
                    args.dry_run,
                    action,
                ),
            )?;
        }
        if !positional_matched {
            return Err(SkillManagerError::NotFound {
                kind: "skill matching positional pattern",
                reference: operands.skill_patterns.join(", "),
            });
        }
        self.reporter.event(
            "summary",
            Level::Info,
            json!({
                "action": if update_only { "update" } else { "load" },
                "changed": changed,
                "skipped": skipped,
                "dry_run": args.dry_run
            }),
        )?;
        Ok(true)
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

    fn run_copy(&mut self, config: &Config, args: &CopyArgs) -> Result<()> {
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
        let target = Target {
            name: "copy".into(),
            label: "Copy destination".into(),
            path: destination,
            enabled: true,
            builtin: false,
            legacy_override: false,
        };
        let mut copied = 0_usize;
        for candidate in discovery.winners.values() {
            let output = target.path.join(&candidate.name);
            let output_existed = output.is_dir();
            if !args.dry_run {
                deploy_skill(
                    &candidate.path,
                    &target.path,
                    self.repository.cache_root(),
                    self.hook,
                )?;
            }
            copied += 1;
            self.reporter.human(&format!(
                "Copied {} -> {}{}",
                candidate.name,
                output.display(),
                if args.dry_run { " (dry-run)" } else { "" }
            ))?;
            let action = if output_existed {
                "overwritten"
            } else {
                "copied"
            };
            self.reporter.event(
                "skill.copied",
                Level::Info,
                skill_action_data(candidate, &target, None, &output, args.dry_run, action),
            )?;
        }
        self.reporter.event(
            "summary",
            Level::Info,
            json!({ "action": "copy", "copied": copied, "dry_run": args.dry_run }),
        )
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
        let target_templates =
            self.select_target_templates(config, &args.targets, false, args.dry_run)?;
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
        let templates =
            self.select_target_templates(config, &TargetSelection::default(), false, false)?;
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

    // Resolution, confirmation, execution, and reporting form one ordered
    // partial-commit operation and are therefore deliberately colocated.
    #[allow(clippy::too_many_lines)]
    fn run_remove(&mut self, config: &Config, args: &RemoveArgs) -> Result<bool> {
        let target_templates =
            self.select_target_templates(config, &args.targets, false, args.dry_run)?;
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
                        names.insert(fold(&name), name);
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
                            names.insert(fold(&name), name);
                        }
                    }
                } else {
                    validate_skill_name(raw)?;
                    if matches_patterns(raw, &args.filters)? {
                        names.insert(fold(raw), raw.clone());
                    }
                }
            }
        }
        let expansion = expand_skill_patterns(
            &positional_patterns,
            deployed_names.values().map(String::as_str),
        )?;
        self.emit_unmatched_patterns(&expansion.unmatched_patterns)?;
        for name in expansion.matched {
            if matches_patterns(&name, &args.filters)? {
                names.insert(fold(&name), name);
            }
        }

        let mut plan = Vec::<(String, Target, Scope)>::new();
        let mut dual = Vec::<(String, ScopedTarget)>::new();
        for name in names.values() {
            for template in &target_templates {
                if let Some(scope) = explicit_scope(&args.scope) {
                    let target = scoped_target(template, scope, &self.home, project_root);
                    if target.target.path.join(name).is_dir() {
                        plan.push((name.clone(), target.target, scope));
                    }
                    continue;
                }
                let global = scoped_target(template, Scope::Global, &self.home, project_root);
                let project = scoped_target(template, Scope::Project, &self.home, project_root);
                let global_exists = global.target.path.join(name).is_dir();
                let project_exists =
                    scope_context.project_available && project.target.path.join(name).is_dir();
                match (global_exists, project_exists) {
                    (true, false) => plan.push((name.clone(), global.target, Scope::Global)),
                    (false, true) => plan.push((name.clone(), project.target, Scope::Project)),
                    (true, true) => dual.push((name.clone(), template.clone())),
                    (false, false) => {}
                }
            }
        }
        if !dual.is_empty() {
            if self.no_input {
                return Err(SkillManagerError::InteractionRequired(
                    "one or more skills are installed globally and in the project; pass --global or --project"
                        .into(),
                ));
            }
            let choices = vec!["project".to_owned(), "global".to_owned(), "both".to_owned()];
            let selected = self.prompt.choose(
                &format!(
                    "{} selected deployment(s) exist in both scopes; choose which copies to remove",
                    dual.len()
                ),
                &choices,
            )?;
            let scopes = match selected {
                0 => vec![Scope::Project],
                1 => vec![Scope::Global],
                _ => vec![Scope::Global, Scope::Project],
            };
            for (name, template) in dual {
                for scope in &scopes {
                    let target = scoped_target(&template, *scope, &self.home, project_root);
                    plan.push((name.clone(), target.target, *scope));
                }
            }
        }
        if plan.is_empty() {
            if !positional_patterns.is_empty() {
                return Err(SkillManagerError::NotFound {
                    kind: "deployed skill matching positional pattern",
                    reference: positional_patterns.join(", "),
                });
            }
            self.reporter.human("No deployed skills matched.")?;
            self.reporter.event(
                "summary",
                Level::Info,
                json!({ "action": "remove", "removed": 0, "dry_run": args.dry_run }),
            )?;
            return Ok(true);
        }
        if !args.dry_run && !args.yes {
            if self.no_input {
                return Err(SkillManagerError::InteractionRequired(
                    "remove requires confirmation; pass --yes in noninteractive mode".into(),
                ));
            }
            if !self.prompt.confirm(
                &format!("Remove {} skill deployment(s)?", plan.len()),
                false,
            )? {
                self.report_cancelled("remove")?;
                return Ok(false);
            }
        }
        let mut removed = 0_usize;
        for (name, target, scope) in plan {
            let destination = target.path.join(&name);
            if !args.dry_run {
                let _did_remove =
                    remove_skill(&name, &target.path, self.repository.cache_root(), self.hook)?;
            }
            removed += 1;
            self.reporter.human(&format!(
                "Removed {} from {} ({}){}",
                name,
                target.name,
                scope.as_str(),
                if args.dry_run { " (dry-run)" } else { "" }
            ))?;
            self.reporter.event(
                "skill.removed",
                Level::Info,
                json!({
                    "skill": name,
                    "target": target.name,
                    "scope": scope,
                    "target_path": target.path,
                    "path": destination,
                    "action": "removed",
                    "dry_run": args.dry_run
                }),
            )?;
        }
        self.reporter.event(
            "summary",
            Level::Info,
            json!({ "action": "remove", "removed": removed, "dry_run": args.dry_run }),
        )?;
        Ok(true)
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
        let target_templates = self.select_target_templates(config, &args.targets, false, false)?;
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
        let expansion = expand_skill_patterns(
            &positional_patterns,
            discovery.collisions.values().filter_map(|candidates| {
                candidates.first().map(|candidate| candidate.name.as_str())
            }),
        )?;
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
        prompt_for_implicit: bool,
        dry_run: bool,
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
        if prompt_for_implicit && !dry_run {
            if self.no_input {
                return Err(SkillManagerError::InteractionRequired(
                    "target selection is required in noninteractive mode; pass --all or --target"
                        .into(),
                ));
            }
            if !self.prompt.confirm(
                &format!("Use all {} enabled target(s)?", selected.len()),
                true,
            )? {
                return Err(SkillManagerError::Cancelled);
            }
        }
        Ok(selected)
    }

    fn load_scope(
        &mut self,
        _config: &Config,
        targets: &[ScopedTarget],
        selection: &ScopeSelection,
        project_root: &Path,
    ) -> Result<Scope> {
        if let Some(scope) = explicit_scope(selection) {
            return Ok(scope);
        }
        if !project_scope_available(&self.home, project_root) {
            return Ok(Scope::Global);
        }
        if self.no_input {
            return Err(SkillManagerError::InteractionRequired(
                "load scope is required in noninteractive mode; pass --global or --project".into(),
            ));
        }
        let project_default = targets.iter().any(|target| {
            target
                .template
                .components()
                .next()
                .is_some_and(|component| project_root.join(component.as_os_str()).is_dir())
        });
        let project = self
            .prompt
            .confirm("Install skills at project scope?", project_default)?;
        Ok(if project {
            Scope::Project
        } else {
            Scope::Global
        })
    }

    fn emit_unmatched_patterns(&mut self, patterns: &[String]) -> Result<()> {
        for pattern in patterns {
            let message = format!("skill pattern matched nothing: {pattern}");
            self.reporter.diagnostic(&format!("Warning: {message}"))?;
            self.reporter.event(
                "diagnostic",
                Level::Warning,
                json!({ "message": message, "pattern": pattern }),
            )?;
        }
        Ok(())
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
        Command::Load(args) => args.dry_run,
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
    if path.is_absolute() {
        return Ok(path.canonicalize().unwrap_or(path));
    }
    let absolute = std::env::current_dir()
        .map_err(|error| SkillManagerError::io(".", error))?
        .join(path);
    Ok(absolute.canonicalize().unwrap_or(absolute))
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
        Command, CopyArgs, ImportArgs, RemoveArgs, SourceAction, SourceAddArgs, SourceArgs,
        SourceModeArg, SourceRemoveArgs, SourceUpdateArgs, StatusArgs, SyncArgs, TargetAction,
        TargetArgs, TargetNameArgs, TargetPathArgs, UpdateArgs,
    };
    use crate::config::{Config, FileConfigRepository, resolved_targets, source_from_reference};
    use crate::domain::{ResolvedSource, Scope, SkillCandidate, TargetEntry};
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
        assert!(command_dry_run(&Command::Load(sync.clone())));
        assert!(command_dry_run(&Command::Update(UpdateArgs {
            sync,
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
            root.path()
                .canonicalize()
                .unwrap_or_else(|error| unreachable!("{error}"))
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
