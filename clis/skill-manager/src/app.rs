//! Application service and command orchestration.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::cache::{GitHubTransport, materialize_source};
use crate::cli::{
    Command, CopyArgs, RemoveArgs, ResolveArgs, SourceAction, SourceAddArgs, SourceModeArg,
    SourceUpdateArgs, StatusArgs, SyncArgs, TargetAction, TargetSelection,
};
use crate::config::{
    Config, ConfigRepository, FileConfigRepository, find_source_index, fold, is_builtin_name,
    manager_home, resolved_targets, source_from_reference, source_reference,
};
use crate::domain::{ResolvedSource, SkillCandidate, SourceEntry, SourceMode, Target, TargetEntry};
use crate::error::{Result, SkillManagerError};
use crate::event::{Level, Reporter};
use crate::prompt::Prompt;
use crate::skills::{
    deployed_skills, detect_skill_dirs, discover_skills, matches_patterns, skill_name, skill_state,
    validate_skill_name,
};
use crate::transaction::{TransactionHook, deploy_skill, remove_skill};

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
        let dry_run = command_dry_run(&command);
        let loaded = self.repository.load(dry_run)?;
        if let Some(warning) = loaded.warning {
            self.reporter.diagnostic(&format!("Warning: {warning}"))?;
            self.reporter
                .event("diagnostic", Level::Warning, json!({ "message": warning }))?;
        }
        let mut config = loaded.config;
        match command {
            Command::Load(args) => {
                self.run_sync(&config, &args, false)?;
            }
            Command::Update(args) => {
                self.run_sync(&config, &args, true)?;
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
            Command::GenerateCompletions(_) | Command::GenerateMan(_) => {
                return Err(SkillManagerError::InvalidInput(
                    "generation commands must be handled at the executable boundary".into(),
                ));
            }
        }
        Ok(RunOutcome::Success)
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
                let index = find_source_index(config, &selector)?.ok_or_else(|| {
                    SkillManagerError::NotFound {
                        kind: "source",
                        reference: selector.clone(),
                    }
                })?;
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
                for source in &config.sources {
                    self.reporter.human(&format!(
                        "{}\t{}\t{}",
                        source.name,
                        source.label,
                        source_reference(source)
                    ))?;
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
        if config.sources.iter().any(|entry| entry.id == source.id) {
            return Err(SkillManagerError::InvalidInput(format!(
                "source is already configured: {}",
                source_reference(&source)
            )));
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
        let index = find_source_index(config, &args.source)?.ok_or_else(|| {
            SkillManagerError::NotFound {
                kind: "source",
                reference: args.source.clone(),
            }
        })?;
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
        let source = config.sources.get_mut(index).ok_or_else(|| {
            SkillManagerError::InvalidInput("source index changed unexpectedly".into())
        })?;
        if let Some(name) = args.name {
            source.name = name;
        }
        if let Some(label) = args.label {
            source.label = label;
        }
        if args.clear_exclude {
            source.exclude.clear();
        }
        for pattern in normalized_patterns(args.exclude) {
            if !source.exclude.iter().any(|existing| existing == &pattern) {
                source.exclude.push(pattern);
            }
        }
        if let Some(ttl) = args.cache_ttl_hours {
            source.cache_ttl_hours = Some(ttl);
        }
        let changed = source.clone();
        self.repository.save(active_path, config)?;
        self.reporter
            .human(&format!("Updated source {}", changed.name))?;
        self.reporter
            .event("source.updated", Level::Info, source_data(&changed))
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
                        path: absolute_path(args.path)?,
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
                let path = absolute_path(args.path)?;
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

    fn run_sync(&mut self, config: &Config, args: &SyncArgs, update_only: bool) -> Result<()> {
        let sources = self.resolve_sources(
            config,
            &args.sources,
            &args.source_selection,
            args.refresh,
            args.dry_run,
        )?;
        let discovery = discover_skills(&sources, &args.filters, &config.exclude)?;
        self.emit_collisions(&discovery.collisions)?;
        let targets = self.select_targets(config, &args.targets, true, args.dry_run)?;
        let mut changed = 0_usize;
        let mut skipped = 0_usize;
        for candidate in discovery.winners.values() {
            for target in &targets {
                let destination = target.path.join(&candidate.name);
                let destination_existed = destination.is_dir();
                if update_only && !destination_existed {
                    skipped += 1;
                    self.reporter.event(
                        "skill.skipped",
                        Level::Info,
                        skill_action_data(candidate, target, &destination, args.dry_run, "skipped"),
                    )?;
                    continue;
                }
                let same = destination_existed
                    && crate::skills::directories_equal(&candidate.path, &destination)?;
                if same {
                    skipped += 1;
                    self.reporter.event(
                        "skill.skipped",
                        Level::Info,
                        skill_action_data(candidate, target, &destination, args.dry_run, "skipped"),
                    )?;
                    continue;
                }
                if !args.dry_run {
                    deploy_skill(
                        &candidate.path,
                        &target.path,
                        self.repository.cache_root(),
                        self.hook,
                    )?;
                }
                changed += 1;
                self.reporter.human(&format!(
                    "{} {} -> {}{}",
                    if update_only { "Updated" } else { "Loaded" },
                    candidate.name,
                    target.name,
                    if args.dry_run { " (dry-run)" } else { "" }
                ))?;
                let action = if update_only {
                    "updated"
                } else if destination_existed {
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
                    skill_action_data(candidate, target, &destination, args.dry_run, action),
                )?;
            }
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
        )
    }

    fn run_copy(&mut self, config: &Config, args: &CopyArgs) -> Result<()> {
        let entry = find_source_index(config, &args.source)?
            .and_then(|index| config.sources.get(index).cloned())
            .map_or_else(|| source_from_reference(&args.source, None), Ok)?;
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
                skill_action_data(candidate, &target, &output, args.dry_run, action),
            )?;
        }
        self.reporter.event(
            "summary",
            Level::Info,
            json!({ "action": "copy", "copied": copied, "dry_run": args.dry_run }),
        )
    }

    // Resolution, confirmation, execution, and reporting form one ordered
    // partial-commit operation and are therefore deliberately colocated.
    #[allow(clippy::too_many_lines)]
    fn run_remove(&mut self, config: &Config, args: &RemoveArgs) -> Result<bool> {
        let targets = self.select_targets(config, &args.targets, false, args.dry_run)?;
        let mut names = BTreeMap::<String, String>::new();
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
        let mut plan = Vec::new();
        for name in names.values() {
            for target in &targets {
                if target.path.join(name).is_dir() {
                    plan.push((name.clone(), target.clone()));
                }
            }
        }
        if plan.is_empty() {
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
                self.reporter.human("Cancelled.")?;
                self.reporter.event(
                    "command.cancelled",
                    Level::Info,
                    json!({ "action": "remove" }),
                )?;
                return Ok(false);
            }
        }
        let mut removed = 0_usize;
        for (name, target) in plan {
            let destination = target.path.join(&name);
            if !args.dry_run {
                let _did_remove =
                    remove_skill(&name, &target.path, self.repository.cache_root(), self.hook)?;
            }
            removed += 1;
            self.reporter.human(&format!(
                "Removed {} from {}{}",
                name,
                target.name,
                if args.dry_run { " (dry-run)" } else { "" }
            ))?;
            self.reporter.event(
                "skill.removed",
                Level::Info,
                json!({
                    "skill": name,
                    "target": target.name,
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
        self.reporter.human("Sources:")?;
        for source in &sources {
            self.reporter.human(&format!(
                "{}\t({})\t{}",
                source.entry.name,
                source.entry.label,
                source_reference(&source.entry)
            ))?;
        }
        self.reporter.human("")?;
        let discovery = discover_skills(&sources, &[], &config.exclude)?;
        self.emit_collisions(&discovery.collisions)?;
        let targets = self.select_targets(config, &args.targets, false, false)?;
        let mut names = BTreeMap::<String, String>::new();
        for (identity, candidate) in &discovery.winners {
            names.insert(identity.clone(), candidate.name.clone());
        }
        for target in &targets {
            for (identity, name) in deployed_skills(&target.path)? {
                names.entry(identity).or_insert(name);
            }
        }
        let mut filters = args.filters.clone();
        filters.extend(args.option_filters.clone());
        if names.is_empty() {
            self.reporter
                .human("No skills found in sources or deployed targets.")?;
        } else {
            self.reporter.human(&format!(
                "skill\tsource\t{}",
                targets
                    .iter()
                    .map(|target| target.name.as_str())
                    .collect::<Vec<_>>()
                    .join("\t")
            ))?;
        }
        let mut rows = 0_usize;
        let mut counts = BTreeMap::from([
            ("up-to-date", 0_usize),
            ("needs-update", 0),
            ("not-loaded", 0),
            ("no-connection", 0),
        ]);
        for (identity, name) in names {
            let candidate = discovery.winners.get(&identity);
            if !status_matches(&name, candidate, &filters)? {
                continue;
            }
            let mut states = IndexMap::new();
            for target in &targets {
                let state = skill_state(
                    candidate.map(|value| value.path.as_path()),
                    &target.path,
                    &name,
                )?;
                if let Some(count) = counts.get_mut(state.as_str()) {
                    *count += 1;
                }
                states.insert(target.name.clone(), state.as_str());
            }
            let source_reference_text =
                candidate.map(|value| source_reference(&value.source.entry));
            let source = candidate.map(|value| source_data(&value.source.entry));
            let display_states = if self.reporter.is_interactive() {
                states
                    .iter()
                    .map(|(target, state)| {
                        let symbol = match *state {
                            "up-to-date" => "✓",
                            "needs-update" => "↑",
                            "not-loaded" => "✗",
                            _ => "~",
                        };
                        format!("{target}:{symbol} {state}")
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                states
                    .iter()
                    .map(|(target, state)| format!("{target}:{state}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            self.reporter.human(&format!(
                "{}\t{}\t{}",
                name,
                source_reference_text.as_deref().unwrap_or("-"),
                display_states
            ))?;
            self.reporter.event(
                "status.row",
                Level::Info,
                json!({ "skill": name, "source": source, "targets": states }),
            )?;
            rows += 1;
        }
        if rows > 0 {
            self.reporter.human("")?;
            self.reporter.human(&format!(
                "Summary: up-to-date: {}, needs-update: {}, not-loaded: {}, no-connection: {}",
                counts["up-to-date"],
                counts["needs-update"],
                counts["not-loaded"],
                counts["no-connection"]
            ))?;
        }
        self.reporter.event(
            "summary",
            Level::Info,
            json!({ "action": "status", "skills": rows }),
        )
    }

    fn run_resolve(
        &mut self,
        config: &mut Config,
        active_path: &Path,
        args: &ResolveArgs,
    ) -> Result<()> {
        let sources =
            self.resolve_sources(config, &[], &args.source_selection, args.refresh, false)?;
        let discovery = discover_skills(&sources, &[], &config.exclude)?;
        let selected: BTreeSet<String> = args.skills.iter().map(|value| fold(value)).collect();
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
                let entry = find_source_index(config, reference)?
                    .and_then(|index| config.sources.get(index).cloned())
                    .map_or_else(|| source_from_reference(reference, None), Ok)?;
                entries.push(entry);
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

    fn select_targets(
        &mut self,
        config: &Config,
        selection: &TargetSelection,
        prompt_for_implicit: bool,
        dry_run: bool,
    ) -> Result<Vec<Target>> {
        let all = resolved_targets(config, &self.home);
        let mut explicit_names = BTreeSet::new();
        for requested in &selection.target_names {
            let target = all
                .values()
                .find(|target| fold(&target.name) == fold(requested))
                .ok_or_else(|| SkillManagerError::NotFound {
                    kind: "target",
                    reference: requested.clone(),
                })?;
            explicit_names.insert(fold(&target.name));
        }
        for (requested, enabled) in [
            ("claude", selection.claude),
            ("shared", selection.shared),
            ("antigravity", selection.antigravity),
        ] {
            if enabled
                && !explicit_names.contains(requested)
                && all.get(requested).is_some_and(|target| !target.enabled)
            {
                return Err(SkillManagerError::InvalidInput(format!(
                    "target '{requested}' is disabled; use --target {requested} to override"
                )));
            }
        }
        let mut selected = Vec::new();
        for target in all.values() {
            let wanted = explicit_names.contains(&fold(&target.name))
                || selection.all_targets && target.enabled
                || selection.claude && target.name == "claude"
                || selection.shared && target.name == "shared"
                || selection.antigravity && target.name == "antigravity";
            if wanted {
                selected.push(target.clone());
            }
        }
        if selection.is_explicit() {
            return Ok(selected);
        }
        selected.extend(all.values().filter(|target| target.enabled).cloned());
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
        Command::Load(args) | Command::Update(args) => args.dry_run,
        Command::Copy(args) => args.dry_run,
        Command::Remove(args) => args.dry_run,
        _ => false,
    }
}

fn source_data(source: &SourceEntry) -> Value {
    json!({
        "source": source_reference(source),
        "source_id": source.id,
        "source_name": source.name,
        "source_label": source.label,
        "source_type": source.source_type,
        "mode": source.mode
    })
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
    let reference = source_reference(source);
    [source.id.as_str(), source.name.as_str(), reference.as_str()]
        .iter()
        .any(|value| fold(value) == fold(selector))
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
        Command, CopyArgs, RemoveArgs, SourceAction, SourceAddArgs, SourceArgs, SourceModeArg,
        SourceRemoveArgs, SourceUpdateArgs, StatusArgs, SyncArgs, TargetAction, TargetArgs,
        TargetNameArgs, TargetPathArgs,
    };
    use crate::config::{Config, FileConfigRepository, resolved_targets, source_from_reference};
    use crate::domain::{ResolvedSource, SkillCandidate, TargetEntry};
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
        assert!(command_dry_run(&Command::Update(sync)));
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
        let action = skill_action_data(&candidate, &target, &destination, true, "loaded");
        assert_eq!(action["skill"], "demo");
        assert_eq!(action["target"], "claude");
        assert_eq!(action["destination"], serde_json::json!(destination));
        assert_eq!(action["dry_run"], true);
        assert_eq!(action["action"], "loaded");
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
                path: home.path().join("custom"),
            }),
        }))
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            app.run(Command::Target(TargetArgs {
                action: TargetAction::Add(TargetPathArgs {
                    name: "CUSTOM-TARGET".into(),
                    path: home.path().join("duplicate"),
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
                path: home.path().join("custom-new"),
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
