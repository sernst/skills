//! Command-line contract for the skill-manager executable.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Manage reusable agent skills across AI development tools.
#[derive(Clone, Debug, Parser)]
#[command(name = "skill-manager", version, about)]
pub struct Cli {
    /// Emit NDJSON; `--json=OBJECT` also supplies a recipe object.
    #[arg(
        long,
        global = true,
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = ""
    )]
    pub json: Option<String>,
    /// Read one recipe object from standard input and emit NDJSON.
    #[arg(long, global = true)]
    pub json_input: bool,
    /// Read one recipe object from a file and emit NDJSON.
    #[arg(long, global = true, value_name = "FILE")]
    pub input: Option<PathBuf>,
    /// Disable interactive prompts.
    #[arg(long, global = true)]
    pub no_input: bool,
    /// Color policy for human output.
    #[arg(long, global = true, value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,
    /// Show advanced details and full paths in human output.
    #[arg(long, global = true)]
    pub verbose: bool,
    /// Override the manager home; beats `SKILL_MANAGER_HOME` and the OS home.
    #[arg(
        long,
        global = true,
        value_name = "DIR",
        value_parser = parse_home_override
    )]
    pub home: Option<PathBuf>,
    /// Requested operation; omitted means `status`.
    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// Return whether machine output/input is active.
    #[must_use]
    pub fn machine_mode(&self) -> bool {
        self.json.is_some() || self.json_input || self.input.is_some()
    }
}

/// Reject a blank or whitespace-only `--home` value at parse time.
///
/// An empty `PathBuf` still wins the `--home` > `SKILL_MANAGER_HOME` > OS
/// home precedence order, which would silently root `.skill-manager` in the
/// current working directory instead of failing loudly — exactly the
/// isolation hazard `--home` exists to prevent. The pre-existing
/// `SKILL_MANAGER_HOME` environment variable already ignores an empty value
/// (see `manager_home` in `config.rs`); this parser keeps the flag at least
/// as strict by rejecting the value outright rather than falling through.
fn parse_home_override(raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err(
            "--home must not be blank; provide a directory path or omit the flag".to_owned(),
        );
    }
    Ok(PathBuf::from(raw))
}

/// Color selection for human output.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ColorChoice {
    /// Enable color only for an interactive terminal.
    #[default]
    Auto,
    /// Always emit color.
    Always,
    /// Never emit color.
    Never,
}

/// Top-level commands.
#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Deploy source skills, replacing existing deployments.
    #[command(visible_alias = "install")]
    Load(LoadArgs),
    /// Refresh only skills already deployed. Alias: up.
    #[command(visible_alias = "up")]
    Update(UpdateArgs),
    /// Adopt a deployed skill copy as the new source content.
    Import(ImportArgs),
    /// Copy source skills into an arbitrary destination.
    Copy(CopyArgs),
    /// Remove deployed skills.
    Remove(RemoveArgs),
    /// Display source/deployment status.
    #[command(alias = "ls", alias = "list")]
    Status(StatusArgs),
    /// Resolve source collisions by persisting exclusions.
    Resolve(ResolveArgs),
    /// Manage stored sources.
    Source(SourceArgs),
    /// Manage deployment targets.
    Target(TargetArgs),
    /// Display, reset, or restore the stored configuration.
    Configs(ConfigsArgs),
    /// Generate a shell completion script.
    #[command(hide = true)]
    GenerateCompletions(GenerateCompletionsArgs),
    /// Generate the manual page.
    #[command(hide = true)]
    GenerateMan(GenerateManArgs),
}

/// Source selection shared by discovery commands.
#[derive(Clone, Debug, Default, Args)]
pub struct SourceSelection {
    /// Add the current directory to configured sources.
    #[arg(long, conflicts_with = "cd_only")]
    pub cd: bool,
    /// Use only the current directory.
    #[arg(long, conflicts_with_all = ["cd", "no_cd"])]
    pub cd_only: bool,
    /// Compatibility spelling for configured-sources-only behavior.
    #[arg(long, conflicts_with_all = ["cd", "cd_only"])]
    pub no_cd: bool,
}

