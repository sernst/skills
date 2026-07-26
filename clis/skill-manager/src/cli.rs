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
    Load(SyncArgs),
    /// Refresh only skills already deployed.
    Update(SyncArgs),
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
    /// Source paths, GitHub references, source names, or source IDs.
    #[arg(value_name = "SOURCE")]
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
    /// Plan without changing persistent state.
    #[arg(long)]
    pub dry_run: bool,
    /// Force remote cache refresh.
    #[arg(long)]
    pub refresh: bool,
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
}

/// Arguments for `remove`.
#[derive(Clone, Debug, Default, Args)]
pub struct RemoveArgs {
    /// Skill names, skill directories, or collection directories.
    #[arg(value_name = "SKILL")]
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
    /// Force remote cache refresh.
    #[arg(long)]
    pub refresh: bool,
}

/// Arguments for `resolve`.
#[derive(Clone, Debug, Default, Args)]
pub struct ResolveArgs {
    /// Exact collided skill names; omitted means every collision.
    #[arg(value_name = "SKILL")]
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
