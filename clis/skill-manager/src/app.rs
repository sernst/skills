//! Application service and command orchestration.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_json::{Map, Value, json};

use crate::authorize::selection_range;
use crate::authorize::{Authorization, Authorizer, SelectionOption};
use crate::cache::{GitHubTransport, materialize_source};
use crate::cli::{
    Command, ConfigsAction, ConfigsArgs, ConfigsCopyArgs, CopyArgs, DescribeAction, DescribeArgs,
    DescribeSelection, ImportArgs, RemoveArgs, ResolveArgs, ScopeSelection, SourceAction,
    SourceAddArgs, SourceAlternateArgs, SourceLocateArgs, SourceModeArg, SourceSelection,
    SourceSwapArgs, SourceUpdateArgs, StatusArgs, SyncArgs, TargetAction, TargetSelection,
};
use crate::config::{
    CONFIG_SCHEMA_VERSION, Config, ConfigBackup, ConfigRepository, FileConfigRepository,
    derive_salted_source_id, expand_home, find_source_index, fold, is_builtin_name,
    is_github_reference, location_from_reference, location_identity, location_reference,
    locations_equal, manager_home, normalize_config_targets, normalize_target_template,
    paths_equal, portable_canonicalize, portable_path, resolved_targets,
    resolved_targets_for_scope, set_source_location, source_from_reference, source_location,
    source_reference,
};
use crate::domain::{
    ResolvedSource, Scope, ScopedTarget, SkillCandidate, SkillDiscovery, SourceEntry,
    SourceLocation, SourceMode, SourceType, Target, TargetEntry,
};
use crate::error::{Result, SkillManagerError};
use crate::event::{Level, Reporter};
use crate::plan::{
    DiffStat, FileChange, FileDelta, PlanAction, creation_line, diff_directories,
    diff_directory_maps, totals_line,
};
use crate::prompt::Prompt;
use crate::review::{
    ChangePlan, Decision, DecisionOption, Destination, DestinationKind, OptionConsequence,
    OptionDetail, PlanAuthorization, PlanRow, PlanSelection, PlannedAction, PreviewBlock,
    PreviewEntry, PreviewField, RenderStyle, ResultEntry, ResultMarker, action_text, colored,
    destination_label, heading, location_of, location_text, plan_event_data, plan_event_name,
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

/// One deployed copy that differs from its source and can be imported.
#[derive(Clone)]
struct ImportCandidate {
    target: Target,
    scope: Scope,
    deployment: PathBuf,
    stat: DiffStat,
}

/// One enabled, populated deployment considered for propagation, regardless
/// of whether it becomes the resolved import source.
///
/// Propagation breadth is invariant to `--target`/`--scope`: it always walks
/// every enabled target and both scopes, mirroring the legacy
/// `offer_import_sync` sweep, so narrowing which copy supplies the source
/// content never narrows what that content would reach.
#[derive(Clone)]
struct ImportDeployment {
    id: String,
    label: String,
    target: Target,
    scope: Scope,
    path: PathBuf,
}

/// Scope roots and whether the current directory represents a real project.
struct ScopeContext {
    project_root: PathBuf,
    project_available: bool,
}

/// Normalized selection shared by the three `describe` entry points.
#[allow(clippy::struct_excessive_bools)]
struct DescribeRequest {
    selectors: Vec<String>,
    source_selectors: Vec<String>,
    skills: bool,
    sources: bool,
    all_skills: bool,
    all_sources: bool,
    installed: bool,
    outdated: bool,
    not_installed: bool,
}

/// One physical skill copy and its resolver relationship to other copies.
#[derive(Clone)]
struct DescribedSkill {
    candidate: SkillCandidate,
    resolver_status: &'static str,
    resolver_detail: Option<String>,
}

/// Installation observations used both for filtering and structured output.
struct DescribeInstallation {
    installed: bool,
    outdated: bool,
    deployments: Vec<Value>,
}

/// Bounded source-file excerpt embedded in a description.
struct DescribeExcerpt {
    kind: &'static str,
    lines: Vec<String>,
    total_lines: usize,
    truncated: bool,
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
            Command::Describe(args) => {
                self.run_describe(&config, args)?;
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
            Some(ConfigsAction::Copy(copy)) => self.run_configs_copy(copy),
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

    /// Seed a destination manager home from an existing one: the manager
    /// configuration plus every resolved target directory that actually
    /// exists under `<FROM>`, merged into `<TO>` by path without ever
    /// deleting content already present at the destination.
    ///
    /// This is deliberately not built on the shared [`ChangePlan`]/[`PlanRow`]
    /// review model: that model's rendered identity column and JSON row key
    /// are hardcoded to `skill` (see `review::render_table` and
    /// `review::row_value`), which would mislabel rows that are directories
    /// (`configuration`, `claude`, `shared`, ...) rather than skills. Reusing
    /// it as-is would either misrepresent what this command does or require
    /// changing that shared vocabulary for five already-shipped commands.
    /// Instead this reuses only the low-level, genuinely generic primitives —
    /// [`PlanAction`], [`colored`], the `status` column helpers,
    /// [`diff_directory_maps`], [`creation_line`]/[`totals_line`], and
    /// [`result_footer`] — the same way `configs`'s plain source table
    /// already renders outside that model via `aligned_table`.
    fn run_configs_copy(&mut self, args: &ConfigsCopyArgs) -> Result<RunOutcome> {
        let mut progress = SeedProgress::default();
        let result = self.configs_copy_inner(args, &mut progress);
        // Defect 6: a terminal `summary` must close every exit path, including
        // error exits. The success, dry-run, and no-op paths inside
        // `configs_copy_inner` emit their own summary reflecting the full
        // plan; an error returns before reaching that point, so finalize here
        // with whatever was actually committed — zero for a pre-apply
        // validation failure, partial for a mid-apply failure — so a consumer
        // always sees a summary before `command.failed`.
        if result.is_err() {
            let _ = self.emit_configs_copy_summary(&progress, args.dry_run);
        }
        result
    }

    /// The full `configs copy` pipeline. Every filesystem decision is made
    /// against `<FROM>`/`<TO>` directly; the active-home repository is never
    /// migrated or locked, and it is only *read* (never written) when target
    /// discovery falls through to it because `<FROM>` has no usable
    /// configuration of its own.
    #[allow(clippy::too_many_lines)]
    fn configs_copy_inner(
        &mut self,
        args: &ConfigsCopyArgs,
        progress: &mut SeedProgress,
    ) -> Result<RunOutcome> {
        // Resolve and validate both operands BEFORE touching any repository
        // state (defects 1 and 2): a dry run must change nothing anywhere,
        // and the canonical `configs copy ~ ./temp/...` shape routinely makes
        // `<FROM>` alias the active home, so the active home must never gain
        // migration markers or lock files as a side effect of this command.
        let from = self.resolve_seed_directory(&args.from)?;
        let to = self.resolve_seed_destination(&args.to)?;

        if !from.is_dir() {
            return Err(SkillManagerError::InvalidInput(format!(
                "seed source {} does not exist or is not a directory",
                from.display()
            )));
        }
        if to.exists() && !to.is_dir() {
            return Err(SkillManagerError::InvalidInput(format!(
                "seed destination {} exists and is not a directory",
                to.display()
            )));
        }

        let (config, target_source) = self.discover_seed_config(&from)?;
        let items = build_seed_items(&config, &from, &to, args.include_cache)?;
        if items.is_empty() {
            return Err(SkillManagerError::InvalidInput(format!(
                "{} has no configuration and no existing target skill directories to copy",
                from.display()
            )));
        }

        // Reject only a genuine recursion or self-overwrite hazard (finding N),
        // never mere nesting of `<TO>` somewhere under `<FROM>`. This command
        // copies only `<FROM>/.skill-manager` plus each resolved target root —
        // not all of `<FROM>` — so the canonical `configs copy ~ ./temp/...`
        // shape, where `<TO>` sits under the home but outside every copied
        // subtree, is a supported and common case. The hazard exists precisely
        // when a copied source root and `<TO>` overlap, so the check runs after
        // the item set is resolved but still before any preflight, plan render,
        // or write, and it reads only already-resolved paths — no repository
        // access, so the defect 1/2 no-mutation invariant is preserved.
        reject_seed_recursion(&from, &to, &items)?;

        // Preflight every destination path (and every ancestor within `<TO>`)
        // before rendering the plan or writing anything (defects 3 and 4):
        // reject destination symlinks/reparse points that could redirect a
        // write outside `<TO>`, and reject file/directory conflicts so the
        // plan can never promise a seed it would then only partially apply.
        for item in &items {
            // A link-skipped source is never read or written, so it has no
            // destination preflight; walking its source in `reject_seed_conflicts`
            // would follow the link and read outside `<FROM>` (findings A/G).
            if item.source_is_link {
                continue;
            }
            preflight_seed_destination(&to, item)?;
        }

        let mut rows = Vec::with_capacity(items.len());
        for item in items {
            // A configured item whose source root is a link/reparse point is
            // never descended (findings G/K): carry it as an explicit
            // link-skip row instead of reading it. Its destination `existed`
            // flag is still informative for rendering, but nothing is written.
            if item.source_is_link {
                let existed = item.destination.is_dir();
                rows.push(SeedRow {
                    item,
                    existed,
                    action: SeedAction::LinkSkipped,
                    stat: DiffStat::default(),
                });
                continue;
            }
            let before = merge_directory_files(&item.destination, item.excluded)?;
            let after = merge_directory_files(&item.source, item.excluded)?;
            let mut stat = diff_directory_maps(&before, &after)?;
            stat.files.retain(|file| file.change != FileChange::Deleted);
            let existed = item.destination.is_dir();
            // A directory present in the source but missing at the destination is
            // real work even when it contains no regular files (finding H):
            // file-only diffing would classify such an item as a no-op and the
            // merge would never recreate the folder, violating the "copies
            // folders, deletes nothing" contract. `seed_source_entries` already
            // enumerates directories for preflight, so reuse it here.
            let missing_directory = seed_source_entries(&item.source, item.excluded)?
                .into_iter()
                .any(|(relative, is_dir)| {
                    is_dir
                        && !relative
                            .split('/')
                            .fold(item.destination.clone(), |path, part| path.join(part))
                            .is_dir()
                });
            let action = if !existed {
                SeedAction::Copied
            } else if stat.is_empty() && !missing_directory {
                SeedAction::Skipped
            } else {
                SeedAction::Merged
            };
            rows.push(SeedRow {
                item,
                existed,
                action,
                stat,
            });
        }

        let style = self.render_style();

        // How many rows would actually write, versus how many are deliberate
        // link-skips (findings G/K). A link-skip is NOT a no-op: it must be
        // surfaced, so a run whose only non-identical rows are link-skips must
        // still render its plan and report the skip.
        let writes = rows
            .iter()
            .filter(|row| matches!(row.action, SeedAction::Copied | SeedAction::Merged))
            .count();
        let link_skips = rows
            .iter()
            .filter(|row| row.action == SeedAction::LinkSkipped)
            .count();

        // Genuine no-op (finding E / defect 10): every planned item is already
        // identical — nothing to write and nothing skipped for cause. Match the
        // sibling commands' no-work rendering — no `plan` event, no plan table,
        // no "0 changes" footer, and never a confirmation prompt — and state
        // only the specific no-op result. This precedes the dry-run branch
        // because a no-op changes nothing regardless of `--dry-run`. A
        // link-skip is deliberately excluded here so it can never masquerade as
        // a clean no-op.
        if writes == 0 && link_skips == 0 {
            progress.record_plan(&rows);
            self.reporter.human(&format!(
                "Nothing to copy: {} already matches {}.",
                to.display(),
                from.display()
            ))?;
            self.reporter.human(&result_footer(
                &[ResultEntry {
                    marker: ResultMarker::Unchanged,
                    count: rows.len(),
                    description: "already identical".to_owned(),
                }],
                style,
            ))?;
            self.emit_configs_copy_summary(progress, args.dry_run)?;
            return Ok(RunOutcome::Success);
        }

        let authorization = self.configs_copy_authorization(args);
        self.reporter.event(
            "plan",
            Level::Info,
            configs_copy_plan_data(
                &from,
                &to,
                target_source,
                args.include_cache,
                &rows,
                0,
                args.dry_run,
                authorization,
            ),
        )?;
        for line in render_configs_copy_plan(
            &from,
            &to,
            &target_source.label(&self.home),
            args.include_cache,
            &rows,
            style,
        ) {
            self.reporter.human(&line)?;
        }
        self.reporter
            .human(&configs_copy_plan_footer(&rows, style))?;

        if args.dry_run {
            progress.record_plan(&rows);
            self.reporter.human("")?;
            self.reporter.human("Dry run — no changes were made.")?;
            self.emit_configs_copy_summary(progress, args.dry_run)?;
            return Ok(RunOutcome::Success);
        }

        // Authorization gates writes only. When the run has no writes (its only
        // work is reporting link-skips), there is nothing to consent to, so it
        // proceeds straight to the reporting pass without a prompt.
        if writes > 0 && !self.authorize_configs_copy(args, &rows, &to)? {
            self.report_cancelled("configs.copy")?;
            return Ok(RunOutcome::Cancelled);
        }
        self.reporter.human("")?;

        for row in &rows {
            if row.action == SeedAction::Skipped {
                progress.skipped += 1;
                continue;
            }
            if row.action == SeedAction::LinkSkipped {
                // Report the deliberate omission of a linked source root
                // (findings G/K): no write, so no ancestor recheck and no
                // `configs.copy.item` event, but it is visibly recorded.
                progress.linked_skipped += 1;
                self.reporter.human(&format!(
                    "Skipped {} (linked source not copied): {}",
                    row.item.label,
                    row.item.source.display()
                ))?;
                continue;
            }
            // Re-run the link/ancestor rejection immediately before writing this
            // item (finding C). Preflight already ran, but an attacker could
            // swap a destination ancestor (for example `TO/.claude`) for a
            // symlink or junction while the confirmation prompt was waiting.
            // This shrinks the window to "checked immediately before the write";
            // it is deliberately NOT a per-handle TOCTOU guarantee.
            reject_linked_ancestors(&to, &row.item.destination)?;
            reject_links_in_tree(&row.item.destination)?;
            std::fs::create_dir_all(&row.item.destination)
                .map_err(|error| SkillManagerError::io(&row.item.destination, error))?;
            merge_copy_tree(&row.item.source, &row.item.destination, row.item.excluded)?;
            let verb = if row.existed { "Merged" } else { "Copied" };
            self.reporter.human(&format!(
                "{verb} {} -> {}",
                row.item.label,
                row.item.destination.display()
            ))?;
            let action = if row.existed {
                progress.merged += 1;
                "merged"
            } else {
                progress.copied += 1;
                "copied"
            };
            self.reporter.event(
                "configs.copy.item",
                Level::Info,
                json!({
                    "item": row.item.id,
                    "path": row.item.destination,
                    "action": action,
                    "files_changed": row.stat.files_changed(),
                }),
            )?;
        }
        self.reporter.human("")?;
        let changed = progress.copied + progress.merged;
        let mut breakdown = Vec::new();
        if progress.copied > 0 {
            breakdown.push(format!("{} new", progress.copied));
        }
        if progress.merged > 0 {
            breakdown.push(format!("{} merged", progress.merged));
        }
        let mut entries = Vec::new();
        // Only claim a "seeded" result when something was actually written; a
        // run whose only outcome is link-skips must not read as "0 seeded".
        if changed > 0 {
            let mut description =
                format!("director{} seeded", if changed == 1 { "y" } else { "ies" });
            if !breakdown.is_empty() {
                description = format!("{description} ({})", breakdown.join(", "));
            }
            entries.push(ResultEntry {
                marker: ResultMarker::Completed,
                count: changed,
                description,
            });
        }
        if progress.skipped > 0 {
            entries.push(ResultEntry {
                marker: ResultMarker::Unchanged,
                count: progress.skipped,
                description: "already identical".to_owned(),
            });
        }
        if progress.linked_skipped > 0 {
            entries.push(ResultEntry {
                marker: ResultMarker::Unchanged,
                count: progress.linked_skipped,
                description: "skipped (linked source)".to_owned(),
            });
        }
        self.reporter.human(&result_footer(&entries, style))?;
        self.emit_configs_copy_summary(progress, args.dry_run)?;
        Ok(RunOutcome::Success)
    }

    /// Resolve a `configs copy` `<FROM>`/`<TO>` argument: `~` expansion
    /// against the active `--home`, then ordinary CWD-relative resolution.
    fn resolve_seed_directory(&self, raw: &str) -> Result<PathBuf> {
        absolute_path(expand_home(raw, &self.home))
    }

    /// Resolve a `configs copy` `<TO>` argument without allowing its original
    /// spelling to pass through a link. Unlike ordinary path operands, every
    /// existing component of a seed destination is security-sensitive: even a
    /// dangling junction could redirect the later `create_dir_all` outside
    /// `<TO>`. Validate that component walk before physical normalization so
    /// the link itself is not lost when [`absolute_path`] follows it.
    fn resolve_seed_destination(&self, raw: &str) -> Result<PathBuf> {
        let absolute = make_absolute(expand_home(raw, &self.home))?;
        reject_linked_path_components(&absolute)?;
        canonicalize_existing_ancestor(&absolute)
    }

    /// Discover which configuration decides `configs copy`'s target
    /// directories.
    ///
    /// Precedence is `<FROM>`'s own schema-v2 configuration, then the active
    /// `--home` configuration, then built-in defaults — but every tier is
    /// read directly from disk, never through [`ConfigRepository::load`] or
    /// [`ConfigRepository::migrate_layout`]. Those seams migrate layout, back
    /// up displaced files, and take a persistent lock, all of which would
    /// mutate their target. `<FROM>` is frequently the caller's real home (the
    /// canonical `configs copy ~ ./temp/...` shape), and the active home is
    /// exactly the state `--home` exists to protect, so neither may be written
    /// merely to answer a target-discovery question (defects 1, 2, and 9).
    ///
    /// `<FROM>`'s own configuration is read strictly: a present-but-unreadable
    /// or wrong-schema file is a hard error naming that file rather than a
    /// silent fall-through that would omit its custom targets. The active home
    /// is read leniently: it is only a fallback source of target definitions,
    /// so an unreadable or non-current file simply yields built-in defaults.
    fn discover_seed_config(&self, from: &Path) -> Result<(Config, SeedTargetSource)> {
        if let Some(config) = read_seed_config(from)? {
            return Ok((config, SeedTargetSource::FromConfig));
        }
        // When the active home aliases `<FROM>`, reading it again would just
        // re-read the source we already found no strict config in, so skip
        // straight to defaults instead of risking a redundant read.
        if !paths_equal(&self.home, from)
            && let Some(config) = read_seed_config(&self.home).unwrap_or(None)
        {
            return Ok((config, SeedTargetSource::ActiveHome));
        }
        Ok((Config::default(), SeedTargetSource::Defaults))
    }

    /// Describe how this invocation authorizes its seeding plan.
    fn configs_copy_authorization(&self, args: &ConfigsCopyArgs) -> PlanAuthorization {
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

    /// Obtain consent for the rendered seeding plan.
    fn authorize_configs_copy(
        &mut self,
        args: &ConfigsCopyArgs,
        rows: &[SeedRow],
        to: &Path,
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
        let prompt = if rows.len() == 1 {
            format!("Seed {} into {}?", rows[0].item.label, to.display())
        } else {
            format!("Seed these {} items into {}?", rows.len(), to.display())
        };
        Ok(Authorizer::new(self.prompt)
            .confirm(&prompt, false)?
            .is_approved())
    }

    /// Emit the terminal `summary` event from every exit path (dry run,
    /// applied, no-op, or error), reusing the same shared event name every
    /// sibling command finishes with (see `report_sync_summary`/
    /// `report_import_summary`/`emit_copy_summary`/`report_remove_summary`
    /// above): `docs/json.md`'s event stream section states a summary is last
    /// for every mutating command, and `events.md` documents `summary` as
    /// carrying "command-specific final counts" rather than one fixed shape,
    /// so `configs copy` gets its own `summary-configs-copy` payload family
    /// alongside `summary-copy`, `summary-load-update`, and the rest instead
    /// of a bespoke event name. On a dry run or no-op the counts describe the
    /// full plan; on success they describe everything committed; on an error
    /// they describe whatever was committed before the failure.
    fn emit_configs_copy_summary(&mut self, progress: &SeedProgress, dry_run: bool) -> Result<()> {
        self.reporter.event(
            "summary",
            Level::Info,
            json!({
                "action": "configs.copy",
                "items": progress.finalized_items(),
                "new": progress.copied,
                "merged": progress.merged,
                "skipped": progress.skipped,
                "skipped_linked": progress.linked_skipped,
                "dry_run": dry_run
            }),
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
                let index = source_selector_index(config, &selector, &self.home)?;
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
        let noninteractive = self.no_input || args.yes;
        let (reference, supplied_name) = self.resolve_source_add_operands(
            args.source,
            args.source_name,
            args.name,
            noninteractive,
        )?;
        let mode = args.mode.map(|mode| match mode {
            SourceModeArg::Collection => SourceMode::Collection,
            SourceModeArg::Single => SourceMode::Single,
        });
        let mut source = source_from_reference(&reference, mode, &self.home)?;
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
        source.name = match supplied_name {
            Some(name) if !name.trim().is_empty() => name,
            Some(_) => {
                return Err(SkillManagerError::InvalidInput(
                    "source name must not be blank".into(),
                ));
            }
            None if noninteractive => {
                return Err(SkillManagerError::InteractionRequired(
                    "source name is required in noninteractive mode; pass SOURCE --name=NAME"
                        .into(),
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
            Some(_) | None if noninteractive => default_label,
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

    fn resolve_source_add_operands(
        &mut self,
        source: Option<String>,
        positional_name: Option<String>,
        explicit_name: Option<String>,
        noninteractive: bool,
    ) -> Result<(String, Option<String>)> {
        if explicit_name.is_some() && positional_name.is_some() {
            return Err(SkillManagerError::InvalidInput(
                "source add accepts either a second positional argument or --name, not both".into(),
            ));
        }
        match (source, positional_name, explicit_name) {
            (Some(reference), None, name) => Ok((reference, name)),
            (Some(first), Some(second), None) => {
                let (reference, name) = self.resolve_add_argument_roles(
                    "source.add",
                    "Source",
                    &first,
                    &second,
                    true,
                    noninteractive,
                )?;
                Ok((reference, Some(name)))
            }
            (None, None, name) => Ok((
                std::env::current_dir()
                    .map(|path| path.display().to_string())
                    .map_err(|error| SkillManagerError::io(".", error))?,
                name,
            )),
            (None, Some(_), _) => Err(SkillManagerError::InvalidInput(
                "source add received a second positional argument without a first".into(),
            )),
            (Some(_), Some(_), Some(_)) => unreachable!("validated above"),
        }
    }

    /// Resolve the one genuinely ambiguous dimension shared by `source add`
    /// and `target add`: which positional operand names the location.
    fn resolve_add_argument_roles(
        &mut self,
        command: &str,
        location_label: &str,
        first: &str,
        second: &str,
        recognize_github: bool,
        noninteractive: bool,
    ) -> Result<(String, String)> {
        if first == second {
            return Ok((first.to_owned(), second.to_owned()));
        }

        // A canonical GitHub reference is conclusive before filesystem
        // probing. In particular, pairing one with an existing directory must
        // not cause the directory to steal the source-location role. Reuse the
        // normal source parser so shorthand recognition cannot drift from the
        // references `source add` actually accepts.
        let both_are_github = if recognize_github {
            let first_is_github = is_github_reference(first);
            let second_is_github = is_github_reference(second);
            match (first_is_github, second_is_github) {
                (true, false) => return Ok((first.to_owned(), second.to_owned())),
                (false, true) => return Ok((second.to_owned(), first.to_owned())),
                (true, true) => true,
                (false, false) => false,
            }
        } else {
            false
        };

        if !both_are_github {
            let first_is_directory = operand_is_existing_directory(first, &self.home)?;
            let second_is_directory = operand_is_existing_directory(second, &self.home)?;
            match (first_is_directory, second_is_directory) {
                (true, false) => return Ok((first.to_owned(), second.to_owned())),
                (false, true) => return Ok((second.to_owned(), first.to_owned())),
                (true, true) | (false, false) => {}
            }
        }

        let options = [
            SelectionOption::numbered(
                0,
                format!("{location_label} {first} · Name {second}"),
                false,
            ),
            SelectionOption::numbered(
                1,
                format!("{location_label} {second} · Name {first}"),
                false,
            ),
        ];
        let message = format!(
            "{command} cannot determine which argument is the {} and which is the name",
            location_label.to_ascii_lowercase()
        );
        self.reporter.event(
            "diagnostic",
            Level::Warning,
            json!({
                "message": message,
                "command": command,
                "kind": "ambiguous-argument-roles",
                "operands": [first, second],
                "mappings": [
                    {
                        "token": options[0].token,
                        "location": first,
                        "name": second,
                    },
                    {
                        "token": options[1].token,
                        "location": second,
                        "name": first,
                    },
                ],
                "resolution": format!(
                    "pass {} --name=NAME",
                    location_label.to_ascii_uppercase()
                ),
            }),
        )?;
        self.reporter.diagnostic(&format!("Warning: {message}."))?;
        self.reporter.human(&format!(
            "{} plan — argument roles unresolved",
            command.replace('.', " ")
        ))?;
        self.reporter.human("")?;
        for option in &options {
            self.reporter
                .human(&format!("  {}  {}", option.token, option.label))?;
        }

        if noninteractive {
            return Err(SkillManagerError::InteractionRequired(format!(
                "{command} arguments are ambiguous in noninteractive mode; pass {location} --name=NAME",
                location = location_label.to_ascii_uppercase()
            )));
        }
        let question = format!("Select argument roles [{}]", selection_range(&options));
        match Authorizer::new(self.prompt).select(&question, &options)? {
            Authorization::Approved(0) => Ok((first.to_owned(), second.to_owned())),
            Authorization::Approved(1) => Ok((second.to_owned(), first.to_owned())),
            Authorization::Approved(_) => unreachable!("two argument-role options"),
            Authorization::Cancelled => {
                self.report_cancelled(command)?;
                Err(SkillManagerError::Cancelled)
            }
        }
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
        let index = source_selector_index(config, &args.source, &self.home)?;
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
            let replacement = location_from_reference(location, proposed.mode, &self.home)?;
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
        let index = source_selector_index(config, &args.source, &self.home)?;
        let previous = config.sources[index].clone();
        let replacement = location_from_reference(&args.location, previous.mode, &self.home)?;
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
        let index = source_selector_index(config, &args.source, &self.home)?;
        let previous = config.sources[index].clone();
        let replacement = match (args.location, args.clear) {
            (Some(location), false) => Some(location_from_reference(
                &location,
                previous.mode,
                &self.home,
            )?),
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
        let index = source_selector_index(config, &args.source, &self.home)?;
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
                let (path, name) = if let Some(name) = args.name {
                    if args.second.is_some() {
                        return Err(SkillManagerError::InvalidInput(
                            "target add accepts either a second positional argument or --name, not both"
                                .into(),
                        ));
                    }
                    (args.first, name)
                } else {
                    let second = args.second.ok_or_else(|| {
                        SkillManagerError::InvalidInput(
                            "target add requires NAME and PATH, or PATH --name=NAME".into(),
                        )
                    })?;
                    self.resolve_add_argument_roles(
                        "target.add",
                        "Path",
                        &args.first,
                        &second,
                        false,
                        self.no_input || args.yes,
                    )?
                };
                if name.trim().is_empty() {
                    return Err(SkillManagerError::InvalidInput(
                        "target name must not be blank".into(),
                    ));
                }
                if is_builtin_name(&name) {
                    return Err(SkillManagerError::InvalidInput(format!(
                        "custom target name is reserved: {name}"
                    )));
                }
                if config
                    .targets
                    .keys()
                    .any(|entry| fold(entry) == fold(&name))
                {
                    return Err(SkillManagerError::InvalidInput(format!(
                        "target already exists: {name}"
                    )));
                }
                config.targets.insert(
                    name.clone(),
                    TargetEntry {
                        path: normalize_target_template(&path)?,
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
            if configured_source_index(config, operand, &self.home)?.is_some()
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
                    let entry = configured_source_or_reference(config, word, None, &self.home)?;
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

    /// Emit the import summary event from one shared, auditable site.
    fn report_import_summary(
        &mut self,
        imported: usize,
        skipped: usize,
        dry_run: bool,
    ) -> Result<()> {
        self.reporter.event(
            "summary",
            Level::Info,
            json!({
                "action": "import",
                "imported": imported,
                "skipped": skipped,
                "dry_run": dry_run
            }),
        )
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
        let entry = configured_source_or_reference(config, &args.source, None, &self.home)?;
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

    /// Import an explicit skill by reviewing the complete two-dimensional
    /// plan -- source-copy selection, then propagation mode -- before any
    /// prompt.
    ///
    /// Both dimensions live in the shared plan-review model as sibling
    /// [`Decision`]s: `source_copy` first, `propagation` deferred behind it.
    /// `source_copy` is always present, even with exactly one candidate, so
    /// the resolved/pending shape never varies by candidate count; gating
    /// alone decides whether it ever earns a rendered question. Answering
    /// `source_copy` narrows and re-renders `propagation` (unless
    /// `propagation` was already resolved by flag, in which case answering
    /// `source_copy` was the final decision and apply begins immediately, per
    /// the "no extra revision" rule); answering `propagation` always applies
    /// immediately, since it is provably the last dimension.
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

        // Detection compares deployments with the source's own content, so it
        // runs before the destination is resolved: nothing-to-import must
        // never ask where an import would have been written. `--target`/
        // `--scope` narrow which candidate can become the source here, but
        // never the propagation breadth computed further below.
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
        if detected.is_empty() {
            self.reporter.human(&format!(
                "{} has no changed deployment to import from the enabled targets in {} scope.",
                candidate.name,
                scope_phrase(&args.scope)
            ))?;
            self.reporter.event(
                "skill.import-skipped",
                Level::Info,
                skill_import_skipped_data(&candidate, args.dry_run),
            )?;
            self.report_import_summary(0, 1, args.dry_run)?;
            return Ok(true);
        }

        let (destination, used_alternate) = import_destination(&candidate)?;
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
        // Source copies follow effective project scope first, then
        // configured target order -- deliberately different from the
        // propagation breadth below, which stays target-major so it matches
        // `remove_destinations`'s own convention.
        candidates.sort_by_key(|item| {
            let scope_rank = match item.scope {
                Scope::Project => 0,
                Scope::Global => 1,
            };
            let target_rank = target_templates
                .iter()
                .position(|template| fold(&template.target.name) == fold(&item.target.name))
                .unwrap_or(usize::MAX);
            (scope_rank, target_rank)
        });

        let source_label = source_display_name(&candidate.source.entry).to_owned();
        let source_destination_id = format!("{source_label}:source");
        let into_value = if used_alternate {
            format!(
                "{source_label} local alternate ({})",
                portable_canonicalize(&destination).display()
            )
        } else {
            format!("{source_label} (source)")
        };

        let propagation_templates =
            self.select_target_templates(config, &TargetSelection::default())?;
        let propagation_target_names = propagation_templates
            .iter()
            .map(|template| template.target.name.clone())
            .collect::<Vec<_>>();
        let propagation_scopes =
            available_scopes(&ScopeSelection::default(), scope_context.project_available);
        let mut deployed = Vec::new();
        for template in &propagation_templates {
            for scope in &propagation_scopes {
                let target = scoped_target(template, *scope, &self.home, project_root);
                let path = target.target.path.join(&candidate.name);
                if !path.is_dir() {
                    continue;
                }
                deployed.push(ImportDeployment {
                    id: destination_id(&template.target.name, *scope),
                    label: format!("{} · {}", template.target.name, scope.as_str()),
                    target: target.target,
                    scope: *scope,
                    path,
                });
            }
        }

        let mut destinations = vec![import_source_destination(&source_label)];
        destinations.extend(remove_destinations(&propagation_target_names));

        let mut source_options = Vec::with_capacity(candidates.len());
        // `updated`/`skipped` per candidate, in the same order as `candidates`,
        // so genuineness (below) can be evaluated per option without
        // recomputing every candidate's own propagation diff twice.
        let mut per_candidate = Vec::with_capacity(candidates.len());
        for (index, item) in candidates.iter().enumerate() {
            let (option, entries, updated, skipped) =
                import_source_option(item, index, &source_destination_id, &deployed)?;
            source_options.push(option);
            per_candidate.push((entries, updated, skipped));
        }

        // A source copy that is byte-identical to every other candidate is
        // not a genuine branch either -- adopting any of them produces the
        // same resulting source and the same (empty) propagation, so forcing
        // a choice between them would ask a question with no observable
        // answer. Resolve to the first in configured order instead, exactly
        // like the single-real-candidate case already does.
        let all_candidates_identical = candidates.len() > 1
            && candidates.iter().skip(1).all(|item| {
                diff_directories(&candidates[0].deployment, &item.deployment)
                    .is_ok_and(|stat| stat.is_empty())
            });
        let source_resolved_index =
            (candidates.len() == 1 || all_candidates_identical).then_some(0);
        // The footer's "N source copies" wording describes how many genuine
        // alternatives a reader could have been asked to choose between.
        // When every candidate collapsed to one by byte-identity (never
        // offered as a choice at all), that count is 1, not `candidates.len()`
        // -- otherwise a plan that never rendered an `Available source
        // copies` section would still claim "2 source copies" pending,
        // falsely implying an unresolved choice existed.
        let genuine_candidates = if all_candidates_identical {
            1
        } else {
            candidates.len()
        };

        // Propagation is a genuine second dimension only when the resolved
        // source copy would actually leave at least one other deployment
        // out of date. When every candidate is degenerate this way, no
        // possible answer to `source_copy` could ever make propagation
        // matter, so it resolves silently right here, at revision 0, rather
        // than deferring a question whose answer is already known. A mixed
        // population (some candidates genuine, some not) is provably
        // unreachable under this per-candidate diff model: degeneracy for a
        // candidate requires every *other* deployment (candidate or
        // bystander) to already match it, so if any two deployments differ
        // anywhere in the set, every candidate is genuine; otherwise every
        // candidate is degenerate. There is no in-between at revision 0.
        //
        // An explicit `--update`/`--no-update` always wins over the silent
        // degenerate default: the degenerate default exists only to spare
        // the user a question with no observable answer, never to overrule
        // a choice they actually made. The machine `resolved` value must
        // stay honest about which mode was asked for even when it turns out
        // to change nothing -- see `import_resolved_footer` and
        // `import_result_footer` for how the human-facing renders still
        // avoid printing the resulting zero counts.
        let all_degenerate = per_candidate.iter().all(|(_, updated, _)| *updated == 0);
        let propagation_resolved_id: Option<String> = if args.update {
            Some("import-update".to_owned())
        } else if args.no_update || all_degenerate {
            Some("import-only".to_owned())
        } else {
            None
        };

        // Reference totals use the first candidate whenever the source is
        // still pending: nothing can be applied before a source is chosen, so
        // a pre-resolution propagation preview is necessarily provisional.
        // Once a candidate is resolved -- from the start or via the first
        // prompt -- the propagation preview is always recomputed from that
        // exact candidate (see the narrowing branch below and the E8 test),
        // so the advertised numbers can never drift from what apply writes.
        let reference_index = source_resolved_index.unwrap_or(0);
        let (reference_entries, reference_updated, reference_skipped) = {
            let (entries, updated, skipped) = &per_candidate[reference_index];
            (entries.clone(), *updated, *skipped)
        };

        let source_decision = import_source_decision(
            source_options.clone(),
            source_resolved_index.map(|index| {
                destination_id(&candidates[index].target.name, candidates[index].scope)
            }),
        );
        let propagation_decision = import_propagation_decision(
            deployed.len(),
            reference_updated,
            reference_skipped,
            source_resolved_index.is_some(),
            propagation_resolved_id.clone(),
        );

        // The "Mode" metadata line is reserved for a propagation answer the
        // caller actually supplied; a silent (degenerate) resolution never
        // earns it, since nothing was decided.
        let both_resolved_from_start =
            source_resolved_index.is_some() && !all_degenerate && propagation_resolved_id.is_some();
        let mut metadata = Vec::new();
        let mut blocks = Vec::new();
        if let Some(index) = source_resolved_index {
            let resolved = &candidates[index];
            metadata.push((
                "From".to_owned(),
                format!("{} · {}", resolved.target.name, resolved.scope.as_str()),
            ));
            metadata.push((
                "Path".to_owned(),
                portable_canonicalize(&resolved.deployment)
                    .display()
                    .to_string(),
            ));
            metadata.push(("Into".to_owned(), into_value.clone()));
            if both_resolved_from_start {
                let mode_label = if propagation_resolved_id.as_deref() == Some("import-update") {
                    "import + update (recommended, explicitly selected)"
                } else {
                    "import only (explicitly selected)"
                };
                metadata.push(("Mode".to_owned(), mode_label.to_owned()));
            }
            blocks.push(PreviewBlock {
                heading: "Source replacement".to_owned(),
                heading_value: None,
                lead: Some(format!(
                    "{} {}",
                    PlanAction::Import.symbol(),
                    totals_line(&resolved.stat)
                )),
                lead_color: Some(33),
                entries: import_source_entries(&resolved.stat),
            });
            // Every entry in this preview reads as a none-value (nothing to
            // update anywhere) exactly when `reference_updated` is zero, so
            // the whole block is elided then rather than showing an
            // all-"already synchronized" block with nothing to teach.
            if reference_updated > 0 {
                if propagation_resolved_id.as_deref() == Some("import-only") {
                    // Propagation was explicitly resolved to import-only, so
                    // nothing will be written -- rendering this as a
                    // "Propagation preview" using the update symbol would
                    // promise writes on the very line before the footer
                    // says those deployments are being left out of date.
                    // Reframe as staleness instead, using the same
                    // "actions" the option's own consequence carries (never
                    // a fresh diff), so the count can never drift from what
                    // choosing this candidate actually enumerated. Entries
                    // that are already synchronized -- including the
                    // resolved copy's own identity -- carry nothing under
                    // this framing and are dropped.
                    let reference_actions =
                        &source_options[reference_index].consequence.actions[1..];
                    blocks.push(import_staleness_block(&deployed, reference_actions));
                } else {
                    blocks.push(PreviewBlock {
                        heading: "Propagation preview".to_owned(),
                        entries: reference_entries.clone(),
                        ..PreviewBlock::default()
                    });
                }
            }
        } else {
            metadata.push(("Into".to_owned(), into_value.clone()));
        }

        let style = self.render_style();
        let prompting = !args.dry_run && !args.yes && !self.no_input;
        let plan = ChangePlan {
            command: "import".to_owned(),
            plan_id: format!("import:{}", candidate.name),
            heading: "Import plan".to_owned(),
            metadata,
            destinations,
            body_heading: None,
            metric_header: None,
            detail_heading: "Destination-specific changes".to_owned(),
            connector: None,
            rows: Vec::new(),
            blocks,
            decisions: vec![source_decision, propagation_decision],
            prompting,
            distinguishes_overwrites: false,
        };
        let view = plan.view();
        let pending_ids = view
            .decisions()
            .iter()
            .map(|decision| decision.id.clone())
            .collect::<Vec<_>>();
        // Import's plan carries no rows, so `view.uniform_scope()` (which
        // derives a value from row destinations) can never recover the
        // scope the caller actually selected or the candidates actually
        // share. Source it directly from the selection instead: the
        // explicit flag when one was given, or the one scope every
        // candidate shares when there is one.
        let selection_scope = if args.scope.global {
            Some(Scope::Global)
        } else if args.scope.project {
            Some(Scope::Project)
        } else {
            let scopes = candidates
                .iter()
                .map(|item| item.scope)
                .collect::<BTreeSet<_>>();
            (scopes.len() == 1).then(|| {
                *scopes.iter().next().unwrap_or_else(|| {
                    unreachable!("a non-empty candidate list has at least one scope")
                })
            })
        };
        let selection = PlanSelection {
            targets: target_templates
                .iter()
                .map(|template| template.target.name.clone())
                .collect(),
            targets_explicit: args.targets.is_explicit(),
            scope: selection_scope,
            scope_explicit: args.scope.is_explicit(),
        };
        let authorization = self.import_authorization(args, &pending_ids, prompting);
        let data = plan_event_data(&view, 0, args.dry_run, authorization, &selection);
        self.reporter.event(plan_event_name(0), Level::Info, data)?;
        for line in render_plan(&view, style) {
            self.reporter.human(&line)?;
        }

        if pending_ids.is_empty() {
            // Both dimensions were pre-resolved (one candidate, plus an
            // explicit `--update`/`--no-update`), so this is an ordinary
            // binary confirmation over one fully-resolved plan -- never the
            // "final prompt applies immediately" shape, because nothing was
            // interactively narrowed to reach here.
            let resolved_index = source_resolved_index.unwrap_or_else(|| {
                unreachable!("an empty pending list requires a resolved source copy")
            });
            let resolved = &candidates[resolved_index];
            let update = propagation_resolved_id.as_deref() == Some("import-update");
            let target_label = format!("{} · {}", resolved.target.name, resolved.scope.as_str());
            self.reporter.human(&import_resolved_footer(
                update,
                &target_label,
                deployed.len(),
                reference_updated,
                reference_skipped,
            ))?;
            if args.dry_run {
                self.reporter.human("")?;
                self.reporter.human("Dry run — no changes were made.")?;
                self.report_import_summary(0, 0, true)?;
                return Ok(true);
            }
            if args.yes {
                self.reporter.human("")?;
            } else if self.no_input {
                return Err(SkillManagerError::InteractionRequired(
                    "applying this plan noninteractively requires --yes.".into(),
                ));
            } else {
                let question = format!(
                    "Apply this import plan from {} · {}?",
                    resolved.target.name,
                    resolved.scope.as_str()
                );
                if Authorizer::new(self.prompt)
                    .confirm(&question, true)?
                    .is_approved()
                {
                    self.reporter.human("")?;
                } else {
                    self.report_cancelled("import")?;
                    return Ok(false);
                }
            }
            return self.apply_import(
                &candidate,
                resolved,
                &destination,
                &source_label,
                update,
                &deployed,
                style,
            );
        }

        if args.dry_run {
            self.reporter.human(&import_pending_footer(
                genuine_candidates,
                pending_ids.iter().any(|id| id == "source_copy"),
                pending_ids.iter().any(|id| id == "propagation"),
                false,
            ))?;
            self.reporter.human("")?;
            let alternatives = plan
                .decisions
                .iter()
                .find(|decision| pending_ids.contains(&decision.id))
                .map_or(0, |decision| decision.options.len());
            self.reporter.human(&format!(
                "Dry run — {alternatives} alternatives shown; no option selected and no changes were made."
            ))?;
            self.report_import_summary(0, 0, true)?;
            return Ok(true);
        }
        if args.yes {
            let message = if pending_ids.iter().any(|id| id == "source_copy") {
                "import requires a source copy and propagation mode before --yes; choose exactly one target and scope, then pass --update or --no-update."
            } else {
                "propagation choice is required before --yes; pass --update or --no-update."
            };
            return Err(SkillManagerError::InteractionRequired(message.into()));
        }
        if self.no_input {
            return Err(SkillManagerError::InteractionRequired(
                "applying this plan noninteractively requires --yes.".into(),
            ));
        }

        let mut resolved_index = source_resolved_index;
        if pending_ids.first().map(String::as_str) == Some("source_copy") {
            self.reporter.human(&import_pending_footer(
                genuine_candidates,
                true,
                propagation_resolved_id.is_none(),
                false,
            ))?;
            let options = source_options
                .iter()
                .map(|option| SelectionOption {
                    token: option.token.clone(),
                    label: option.label.clone(),
                    destructive: true,
                })
                .collect::<Vec<_>>();
            let question = format!("Select source copy [{}]", selection_range(&options));
            let choice = match Authorizer::new(self.prompt).select(&question, &options)? {
                Authorization::Cancelled => {
                    self.report_cancelled("import")?;
                    return Ok(false);
                }
                Authorization::Approved(index) => index,
            };
            resolved_index = Some(choice);
            self.reporter.human("")?;

            let resolved = &candidates[choice];
            let resolved_source_id = destination_id(&resolved.target.name, resolved.scope);
            let (entries, actions) =
                import_propagation(&deployed, &resolved.deployment, &resolved_source_id)?;
            let (updated, skipped) = import_propagation_counts(&actions);
            // Defensive only: reaching this branch at all means source_copy
            // was genuinely pending (2+ non-identical candidates), and a
            // mixed population where some candidates are degenerate and
            // others are not is unreachable there -- degeneracy requires
            // every *other* deployment to already match a candidate, so any
            // two differing deployments anywhere in the set make every
            // candidate genuine. `all_degenerate` above already covers the
            // only case where a candidate can be degenerate. This fallback
            // exists purely so a future change to candidacy rules fails
            // loudly (a real second prompt) rather than silently, should it
            // ever make a mixed population possible; do not remove it on
            // the assumption it is provably dead today.
            let narrowed_propagation_resolved = propagation_resolved_id
                .clone()
                .or_else(|| (updated == 0).then(|| "import-only".to_owned()));
            let narrowed_metadata = vec![
                (
                    "From".to_owned(),
                    format!("{} · {}", resolved.target.name, resolved.scope.as_str()),
                ),
                (
                    "Path".to_owned(),
                    portable_canonicalize(&resolved.deployment)
                        .display()
                        .to_string(),
                ),
                ("Into".to_owned(), into_value.clone()),
            ];
            let mut narrowed_blocks = vec![PreviewBlock {
                heading: "Source replacement".to_owned(),
                heading_value: None,
                lead: Some(format!(
                    "{} {}",
                    PlanAction::Import.symbol(),
                    totals_line(&resolved.stat)
                )),
                lead_color: Some(33),
                entries: import_source_entries(&resolved.stat),
            }];
            if updated > 0 {
                narrowed_blocks.push(PreviewBlock {
                    heading: "Propagation preview".to_owned(),
                    entries,
                    ..PreviewBlock::default()
                });
            }
            let narrowed_propagation = import_propagation_decision(
                deployed.len(),
                updated,
                skipped,
                true,
                narrowed_propagation_resolved.clone(),
            );
            let narrowed_plan = ChangePlan {
                command: "import".to_owned(),
                plan_id: plan.plan_id.clone(),
                heading: format!("Import plan — source copy {} selected", choice + 1),
                metadata: narrowed_metadata,
                destinations: plan.destinations.clone(),
                body_heading: None,
                metric_header: None,
                detail_heading: "Destination-specific changes".to_owned(),
                connector: None,
                rows: Vec::new(),
                blocks: narrowed_blocks,
                decisions: vec![
                    import_source_decision(source_options.clone(), Some(resolved_source_id)),
                    narrowed_propagation,
                ],
                prompting: true,
                distinguishes_overwrites: false,
            };
            let narrowed_view = narrowed_plan.view();
            let narrowed_pending = narrowed_view
                .decisions()
                .iter()
                .map(|decision| decision.id.clone())
                .collect::<Vec<_>>();
            if narrowed_pending.is_empty() {
                // Propagation was already resolved -- by flag, or silently
                // because this copy leaves nothing out of date -- so
                // answering `source_copy` was the final decision: apply
                // begins immediately, with no extra render to narrow.
                let update = narrowed_propagation_resolved.as_deref() == Some("import-update");
                return self.apply_import(
                    &candidate,
                    resolved,
                    &destination,
                    &source_label,
                    update,
                    &deployed,
                    style,
                );
            }
            let narrowed_authorization = self.import_authorization(args, &narrowed_pending, true);
            let narrowed_data = plan_event_data(
                &narrowed_view,
                1,
                args.dry_run,
                narrowed_authorization,
                &selection,
            );
            self.reporter
                .event(plan_event_name(1), Level::Info, narrowed_data)?;
            for line in render_plan(&narrowed_view, style) {
                self.reporter.human(&line)?;
            }
            self.reporter
                .human(&import_pending_footer(candidates.len(), false, true, true))?;
        } else {
            self.reporter.human(&import_pending_footer(
                genuine_candidates,
                false,
                true,
                false,
            ))?;
        }

        // Propagation is now provably the sole remaining dimension: its
        // answer is the last authorization, and apply begins immediately.
        let resolved = &candidates[resolved_index
            .unwrap_or_else(|| unreachable!("source copy is resolved before propagation"))];
        let options = vec![
            SelectionOption {
                token: "1".to_owned(),
                label: "Import + update".to_owned(),
                destructive: true,
            },
            SelectionOption {
                token: "2".to_owned(),
                label: "Import only".to_owned(),
                destructive: true,
            },
        ];
        let question = format!("Select propagation [{}]", selection_range(&options));
        let update = match Authorizer::new(self.prompt).select(&question, &options)? {
            Authorization::Cancelled => {
                self.report_cancelled("import")?;
                return Ok(false);
            }
            Authorization::Approved(0) => true,
            Authorization::Approved(_) => false,
        };
        self.reporter.human("")?;
        self.apply_import(
            &candidate,
            resolved,
            &destination,
            &source_label,
            update,
            &deployed,
            style,
        )
    }

    /// Apply the resolved import: replace the source, then -- when
    /// propagation is `import + update` -- synchronize every deployment the
    /// plan already promised, in the same order the plan enumerated them, so
    /// plan order equals apply order.
    fn apply_import(
        &mut self,
        candidate: &SkillCandidate,
        resolved: &ImportCandidate,
        destination: &Path,
        source_label: &str,
        update: bool,
        deployed: &[ImportDeployment],
        style: RenderStyle,
    ) -> Result<bool> {
        import_skill(
            &resolved.deployment,
            destination,
            self.repository.cache_root(),
            self.hook,
        )?;
        self.reporter.human(&format!(
            "Imported {} from {} · {} into {source_label} (source).",
            candidate.name,
            resolved.target.name,
            resolved.scope.as_str()
        ))?;
        self.reporter.event(
            "skill.imported",
            Level::Info,
            skill_import_data(candidate, resolved, destination, false, "imported"),
        )?;
        let resolved_source_id = destination_id(&resolved.target.name, resolved.scope);
        let mut updated = 0_usize;
        let mut skipped = 0_usize;
        if update {
            // A destination whose diff comes back empty is already
            // synchronized -- either because it *is* the copy just
            // imported, or because it happened to already match -- so the
            // preview shown before the prompt and what actually gets
            // written here can never disagree. Only the copy that was
            // actually chosen is labeled "(source copy)"; a merely-identical
            // deployment gets its own honest label, and the footer
            // breakdown below counts both so it always sums to the total.
            for entry in deployed {
                let diff = diff_directories(&entry.path, &resolved.deployment)?;
                if diff.is_empty() {
                    skipped += 1;
                    let suffix = if entry.id == resolved_source_id {
                        "(source copy)"
                    } else {
                        "(already up to date)"
                    };
                    self.reporter.human(&format!(
                        "Synchronized {} -> {} ({}) {suffix}",
                        candidate.name,
                        entry.target.name,
                        entry.scope.as_str()
                    ))?;
                    self.reporter.event(
                        "skill.skipped",
                        Level::Info,
                        skill_action_data(
                            candidate,
                            &entry.target,
                            Some(entry.scope),
                            &entry.path,
                            false,
                            "skipped",
                        ),
                    )?;
                    continue;
                }
                deploy_skill(
                    &resolved.deployment,
                    &entry.target.path,
                    self.repository.cache_root(),
                    self.hook,
                )?;
                updated += 1;
                self.reporter.human(&format!(
                    "Updated {} -> {} ({})",
                    candidate.name,
                    entry.target.name,
                    entry.scope.as_str()
                ))?;
                self.reporter.event(
                    "skill.updated",
                    Level::Info,
                    skill_action_data(
                        candidate,
                        &entry.target,
                        Some(entry.scope),
                        &entry.path,
                        false,
                        "updated",
                    ),
                )?;
            }
        }
        self.reporter.human("")?;
        self.reporter.human(&import_result_footer(
            &resolved.stat,
            update,
            deployed.len(),
            updated,
            skipped,
            style,
        ))?;
        self.report_import_summary(1, 0, false)?;
        Ok(true)
    }

    /// Describe how this invocation authorizes its import plan.
    fn import_authorization(
        &self,
        args: &ImportArgs,
        pending: &[String],
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
        let kind = if pending.is_empty() {
            "binary"
        } else {
            "progressive"
        };
        PlanAuthorization {
            kind,
            mode,
            default: (kind == "binary")
                .then_some(prompting.then_some(false))
                .flatten(),
        }
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
                    let entry =
                        source_from_reference(raw, Some(SourceMode::Collection), &self.home)?;
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

    /// Resolve, filter, and render human-readable skill/source descriptions.
    #[allow(
        clippy::too_many_lines,
        reason = "Description selection, physical-copy resolution, and dual-channel rendering are one read-only operation."
    )]
    fn run_describe(&mut self, config: &Config, args: DescribeArgs) -> Result<()> {
        let request = normalize_describe_request(args);
        let mut requested_source_indices = BTreeSet::new();
        let mut source_selector_misses = Vec::new();
        for selector in &request.source_selectors {
            match find_source_index(config, selector, &self.home)? {
                Some(index) => {
                    requested_source_indices.insert(index);
                }
                None => source_selector_misses.push(selector.clone()),
            }
        }
        let scoped_to_sources = !request.source_selectors.is_empty();

        // Describe deliberately uses normal cache semantics: an existing remote
        // cache is reused, while an absent one may be materialized. One broken
        // source must not hide useful descriptions from every other source.
        let mut resolved = Vec::new();
        let mut materialization_misses = Vec::new();
        for source in &config.sources {
            match materialize_source(self.repository, self.github, source, false, false) {
                Ok(value) => resolved.push(value),
                Err(error) => materialization_misses.push(format!(
                    "could not inspect source '{}': {error}",
                    source.name
                )),
            }
        }

        let mut physical = Vec::<DescribedSkill>::new();
        let mut winner_by_name = IndexMap::<String, String>::new();
        for source in &resolved {
            let paths = match detect_skill_dirs(source) {
                Ok(paths) => paths,
                Err(error) => {
                    materialization_misses.push(format!(
                        "could not inspect skills in source '{}': {error}",
                        source.entry.name
                    ));
                    continue;
                }
            };
            for path in paths {
                let name = skill_name(&path)?;
                let identity = fold(&name);
                let globally_excluded =
                    !config.exclude.is_empty() && matches_patterns(&name, &config.exclude)?;
                let locally_excluded = !source.entry.exclude.is_empty()
                    && matches_patterns(&name, &source.entry.exclude)?;
                let (resolver_status, resolver_detail) = if globally_excluded || locally_excluded {
                    let scope = match (globally_excluded, locally_excluded) {
                        (true, true) => "global and source exclusions",
                        (true, false) => "a global exclusion",
                        (false, true) => "a source exclusion",
                        (false, false) => unreachable!(),
                    };
                    ("excluded", Some(format!("matched {scope}")))
                } else if let Some(winner_source) = winner_by_name.get(&identity) {
                    (
                        "shadowed",
                        Some(format!("effective copy is from {winner_source}")),
                    )
                } else {
                    winner_by_name.insert(identity, source.entry.name.clone());
                    ("effective", None)
                };
                physical.push(DescribedSkill {
                    candidate: SkillCandidate {
                        name,
                        path,
                        source: source.clone(),
                    },
                    resolver_status,
                    resolver_detail,
                });
            }
        }

        let mut selected_skill_keys = BTreeSet::<String>::new();
        let mut selected_source_indices = BTreeSet::<usize>::new();
        let mut unmatched = Vec::<String>::new();

        if request.skills && request.all_skills {
            for skill in &physical {
                let in_scope = !scoped_to_sources
                    || requested_source_indices.iter().any(|index| {
                        config
                            .sources
                            .get(*index)
                            .is_some_and(|source| source.id == skill.candidate.source.entry.id)
                    });
                if in_scope && (scoped_to_sources || skill.resolver_status == "effective") {
                    selected_skill_keys.insert(described_skill_key(skill));
                }
            }
        }
        if request.sources && request.all_sources {
            selected_source_indices.extend(0..config.sources.len());
        }

        for selector in &request.selectors {
            let mut matched = false;
            if request.skills {
                let qualified = describe_qualified_selector(config, selector, &self.home)?;
                for skill in &physical {
                    let source_in_flag_scope = !scoped_to_sources
                        || requested_source_indices.iter().any(|index| {
                            config
                                .sources
                                .get(*index)
                                .is_some_and(|source| source.id == skill.candidate.source.entry.id)
                        });
                    if !source_in_flag_scope {
                        continue;
                    }
                    let (source_index, pattern) = qualified
                        .as_ref()
                        .map_or((None, selector.as_str()), |(index, pattern)| {
                            (Some(*index), pattern.as_str())
                        });
                    if source_index.is_some_and(|index| {
                        config
                            .sources
                            .get(index)
                            .is_none_or(|source| source.id != skill.candidate.source.entry.id)
                    }) {
                        continue;
                    }
                    if qualified.is_none()
                        && !scoped_to_sources
                        && skill.resolver_status != "effective"
                    {
                        continue;
                    }
                    if matches_patterns(&skill.candidate.name, &[pattern.to_owned()])? {
                        selected_skill_keys.insert(described_skill_key(skill));
                        matched = true;
                    }
                }
            }
            // A positional operand falls back to source matching only when it
            // matched no skill at all. `--source` is a skill scope and disables
            // this fallback, exactly like spelling SOURCE:PATTERN.
            if !matched && request.sources && !scoped_to_sources {
                for (index, source) in config.sources.iter().enumerate() {
                    if describe_source_matches(source, selector)? {
                        selected_source_indices.insert(index);
                        matched = true;
                    }
                }
            }
            if !matched {
                unmatched.push(selector.clone());
            }
        }

        for selector in source_selector_misses {
            unmatched.push(format!("--source={selector}"));
        }

        let mut selected_skills = Vec::new();
        for skill in &physical {
            if !selected_skill_keys.contains(&described_skill_key(skill)) {
                continue;
            }
            let installation = describe_installation(config, &self.home, skill)?;
            if describe_state_matches(&request, &installation) {
                selected_skills.push((skill.clone(), installation));
            }
        }

        for message in &materialization_misses {
            self.emit_message_diagnostic(message)?;
        }

        if selected_skills.is_empty() && selected_source_indices.is_empty() {
            return Err(SkillManagerError::NotFound {
                kind: "skill or source description",
                reference: if request.selectors.is_empty() {
                    "requested filters".into()
                } else {
                    request.selectors.join(", ")
                },
            });
        }

        for selector in &unmatched {
            self.emit_pattern_diagnostic(
                &format!("describe selector matched nothing: {selector}"),
                selector,
            )?;
        }

        let mut first = true;
        for (skill, installation) in &selected_skills {
            if !first {
                self.reporter.human(&"-".repeat(72))?;
            }
            first = false;
            self.render_described_skill(skill, installation)?;
        }
        for index in &selected_source_indices {
            let Some(source) = config.sources.get(*index) else {
                continue;
            };
            if !first {
                self.reporter.human(&"-".repeat(72))?;
            }
            first = false;
            let source_skills = physical
                .iter()
                .filter(|skill| skill.candidate.source.entry.id == source.id)
                .cloned()
                .collect::<Vec<_>>();
            let source_root = resolved
                .iter()
                .find(|item| item.entry.id == source.id)
                .map(|item| item.path.as_path());
            self.render_described_source(source, source_root, &source_skills)?;
        }
        self.reporter.event(
            "summary",
            Level::Info,
            json!({
                "action": "describe",
                "skills": selected_skills.len(),
                "sources": selected_source_indices.len(),
            }),
        )
    }

    fn render_described_skill(
        &mut self,
        skill: &DescribedSkill,
        installation: &DescribeInstallation,
    ) -> Result<()> {
        let trigger = skill_trigger(&skill.candidate.path.join("SKILL.md"))?;
        let excerpt = skill_excerpt(&skill.candidate.path)?;
        let color = self.reporter.color_enabled();
        self.reporter
            .human(&heading(&format!("Skill: {}", skill.candidate.name), color))?;
        self.reporter.human("")?;
        self.reporter
            .human(&describe_field("Trigger", &trigger, color))?;
        self.reporter.human(&describe_field(
            "Source",
            &skill.candidate.source.entry.name,
            color,
        ))?;
        let resolver = skill.resolver_detail.as_ref().map_or_else(
            || skill.resolver_status.to_owned(),
            |detail| format!("{} ({detail})", skill.resolver_status),
        );
        self.reporter
            .human(&describe_field("Resolver", &resolver, color))?;
        let installed = match (installation.installed, installation.outdated) {
            (true, true) => "installed; needs update",
            (true, false) => "installed; up to date",
            (false, _) => "not installed",
        };
        self.reporter
            .human(&describe_field("Installation", installed, color))?;
        self.reporter.human("")?;
        let content_heading = if excerpt.kind == "readme" {
            "README.md"
        } else {
            "SKILL.md excerpt"
        };
        self.reporter.human(&heading(content_heading, color))?;
        self.reporter.human("")?;
        for line in &excerpt.lines {
            self.reporter.human(line)?;
        }
        if excerpt.truncated {
            self.reporter.human(&describe_dimmed(
                &format!(
                    "… truncated after {} of {} lines",
                    excerpt.lines.len(),
                    excerpt.total_lines
                ),
                color,
            ))?;
        }
        let mut data = json!({
            "skill": skill.candidate.name,
            "source": source_data(&skill.candidate.source.entry),
            "trigger": trigger,
            "resolver_status": skill.resolver_status,
            "installation": {
                "installed": installation.installed,
                "outdated": installation.outdated,
                "deployments": installation.deployments,
            },
            "content": excerpt_data(&excerpt),
        });
        if let (Some(object), Some(detail)) = (data.as_object_mut(), &skill.resolver_detail) {
            object.insert("resolver_detail".into(), json!(detail));
        }
        self.reporter.event("describe.skill", Level::Info, data)
    }

    fn render_described_source(
        &mut self,
        source: &SourceEntry,
        root: Option<&Path>,
        skills: &[DescribedSkill],
    ) -> Result<()> {
        let color = self.reporter.color_enabled();
        self.reporter
            .human(&heading(&format!("Source: {}", source.name), color))?;
        self.reporter.human("")?;
        for (key, value) in describe_source_fields(source) {
            self.reporter.human(&describe_field(&key, &value, color))?;
        }
        let excerpt = root.and_then(source_excerpt).transpose()?;
        if let Some(excerpt) = &excerpt {
            self.reporter.human("")?;
            self.reporter.human(&heading("README.md", color))?;
            self.reporter.human("")?;
            for line in &excerpt.lines {
                self.reporter.human(line)?;
            }
            if excerpt.truncated {
                self.reporter.human(&describe_dimmed(
                    &format!(
                        "… truncated after {} of {} lines",
                        excerpt.lines.len(),
                        excerpt.total_lines
                    ),
                    color,
                ))?;
            }
        }
        self.reporter.human("")?;
        self.reporter.human(&heading("Available skills", color))?;
        if skills.is_empty() {
            self.reporter.human("  None")?;
        } else {
            for skill in skills {
                let trigger = skill_trigger(&skill.candidate.path.join("SKILL.md"))?;
                let status = skill.resolver_detail.as_ref().map_or_else(
                    || skill.resolver_status.to_owned(),
                    |detail| format!("{}; {detail}", skill.resolver_status),
                );
                self.reporter.human(&format!(
                    "  {}  [{}]\n    {}",
                    skill.candidate.name, status, trigger
                ))?;
            }
        }
        let nested_skills = skills
            .iter()
            .map(|skill| {
                let mut data = json!({
                    "skill": skill.candidate.name,
                    "trigger": skill_trigger(&skill.candidate.path.join("SKILL.md"))?,
                    "resolver_status": skill.resolver_status,
                });
                if let (Some(object), Some(detail)) = (data.as_object_mut(), &skill.resolver_detail)
                {
                    object.insert("resolver_detail".into(), json!(detail));
                }
                Ok(data)
            })
            .collect::<Result<Vec<_>>>()?;
        let mut data = json!({
            "source": describe_source_data(source),
            "skills": nested_skills,
        });
        if let (Some(object), Some(excerpt)) = (data.as_object_mut(), excerpt.as_ref()) {
            object.insert("content".into(), excerpt_data(excerpt));
        }
        self.reporter.event("describe.source", Level::Info, data)
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
            let _configured = configured_source_index(config, preferred, &self.home)?;
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
                    .position(|candidate| {
                        source_matches(&candidate.source.entry, preferred, &self.home)
                    })
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
                entries.push(configured_source_or_reference(
                    config, reference, None, &self.home,
                )?);
            }
        } else if selection.cd_only {
            entries.push(source_from_reference(
                &std::env::current_dir()
                    .map_err(|error| SkillManagerError::io(".", error))?
                    .display()
                    .to_string(),
                None,
                &self.home,
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
                    &self.home,
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

fn normalize_describe_request(args: DescribeArgs) -> DescribeRequest {
    match args.action {
        Some(DescribeAction::Skill(args)) => {
            let implied_all = args.selectors.is_empty()
                && (args.all
                    || !args.sources.is_empty()
                    || args.installed
                    || args.outdated
                    || args.not_installed);
            DescribeRequest {
                selectors: args.selectors,
                source_selectors: args.sources,
                skills: true,
                sources: false,
                all_skills: args.all || implied_all,
                all_sources: false,
                installed: args.installed,
                outdated: args.outdated,
                not_installed: args.not_installed,
            }
        }
        Some(DescribeAction::Source(args)) => DescribeRequest {
            all_sources: args.all,
            selectors: args.selectors,
            source_selectors: Vec::new(),
            skills: false,
            sources: true,
            all_skills: false,
            installed: false,
            outdated: false,
            not_installed: false,
        },
        None => normalize_describe_selection(args.selection),
    }
}

fn normalize_describe_selection(args: DescribeSelection) -> DescribeRequest {
    let skills = !args.sources_only;
    let sources = !args.skills;
    let implied_skills = skills
        && args.selectors.is_empty()
        && (args.skills
            || !args.sources.is_empty()
            || args.installed
            || args.outdated
            || args.not_installed);
    let implied_sources = sources && args.selectors.is_empty() && args.sources_only;
    DescribeRequest {
        selectors: args.selectors,
        source_selectors: args.sources,
        skills,
        sources,
        all_skills: args.all || args.all_skills || implied_skills,
        all_sources: args.all || args.all_sources || implied_sources,
        installed: args.installed,
        outdated: args.outdated,
        not_installed: args.not_installed,
    }
}

fn described_skill_key(skill: &DescribedSkill) -> String {
    format!(
        "{}\u{0}{}",
        skill.candidate.source.entry.id,
        fold(&skill.candidate.name)
    )
}

fn describe_qualified_selector(
    config: &Config,
    selector: &str,
    home: &Path,
) -> Result<Option<(usize, String)>> {
    let Some((source, pattern)) = selector.split_once(':') else {
        return Ok(None);
    };
    if pattern.is_empty() {
        return Ok(None);
    }
    Ok(find_source_index(config, source, home)?.map(|index| (index, pattern.to_owned())))
}

fn describe_source_matches(source: &SourceEntry, selector: &str) -> Result<bool> {
    let pattern = [selector.to_owned()];
    let reference = source_reference(source);
    for candidate in [
        source.id.as_str(),
        source.name.as_str(),
        source.label.as_str(),
        reference.as_str(),
    ] {
        if matches_patterns(candidate, &pattern)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn describe_state_matches(request: &DescribeRequest, installation: &DescribeInstallation) -> bool {
    if !request.installed && !request.outdated && !request.not_installed {
        return true;
    }
    request.installed && installation.installed
        || request.outdated && installation.outdated
        || request.not_installed && !installation.installed
}

fn describe_installation(
    config: &Config,
    home: &Path,
    skill: &DescribedSkill,
) -> Result<DescribeInstallation> {
    let project_root = current_project_root()?;
    let scopes = if project_scope_available(home, &project_root) {
        vec![Scope::Global, Scope::Project]
    } else {
        vec![Scope::Global]
    };
    let mut installed = false;
    let mut outdated = false;
    let mut deployments = Vec::new();
    for scope in scopes {
        for target in resolved_targets_for_scope(config, home, &project_root, scope).values() {
            let path = target.target.path.join(&skill.candidate.name);
            if !path.is_dir() {
                continue;
            }
            installed = true;
            let needs_update = !directories_equal(&skill.candidate.path, &path)?;
            outdated |= needs_update;
            deployments.push(json!({
                "target": target.target.name,
                "scope": scope,
                "path": path,
                "enabled": target.target.enabled,
                "state": if needs_update { "needs-update" } else { "up-to-date" },
            }));
        }
    }
    Ok(DescribeInstallation {
        installed,
        outdated,
        deployments,
    })
}

fn skill_trigger(path: &Path) -> Result<String> {
    let contents = fs::read_to_string(path).map_err(|error| SkillManagerError::io(path, error))?;
    let mut lines = contents.lines();
    if lines.next().is_none_or(|line| line.trim() != "---") {
        return Ok(String::new());
    }
    let frontmatter = lines
        .take_while(|line| line.trim() != "---")
        .collect::<Vec<_>>();
    for (index, line) in frontmatter.iter().enumerate() {
        let trimmed = line.trim_start();
        let Some(raw) = trimmed.strip_prefix("description:") else {
            continue;
        };
        let scalar = raw.trim();
        if !scalar.is_empty() && !matches!(scalar, "|" | ">" | "|-" | ">-") {
            return Ok(unquote_yaml_scalar(scalar));
        }
        let indentation = line.len() - trimmed.len();
        let mut continuation = Vec::new();
        for continued in frontmatter.iter().skip(index + 1) {
            if continued.trim().is_empty() {
                continue;
            }
            let continued_indent = continued.len() - continued.trim_start().len();
            if continued_indent <= indentation {
                break;
            }
            continuation.push(continued.trim().to_owned());
        }
        return Ok(continuation.join(" "));
    }
    Ok(String::new())
}

fn unquote_yaml_scalar(value: &str) -> String {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

fn skill_excerpt(root: &Path) -> Result<DescribeExcerpt> {
    let readme = root.join("README.md");
    if readme.is_file() {
        return read_excerpt(&readme, "readme", 100);
    }
    read_excerpt(&root.join("SKILL.md"), "skill", 20)
}

fn source_excerpt(root: &Path) -> Option<Result<DescribeExcerpt>> {
    let path = root.join("README.md");
    path.is_file().then(|| read_excerpt(&path, "readme", 100))
}

fn read_excerpt(path: &Path, kind: &'static str, limit: usize) -> Result<DescribeExcerpt> {
    let contents = fs::read_to_string(path).map_err(|error| SkillManagerError::io(path, error))?;
    let all_lines = contents.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let total_lines = all_lines.len();
    let lines = all_lines.into_iter().take(limit).collect::<Vec<_>>();
    Ok(DescribeExcerpt {
        kind,
        truncated: total_lines > lines.len(),
        total_lines,
        lines,
    })
}

fn excerpt_data(excerpt: &DescribeExcerpt) -> Value {
    json!({
        "kind": excerpt.kind,
        "lines": excerpt.lines,
        "truncated": excerpt.truncated,
        "total_lines": excerpt.total_lines,
    })
}

fn describe_field(key: &str, value: &str, color: bool) -> String {
    format!("{}  {value}", colored(key, Some(36), color))
}

fn describe_dimmed(value: &str, color: bool) -> String {
    if color {
        format!("\u{1b}[2m{value}\u{1b}[0m")
    } else {
        value.to_owned()
    }
}

fn describe_source_fields(source: &SourceEntry) -> Vec<(String, String)> {
    let mut fields = vec![
        ("ID".into(), source.id.clone()),
        ("Name".into(), source.name.clone()),
        ("Label".into(), source.label.clone()),
        (
            "Type".into(),
            serde_json::to_value(source.source_type)
                .ok()
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| "unknown".into()),
        ),
        (
            "Mode".into(),
            serde_json::to_value(source.mode)
                .ok()
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| "unknown".into()),
        ),
        ("Location".into(), source_reference(source)),
        (
            "Alternate".into(),
            source
                .alternate
                .as_ref()
                .map_or_else(|| "—".into(), location_reference),
        ),
        (
            "Exclusions".into(),
            if source.exclude.is_empty() {
                "—".into()
            } else {
                source.exclude.join(", ")
            },
        ),
        (
            "Cache TTL hours".into(),
            source
                .cache_ttl_hours
                .map_or_else(|| "default".into(), |ttl| ttl.to_string()),
        ),
    ];
    fields.extend(source.extra.iter().map(|(key, value)| {
        (
            format!("Extra.{key}"),
            serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".into()),
        )
    }));
    fields
}

fn describe_source_data(source: &SourceEntry) -> Value {
    let mut value = serde_json::to_value(source).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert("location".into(), json!(source_reference(source)));
        if let Some(alternate) = source.alternate.as_ref() {
            object.insert(
                "alternate_location".into(),
                json!(location_reference(alternate)),
            );
        }
    }
    value
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
/// such a skip is dormant:
/// [`PlanView::visible_rows`](crate::review::PlanView::visible_rows) hides it
/// from the table, from column significance, and from progress lines, and it
/// is counted only in the footer.
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

/// One configuration directory or resolved target directory `configs copy`
/// seeds from `<FROM>` into `<TO>`.
struct SeedItem {
    /// Stable machine identifier: `configuration`, or the target's own name.
    id: String,
    /// Human-facing row identity.
    label: String,
    /// Existing directory read from `<FROM>`.
    source: PathBuf,
    /// Directory merged into under `<TO>`.
    destination: PathBuf,
    /// Top-level child names skipped while walking `source`/`destination`;
    /// only the `configuration` item ever excludes anything (its regenerable
    /// cache/backup/lock subdirectories), never a target skill directory.
    excluded: &'static [&'static str],
    /// Whether `source` is a symlink or reparse point rather than a real
    /// directory (findings G/K). Such a root is never descended or copied; it
    /// is carried as an item only so the copy can report it as an EXPLICIT
    /// link-skip instead of silently omitting a configured target.
    source_is_link: bool,
}

/// One [`SeedItem`] reviewed against its destination before any write.
struct SeedRow {
    item: SeedItem,
    /// Whether `item.destination` already existed before this invocation.
    existed: bool,
    /// How this item will be applied, decided from `existed` and whether the
    /// filtered content already matches.
    action: SeedAction,
    /// Per-file changes this item's merge would apply, with destination-only
    /// files already filtered out: nothing is ever deleted, so a file that
    /// exists only at the destination is not part of the plan at all.
    stat: DiffStat,
}

/// How one [`SeedRow`] is (or would be) applied.
#[derive(Clone, Copy, Eq, PartialEq)]
enum SeedAction {
    /// The destination did not exist; the whole tree is copied fresh.
    Copied,
    /// The destination existed and its content differs; it is merged.
    Merged,
    /// The destination existed and already matches; nothing is written.
    Skipped,
    /// The source root is a link/reparse point; deliberately not copied and
    /// reported as an explicit link-skip (findings G/K). Distinct from
    /// `Skipped`, which means "already identical" — a link-skip is NOT a no-op.
    LinkSkipped,
}

/// Running tally of what `configs copy` has finalized, so the terminal
/// `summary` is accurate on every exit path — including a mid-apply error,
/// where only the items committed before the failure are counted.
#[derive(Default)]
struct SeedProgress {
    /// Items newly copied (their destination did not exist).
    copied: usize,
    /// Items merged into an existing destination.
    merged: usize,
    /// Items left untouched because they already matched.
    skipped: usize,
    /// Configured items skipped because their source root is a link/reparse
    /// point (findings G/K). Tracked separately from `skipped` so a link-skip
    /// is never conflated with an "already identical" no-op.
    linked_skipped: usize,
}

impl SeedProgress {
    /// Total items finalized so far (committed writes plus recorded skips).
    const fn finalized_items(&self) -> usize {
        self.copied + self.merged + self.skipped + self.linked_skipped
    }

    /// Adopt a full plan's classification for a path that finalizes without a
    /// per-item apply loop: a dry run (which commits nothing but reports what
    /// it would do) or an all-identical no-op.
    fn record_plan(&mut self, rows: &[SeedRow]) {
        self.copied = rows
            .iter()
            .filter(|row| row.action == SeedAction::Copied)
            .count();
        self.merged = rows
            .iter()
            .filter(|row| row.action == SeedAction::Merged)
            .count();
        self.skipped = rows
            .iter()
            .filter(|row| row.action == SeedAction::Skipped)
            .count();
        self.linked_skipped = rows
            .iter()
            .filter(|row| row.action == SeedAction::LinkSkipped)
            .count();
    }
}

/// Build the ordered seed items for one `configs copy` invocation.
///
/// The configuration item is included only when its filtered content actually
/// contains an entry (defect 8): an otherwise-empty `<FROM>/.skill-manager`
/// must report "nothing to copy" rather than silently creating an empty
/// destination directory. A resolved target is included only when its
/// directory exists under `<FROM>` and — unless `--include-cache` is set — is
/// not itself the manager's regenerable cache, backup, or lock storage
/// (defect 7), so a target configured to point at `.skill-manager/cache` can
/// never smuggle excluded content past the exclusion.
fn build_seed_items(
    config: &Config,
    from: &Path,
    to: &Path,
    include_cache: bool,
) -> Result<Vec<SeedItem>> {
    let excluded: &'static [&'static str] = if include_cache {
        &[]
    } else {
        &["cache", "backups", "locks"]
    };
    let reserved: Vec<PathBuf> = if include_cache {
        Vec::new()
    } else {
        ["cache", "backups", "locks"]
            .iter()
            .map(|name| from.join(".skill-manager").join(name))
            .collect()
    };

    let mut items = Vec::new();
    let config_source_dir = from.join(".skill-manager");
    match classify_source_root(&config_source_dir)? {
        SourceRootKind::Directory => {
            if !merge_directory_files(&config_source_dir, excluded)?.is_empty() {
                items.push(SeedItem {
                    id: "configuration".to_owned(),
                    label: "configuration".to_owned(),
                    source: config_source_dir,
                    destination: to.join(".skill-manager"),
                    excluded,
                    source_is_link: false,
                });
            }
        }
        SourceRootKind::Link => {
            // A linked `.skill-manager` root is never read or copied (findings
            // J/G); carry it so the copy reports the skip visibly (finding K).
            items.push(SeedItem {
                id: "configuration".to_owned(),
                label: "configuration".to_owned(),
                source: config_source_dir,
                destination: to.join(".skill-manager"),
                excluded,
                source_is_link: true,
            });
        }
        SourceRootKind::Absent => {}
    }
    for scoped in resolved_targets_for_scope(config, from, from, Scope::Global).values() {
        // Every resolved target must stay inside `<FROM>` (finding A). Source
        // configs are normalized in `read_seed_config` and built-in/active-home
        // templates are already safe, so this never fires for a well-formed
        // config; it is a by-construction backstop that refuses — naming the
        // offending target and its path — rather than ever reading outside
        // `<FROM>` or, through the destination join, writing outside `<TO>`.
        if !path_is_within(&scoped.target.path, from) {
            return Err(SkillManagerError::InvalidInput(format!(
                "target '{}' resolves outside the seed source: {}",
                scoped.target.name,
                scoped.target.path.display()
            )));
        }
        match classify_source_root(&scoped.target.path)? {
            SourceRootKind::Directory => {
                if reserved
                    .iter()
                    .any(|reserved| path_is_within(&scoped.target.path, reserved))
                {
                    continue;
                }
                items.push(SeedItem {
                    id: scoped.target.name.clone(),
                    label: scoped.target.label.clone(),
                    source: scoped.target.path.clone(),
                    destination: to.join(&scoped.template),
                    excluded: &[],
                    source_is_link: false,
                });
            }
            SourceRootKind::Link => {
                // Never descend a linked target ROOT (finding G): `is_dir()`
                // would follow it and `WalkDir` descends a linked root even
                // with `follow_links(false)`, so it could smuggle outside
                // content into `<TO>`. Carry it as an explicit link-skip so the
                // configured target is not silently dropped (finding K).
                items.push(SeedItem {
                    id: scoped.target.name.clone(),
                    label: scoped.target.label.clone(),
                    source: scoped.target.path.clone(),
                    destination: to.join(&scoped.template),
                    excluded: &[],
                    source_is_link: true,
                });
            }
            SourceRootKind::Absent => {}
        }
    }
    Ok(items)
}

/// Reject a destination that a merge into `<TO>` could not safely apply,
/// before the plan is rendered or anything is written.
///
/// This mirrors the deployment transaction's own defense (see
/// `transaction::recover_journal`, which refuses to act through a linked
/// manager root): a symlink or reparse point anywhere under the destination —
/// or at any of its ancestors within `<TO>` — could redirect a write outside
/// `<TO>` and overwrite an unrelated file (defect 3), and a file where a
/// directory must be created (or vice versa) would let the plan promise a
/// seed it could then only partially apply (defect 4). Both are rejected here
/// with an actionable error naming the offending path.
fn preflight_seed_destination(to: &Path, item: &SeedItem) -> Result<()> {
    reject_linked_ancestors(to, &item.destination)?;
    reject_links_in_tree(&item.destination)?;
    reject_seed_conflicts(item)?;
    Ok(())
}

/// Reject any existing symlink/reparse point among `<TO>` itself and each
/// intermediate directory between it and `destination`.
fn reject_linked_ancestors(to: &Path, destination: &Path) -> Result<()> {
    reject_link(to)?;
    let Ok(relative) = destination.strip_prefix(to) else {
        return Ok(());
    };
    let mut current = to.to_path_buf();
    for component in relative.components() {
        current = current.join(component);
        if current == destination {
            break;
        }
        reject_link(&current)?;
    }
    Ok(())
}

/// Reject any symlink/reparse point anywhere inside an existing destination
/// tree, so a pre-planted link cannot survive to redirect a later write.
fn reject_links_in_tree(root: &Path) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    for item in walkdir::WalkDir::new(root)
        .sort_by_file_name()
        .follow_links(false)
    {
        let item = item.map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
        reject_link(item.path())?;
    }
    Ok(())
}

/// Reject `path` when it exists and is a symlink, junction, or mount point.
///
/// Windows has several unrelated reparse-point families (including cloud-file
/// placeholders and filesystem virtualization metadata). [`is_link_like`]
/// deliberately uses the platform's link-like file-type classification rather
/// than treating the broad reparse-point attribute as proof that `read_link`
/// is valid. Junctions and mount points remain link-like and are still rejected
/// anywhere they could redirect a write outside `<TO>` (finding C).
fn reject_link(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if is_link_like(&metadata) => Err(SkillManagerError::InvalidInput(format!(
            "seed destination path must not be a link: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SkillManagerError::io(path, error)),
    }
}

/// Whether `metadata` describes a filesystem link that path resolution may
/// follow. On Windows the extension methods are backed by the reparse tag, so
/// they include directory junctions/mount points while excluding unrelated
/// reparse-point families such as OneDrive/Cloud Files, `ProjFS`, and `WOF`.
#[cfg(windows)]
fn is_link_like(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::FileTypeExt;

    let file_type = metadata.file_type();
    file_type.is_symlink() || file_type.is_symlink_dir() || file_type.is_symlink_file()
}

#[cfg(not(windows))]
fn is_link_like(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

/// Whether `path` is a real directory that may be descended into as a copy
/// source root — a genuine directory that is not itself a symlink or Windows
/// reparse point (junction/mount point).
///
/// This matters because `Path::is_dir()` follows links, and `WalkDir` descends
/// a linked ROOT even with `follow_links(false)`. Without this, a target root
/// (or the `.skill-manager` root) that is a link pointing outside `<FROM>`
/// would be walked, letting the copy read outside `<FROM>` and write its
/// content into `<TO>` (finding G). The lexical within-`<FROM>` guard in
/// [`build_seed_items`] does not catch it, because the link path is itself
/// lexically inside `<FROM>`. A linked root is treated exactly like the
/// symlinks that [`merge_directory_files`] already skips inside a tree: it is
/// not a descendable directory, so the caller reports it as an explicit
/// link-skip (documented in `docs/cli.md`, "A configured source ROOT that is a
/// symlink or reparse point ... is never descended").
fn is_descendable_dir(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata.is_dir() && !is_link_like(&metadata),
        Err(_) => false,
    }
}

/// How a copy source ROOT under `<FROM>` presents on disk.
enum SourceRootKind {
    /// A genuine directory that may be descended and copied.
    Directory,
    /// A symlink or Windows reparse point (junction/mount point): never
    /// descended (finding G), and surfaced as an explicit link-skip so the
    /// omission is visible rather than silent (finding K).
    Link,
    /// Absent, or present as a plain file: not a copyable source directory.
    Absent,
}

/// Classify a copy source root without ever following a link (findings G/K).
fn classify_source_root(path: &Path) -> Result<SourceRootKind> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if is_link_like(&metadata) => Ok(SourceRootKind::Link),
        Ok(metadata) if metadata.is_dir() => Ok(SourceRootKind::Directory),
        Ok(_) => Ok(SourceRootKind::Absent),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SourceRootKind::Absent),
        Err(error) => Err(SkillManagerError::io(path, error)),
    }
}

/// Reject file/directory conflicts between an item's source tree and its
/// destination in BOTH directions (defect 4, extended by finding B): the
/// destination itself must be a directory or absent; every incoming path —
/// whether a file or a directory — is checked against what already exists at
/// the destination so that an incoming directory colliding with an existing
/// file, and an incoming file colliding with an existing directory, are both
/// caught here before the plan is rendered or a single byte is written. Any
/// symlink met while walking the destination is rejected too.
fn reject_seed_conflicts(item: &SeedItem) -> Result<()> {
    if item.destination.exists() && !item.destination.is_dir() {
        return Err(SkillManagerError::InvalidInput(format!(
            "seed destination {} exists and is not a directory",
            item.destination.display()
        )));
    }
    for (relative, incoming_is_dir) in seed_source_entries(&item.source, item.excluded)? {
        let parts: Vec<&str> = relative.split('/').collect();
        let mut current = item.destination.clone();
        for (index, part) in parts.iter().enumerate() {
            current = current.join(part);
            let is_last = index + 1 == parts.len();
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if is_link_like(&metadata) => {
                    return Err(SkillManagerError::InvalidInput(format!(
                        "seed destination path must not be a link: {}",
                        current.display()
                    )));
                }
                Ok(metadata) if is_last && incoming_is_dir && !metadata.is_dir() => {
                    return Err(SkillManagerError::InvalidInput(format!(
                        "seed destination {} already exists as a file but the source is a directory",
                        current.display()
                    )));
                }
                Ok(metadata) if is_last && !incoming_is_dir && metadata.is_dir() => {
                    return Err(SkillManagerError::InvalidInput(format!(
                        "seed destination {} already exists as a directory but the source is a file",
                        current.display()
                    )));
                }
                Ok(metadata) if !is_last && !metadata.is_dir() => {
                    return Err(SkillManagerError::InvalidInput(format!(
                        "seed destination {} exists and is not a directory",
                        current.display()
                    )));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(SkillManagerError::io(&current, error)),
            }
        }
    }
    Ok(())
}

/// Map slash-separated relative paths to every regular file and directory of
/// one tree (`true` marks a directory), pruning excluded top-level child names
/// before walking into them, and skipping symlinks/special entries the same
/// way [`merge_directory_files`] does. Directories are enumerated so that
/// preflight sees an incoming empty directory that would collide with an
/// existing destination file (finding B) rather than discovering it only when
/// apply fails at `create_dir_all` after already writing other items.
fn seed_source_entries(root: &Path, excluded_top_level: &[&str]) -> Result<BTreeMap<String, bool>> {
    let mut entries = BTreeMap::new();
    if !root.is_dir() {
        return Ok(entries);
    }
    let excluded = excluded_top_level
        .iter()
        .map(|name| fold(name))
        .collect::<BTreeSet<_>>();
    let walker = walkdir::WalkDir::new(root)
        .sort_by_file_name()
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() != 1 {
                return true;
            }
            !excluded.contains(&fold(&entry.file_name().to_string_lossy()))
        });
    for item in walker {
        let item = item.map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
        let metadata = std::fs::symlink_metadata(item.path())
            .map_err(|error| SkillManagerError::io(item.path(), error))?;
        if is_link_like(&metadata) || !(metadata.is_file() || metadata.is_dir()) {
            continue;
        }
        let relative = item.path().strip_prefix(root).map_err(|error| {
            SkillManagerError::InvalidInput(format!("invalid seed path: {error}"))
        })?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let relative = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        entries.insert(relative, metadata.is_dir());
    }
    Ok(entries)
}

/// Which configuration decided `configs copy`'s resolved target directories.
#[derive(Clone, Copy, Eq, PartialEq)]
enum SeedTargetSource {
    /// `<FROM>` has its own readable schema-v2 `.skill-manager/config.json`.
    FromConfig,
    /// `<FROM>` has no readable configuration; the active `--home`
    /// configuration decided targets.
    ActiveHome,
    /// Neither had a persisted configuration; built-in defaults decided targets.
    Defaults,
}

impl SeedTargetSource {
    /// Human-facing description for the plan's metadata line.
    fn label(self, home: &Path) -> String {
        match self {
            Self::FromConfig => "the source's own configuration".to_owned(),
            Self::ActiveHome => format!("the active configuration at {}", home.display()),
            Self::Defaults => "built-in defaults (no configuration found)".to_owned(),
        }
    }

    /// Stable machine-facing token for the `plan` event.
    const fn as_str(self) -> &'static str {
        match self {
            Self::FromConfig => "from-config",
            Self::ActiveHome => "active-config",
            Self::Defaults => "defaults",
        }
    }
}

/// Read a manager configuration directly from disk, never through
/// [`FileConfigRepository`], which would migrate, back up, and lock a home
/// that may be the caller's real one even under a dry run.
///
/// A missing `.skill-manager/config.json` returns `Ok(None)` so the caller
/// can fall through to the next precedence tier. A file that is present but
/// unreadable, not valid JSON, or not the current schema is an error naming
/// that file rather than a silent fall-through: for `<FROM>` that surfaces a
/// custom configuration the caller clearly intended to seed from, instead of
/// quietly copying the bytes while resolving targets from somewhere else
/// (defect 9). Layout is never migrated, so `<FROM>` is never mutated.
///
/// The configuration root and `config.json` are checked for links first
/// (finding J): a linked `.skill-manager` or `config.json` is treated as "no
/// usable configuration" and never read through, so an outside configuration
/// can never steer the copy.
fn read_seed_config(home: &Path) -> Result<Option<Config>> {
    // Gate the configuration ROOT before any read (finding J): if
    // `<home>/.skill-manager` is a symlink or reparse point, or `config.json`
    // is itself a link, an outside configuration could otherwise be read and
    // steer which target directories the copy pulls in. A link here is treated
    // as "no usable configuration", falling through to the next precedence tier
    // exactly as an absent config does, and is never read through.
    let config_root = home.join(".skill-manager");
    if !is_descendable_dir(&config_root) {
        return Ok(None);
    }
    let path = config_root.join("config.json");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if is_link_like(&metadata) => return Ok(None),
        Ok(metadata) if !metadata.is_file() => return Ok(None),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(SkillManagerError::io(&path, error)),
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(SkillManagerError::io(&path, error)),
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        SkillManagerError::InvalidInput(format!(
            "source configuration {} is not valid JSON: {error}",
            path.display()
        ))
    })?;
    let schema = value.get("schema_version").and_then(Value::as_u64);
    if schema != Some(u64::from(CONFIG_SCHEMA_VERSION)) {
        return Err(SkillManagerError::InvalidInput(format!(
            "source configuration {} has unsupported schema_version {}; expected {CONFIG_SCHEMA_VERSION}",
            path.display(),
            schema.map_or_else(|| "none".to_owned(), |value| value.to_string())
        )));
    }
    let mut config = serde_json::from_value::<Config>(value).map_err(|error| {
        SkillManagerError::InvalidInput(format!(
            "source configuration {} could not be parsed: {error}",
            path.display()
        ))
    })?;
    // Normalize target templates the same way the repository does on load, but
    // without ever writing back (finding A): a source config whose target path
    // contains `..` or is absolute could otherwise resolve a read outside
    // `<FROM>` and, via the destination join, a write outside `<TO>`, and an
    // un-normalized spelling like `.skill-manager/x/../cache` would slip past
    // the reserved cache/backup/lock exclusion. Rejecting the config here makes
    // both impossible by construction; the error names the offending source.
    normalize_config_targets(&mut config).map_err(|error| {
        SkillManagerError::InvalidInput(format!(
            "source configuration {} has an invalid target path: {error}",
            path.display()
        ))
    })?;
    Ok(Some(config))
}

/// Whether `path` is `root` itself or lies anywhere beneath it.
///
/// Path components are compared case-insensitively on Windows, matching
/// [`paths_equal`]'s handling of case-insensitive filesystem spellings, since
/// a lexical `==`/`starts_with` would treat two spellings of the same
/// directory as unrelated.
fn path_is_within(path: &Path, root: &Path) -> bool {
    let mut path_components = path.components();
    for root_component in root.components() {
        match path_components.next() {
            Some(component) if components_match(component, root_component) => {}
            _ => return false,
        }
    }
    true
}

/// Reject only a genuine recursion or self-overwrite hazard between the copied
/// source roots and `<TO>` (finding N), replacing the old blanket "`<TO>` must
/// not be nested under `<FROM>`" test that rejected the feature's headline
/// `configs copy ~ ./temp/...` use case whenever the destination happened to
/// live under the home.
///
/// Because the copy touches only `<FROM>/.skill-manager` and each resolved
/// target root — not the whole of `<FROM>` — the only real hazards are:
///
/// - `<TO>` is the same directory as `<FROM>` (a self-copy);
/// - `<TO>` lies inside a source root that will actually be copied (walking the
///   root would descend into the destination being written);
/// - a source root that will actually be copied lies inside `<TO>` (writing
///   `<TO>` could overwrite the source mid-read).
///
/// Link-skipped roots are never read or written, so they carry no hazard and
/// are excluded. Comparisons are lexical on component boundaries (so `C:\a\bc`
/// is not treated as nested inside `C:\a\b`) and case-insensitive on Windows,
/// via [`path_is_within`]; the identical-directory case uses [`paths_equal`] so
/// equivalent symlinked spellings still compare equal. The error names the
/// specific colliding source root rather than just reporting "nested".
fn reject_seed_recursion(from: &Path, to: &Path, items: &[SeedItem]) -> Result<()> {
    if paths_equal(from, to) {
        return Err(SkillManagerError::InvalidInput(format!(
            "seed source {} and seed destination {} are the same directory",
            from.display(),
            to.display()
        )));
    }
    for item in items {
        if item.source_is_link {
            continue;
        }
        if path_is_within(to, &item.source) {
            return Err(SkillManagerError::InvalidInput(format!(
                "seed destination {} is inside the copied source directory {} ({}), which would recurse into the destination as it is written",
                to.display(),
                item.label,
                item.source.display()
            )));
        }
        if path_is_within(&item.source, to) {
            return Err(SkillManagerError::InvalidInput(format!(
                "copied source directory {} ({}) is inside seed destination {}, which would overwrite the source as it is read",
                item.source.display(),
                item.label,
                to.display()
            )));
        }
    }
    Ok(())
}

fn components_match(left: std::path::Component<'_>, right: std::path::Component<'_>) -> bool {
    #[cfg(windows)]
    {
        left.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

/// Map slash-separated relative paths to the regular files of one tree,
/// pruning excluded top-level child names before ever walking into them.
///
/// Unlike [`crate::skills::directory_files`], a symlink or other special
/// entry is skipped rather than treated as an error: a real manager home or
/// target directory is not a skill tree subject to that portability
/// contract, and this best-effort seeding convenience should not fail on
/// content it merely will not copy.
fn merge_directory_files(
    root: &Path,
    excluded_top_level: &'static [&'static str],
) -> Result<BTreeMap<String, PathBuf>> {
    let mut files = BTreeMap::new();
    if !root.is_dir() {
        return Ok(files);
    }
    let excluded = excluded_top_level
        .iter()
        .map(|name| fold(name))
        .collect::<BTreeSet<_>>();
    let walker = walkdir::WalkDir::new(root)
        .sort_by_file_name()
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() != 1 {
                return true;
            }
            !excluded.contains(&fold(&entry.file_name().to_string_lossy()))
        });
    for item in walker {
        let item = item.map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
        let metadata = std::fs::symlink_metadata(item.path())
            .map_err(|error| SkillManagerError::io(item.path(), error))?;
        if is_link_like(&metadata) || !metadata.is_file() {
            continue;
        }
        let relative = item.path().strip_prefix(root).map_err(|error| {
            SkillManagerError::InvalidInput(format!("invalid seed path: {error}"))
        })?;
        let relative = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        files.insert(relative, item.path().to_path_buf());
    }
    Ok(files)
}

/// Merge-copy `source` into `destination`: create directories, overwrite
/// files by path, and never remove anything already present only at the
/// destination. Excluded top-level child names are pruned the same way
/// [`merge_directory_files`] prunes them from the plan.
fn merge_copy_tree(source: &Path, destination: &Path, excluded_top_level: &[&str]) -> Result<()> {
    // Reject a source ROOT that is a link or reparse point (finding G): this is
    // the apply-time counterpart to the preflight `is_descendable_dir` skip, so
    // a root swapped for a link between preflight and this write cannot make the
    // copy descend outside `<FROM>`. Preflight would have skipped a linked root,
    // so reaching one here means it was planted mid-flight — an error, matching
    // the apply-time destination-link recheck.
    match std::fs::symlink_metadata(source) {
        Ok(metadata) if is_link_like(&metadata) => {
            return Err(SkillManagerError::InvalidInput(format!(
                "seed source path must not be a link: {}",
                source.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(SkillManagerError::io(source, error)),
    }
    if !source.is_dir() {
        return Ok(());
    }
    let excluded = excluded_top_level
        .iter()
        .map(|name| fold(name))
        .collect::<BTreeSet<_>>();
    let walker = walkdir::WalkDir::new(source)
        .sort_by_file_name()
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() != 1 {
                return true;
            }
            !excluded.contains(&fold(&entry.file_name().to_string_lossy()))
        });
    for item in walker {
        let item = item.map_err(|error| SkillManagerError::InvalidInput(error.to_string()))?;
        let metadata = std::fs::symlink_metadata(item.path())
            .map_err(|error| SkillManagerError::io(item.path(), error))?;
        if is_link_like(&metadata) {
            continue;
        }
        let relative = item.path().strip_prefix(source).map_err(|error| {
            SkillManagerError::InvalidInput(format!("invalid seed path: {error}"))
        })?;
        let target = destination.join(relative);
        if metadata.is_dir() {
            reject_link(&target)?;
            std::fs::create_dir_all(&target)
                .map_err(|error| SkillManagerError::io(&target, error))?;
        } else if metadata.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| SkillManagerError::io(parent, error))?;
            }
            // Traversal-safe write (defect 3): never follow a destination
            // symlink/reparse point into an outside file. Preflight already
            // rejected pre-planted links, so this is defense in depth against
            // any that appeared since, matching how deployment writes to fresh
            // inodes rather than through a link.
            reject_link(&target)?;
            std::fs::copy(item.path(), &target)
                .map_err(|error| SkillManagerError::io(&target, error))?;
        }
    }
    Ok(())
}

/// Build the `configs copy` `plan` event payload.
///
/// This intentionally does not reuse [`plan_event_data`]: that shared shape
/// hardcodes an `entries[].skill` row key and a `summary.skills` count for
/// the five skill-oriented commands built on [`ChangePlan`]. This plan's rows
/// are directories, not skills, so it uses its own `items`/`summary.items`
/// vocabulary, documented separately in `docs/json.md`.
fn configs_copy_plan_data(
    from: &Path,
    to: &Path,
    target_source: SeedTargetSource,
    include_cache: bool,
    rows: &[SeedRow],
    revision: u64,
    dry_run: bool,
    authorization: PlanAuthorization,
) -> Value {
    let mut authorization_value = Map::new();
    authorization_value.insert("kind".into(), json!(authorization.kind));
    authorization_value.insert("mode".into(), json!(authorization.mode));
    if let Some(default) = authorization.default {
        authorization_value.insert("default".into(), json!(default));
    }

    let items = rows
        .iter()
        .map(|row| {
            let mut value = Map::new();
            value.insert("item".into(), json!(row.item.id));
            value.insert("path".into(), json!(row.item.destination));
            value.insert("existed".into(), json!(row.existed));
            if !row.stat.is_empty() {
                let mut diff = Map::new();
                diff.insert("files_changed".into(), json!(row.stat.files_changed()));
                if row.stat.insertions() > 0 {
                    diff.insert("insertions".into(), json!(row.stat.insertions()));
                }
                if row.stat.deletions() > 0 {
                    diff.insert("deletions".into(), json!(row.stat.deletions()));
                }
                value.insert("diff".into(), Value::Object(diff));
            }
            Value::Object(value)
        })
        .collect::<Vec<_>>();

    let new_count = rows
        .iter()
        .filter(|row| row.action == SeedAction::Copied)
        .count();
    let overwrite_count = rows
        .iter()
        .filter(|row| row.action == SeedAction::Merged)
        .count();
    let skipped_count = rows
        .iter()
        .filter(|row| row.action == SeedAction::Skipped)
        .count();
    let skipped_linked_count = rows
        .iter()
        .filter(|row| row.action == SeedAction::LinkSkipped)
        .count();
    let mut totals = Map::new();
    totals.insert("items".into(), json!(rows.len()));
    if new_count > 0 {
        totals.insert("new".into(), json!(new_count));
    }
    if overwrite_count > 0 {
        totals.insert("overwrite".into(), json!(overwrite_count));
    }
    if skipped_count > 0 {
        totals.insert("skipped".into(), json!(skipped_count));
    }
    if skipped_linked_count > 0 {
        totals.insert("skipped_linked".into(), json!(skipped_linked_count));
    }

    let mut data = Map::new();
    data.insert(
        "plan_id".into(),
        json!(format!("configs.copy:{}->{}", from.display(), to.display())),
    );
    data.insert("revision".into(), json!(revision));
    data.insert("command".into(), json!("configs.copy"));
    data.insert("dry_run".into(), json!(dry_run));
    data.insert("authorization".into(), Value::Object(authorization_value));
    data.insert("from".into(), json!(from));
    data.insert("to".into(), json!(to));
    data.insert("target_source".into(), json!(target_source.as_str()));
    if include_cache {
        data.insert("include_cache".into(), json!(true));
    }
    data.insert("items".into(), json!(items));
    data.insert("totals".into(), Value::Object(totals));
    Value::Object(data)
}

/// Render the `configs copy` plan: a `Configs copy plan` heading, `From`/`To`
/// metadata plus a `Target discovery` line naming which configuration
/// decided the resolved directories, then either the degenerate sentence (one
/// item) or a table (two or more items) of every directory that would be
/// merged — matching the degenerate-rendering rule in `docs/ux-guidelines.md`
/// exactly (a table needs at least two rows; a single-destination command
/// otherwise reduces to a sentence).
fn render_configs_copy_plan(
    from: &Path,
    to: &Path,
    target_source_label: &str,
    include_cache: bool,
    rows: &[SeedRow],
    style: RenderStyle,
) -> Vec<String> {
    let mut lines = vec![heading("Configs copy plan", style.color)];
    lines.push(String::new());
    let mut metadata = vec![
        ("From".to_owned(), from.display().to_string()),
        ("To".to_owned(), to.display().to_string()),
        (
            "Target discovery".to_owned(),
            target_source_label.to_owned(),
        ),
    ];
    if include_cache {
        metadata.push((
            "Cache/backups".to_owned(),
            "included (--include-cache)".to_owned(),
        ));
    }
    let label_width = metadata
        .iter()
        .map(|(label, _)| display_width(label))
        .max()
        .unwrap_or(0);
    for (label, value) in &metadata {
        lines.push(join_columns(&[padded(label, label_width), value.clone()]));
    }
    lines.push(String::new());

    if let [row] = rows {
        lines.push(seed_row_sentence(row, style));
    } else {
        lines.extend(seed_row_table(rows, style));
    }
    lines
}

fn seed_row_action(row: &SeedRow) -> PlanAction {
    match row.action {
        SeedAction::Copied => PlanAction::Copy,
        SeedAction::Merged => PlanAction::Update,
        SeedAction::Skipped | SeedAction::LinkSkipped => PlanAction::Skip,
    }
}

fn seed_row_change(row: &SeedRow) -> String {
    match row.action {
        SeedAction::Copied => creation_line("copy", &row.stat),
        SeedAction::Merged => totals_line(&row.stat),
        SeedAction::Skipped => "already identical".to_owned(),
        SeedAction::LinkSkipped => "linked source, not copied".to_owned(),
    }
}

fn seed_row_sentence(row: &SeedRow, style: RenderStyle) -> String {
    let action = seed_row_action(row);
    let marker = colored(
        action_text(action, style.symbols),
        action.color_code(),
        style.color,
    );
    format!("{marker} {}: {}", row.item.label, seed_row_change(row))
}

fn seed_row_table(rows: &[SeedRow], style: RenderStyle) -> Vec<String> {
    let headers = ["item", "change", "action"];
    let table_rows = rows
        .iter()
        .map(|row| {
            let action = seed_row_action(row);
            let plain_action = action_text(action, style.symbols).to_owned();
            let styled_action = colored(&plain_action, action.color_code(), style.color);
            let change = seed_row_change(row);
            (
                vec![row.item.label.clone(), change.clone(), plain_action],
                vec![row.item.label.clone(), change, styled_action],
            )
        })
        .collect::<Vec<_>>();

    let mut widths = headers
        .iter()
        .map(|header| display_width(header))
        .collect::<Vec<_>>();
    for (measured, _) in &table_rows {
        for (index, cell) in measured.iter().enumerate() {
            widths[index] = widths[index].max(display_width(cell));
        }
    }
    let header = headers
        .iter()
        .enumerate()
        .map(|(index, header)| padded(header, widths[index]))
        .collect::<Vec<_>>();
    let mut lines = vec![join_columns(&header), separator(&widths)];
    for (measured, rendered) in &table_rows {
        let columns = measured
            .iter()
            .zip(rendered)
            .enumerate()
            .map(|(index, (raw, styled))| {
                let padding = " ".repeat(widths[index].saturating_sub(display_width(raw)));
                format!("{styled}{padding}")
            })
            .collect::<Vec<_>>();
        lines.push(join_columns(&columns));
    }
    lines
}

/// The `configs copy` plan footer: total actionable items merged into the one
/// destination, then nonzero-only new, overwrite, and already-identical
/// clauses — the same grammar `load`'s own footer uses (see
/// [`load_plan_footer`]), so an all-identical no-op reads as `✓ N already
/// identical` rather than a bare, empty "changes" line.
fn configs_copy_plan_footer(rows: &[SeedRow], style: RenderStyle) -> String {
    let new_count = rows
        .iter()
        .filter(|row| row.action == SeedAction::Copied)
        .count();
    let overwrite_count = rows
        .iter()
        .filter(|row| row.action == SeedAction::Merged)
        .count();
    let skipped_count = rows
        .iter()
        .filter(|row| row.action == SeedAction::Skipped)
        .count();
    let linked_skipped_count = rows
        .iter()
        .filter(|row| row.action == SeedAction::LinkSkipped)
        .count();
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
    if skipped_count > 0 {
        clauses.push(clause(
            "✓",
            skipped_count,
            "already identical",
            PlanAction::Skip.color_code(),
        ));
    }
    if linked_skipped_count > 0 {
        // Distinct reason from "already identical": a link-skip is a
        // deliberately-not-acted-on omission, rendered with the neutral "—"
        // marker rather than the "✓" a genuine no-op uses.
        clauses.push(clause(
            "—",
            linked_skipped_count,
            "skipped (linked source)",
            None,
        ));
    }
    format!(
        "{} to 1 destination: {}",
        counted_noun(new_count + overwrite_count, "change"),
        clauses.join(", ")
    )
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
/// open, or concrete
/// [`crate::plan::PlanAction::Remove`] actions once it is resolved (whether by
/// explicit scope, `--both`, or an inference that never actually branched).
/// Also returns the flat apply list for the resolved case; the deferred case
/// builds its apply list only after interactive selection.
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
/// [`crate::app::Application::apply_remove_items`] for this choice and diffs
/// each item for real, rather than combining a per-skill representative count
/// across cells. This is what keeps a branch option's advertised blast radius
/// from drifting away from what selecting it actually deletes when deployments
/// across scopes have genuinely diverged.
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

fn styled_heading(text: &str, color: bool) -> String {
    if color {
        format!("\u{1b}[1;36m{text}\u{1b}[0m")
    } else {
        text.to_owned()
    }
}

/// Resolve the local source directory an import would overwrite.
///
/// A GitHub-backed source with a configured local alternate resolves to that
/// alternate automatically, before any plan renders — the owner rejected a
/// preliminary "use the alternate?" confirmation, so the alternate is simply
/// folded into the plan instead of asked about. A GitHub-backed source with
/// no alternate fails immediately, before rendering any alternatives.
fn import_destination(candidate: &SkillCandidate) -> Result<(PathBuf, bool)> {
    let entry = &candidate.source.entry;
    if entry.source_type == SourceType::Local {
        return Ok((portable_path(&candidate.path), false));
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
    Ok((destination, true))
}

/// The single [`Destination`] representing the canonical source itself.
fn import_source_destination(source_label: &str) -> Destination {
    Destination {
        id: format!("{source_label}:source"),
        column: source_label.to_owned(),
        label: format!("{source_label} (source)"),
        kind: DestinationKind::Source {
            source: source_label.to_owned(),
        },
        path: None,
    }
}

/// Replicate `plan::delta_cell`'s private formatting locally: `+ins/-del`, or
/// a signed byte count for binary content.
fn import_delta_value(file: &FileDelta) -> String {
    if file.binary {
        let sign = if file.bytes < 0 { "-" } else { "+" };
        return format!("bin {sign}{} bytes", file.bytes.unsigned_abs());
    }
    format!("+{}/-{}", file.insertions, file.deletions)
}

/// One nested consequence line per changed file in a source copy's own diff.
fn import_source_entries(stat: &DiffStat) -> Vec<PreviewEntry> {
    stat.files
        .iter()
        .map(|file| PreviewEntry {
            marker: Some(file.change.symbol().to_owned()),
            marker_color: Some(match file.change {
                FileChange::Added => 32,
                FileChange::Deleted => 31,
                FileChange::Modified => 33,
            }),
            label: file.path.clone(),
            value: import_delta_value(file),
            value_color: None,
        })
        .collect()
}

/// Build one destination's propagation preview entry and planned action.
///
/// `incoming` is always the source copy's own content — the same content
/// that would flow into every other deployment if this copy were chosen.
/// Diffing each deployment's current directory (`existing`) against that one
/// path, rather than combining a per-skill representative count, is what
/// keeps an option's advertised propagation preview identical to what
/// `apply_import` would actually write for that exact copy. `own_id` is the
/// resolved candidate's own destination identity: only that entry is labeled
/// "source copy", because that is the one actually being adopted. A
/// *different* destination that merely happens to already carry identical
/// content is still a no-write, but it is labeled "synchronized" on its own
/// — never "source copy" — since content equality is not the same as being
/// the copy chosen.
fn import_propagation(
    deployed: &[ImportDeployment],
    incoming: &Path,
    own_id: &str,
) -> Result<(Vec<PreviewEntry>, Vec<PlannedAction>)> {
    let mut entries = Vec::with_capacity(deployed.len());
    let mut actions = Vec::with_capacity(deployed.len());
    for item in deployed {
        let stat = diff_directories(&item.path, incoming)?;
        let synchronized = stat.is_empty();
        let value = if item.id == own_id {
            "✓ source copy; synchronized, no file changes".to_owned()
        } else if synchronized {
            "✓ synchronized, no file changes".to_owned()
        } else {
            format!("{} {}", PlanAction::Update.symbol(), totals_line(&stat))
        };
        entries.push(PreviewEntry {
            marker: None,
            marker_color: None,
            label: item.label.clone(),
            value,
            value_color: (!synchronized).then_some(33),
        });
        actions.push(PlannedAction {
            destination: item.id.clone(),
            action: if synchronized {
                PlanAction::Skip
            } else {
                PlanAction::Update
            },
            existed: true,
            description: String::new(),
            stat,
        });
    }
    Ok((entries, actions))
}

/// Build the "Left out of date" block that replaces "Propagation preview"
/// once propagation is resolved to import-only: nothing will be written, so
/// entries must read as staleness (`N file(s) behind, +ins/-del`) rather
/// than a pending write (`↑ ...`), which would promise a synchronization
/// that import-only deliberately does not perform. Built from `actions` --
/// the exact per-destination consequences the resolved option already
/// carries -- rather than a fresh diff, so the rendered count can never
/// drift from what the option actually enumerated. A destination whose
/// action is `Skip` (already synchronized, or the resolved copy's own
/// identity) reads as a none-value under this framing and is dropped, same
/// as any other recursively-gated none-value column.
///
/// `actions` must be in the same order as `deployed` (as `import_propagation`
/// produces them), so the two can be zipped by position.
fn import_staleness_block(
    deployed: &[ImportDeployment],
    actions: &[PlannedAction],
) -> PreviewBlock {
    let entries = deployed
        .iter()
        .zip(actions)
        .filter(|(_, action)| action.action == PlanAction::Update)
        .map(|(item, action)| PreviewEntry {
            marker: None,
            marker_color: None,
            label: item.label.clone(),
            value: format!(
                "{} behind, +{}/-{}",
                counted_noun(action.stat.files_changed(), "file"),
                action.stat.insertions(),
                action.stat.deletions()
            ),
            value_color: Some(33),
        })
        .collect();
    PreviewBlock {
        heading: "Left out of date".to_owned(),
        entries,
        ..PreviewBlock::default()
    }
}

/// Count how many propagation actions are genuine updates versus already
/// synchronized skips (normally exactly one skip: the copy's own deployment).
fn import_propagation_counts(actions: &[PlannedAction]) -> (usize, usize) {
    let updated = actions
        .iter()
        .filter(|action| action.action == PlanAction::Update)
        .count();
    let skipped = actions
        .iter()
        .filter(|action| action.action == PlanAction::Skip)
        .count();
    (updated, skipped)
}

/// Build one source-copy alternative: its own diff, and — only when
/// choosing it would actually leave something to propagate — a nested
/// preview of what it would propagate to every other deployment.
///
/// Propagation is not a genuine dimension for a candidate whose own content
/// already matches every other deployment: nothing would be written either
/// way, so the nested "Propagation with import + update" block is elided for
/// that candidate the same way a genuinely empty preview would be gated
/// anywhere else. The caller uses the returned `(updated, skipped)` counts to
/// decide whether the whole `propagation` dimension is genuine across every
/// candidate.
fn import_source_option(
    item: &ImportCandidate,
    index: usize,
    source_destination_id: &str,
    deployed: &[ImportDeployment],
) -> Result<(DecisionOption, Vec<PreviewEntry>, usize, usize)> {
    let id = destination_id(&item.target.name, item.scope);
    let path_string = portable_canonicalize(&item.deployment)
        .display()
        .to_string();
    let (entries, propagation_actions) = import_propagation(deployed, &item.deployment, &id)?;
    let (updated, skipped) = import_propagation_counts(&propagation_actions);
    let mut actions = propagation_actions.clone();
    actions.insert(
        0,
        PlannedAction {
            destination: source_destination_id.to_owned(),
            action: PlanAction::Import,
            existed: true,
            description: String::new(),
            stat: item.stat.clone(),
        },
    );
    let mut detail = vec![OptionDetail::Fields(vec![
        PreviewField {
            label: "Path".to_owned(),
            value: path_string.clone(),
            ..PreviewField::default()
        },
        PreviewField {
            label: "Source".to_owned(),
            value: format!(
                "{} {}",
                PlanAction::Import.symbol(),
                totals_line(&item.stat)
            ),
            value_color: Some(33),
            entries: import_source_entries(&item.stat),
        },
    ])];
    if updated > 0 {
        detail.push(OptionDetail::Block(PreviewBlock {
            heading: "Propagation with import + update".to_owned(),
            heading_value: Some(counted_noun(deployed.len(), "deployment")),
            entries: entries.clone(),
            ..PreviewBlock::default()
        }));
    }
    let option = DecisionOption {
        id,
        token: (index + 1).to_string(),
        label: format!("{} · {}", item.target.name, item.scope.as_str()),
        detail,
        consequence: OptionConsequence {
            operation: Some(PlanAction::Import),
            path: Some(PathBuf::from(path_string)),
            actions,
            totals: vec![("deployments".to_owned(), deployed.len() as u64)],
        },
        ..DecisionOption::default()
    };
    Ok((option, entries, updated, skipped))
}

/// The two propagation alternatives, with notes and typed consequences
/// computed from the actual candidate rather than a fixed formula, so the
/// promised counts can never drift from what applying that option writes.
///
/// `updated` is how many deployments differ from the resolved source copy —
/// exactly what stays out of date if `Import only` is chosen instead, which
/// is why the same number appears in both options' prose.
fn import_propagation_options(
    deployed_len: usize,
    updated: usize,
    skipped: usize,
    resolved: bool,
) -> Vec<DecisionOption> {
    // A zero count is never printed, even in the shared provisional note
    // rendered while the source copy is still pending: this branch is not
    // normally reachable once the source resolves (a degenerate candidate
    // auto-resolves `propagation` before this text renders again), but the
    // guard stays unconditional so no future caller can slip a literal `0`
    // past it.
    let update_note = if resolved {
        if updated == 0 {
            format!(
                "Replace the source and synchronize {} (1 source copy).",
                counted_noun(deployed_len, "deployment")
            )
        } else {
            format!(
                "Replace the source and synchronize {} (1 source copy, {updated} updated).",
                counted_noun(deployed_len, "deployment")
            )
        }
    } else {
        "Replace the source, then synchronize every deployment shown for that copy.".to_owned()
    };
    let only_note = if updated == 0 {
        "Replace the source; write no deployments.".to_owned()
    } else if resolved {
        format!("Replace the source; write no deployments and leave {updated} out of date.")
    } else {
        format!(
            "Replace the source; write no deployments and leave the other {updated} out of date."
        )
    };
    vec![
        DecisionOption {
            id: "import-update".to_owned(),
            token: "1".to_owned(),
            label: "Import + update".to_owned(),
            recommended: true,
            detail: vec![OptionDetail::Note(update_note)],
            consequence: OptionConsequence {
                operation: Some(PlanAction::Update),
                totals: vec![
                    ("deployments".to_owned(), deployed_len as u64),
                    ("updated".to_owned(), updated as u64),
                    ("skipped".to_owned(), skipped as u64),
                ],
                ..OptionConsequence::default()
            },
            ..DecisionOption::default()
        },
        DecisionOption {
            id: "import-only".to_owned(),
            token: "2".to_owned(),
            label: "Import only".to_owned(),
            detail: vec![OptionDetail::Note(only_note)],
            consequence: OptionConsequence {
                operation: Some(PlanAction::Import),
                totals: vec![("stale".to_owned(), updated as u64)],
                ..OptionConsequence::default()
            },
            ..DecisionOption::default()
        },
    ]
}

/// Build `import`'s `source_copy` decision.
///
/// Carries both headings, unlike `propagation`: the mocks show a bare
/// numbered list only while `source_copy` is genuinely active. Every other
/// render — `--dry-run`, an ambiguous `--yes`, a `--no-input` refusal — still
/// shows the same numbered list without a live prompt, and it needs its own
/// label there too so two numbered `1`/`2` lists (this one and
/// `propagation`'s) are never left to read as one.
fn import_source_decision(options: Vec<DecisionOption>, resolved: Option<String>) -> Decision {
    Decision {
        id: "source_copy".to_owned(),
        heading: Some("Available source copies".to_owned()),
        deferred_heading: Some("Source copies (chosen first)".to_owned()),
        prompt: "Select source copy".to_owned(),
        options,
        resolved,
        ..Decision::default()
    }
}

/// Build `import`'s `propagation` decision.
///
/// `heading` stays `None` even while active: the mocks never show a heading
/// line above the propagation options, only the deferred heading while the
/// dimension is still pending behind `source_copy`.
fn import_propagation_decision(
    deployed_len: usize,
    updated: usize,
    skipped: usize,
    resolved_note: bool,
    resolved: Option<String>,
) -> Decision {
    Decision {
        id: "propagation".to_owned(),
        deferred_heading: Some("Propagation modes (chosen after the source copy)".to_owned()),
        prompt: "Select propagation".to_owned(),
        options: import_propagation_options(deployed_len, updated, skipped, resolved_note),
        resolved,
        ..Decision::default()
    }
}

/// The plan footer shown just before a prompt (or, for `--dry-run`, before
/// the alternatives message): how many source copies and propagation modes
/// remain to choose between.
fn import_pending_footer(
    candidates_len: usize,
    source_pending: bool,
    propagation_pending: bool,
    source_selected_by_prompt: bool,
) -> String {
    let copies = if candidates_len == 1 {
        "1 source copy".to_owned()
    } else {
        format!("{candidates_len} source copies")
    };
    if source_pending && propagation_pending {
        return format!("{copies}; propagation decision follows source selection");
    }
    if source_pending {
        // Propagation was already resolved by flag: the deferred clause no
        // longer applies, since there is nothing left to defer it behind.
        return copies;
    }
    let source_clause = if source_selected_by_prompt {
        "1 source copy selected".to_owned()
    } else {
        copies
    };
    format!("{source_clause}; {}", counted_noun(2, "propagation mode"))
}

/// The pre-apply footer for a plan whose both dimensions were already
/// resolved when it rendered — never narrowed by an interactive answer.
///
/// Grammar per the normative spec: the import-only form always names the
/// resolved target/scope (`1 source replacement from claude · global`) and
/// only appends the staleness consequence when it is non-zero, since that
/// consequence is genuinely significant (import-only deliberately creates
/// staleness) even though the target/scope naming already satisfies the
/// spec's minimum. The import+update form keeps its existing wording and
/// additionally reports any deployment that was already identical to the
/// resolved copy, so the breakdown always sums to `deployed_len`.
///
/// The sync form is used only when `update` is set AND `updated` is
/// non-zero: an explicit `--update`/`update:true` on a degenerate plan
/// records an honest `import-update` in the machine stream (see
/// `propagation_resolved_id`), but there is nothing to synchronize, and the
/// sync form's `{updated} updated` clause has no way to omit a zero count
/// the way the plain form's trailing clause can. Falling back to the plain
/// form keeps the human render free of the forbidden `0 updated`, which
/// still teaches correctly here since nothing was written either way.
fn import_resolved_footer(
    update: bool,
    target_label: &str,
    deployed_len: usize,
    updated: usize,
    skipped: usize,
) -> String {
    use std::fmt::Write as _;
    if update && updated > 0 {
        let already_identical = skipped.saturating_sub(1);
        let mut footer = format!(
            "1 source replacement; {} synchronized (1 source copy, {updated} updated",
            counted_noun(deployed_len, "deployment")
        );
        if already_identical > 0 {
            let _ = write!(footer, ", {already_identical} already identical");
        }
        footer.push(')');
        footer
    } else {
        let mut footer = format!("1 source replacement from {target_label}");
        if updated > 0 {
            let _ = write!(
                footer,
                "; {} left out of date",
                counted_noun(updated, "deployment")
            );
        }
        footer
    }
}

/// The post-apply result footer: one `ResultEntry` describing the source
/// replacement and, when propagation genuinely ran, the deployments it
/// synchronized. Same `update && updated > 0` guard as
/// `import_resolved_footer`: an explicit `--update` on a degenerate plan
/// still applies with `update: true` (so the applied stream honestly emits
/// a `skill.skipped` per destination), but `updated` is then always zero,
/// so the synchronized clause -- which has no way to omit that count --
/// must be omitted entirely rather than print the forbidden `0 updated`.
fn import_result_footer(
    stat: &DiffStat,
    update: bool,
    deployed_len: usize,
    updated: usize,
    skipped: usize,
    style: RenderStyle,
) -> String {
    let mut description = format!(
        "source replaced ({}, +{}/-{})",
        counted_noun(stat.files_changed(), "file"),
        stat.insertions(),
        stat.deletions()
    );
    if update && updated > 0 {
        use std::fmt::Write as _;
        let already_identical = skipped.saturating_sub(1);
        let _ = write!(
            description,
            ", {} synchronized (1 source copy, {updated} updated",
            counted_noun(deployed_len, "deployment")
        );
        if already_identical > 0 {
            let _ = write!(description, ", {already_identical} already identical");
        }
        description.push(')');
    }
    result_footer(
        &[ResultEntry {
            marker: ResultMarker::Completed,
            count: 1,
            description,
        }],
        style,
    )
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

fn source_selector_index(config: &Config, selector: &str, home: &Path) -> Result<usize> {
    if let Some(index) = configured_source_index(config, selector, home)? {
        return Ok(index);
    }
    Err(SkillManagerError::NotFound {
        kind: "source",
        reference: selector.to_owned(),
    })
}

fn configured_source_index(config: &Config, selector: &str, home: &Path) -> Result<Option<usize>> {
    if let Some(index) = find_source_index(config, selector, home)? {
        return Ok(Some(index));
    }
    if let Ok(candidate) = location_from_reference(selector, SourceMode::Collection, home)
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
    home: &Path,
) -> Result<SourceEntry> {
    if let Some(index) = configured_source_index(config, reference, home)? {
        return config.sources.get(index).cloned().ok_or_else(|| {
            SkillManagerError::InvalidInput("source index changed unexpectedly".into())
        });
    }
    source_from_reference(reference, mode, home)
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

fn operand_is_existing_directory(operand: &str, home: &Path) -> Result<bool> {
    let expanded = expand_home(operand, home);
    let path = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map_err(|error| SkillManagerError::io(".", error))?
            .join(expanded)
    };
    Ok(path.is_dir())
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

fn source_matches(source: &SourceEntry, selector: &str, home: &Path) -> bool {
    if [source.id.as_str(), source.name.as_str()]
        .iter()
        .any(|value| fold(value) == fold(selector))
    {
        return true;
    }
    location_from_reference(selector, source.mode, home).is_ok_and(|location| {
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
    canonicalize_existing_ancestor(&make_absolute(path)?)
}

/// Make `path` absolute without following links or normalizing components.
/// Keeping this separate lets security-sensitive callers inspect the exact
/// component walk before ordinary physical path resolution changes it.
fn make_absolute(path: PathBuf) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|error| SkillManagerError::io(".", error))?
            .join(path)
    };
    Ok(absolute)
}

/// Reject any link-like component present in the original absolute spelling
/// of a security-sensitive destination.
///
/// Components continue to be inspected after a missing entry: a later `..`
/// can return to an existing ancestor, after which another link could still be
/// traversed by the operating system. A link is rejected before applying a
/// following `..`, which is the distinction lost if the path is canonicalized
/// first.
fn reject_linked_path_components(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                current.push(component.as_os_str());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                current.pop();
            }
            std::path::Component::Normal(name) => {
                current.push(name);
                reject_link(&current)?;
            }
        }
    }
    Ok(())
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
/// each component is resolved in turn. Links are read explicitly and their
/// targets are fed back through the same component walker before later input
/// components are considered. A `..` is therefore applied only after any
/// symlink at that position has been followed. Link traversal is bounded so
/// cycles fail instead of recursing forever. Once a component does not exist,
/// resolution stops and the remaining tail — including any `..` components —
/// is appended literally.
fn canonicalize_existing_ancestor(path: &Path) -> Result<PathBuf> {
    const MAX_SYMLINK_RESOLUTIONS: usize = 40;

    canonicalize_existing_ancestor_bounded(path, 0, MAX_SYMLINK_RESOLUTIONS)
}

fn canonicalize_existing_ancestor_bounded(
    path: &Path,
    followed_links: usize,
    max_followed_links: usize,
) -> Result<PathBuf> {
    let mut resolved = PathBuf::new();
    let mut components = path.components();
    while let Some(component) = components.next() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                resolved.push(component.as_os_str());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::Normal(name) => {
                let candidate = resolved.join(name);
                match fs::symlink_metadata(&candidate) {
                    Ok(metadata) if is_link_like(&metadata) => {
                        if followed_links == max_followed_links {
                            return Err(SkillManagerError::io(
                                &candidate,
                                std::io::Error::other(
                                    "too many levels of symbolic links while resolving path",
                                ),
                            ));
                        }
                        let target = fs::read_link(&candidate)
                            .map_err(|error| SkillManagerError::io(&candidate, error))?;
                        let mut redirected = if target.is_absolute() {
                            target
                        } else if let Some(parent) = candidate.parent() {
                            parent.join(target)
                        } else {
                            target
                        };
                        for remaining in components {
                            redirected.push(remaining.as_os_str());
                        }
                        return canonicalize_existing_ancestor_bounded(
                            &redirected,
                            followed_links + 1,
                            max_followed_links,
                        );
                    }
                    Ok(_) => resolved = candidate,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        resolved = portable_path(
                            &resolved
                                .canonicalize()
                                .map_err(|error| SkillManagerError::io(&resolved, error))?,
                        );
                        resolved.push(name);
                        for remaining in components {
                            resolved.push(remaining.as_os_str());
                        }
                        return Ok(portable_path(&resolved));
                    }
                    Err(error) => return Err(SkillManagerError::io(&candidate, error)),
                }
            }
        }
    }
    let canonical = resolved
        .canonicalize()
        .map_err(|error| SkillManagerError::io(&resolved, error))?;
    Ok(portable_path(&canonical))
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
/// `home_override` is the parsed `--home` flag value, if any, threaded
/// explicitly from `main` rather than read from process-global state; it
/// takes precedence over `SKILL_MANAGER_HOME` and the OS home (see
/// [`manager_home`]).
///
/// # Errors
///
/// Returns an error when the operating system does not provide a user home.
pub fn production_repository(
    home_override: Option<&Path>,
) -> Result<(FileConfigRepository, PathBuf)> {
    let home = manager_home(home_override)?;
    Ok((FileConfigRepository::new(home.clone()), home))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};

    use indexmap::IndexMap;

    use super::{
        Application, RunOutcome, absolute_path, command_dry_run, find_named_key,
        normalized_patterns, path_is_within, set_target_enabled, skill_action_data, source_data,
        source_matches, status_matches, target_data, title_case,
    };
    use crate::cache::GitHubTransport;
    use crate::cli::{
        Command, CopyArgs, DescribeArgs, DescribeSelection, ImportArgs, LoadArgs, RemoveArgs,
        SourceAction, SourceAddArgs, SourceArgs, SourceModeArg, SourceRemoveArgs, SourceUpdateArgs,
        StatusArgs, SyncArgs, TargetAction, TargetAddArgs, TargetArgs, TargetNameArgs,
        TargetPathArgs, UpdateArgs,
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

    #[cfg(unix)]
    fn create_directory_symlink(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).unwrap_or_else(|error| unreachable!("{error}"));
        true
    }

    #[cfg(windows)]
    fn create_directory_symlink(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }

    /// Create a Windows directory junction without requiring symlink
    /// privilege. The stable standard library does not yet expose junction
    /// creation, so tests use the platform's built-in `mklink /J` command and
    /// fail loudly if it cannot create the disposable fixture.
    #[cfg(windows)]
    fn create_directory_junction(target: &Path, link: &Path) {
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            output.status.success(),
            "mklink /J failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The recursion/self-overwrite guard compares on path COMPONENT
    /// boundaries (finding N), so a sibling whose name merely shares a prefix —
    /// `.../a/bc` versus `.../a/b` — must NOT be treated as nested, while a
    /// genuine descendant must. On Windows the comparison is case-insensitive.
    #[test]
    fn path_is_within_matches_on_component_boundaries_not_string_prefixes() {
        use std::path::Path;

        // Genuine nesting: a directory and its descendant, and reflexive self.
        assert!(path_is_within(Path::new("/a/b/c"), Path::new("/a/b")));
        assert!(path_is_within(Path::new("/a/b"), Path::new("/a/b")));

        // Prefix-but-not-nested: `bc` is not inside `b`.
        assert!(!path_is_within(Path::new("/a/bc"), Path::new("/a/b")));
        assert!(!path_is_within(Path::new("/a/b"), Path::new("/a/bc")));

        #[cfg(windows)]
        {
            assert!(path_is_within(
                Path::new(r"C:\Users\me\repo"),
                Path::new(r"c:\users\ME")
            ));
            assert!(!path_is_within(Path::new(r"C:\a\bc"), Path::new(r"C:\a\b")));
        }
    }

    /// The Windows link-like predicate must classify a real junction as a link
    /// while leaving an ordinary directory alone. This uses the reparse-tag-
    /// aware file-type APIs, rather than the broad reparse-point attribute that
    /// also appears on unrelated Cloud Files/ProjFS/WOF entries.
    #[cfg(windows)]
    #[test]
    fn is_link_like_flags_a_real_junction_but_not_a_plain_directory() {
        let scratch = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let target = scratch.path().join("target");
        std::fs::create_dir_all(&target).unwrap_or_else(|error| unreachable!("{error}"));
        let plain = scratch.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap_or_else(|error| unreachable!("{error}"));

        let plain_metadata =
            std::fs::symlink_metadata(&plain).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            !super::is_link_like(&plain_metadata),
            "a plain directory is not link-like"
        );

        let link = scratch.path().join("junction");
        create_directory_junction(&target, &link);
        let junction_metadata =
            std::fs::symlink_metadata(&link).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            super::is_link_like(&junction_metadata),
            "a real junction must be classified as link-like"
        );
    }

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
        event_data: Vec<serde_json::Value>,
        human: Vec<String>,
        diagnostics: Vec<String>,
    }

    impl Reporter for RecordingReporter {
        fn event(&mut self, event: &str, _level: Level, data: serde_json::Value) -> Result<()> {
            self.events.push(event.into());
            self.event_data.push(data);
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
        let mut entry = source_from_reference("owner/repository", None, root.path())
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
        assert!(source_matches(&entry, "PRIMARY-SOURCE", root.path()));
        assert!(source_matches(&entry, "OWNER/REPOSITORY", root.path()));
        assert!(!source_matches(&entry, "secondary", root.path()));
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
        let entry = source_from_reference("owner/repository:main/team", None, root.path())
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

        if !create_directory_symlink(&inner, &alias) {
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

    /// Deterministic Windows coverage for the original `link/../destination`
    /// defect. A junction exercises the same reparse-point traversal without
    /// requiring Developer Mode or symbolic-link privilege.
    #[cfg(windows)]
    #[test]
    fn absolute_path_resolves_parent_after_a_windows_junction_target() {
        let sandbox = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let real_root = sandbox.path().join("real-root");
        let inner = real_root.join("inner");
        std::fs::create_dir_all(&inner).unwrap_or_else(|error| unreachable!("{error}"));
        let alias = sandbox.path().join("junction");
        create_directory_junction(&inner, &alias);

        let resolved = absolute_path(alias.join("..").join("destination"))
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            resolved,
            portable_canonicalize(&real_root).join("destination"),
            "a Windows junction target must be resolved before applying '..'"
        );
    }

    #[test]
    fn absolute_path_resolves_relative_and_chained_directory_symlinks() {
        let sandbox = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let real_root = sandbox.path().join("real-root");
        let inner = real_root.join("inner");
        std::fs::create_dir_all(&inner).unwrap_or_else(|error| unreachable!("{error}"));
        let second_alias = sandbox.path().join("second-alias");
        let first_alias = sandbox.path().join("first-alias");

        if !create_directory_symlink(
            Path::new("real-root").join("inner").as_path(),
            &second_alias,
        ) || !create_directory_symlink(Path::new("second-alias"), &first_alias)
        {
            return;
        }

        let resolved = absolute_path(first_alias.join("..").join("destination"))
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            resolved,
            portable_canonicalize(&real_root).join("destination"),
            "relative and chained link targets must resolve before applying later components"
        );
    }

    /// Deterministic Windows coverage for chained link resolution. The generic
    /// symlink test above keeps relative-target coverage on Unix and runs on
    /// privileged Windows hosts; junctions guarantee the chained Windows path
    /// is always exercised.
    #[cfg(windows)]
    #[test]
    fn absolute_path_resolves_chained_windows_junctions() {
        let sandbox = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let real_root = sandbox.path().join("real-root");
        let inner = real_root.join("inner");
        std::fs::create_dir_all(&inner).unwrap_or_else(|error| unreachable!("{error}"));
        let second_alias = sandbox.path().join("second-junction");
        let first_alias = sandbox.path().join("first-junction");
        create_directory_junction(&inner, &second_alias);
        create_directory_junction(&second_alias, &first_alias);

        let resolved = absolute_path(first_alias.join("..").join("destination"))
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            resolved,
            portable_canonicalize(&real_root).join("destination"),
            "chained Windows junctions must resolve before later components"
        );
    }

    #[test]
    fn absolute_path_keeps_the_tail_literal_after_the_first_missing_component() {
        let sandbox = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let existing = sandbox.path().join("existing");
        std::fs::create_dir_all(&existing).unwrap_or_else(|error| unreachable!("{error}"));
        let requested = existing.join("missing").join("..").join("destination");

        let resolved = absolute_path(requested).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            resolved,
            portable_canonicalize(&existing)
                .join("missing")
                .join("..")
                .join("destination"),
            "components after the deepest existing ancestor must stay literal"
        );
    }

    #[test]
    fn absolute_path_rejects_a_directory_symlink_cycle() {
        let sandbox = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let first = sandbox.path().join("first");
        let second = sandbox.path().join("second");
        if !create_directory_symlink(Path::new("second"), &first)
            || !create_directory_symlink(Path::new("first"), &second)
        {
            return;
        }

        let error = match absolute_path(first.join("destination")) {
            Ok(path) => unreachable!("symlink cycle resolved unexpectedly to {}", path.display()),
            Err(error) => error,
        };
        assert!(matches!(error, SkillManagerError::FileSystem { .. }));
        assert!(
            error
                .to_string()
                .contains("too many levels of symbolic links"),
            "cycle failure must explain the bounded link traversal: {error}"
        );
    }

    /// Deterministic Windows cycle coverage. Build two junctions that target
    /// one another, then verify bounded traversal reports the same explicit
    /// link-depth failure as the Unix symlink cycle.
    #[cfg(windows)]
    #[test]
    fn absolute_path_rejects_a_windows_junction_cycle() {
        let sandbox = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let first = sandbox.path().join("first");
        let second = sandbox.path().join("second");
        std::fs::create_dir_all(&second).unwrap_or_else(|error| unreachable!("{error}"));
        create_directory_junction(&second, &first);
        std::fs::remove_dir(&second).unwrap_or_else(|error| unreachable!("{error}"));
        create_directory_junction(&first, &second);

        let error = match absolute_path(first.join("destination")) {
            Ok(path) => unreachable!("junction cycle resolved unexpectedly to {}", path.display()),
            Err(error) => error,
        };
        assert!(matches!(error, SkillManagerError::FileSystem { .. }));
        assert!(
            error
                .to_string()
                .contains("too many levels of symbolic links"),
            "junction cycle failure must explain the bounded link traversal: {error}"
        );

        std::fs::remove_dir(&first).unwrap_or_else(|cleanup| unreachable!("{cleanup}"));
        std::fs::remove_dir(&second).unwrap_or_else(|cleanup| unreachable!("{cleanup}"));
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
                    yes: false,
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
                yes: false,
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
                    yes: false,
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
    fn describe_preserves_remote_materialization_failure_as_a_diagnostic_before_erroring() {
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
            true,
            home.path().to_path_buf(),
        );
        let source = source_from_reference("owner/repository:main", None, home.path())
            .unwrap_or_else(|error| unreachable!("{error}"));
        let config = Config {
            sources: vec![source],
            ..Config::default()
        };

        let result = app.run_describe(
            &config,
            DescribeArgs {
                selection: DescribeSelection {
                    selectors: vec!["missing".into()],
                    ..DescribeSelection::default()
                },
                action: None,
            },
        );

        assert!(matches!(result, Err(SkillManagerError::NotFound { .. })));
        assert_eq!(app.reporter.events, ["diagnostic"]);
        let message = app.reporter.event_data[0]["message"]
            .as_str()
            .unwrap_or_else(|| unreachable!("diagnostic message"));
        assert!(message.contains("could not inspect source 'repository'"));
        assert!(message.contains("network must not be used"));
        assert!(
            app.reporter
                .diagnostics
                .iter()
                .any(|line| line.contains("network must not be used"))
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

        let entry = source_from_reference("owner/repository", None, home.path())
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

        let entry = source_from_reference("owner/repository", None, home.path())
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
                action: TargetAction::Add(TargetAddArgs {
                    first: home.path().join("reserved").to_string_lossy().into_owned(),
                    second: None,
                    name: Some("claude".into()),
                    yes: false,
                }),
            }))
            .is_err()
        );
        app.run(Command::Target(TargetArgs {
            action: TargetAction::Add(TargetAddArgs {
                first: PathBuf::from(".custom")
                    .join("skills")
                    .to_string_lossy()
                    .into_owned(),
                second: None,
                name: Some("custom-target".into()),
                yes: false,
            }),
        }))
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(
            app.run(Command::Target(TargetArgs {
                action: TargetAction::Add(TargetAddArgs {
                    first: PathBuf::from(".duplicate")
                        .join("skills")
                        .to_string_lossy()
                        .into_owned(),
                    second: None,
                    name: Some("CUSTOM-TARGET".into()),
                    yes: false,
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
                        yes: false,
                    }),
                }))
                .is_err()
            );
        }
    }
}