/// Target selection shared by deployment commands.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, Args)]
pub struct TargetSelection {
    /// Select the Claude Code target.
    #[arg(long)]
    pub claude: bool,
    /// Select the shared agents target.
    #[arg(long)]
    pub shared: bool,
    /// Select the Google Antigravity target.
    #[arg(long, alias = "ag")]
    pub antigravity: bool,
    /// Select all configured targets.
    #[arg(long = "all")]
    pub all_targets: bool,
    /// Select a configured target by name, including disabled targets.
    #[arg(long = "target", value_name = "NAME")]
    pub target_names: Vec<String>,
}

/// Installation scope shared by deployment and status commands.
///
/// The application resolves the selected scope against either its configured
/// home directory or the process working directory. Keeping this selection at
/// the CLI boundary makes the eventual scope policy independent of clap.
#[derive(Clone, Debug, Default, Args)]
pub struct ScopeSelection {
    /// Use the global installation location.
    #[arg(long, short = 'g', conflicts_with = "project")]
    pub global: bool,
    /// Use the current project's installation location.
    #[arg(long, short = 'p', conflicts_with = "global")]
    pub project: bool,
}

impl ScopeSelection {
    /// Return whether a scope was explicitly selected.
    #[must_use]
    pub const fn is_explicit(&self) -> bool {
        self.global || self.project
    }
}

impl TargetSelection {
    /// Whether the user explicitly chose target behavior.
    #[must_use]
    pub fn is_explicit(&self) -> bool {
        self.claude
            || self.shared
            || self.antigravity
            || self.all_targets
            || !self.target_names.is_empty()
    }
}

/// Arguments shared by `load` and `update`.
#[derive(Clone, Debug, Default, Args)]
pub struct SyncArgs {
    /// Source paths/references/names/IDs, skill names, or skill-name patterns.
    ///
    /// A literal operand resolves in order: a configured source (ID, name,
    /// active location, or unique label); a path-shaped or GitHub-ref-shaped
    /// value (absolute, `~`, `./`/`../`, or containing a path separator); a
    /// discovered skill name (case-insensitive); an existing directory below
    /// the current working directory. A discovered skill name wins over a
    /// same-named directory, with a warning suggesting `./name` to force the
    /// directory. An operand matching none of these is a hard error.
    #[arg(value_name = "SOURCE_OR_SKILL_OR_PATTERN")]
    pub sources: Vec<String>,
    /// Include pattern; repeatable and combined with logical OR.
    #[arg(long = "filter", value_name = "PATTERN")]
    pub filters: Vec<String>,
    /// Source selection mode.
    #[command(flatten)]
    pub source_selection: SourceSelection,
    /// Target selection.
    #[command(flatten)]
    pub targets: TargetSelection,
    /// Installation scope selection.
    #[command(flatten)]
    pub scope: ScopeSelection,
    /// Plan without changing persistent state.
    #[arg(long)]
    pub dry_run: bool,
    /// Force remote cache refresh.
    #[arg(long)]
    pub refresh: bool,
}

/// Arguments for `load`.
///
/// `load` shows a change plan before deploying anything, so it accepts one
/// confirmation flag mirroring `update`'s.
#[derive(Clone, Debug, Default, Args)]
pub struct LoadArgs {
    /// Discovery, target, and scope selection shared with `update`.
    #[command(flatten)]
    pub sync: SyncArgs,
    /// Skip the load confirmation; the plan is still displayed.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments for `update`.
///
/// `update` shows a change plan before deploying anything, so it accepts one
/// confirmation flag mirroring `load`'s.
#[derive(Clone, Debug, Default, Args)]
pub struct UpdateArgs {
    /// Discovery, target, and scope selection shared with `load`.
    #[command(flatten)]
    pub sync: SyncArgs,
    /// Skip the update confirmation; the plan is still displayed.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments for `import`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, Args)]
pub struct ImportArgs {
    /// Exactly one deployed skill name; patterns are not accepted.
    #[arg(value_name = "SKILL")]
    pub skill: String,
    /// Target selection narrowing the scanned deployments.
    #[command(flatten)]
    pub targets: TargetSelection,
    /// Installation scope narrowing the scanned deployments.
    #[command(flatten)]
    pub scope: ScopeSelection,
    /// Resolve propagation to import + update (recommended).
    ///
    /// Mutually exclusive with `--no-update`: this answers the propagation
    /// dimension itself, never implied by `--yes`.
    #[arg(long, conflicts_with = "no_update")]
    pub update: bool,
    /// Resolve propagation to import only, leaving other deployments as-is.
    #[arg(long)]
    pub no_update: bool,
    /// Plan without changing the source.
    #[arg(long)]
    pub dry_run: bool,
    /// Skip the destructive source-overwrite confirmation.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments for `copy`.
#[derive(Clone, Debug, Args)]
pub struct CopyArgs {
    /// Source path or reference.
    pub source: String,
    /// Destination directory.
    pub destination: PathBuf,
    /// Include pattern; repeatable and combined with logical OR.
    #[arg(long = "filter", value_name = "PATTERN")]
    pub filters: Vec<String>,
    /// Plan without changing persistent state.
    #[arg(long)]
    pub dry_run: bool,
    /// Force remote cache refresh.
    #[arg(long)]
    pub refresh: bool,
    /// Skip the copy confirmation; the plan is still displayed.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments for `remove`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, Args)]
pub struct RemoveArgs {
    /// Skill names/patterns, skill directories, or collection directories.
    #[arg(value_name = "SKILL_OR_PATTERN")]
    pub skills: Vec<String>,
    /// Include pattern; repeatable and combined with logical OR.
    #[arg(long = "filter", value_name = "PATTERN")]
    pub filters: Vec<String>,
    /// Source selection used when no skill arguments are supplied.
    #[command(flatten)]
    pub source_selection: SourceSelection,
    /// Target selection.
    #[command(flatten)]
    pub targets: TargetSelection,
    /// Installation scope selection.
    #[command(flatten)]
    pub scope: ScopeSelection,
    /// Remove both scopes wherever a skill exists in both.
    ///
    /// Mutually exclusive with `--global`/`--project`: this answers the
    /// removal-scope branch itself (the noninteractive spelling of
    /// interactive option 3), rather than restricting which scope is
    /// inspected in the first place.
    #[arg(long, conflicts_with_all = ["global", "project"])]
    pub both: bool,
    /// Plan without changing persistent state.
    #[arg(long)]
    pub dry_run: bool,
    /// Force remote cache refresh during discovery.
    #[arg(long)]
    pub refresh: bool,
    /// Skip removal confirmation.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments for `status`.
#[derive(Clone, Debug, Default, Args)]
pub struct StatusArgs {
    /// Case-insensitive patterns matching skill names, source names, or labels.
    #[arg(value_name = "FILTER")]
    pub filters: Vec<String>,
    /// Additional case-insensitive filter.
    #[arg(long = "filter", value_name = "PATTERN")]
    pub option_filters: Vec<String>,
    /// Source selection.
    #[command(flatten)]
    pub source_selection: SourceSelection,
    /// Target selection.
    #[command(flatten)]
    pub targets: TargetSelection,
    /// Installation scope selection.
    #[command(flatten)]
    pub scope: ScopeSelection,
    /// Force remote cache refresh.
    #[arg(long)]
    pub refresh: bool,
}

/// Arguments for `resolve`.
#[derive(Clone, Debug, Default, Args)]
pub struct ResolveArgs {
    /// Collided skill names or patterns; omitted means every collision.
    #[arg(value_name = "SKILL_OR_PATTERN")]
    pub skills: Vec<String>,
    /// Source selection.
    #[command(flatten)]
    pub source_selection: SourceSelection,
    /// Deterministically choose this source name, ID, or reference.
    #[arg(long, value_name = "NAME_OR_ID")]
    pub prefer_source: Option<String>,
    /// Force remote cache refresh.
    #[arg(long)]
    pub refresh: bool,
}

/// Source-management command wrapper.
#[derive(Clone, Debug, Args)]
pub struct SourceArgs {
    /// Source lifecycle action.
    #[command(subcommand)]
    pub action: SourceAction,
}

/// Source lifecycle actions.
#[derive(Clone, Debug, Subcommand)]
pub enum SourceAction {
    /// Add a local or GitHub source.
    Add(SourceAddArgs),
    /// Remove a stored source.
    Remove(SourceRemoveArgs),
    /// List stored sources.
    List,
    /// Update source metadata.
    Update(SourceUpdateArgs),
    /// Change the active source location.
    #[command(visible_aliases = ["relocate", "move", "mv"])]
    Locate(SourceLocateArgs),
    /// Set, replace, or clear the inactive source location.
    Alternate(SourceAlternateArgs),
    /// Exchange the active and inactive source locations.
    Swap(SourceSwapArgs),
}

/// Arguments for `source add`.
#[derive(Clone, Debug, Args)]
pub struct SourceAddArgs {
    /// Local path, GitHub tree URL, or `owner/repo[:ref][/path]`.
    #[arg(value_name = "SOURCE")]
    pub source: Option<String>,
    /// Stable unique source name.
    #[arg(value_name = "NAME")]
    pub source_name: Option<String>,
    /// Stable unique source name.
    #[arg(long = "name", conflicts_with = "source_name")]
    pub name: Option<String>,
    /// Human-readable label.
    #[arg(long)]
    pub label: Option<String>,
    /// Source exclusion pattern.
    #[arg(long = "exclude", value_name = "PATTERN")]
    pub exclude: Vec<String>,
    /// Layout override.
    #[arg(long, value_enum)]
    pub mode: Option<SourceModeArg>,
    /// GitHub cache lifetime.
    #[arg(long, value_name = "HOURS")]
    pub cache_ttl_hours: Option<i64>,
}

/// Command-line spelling of source layout.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SourceModeArg {
    /// Immediate children are skills.
    Collection,
    /// The root is one skill.
    Single,
}

/// Arguments for `source remove`.
#[derive(Clone, Debug, Args)]
pub struct SourceRemoveArgs {
    /// Source path, name, ID, or GitHub reference.
    pub source: Option<String>,
}

/// Arguments for `source update`.
#[derive(Clone, Debug, Args)]
pub struct SourceUpdateArgs {
    /// Source name, ID, path, or GitHub reference.
    pub source: String,
    /// Replacement unique name.
    #[arg(long)]
    pub name: Option<String>,
    /// Replacement active local path or GitHub reference.
    #[arg(long, value_name = "LOCATION")]
    pub location: Option<String>,
    /// Replacement human label.
    #[arg(long)]
    pub label: Option<String>,
    /// Exclusion pattern to add.
    #[arg(long = "exclude", value_name = "PATTERN")]
    pub exclude: Vec<String>,
    /// Clear exclusions before applying additions.
    #[arg(long)]
    pub clear_exclude: bool,
    /// Replacement cache lifetime.
    #[arg(long, value_name = "HOURS")]
    pub cache_ttl_hours: Option<i64>,
}

/// Arguments for `source locate`.
#[derive(Clone, Debug, Args)]
pub struct SourceLocateArgs {
    /// Source name, ID, label, path, or GitHub reference.
    pub source: String,
    /// Replacement active local path or GitHub reference.
    pub location: String,
}

/// Arguments for `source alternate`.
#[derive(Clone, Debug, Args)]
pub struct SourceAlternateArgs {
    /// Source name, ID, label, path, or GitHub reference.
    pub source: String,
    /// Replacement inactive local path or GitHub reference.
    #[arg(conflicts_with = "clear")]
    pub location: Option<String>,
    /// Remove the inactive location.
    #[arg(long, conflicts_with = "location")]
    pub clear: bool,
}

/// Arguments for `source swap`.
#[derive(Clone, Debug, Args)]
pub struct SourceSwapArgs {
    /// Source name, ID, label, path, or GitHub reference.
    pub source: String,
}

/// Target-management command wrapper.
#[derive(Clone, Debug, Args)]
pub struct TargetArgs {
    /// Target lifecycle action.
    #[command(subcommand)]
    pub action: TargetAction,
}

/// Target lifecycle actions.
#[derive(Clone, Debug, Subcommand)]
pub enum TargetAction {
    /// Add a custom target.
    Add(TargetPathArgs),
    /// List built-in and custom targets.
    List,
    /// Enable a target.
    Enable(TargetNameArgs),
    /// Disable a target.
    Disable(TargetNameArgs),
    /// Remove a custom target or legacy built-in override.
    Remove(TargetNameArgs),
    /// Change a custom or legacy override path.
    SetPath(TargetPathArgs),
}

/// Target name plus path.
#[derive(Clone, Debug, Args)]
pub struct TargetPathArgs {
    /// Stable target name.
    pub name: String,
    /// Target directory.
    pub path: PathBuf,
}

/// Target name.
#[derive(Clone, Debug, Args)]
pub struct TargetNameArgs {
    /// Stable target name.
    pub name: String,
}

/// Configuration-management command wrapper.
#[derive(Clone, Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct ConfigsArgs {
    /// Write the active configuration bytes exactly as stored.
    #[arg(long, conflicts_with_all = ["json", "json_input", "input"])]
    pub raw: bool,
    /// Configuration lifecycle action; omitted displays the configuration.
    #[command(subcommand)]
    pub action: Option<ConfigsAction>,
}

/// Configuration lifecycle actions.
#[derive(Clone, Debug, Subcommand)]
pub enum ConfigsAction {
    /// Replace the active configuration with an empty configuration.
    Reset(ConfigsConfirmArgs),
    /// Restore a previously archived configuration.
    Restore(ConfigsRestoreArgs),
}

/// Destructive configuration action confirmation.
#[derive(Clone, Debug, Default, Args)]
pub struct ConfigsConfirmArgs {
    /// Confirm a destructive non-interactive operation.
    #[arg(long)]
    pub yes: bool,
}

/// Configuration restore arguments.
#[derive(Clone, Debug, Default, Args)]
pub struct ConfigsRestoreArgs {
    /// Backup identifier; omitted selects the latest backup.
    #[arg(value_name = "BACKUP_ID")]
    pub backup_id: Option<String>,
    /// Confirm a destructive non-interactive operation.
    #[arg(long)]
    pub yes: bool,
}

/// Shell completion generation arguments.
#[derive(Clone, Debug, Args)]
pub struct GenerateCompletionsArgs {
    /// Shell syntax to generate.
    #[arg(long, value_enum)]
    pub shell: CompletionShell,
}

/// Supported completion shells.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CompletionShell {
    /// Bash.
    Bash,
    /// Zsh.
    Zsh,
    /// Fish.
    Fish,
    /// PowerShell.
    Powershell,
}

/// Manual-page generation arguments.
#[derive(Clone, Debug, Args)]
pub struct GenerateManArgs {
    /// Destination file.
    #[arg(long)]
    pub output: PathBuf,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command, ConfigsAction};

    #[test]
    fn scoped_commands_accept_each_short_and_long_scope_flag() {
        for (command, flag, global) in [
            ("load", "-g", true),
            ("update", "--project", false),
            ("remove", "-p", false),
            ("status", "--global", true),
        ] {
            let cli = Cli::try_parse_from(["skill-manager", command, flag])
                .unwrap_or_else(|error| unreachable!("{error}"));
            let selection = match cli.command {
                Some(Command::Load(args)) => args.sync.scope,
                Some(Command::Update(args)) => args.sync.scope,
                Some(Command::Remove(args)) => args.scope,
                Some(Command::Status(args)) => args.scope,
                _ => unreachable!("expected scoped command"),
            };
            assert!(selection.is_explicit());
            assert_eq!(selection.global, global);
            assert_eq!(selection.project, !global);
        }
    }

    #[test]
    fn scoped_commands_reject_conflicting_scope_flags() {
        for command in ["load", "update", "remove", "status"] {
            assert!(
                Cli::try_parse_from(["skill-manager", command, "--global", "--project"]).is_err()
            );
        }
    }

    #[test]
    fn remove_both_flag_parses_and_conflicts_with_either_scope_flag() {
        let cli = Cli::try_parse_from(["skill-manager", "remove", "teach", "--both"])
            .unwrap_or_else(|error| unreachable!("{error}"));
        let Some(Command::Remove(args)) = cli.command else {
            unreachable!("expected remove command");
        };
        assert!(args.both);
        assert!(!args.scope.is_explicit());

        for flag in ["--global", "--project"] {
            assert!(
                Cli::try_parse_from(["skill-manager", "remove", "teach", "--both", flag]).is_err(),
                "--both must conflict with {flag}"
            );
        }
    }

    #[test]
    fn configs_parses_show_reset_restore_and_alias() {
        let show = Cli::try_parse_from(["skill-manager", "configs", "--raw"])
            .unwrap_or_else(|error| unreachable!("{error}"));
        let Some(Command::Configs(show)) = show.command else {
            unreachable!("configs command");
        };
        assert!(show.raw && show.action.is_none());

        let reset = Cli::try_parse_from(["skill-manager", "configs", "reset", "--yes"])
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(matches!(
            reset.command,
            Some(Command::Configs(super::ConfigsArgs {
                action: Some(ConfigsAction::Reset(super::ConfigsConfirmArgs {
                    yes: true
                })),
                ..
            }))
        ));

        let restore =
            Cli::try_parse_from(["skill-manager", "configs", "restore", "backup-01", "--yes"])
                .unwrap_or_else(|error| unreachable!("{error}"));
        let Some(Command::Configs(restore)) = restore.command else {
            unreachable!("configs command");
        };
        let Some(ConfigsAction::Restore(args)) = restore.action else {
            unreachable!("restore action");
        };
        assert_eq!(args.backup_id.as_deref(), Some("backup-01"));
        assert!(args.yes);
    }

    #[test]
    fn raw_configs_conflicts_with_recipe_carriers() {
        for carrier in ["--json={}", "--json-input", "--input=recipe.json"] {
            let parsed = Cli::try_parse_from(["skill-manager", carrier, "configs", "--raw"])
                .unwrap_or_else(|error| unreachable!("{error}"));
            assert!(parsed.machine_mode());
            assert!(matches!(
                parsed.command,
                Some(Command::Configs(super::ConfigsArgs { raw: true, .. }))
            ));
        }
        assert!(Cli::try_parse_from(["skill-manager", "configs", "--raw", "reset"]).is_err());
    }

    #[test]
    fn blank_home_override_is_rejected_at_parse_time() {
        for blank in ["", "   ", "\t"] {
            let error = Cli::try_parse_from(["skill-manager", "--home", blank, "status"])
                .err()
                .unwrap_or_else(|| unreachable!("blank --home {blank:?} must fail to parse"));
            assert!(
                error.to_string().contains("--home must not be blank"),
                "unexpected error for {blank:?}: {error}"
            );
        }

        let parsed = Cli::try_parse_from(["skill-manager", "--home", "some/dir", "status"])
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(
            parsed.home.as_deref(),
            Some(std::path::Path::new("some/dir"))
        );
    }
}
