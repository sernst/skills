//! Strict JSON invocation input and CLI-over-recipe overlay behavior.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};

use crate::cli::{
    Cli, Command, ConfigsAction, ConfigsArgs, ConfigsConfirmArgs, ConfigsRestoreArgs, CopyArgs,
    ImportArgs, LoadArgs, RemoveArgs, ResolveArgs, ScopeSelection, SourceAction, SourceAddArgs,
    SourceAlternateArgs, SourceArgs, SourceLocateArgs, SourceModeArg, SourceRemoveArgs,
    SourceSelection, SourceSwapArgs, SourceUpdateArgs, StatusArgs, SyncArgs, TargetAction,
    TargetArgs, TargetNameArgs, TargetPathArgs, TargetSelection, UpdateArgs,
};
use crate::error::{Result, SkillManagerError};
use crate::skills::is_fnmatch_operand;

/// Apply one optional JSON recipe to parsed CLI arguments.
///
/// Fields explicitly represented by non-default CLI values win over recipe
/// fields. JSON types are strict and unknown keys are rejected.
///
/// # Errors
///
/// Returns an error for malformed input, conflicting carriers or commands,
/// unknown fields, wrong JSON types, and missing required values.
pub fn apply_recipe(cli: &mut Cli) -> Result<()> {
    let carrier_count = usize::from(cli.json.as_ref().is_some_and(|value| !value.is_empty()))
        + usize::from(cli.json_input)
        + usize::from(cli.input.is_some());
    if carrier_count > 1 {
        return Err(SkillManagerError::InvalidInput(
            "--json=OBJECT, --json-input, and --input are mutually exclusive".into(),
        ));
    }
    let (payload, base) = if let Some(raw) = cli.json.as_ref().filter(|value| !value.is_empty()) {
        (Some(raw.clone()), current_directory()?)
    } else if cli.json_input {
        let mut raw = String::new();
        io::stdin()
            .read_to_string(&mut raw)
            .map_err(|error| SkillManagerError::io("<stdin>", error))?;
        (Some(raw), current_directory()?)
    } else if let Some(path) = &cli.input {
        let absolute = resolve_path(path, &current_directory()?);
        let raw = fs::read_to_string(&absolute)
            .map_err(|error| SkillManagerError::io(&absolute, error))?;
        let base = absolute
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        (Some(raw), base)
    } else {
        (None, current_directory()?)
    };
    let Some(payload) = payload else {
        return Ok(());
    };
    let value: Value = serde_json::from_str(&payload)
        .map_err(|error| SkillManagerError::InvalidInput(format!("invalid JSON input: {error}")))?;
    let object = value.as_object().ok_or_else(|| {
        SkillManagerError::InvalidInput("JSON invocation must be one object".into())
    })?;
    if object.values().any(Value::is_null) {
        return Err(SkillManagerError::InvalidInput(
            "JSON invocation does not accept null values".into(),
        ));
    }
    let recipe_command = object.get("command").map(strict_string).transpose()?;
    let cli_command = cli.command.as_ref().map(command_name);
    if let (Some(recipe), Some(parsed)) = (recipe_command, cli_command)
        && canonical_command(recipe)? != parsed
    {
        return Err(SkillManagerError::InvalidInput(format!(
            "recipe command {recipe:?} does not match argv command {parsed:?}"
        )));
    }
    if cli.command.is_none()
        && let Some(command) = recipe_command
    {
        cli.command = Some(default_command(canonical_command(command)?)?);
    }
    let command = cli
        .command
        .get_or_insert_with(|| Command::Status(StatusArgs::default()));
    overlay_command(command, object, &base)?;
    validate_required(command)?;
    if let Some(no_input) = object.get("no_input") {
        cli.no_input |= strict_bool(no_input)?;
    }
    Ok(())
}

fn overlay_command(command: &mut Command, object: &Map<String, Value>, base: &Path) -> Result<()> {
    match command {
        Command::Load(args) => overlay_load(args, object, base),
        Command::Update(args) => overlay_update(args, object, base),
        Command::Import(args) => overlay_import(args, object),
        Command::Copy(args) => overlay_copy(args, object, base),
        Command::Remove(args) => overlay_remove(args, object, base),
        Command::Status(args) => overlay_status(args, object),
        Command::Resolve(args) => overlay_resolve(args, object),
        Command::Source(args) => overlay_source(&mut args.action, object, base),
        Command::Target(args) => overlay_target(&mut args.action, object, base),
        Command::Configs(args) => overlay_configs(args, object),
        Command::GenerateCompletions(_) | Command::GenerateMan(_) => Err(
            SkillManagerError::InvalidInput("generation commands do not accept recipes".into()),
        ),
    }?;
    Ok(())
}

fn overlay_load(args: &mut LoadArgs, object: &Map<String, Value>, _base: &Path) -> Result<()> {
    reject_unknown(
        object,
        &[
            "command",
            "no_input",
            "source",
            "sources",
            "filter",
            "filters",
            "claude",
            "shared",
            "antigravity",
            "all",
            "all_targets",
            "target",
            "targets",
            "cd",
            "cd_only",
            "no_cd",
            "dry_run",
            "refresh",
            "global",
            "project",
            "yes",
        ],
    )?;
    overlay_sync_fields(&mut args.sync, object)?;
    overlay_bool(&mut args.yes, object.get("yes"))
}

fn overlay_update(args: &mut UpdateArgs, object: &Map<String, Value>, _base: &Path) -> Result<()> {
    reject_unknown(
        object,
        &[
            "command",
            "no_input",
            "source",
            "sources",
            "filter",
            "filters",
            "claude",
            "shared",
            "antigravity",
            "all",
            "all_targets",
            "target",
            "targets",
            "cd",
            "cd_only",
            "no_cd",
            "dry_run",
            "refresh",
            "global",
            "project",
            "yes",
        ],
    )?;
    overlay_sync_fields(&mut args.sync, object)?;
    overlay_bool(&mut args.yes, object.get("yes"))
}

fn overlay_import(args: &mut ImportArgs, object: &Map<String, Value>) -> Result<()> {
    reject_unknown(
        object,
        &[
            "command",
            "no_input",
            "skill",
            "claude",
            "shared",
            "antigravity",
            "all",
            "all_targets",
            "target",
            "targets",
            "dry_run",
            "global",
            "project",
            "yes",
        ],
    )?;
    if args.skill.is_empty() {
        args.skill = first_string(object, &["skill"])?.unwrap_or_default();
    }
    overlay_target_selection(&mut args.targets, object)?;
    overlay_scope_selection(&mut args.scope, object)?;
    overlay_bool(&mut args.dry_run, object.get("dry_run"))?;
    overlay_bool(&mut args.yes, object.get("yes"))
}

fn overlay_sync_fields(args: &mut SyncArgs, object: &Map<String, Value>) -> Result<()> {
    overlay_strings(&mut args.sources, object, &["sources", "source"])?;
    overlay_strings(&mut args.filters, object, &["filters", "filter"])?;
    overlay_source_selection(&mut args.source_selection, object)?;
    overlay_target_selection(&mut args.targets, object)?;
    overlay_scope_selection(&mut args.scope, object)?;
    overlay_bool(&mut args.dry_run, object.get("dry_run"))?;
    overlay_bool(&mut args.refresh, object.get("refresh"))
}

fn overlay_copy(args: &mut CopyArgs, object: &Map<String, Value>, base: &Path) -> Result<()> {
    reject_unknown(
        object,
        &[
            "command",
            "no_input",
            "source",
            "destination",
            "filter",
            "filters",
            "dry_run",
            "refresh",
            "yes",
        ],
    )?;
    overlay_strings(&mut args.filters, object, &["filters", "filter"])?;
    overlay_bool(&mut args.dry_run, object.get("dry_run"))?;
    overlay_bool(&mut args.refresh, object.get("refresh"))?;
    overlay_bool(&mut args.yes, object.get("yes"))?;
    if args.source.is_empty() {
        args.source = first_string(object, &["source"])?.unwrap_or_default();
    }
    if let Some(destination) = object.get("destination")
        && args.destination.as_os_str().is_empty()
    {
        args.destination = resolve_path(Path::new(strict_string(destination)?), base);
    }
    Ok(())
}

fn overlay_remove(args: &mut RemoveArgs, object: &Map<String, Value>, base: &Path) -> Result<()> {
    reject_unknown(
        object,
        &[
            "command",
            "no_input",
            "skill",
            "skills",
            "filter",
            "filters",
            "claude",
            "shared",
            "antigravity",
            "all",
            "all_targets",
            "target",
            "targets",
            "cd",
            "cd_only",
            "no_cd",
            "dry_run",
            "refresh",
            "yes",
            "global",
            "project",
        ],
    )?;
    overlay_references(&mut args.skills, object, &["skills", "skill"], base)?;
    overlay_strings(&mut args.filters, object, &["filters", "filter"])?;
    overlay_source_selection(&mut args.source_selection, object)?;
    overlay_target_selection(&mut args.targets, object)?;
    overlay_scope_selection(&mut args.scope, object)?;
    overlay_bool(&mut args.dry_run, object.get("dry_run"))?;
    overlay_bool(&mut args.refresh, object.get("refresh"))?;
    overlay_bool(&mut args.yes, object.get("yes"))
}

fn overlay_status(args: &mut StatusArgs, object: &Map<String, Value>) -> Result<()> {
    reject_unknown(
        object,
        &[
            "command",
            "no_input",
            "filter",
            "filters",
            "claude",
            "shared",
            "antigravity",
            "all",
            "all_targets",
            "target",
            "targets",
            "cd",
            "cd_only",
            "no_cd",
            "refresh",
            "global",
            "project",
        ],
    )?;
    overlay_strings(&mut args.filters, object, &["filters", "filter"])?;
    overlay_source_selection(&mut args.source_selection, object)?;
    overlay_target_selection(&mut args.targets, object)?;
    overlay_scope_selection(&mut args.scope, object)?;
    overlay_bool(&mut args.refresh, object.get("refresh"))
}

fn overlay_resolve(args: &mut ResolveArgs, object: &Map<String, Value>) -> Result<()> {
    reject_unknown(
        object,
        &[
            "command",
            "no_input",
            "skill",
            "skills",
            "prefer_source",
            "cd",
            "cd_only",
            "no_cd",
            "refresh",
        ],
    )?;
    overlay_strings(&mut args.skills, object, &["skills", "skill"])?;
    overlay_source_selection(&mut args.source_selection, object)?;
    overlay_bool(&mut args.refresh, object.get("refresh"))?;
    if args.prefer_source.is_none() {
        args.prefer_source = object
            .get("prefer_source")
            .map(strict_string)
            .transpose()?
            .map(ToOwned::to_owned);
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "Strict field validation and precedence for the source lifecycle stay auditable together."
)]
fn overlay_source(
    action: &mut SourceAction,
    object: &Map<String, Value>,
    base: &Path,
) -> Result<()> {
    match action {
        SourceAction::Add(args) => {
            reject_unknown(
                object,
                &[
                    "command",
                    "no_input",
                    "source",
                    "directory",
                    "source_name",
                    "name",
                    "label",
                    "exclude",
                    "mode",
                    "cache_ttl_hours",
                ],
            )?;
            if args.source.is_none() {
                args.source = first_string(object, &["source", "directory"])?
                    .map(|value| rebase_reference(&value, base, true));
            }
            if args.source_name.is_none() && args.name.is_none() {
                args.source_name = first_string(object, &["source_name", "name"])?;
            }
            if args.label.is_none() {
                args.label = first_string(object, &["label"])?;
            }
            overlay_strings(&mut args.exclude, object, &["exclude"])?;
            if args.mode.is_none() {
                args.mode = object
                    .get("mode")
                    .map(strict_string)
                    .transpose()?
                    .map(|value| match value {
                        "collection" => Ok(SourceModeArg::Collection),
                        "single" => Ok(SourceModeArg::Single),
                        _ => Err(SkillManagerError::InvalidInput(format!(
                            "unsupported source mode: {value}"
                        ))),
                    })
                    .transpose()?;
            }
            if args.cache_ttl_hours.is_none() {
                args.cache_ttl_hours = object.get("cache_ttl_hours").map(strict_i64).transpose()?;
            }
        }
        SourceAction::Remove(args) => {
            reject_unknown(object, &["command", "no_input", "source", "directory"])?;
            if args.source.is_none() {
                args.source = first_string(object, &["source", "directory"])?;
            }
        }
        SourceAction::List => reject_unknown(object, &["command", "no_input"])?,
        SourceAction::Update(args) => {
            reject_unknown(
                object,
                &[
                    "command",
                    "no_input",
                    "source",
                    "directory",
                    "location",
                    "name",
                    "label",
                    "exclude",
                    "clear_exclude",
                    "cache_ttl_hours",
                ],
            )?;
            if args.source.is_empty() {
                args.source = first_string(object, &["source", "directory"])?.unwrap_or_default();
            }
            if args.name.is_none() {
                args.name = first_string(object, &["name"])?;
            }
            if args.location.is_none() {
                args.location = first_string(object, &["location"])?
                    .map(|value| rebase_reference(&value, base, true));
            }
            if args.label.is_none() {
                args.label = first_string(object, &["label"])?;
            }
            overlay_strings(&mut args.exclude, object, &["exclude"])?;
            overlay_bool(&mut args.clear_exclude, object.get("clear_exclude"))?;
            if args.cache_ttl_hours.is_none() {
                args.cache_ttl_hours = object.get("cache_ttl_hours").map(strict_i64).transpose()?;
            }
        }
        SourceAction::Locate(args) => {
            reject_unknown(object, &["command", "no_input", "source", "location"])?;
            if args.source.is_empty() {
                args.source = first_string(object, &["source"])?.unwrap_or_default();
            }
            if args.location.is_empty() {
                args.location = first_string(object, &["location"])?
                    .map(|value| rebase_reference(&value, base, true))
                    .unwrap_or_default();
            }
        }
        SourceAction::Alternate(args) => {
            reject_unknown(
                object,
                &["command", "no_input", "source", "location", "clear"],
            )?;
            if args.source.is_empty() {
                args.source = first_string(object, &["source"])?.unwrap_or_default();
            }
            if args.location.is_some() {
                let _same_field = first_string(object, &["location"])?;
                args.clear = false;
            } else if args.clear {
                let _same_field = object.get("clear").map(strict_bool).transpose()?;
                args.location = None;
            } else {
                let recipe_location = first_string(object, &["location"])?;
                let recipe_clear = object.get("clear").map(strict_bool).transpose()?;
                match (recipe_location, recipe_clear) {
                    (Some(location), None | Some(false)) => {
                        args.location = Some(rebase_reference(&location, base, true));
                    }
                    (None, Some(true)) => args.clear = true,
                    _ => {
                        return Err(SkillManagerError::InvalidInput(
                            "source.alternate requires exactly one of location or clear:true"
                                .into(),
                        ));
                    }
                }
            }
        }
        SourceAction::Swap(args) => {
            reject_unknown(object, &["command", "no_input", "source"])?;
            if args.source.is_empty() {
                args.source = first_string(object, &["source"])?.unwrap_or_default();
            }
        }
    }
    Ok(())
}

fn overlay_target(
    action: &mut TargetAction,
    object: &Map<String, Value>,
    _base: &Path,
) -> Result<()> {
    match action {
        TargetAction::List => reject_unknown(object, &["command", "no_input"]),
        TargetAction::Add(args) | TargetAction::SetPath(args) => {
            reject_unknown(object, &["command", "no_input", "name", "path"])?;
            if args.name.is_empty() {
                args.name = first_string(object, &["name"])?.unwrap_or_default();
            }
            if args.path.as_os_str().is_empty()
                && let Some(path) = object.get("path")
            {
                // Target paths are scope-relative templates, never paths relative
                // to the recipe carrier file.
                args.path = PathBuf::from(strict_string(path)?);
            }
            Ok(())
        }
        TargetAction::Enable(args) | TargetAction::Disable(args) | TargetAction::Remove(args) => {
            reject_unknown(object, &["command", "no_input", "name"])?;
            if args.name.is_empty() {
                args.name = first_string(object, &["name"])?.unwrap_or_default();
            }
            Ok(())
        }
    }
}

fn overlay_configs(args: &mut ConfigsArgs, object: &Map<String, Value>) -> Result<()> {
    match args.action.as_mut() {
        None => reject_unknown(object, &["command", "no_input"]),
        Some(ConfigsAction::Reset(confirm)) => {
            reject_unknown(object, &["command", "no_input", "yes"])?;
            overlay_bool(&mut confirm.yes, object.get("yes"))
        }
        Some(ConfigsAction::Restore(restore)) => {
            reject_unknown(object, &["command", "no_input", "backup", "yes"])?;
            if restore.backup_id.is_none() {
                restore.backup_id = object
                    .get("backup")
                    .map(strict_string)
                    .transpose()?
                    .map(ToOwned::to_owned);
            }
            overlay_bool(&mut restore.yes, object.get("yes"))
        }
    }
}

fn overlay_source_selection(
    selection: &mut SourceSelection,
    object: &Map<String, Value>,
) -> Result<()> {
    overlay_bool(&mut selection.cd, object.get("cd"))?;
    overlay_bool(&mut selection.cd_only, object.get("cd_only"))?;
    overlay_bool(&mut selection.no_cd, object.get("no_cd"))?;
    let count =
        usize::from(selection.cd) + usize::from(selection.cd_only) + usize::from(selection.no_cd);
    if count > 1 {
        return Err(SkillManagerError::InvalidInput(
            "cd, cd_only, and no_cd are mutually exclusive".into(),
        ));
    }
    Ok(())
}

fn overlay_target_selection(
    selection: &mut TargetSelection,
    object: &Map<String, Value>,
) -> Result<()> {
    overlay_bool(&mut selection.claude, object.get("claude"))?;
    overlay_bool(&mut selection.shared, object.get("shared"))?;
    overlay_bool(&mut selection.antigravity, object.get("antigravity"))?;
    overlay_bool(
        &mut selection.all_targets,
        object.get("all_targets").or_else(|| object.get("all")),
    )?;
    overlay_strings(&mut selection.target_names, object, &["targets", "target"])
}

fn overlay_scope_selection(
    selection: &mut ScopeSelection,
    object: &Map<String, Value>,
) -> Result<()> {
    // Scope is one logical choice rather than two independently overlaid
    // booleans. Validate every supplied recipe value even when argv wins, then
    // use the recipe pair only when argv did not explicitly select a scope.
    let recipe_global = object.get("global").map(strict_bool).transpose()?;
    let recipe_project = object.get("project").map(strict_bool).transpose()?;
    if selection.is_explicit() {
        return Ok(());
    }
    selection.global = recipe_global.unwrap_or(false);
    selection.project = recipe_project.unwrap_or(false);
    if selection.global && selection.project {
        return Err(SkillManagerError::InvalidInput(
            "global and project are mutually exclusive".into(),
        ));
    }
    Ok(())
}

fn overlay_bool(destination: &mut bool, value: Option<&Value>) -> Result<()> {
    if !*destination && let Some(raw) = value {
        *destination = strict_bool(raw)?;
    }
    Ok(())
}

fn overlay_strings(
    destination: &mut Vec<String>,
    object: &Map<String, Value>,
    keys: &[&str],
) -> Result<()> {
    if !destination.is_empty() {
        return Ok(());
    }
    for key in keys {
        if let Some(value) = object.get(*key) {
            *destination = strict_strings(value)?;
            break;
        }
    }
    Ok(())
}

fn overlay_references(
    destination: &mut Vec<String>,
    object: &Map<String, Value>,
    keys: &[&str],
    base: &Path,
) -> Result<()> {
    if !destination.is_empty() {
        return Ok(());
    }
    for key in keys {
        if let Some(value) = object.get(*key) {
            *destination = strict_strings(value)?
                .into_iter()
                .map(|reference| {
                    if is_fnmatch_operand(&reference) {
                        reference
                    } else {
                        rebase_reference(&reference, base, false)
                    }
                })
                .collect();
            break;
        }
    }
    Ok(())
}

fn first_string(object: &Map<String, Value>, keys: &[&str]) -> Result<Option<String>> {
    for key in keys {
        if let Some(value) = object.get(*key) {
            return Ok(Some(strict_string(value)?.to_owned()));
        }
    }
    Ok(None)
}

fn strict_strings(value: &Value) -> Result<Vec<String>> {
    if let Some(single) = value.as_str() {
        return Ok(vec![single.to_owned()]);
    }
    let array = value.as_array().ok_or_else(|| {
        SkillManagerError::InvalidInput(
            "repeatable JSON field must be a string or string array".into(),
        )
    })?;
    array
        .iter()
        .map(|item| strict_string(item).map(ToOwned::to_owned))
        .collect()
}

fn strict_string(value: &Value) -> Result<&str> {
    value
        .as_str()
        .ok_or_else(|| SkillManagerError::InvalidInput("JSON field must be a string".into()))
}

fn strict_bool(value: &Value) -> Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| SkillManagerError::InvalidInput("JSON field must be a boolean".into()))
}

fn strict_i64(value: &Value) -> Result<i64> {
    value
        .as_i64()
        .ok_or_else(|| SkillManagerError::InvalidInput("JSON field must be an integer".into()))
}

fn reject_unknown(object: &Map<String, Value>, allowed: &[&str]) -> Result<()> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(SkillManagerError::InvalidInput(format!(
                "unknown JSON invocation field: {key}"
            )));
        }
    }
    Ok(())
}

fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Load(_) => "load",
        Command::Update(_) => "update",
        Command::Import(_) => "import",
        Command::Copy(_) => "copy",
        Command::Remove(_) => "remove",
        Command::Status(_) => "status",
        Command::Resolve(_) => "resolve",
        Command::Source(args) => match args.action {
            SourceAction::Add(_) => "source.add",
            SourceAction::Remove(_) => "source.remove",
            SourceAction::List => "source.list",
            SourceAction::Update(_) => "source.update",
            SourceAction::Locate(_) => "source.locate",
            SourceAction::Alternate(_) => "source.alternate",
            SourceAction::Swap(_) => "source.swap",
        },
        Command::Target(args) => match args.action {
            TargetAction::Add(_) => "target.add",
            TargetAction::List => "target.list",
            TargetAction::Enable(_) => "target.enable",
            TargetAction::Disable(_) => "target.disable",
            TargetAction::Remove(_) => "target.remove",
            TargetAction::SetPath(_) => "target.set-path",
        },
        Command::Configs(args) => match args.action {
            None => "configs",
            Some(ConfigsAction::Reset(_)) => "configs.reset",
            Some(ConfigsAction::Restore(_)) => "configs.restore",
        },
        Command::GenerateCompletions(_) => "generate-completions",
        Command::GenerateMan(_) => "generate-man",
    }
}

fn canonical_command(value: &str) -> Result<&'static str> {
    match value {
        "load" | "install" => Ok("load"),
        "update" | "up" => Ok("update"),
        "import" => Ok("import"),
        "copy" => Ok("copy"),
        "remove" => Ok("remove"),
        "status" | "ls" | "list" => Ok("status"),
        "resolve" => Ok("resolve"),
        "source.add" => Ok("source.add"),
        "source.remove" => Ok("source.remove"),
        "source.list" => Ok("source.list"),
        "source.update" => Ok("source.update"),
        "source.locate" => Ok("source.locate"),
        "source.alternate" => Ok("source.alternate"),
        "source.swap" => Ok("source.swap"),
        "target.add" => Ok("target.add"),
        "target.list" => Ok("target.list"),
        "target.enable" => Ok("target.enable"),
        "target.disable" => Ok("target.disable"),
        "target.remove" => Ok("target.remove"),
        "target.set-path" => Ok("target.set-path"),
        "configs" => Ok("configs"),
        "configs.reset" => Ok("configs.reset"),
        "configs.restore" => Ok("configs.restore"),
        _ => Err(SkillManagerError::InvalidInput(format!(
            "unknown recipe command: {value}"
        ))),
    }
}

fn default_command(name: &str) -> Result<Command> {
    match name {
        "load" => Ok(Command::Load(LoadArgs::default())),
        "update" => Ok(Command::Update(UpdateArgs::default())),
        "import" => Ok(Command::Import(ImportArgs::default())),
        "remove" => Ok(Command::Remove(RemoveArgs::default())),
        "status" => Ok(Command::Status(StatusArgs::default())),
        "resolve" => Ok(Command::Resolve(ResolveArgs::default())),
        "configs" => Ok(Command::Configs(ConfigsArgs {
            raw: false,
            action: None,
        })),
        "configs.reset" => Ok(Command::Configs(ConfigsArgs {
            raw: false,
            action: Some(ConfigsAction::Reset(ConfigsConfirmArgs::default())),
        })),
        "configs.restore" => Ok(Command::Configs(ConfigsArgs {
            raw: false,
            action: Some(ConfigsAction::Restore(ConfigsRestoreArgs::default())),
        })),
        "source.add" => Ok(Command::Source(SourceArgs {
            action: SourceAction::Add(SourceAddArgs {
                source: None,
                source_name: None,
                name: None,
                label: None,
                exclude: Vec::new(),
                mode: None,
                cache_ttl_hours: None,
            }),
        })),
        "source.remove" => Ok(Command::Source(SourceArgs {
            action: SourceAction::Remove(SourceRemoveArgs { source: None }),
        })),
        "source.list" => Ok(Command::Source(SourceArgs {
            action: SourceAction::List,
        })),
        "target.list" => Ok(Command::Target(TargetArgs {
            action: TargetAction::List,
        })),
        // Commands with required positional fields must be expressed on argv.
        "copy" | "source.update" | "source.locate" | "source.alternate" | "source.swap"
        | "target.add" | "target.enable" | "target.disable" | "target.remove"
        | "target.set-path" => build_required_command(name),
        _ => Err(SkillManagerError::InvalidInput(format!(
            "cannot create recipe command: {name}"
        ))),
    }
}

fn validate_required(command: &Command) -> Result<()> {
    let missing = match command {
        Command::Import(args) if args.skill.is_empty() => Some("import.skill"),
        Command::Copy(args) if args.source.is_empty() => Some("copy.source"),
        Command::Copy(args) if args.destination.as_os_str().is_empty() => Some("copy.destination"),
        Command::Source(SourceArgs {
            action: SourceAction::Update(args),
        }) if args.source.is_empty() => Some("source.update.source"),
        Command::Source(SourceArgs {
            action: SourceAction::Locate(args),
        }) if args.source.is_empty() => Some("source.locate.source"),
        Command::Source(SourceArgs {
            action: SourceAction::Locate(args),
        }) if args.location.is_empty() => Some("source.locate.location"),
        Command::Source(SourceArgs {
            action: SourceAction::Alternate(args),
        }) if args.source.is_empty() => Some("source.alternate.source"),
        Command::Source(SourceArgs {
            action: SourceAction::Alternate(args),
        }) if args.location.is_some() == args.clear => {
            return Err(SkillManagerError::InvalidInput(
                "source.alternate requires exactly one of location or clear:true".into(),
            ));
        }
        Command::Source(SourceArgs {
            action: SourceAction::Swap(args),
        }) if args.source.is_empty() => Some("source.swap.source"),
        Command::Target(TargetArgs {
            action: TargetAction::Add(args) | TargetAction::SetPath(args),
        }) if args.name.is_empty() => Some("target.name"),
        Command::Target(TargetArgs {
            action: TargetAction::Add(args) | TargetAction::SetPath(args),
        }) if args.path.as_os_str().is_empty() => Some("target.path"),
        Command::Target(TargetArgs {
            action:
                TargetAction::Enable(TargetNameArgs { name })
                | TargetAction::Disable(TargetNameArgs { name })
                | TargetAction::Remove(TargetNameArgs { name }),
        }) if name.is_empty() => Some("target.name"),
        _ => None,
    };
    missing.map_or(Ok(()), |field| {
        Err(SkillManagerError::InvalidInput(format!(
            "JSON invocation requires field {field}"
        )))
    })
}

fn build_required_command(name: &str) -> Result<Command> {
    // Placeholders are overwritten by strict required-field validation below.
    match name {
        "copy" => Ok(Command::Copy(CopyArgs {
            source: String::new(),
            destination: PathBuf::new(),
            filters: Vec::new(),
            dry_run: false,
            refresh: false,
            yes: false,
        })),
        "source.update" => Ok(Command::Source(SourceArgs {
            action: SourceAction::Update(SourceUpdateArgs {
                source: String::new(),
                name: None,
                location: None,
                label: None,
                exclude: Vec::new(),
                clear_exclude: false,
                cache_ttl_hours: None,
            }),
        })),
        "source.locate" => Ok(Command::Source(SourceArgs {
            action: SourceAction::Locate(SourceLocateArgs {
                source: String::new(),
                location: String::new(),
            }),
        })),
        "source.alternate" => Ok(Command::Source(SourceArgs {
            action: SourceAction::Alternate(SourceAlternateArgs {
                source: String::new(),
                location: None,
                clear: false,
            }),
        })),
        "source.swap" => Ok(Command::Source(SourceArgs {
            action: SourceAction::Swap(SourceSwapArgs {
                source: String::new(),
            }),
        })),
        "target.add" | "target.set-path" => Ok(Command::Target(TargetArgs {
            action: if name == "target.add" {
                TargetAction::Add(TargetPathArgs {
                    name: String::new(),
                    path: PathBuf::new(),
                })
            } else {
                TargetAction::SetPath(TargetPathArgs {
                    name: String::new(),
                    path: PathBuf::new(),
                })
            },
        })),
        "target.enable" | "target.disable" | "target.remove" => {
            let args = TargetNameArgs {
                name: String::new(),
            };
            Ok(Command::Target(TargetArgs {
                action: match name {
                    "target.enable" => TargetAction::Enable(args),
                    "target.disable" => TargetAction::Disable(args),
                    _ => TargetAction::Remove(args),
                },
            }))
        }
        _ => Err(SkillManagerError::InvalidInput(format!(
            "unsupported recipe command: {name}"
        ))),
    }
}

fn current_directory() -> Result<PathBuf> {
    std::env::current_dir().map_err(|error| SkillManagerError::io(".", error))
}

fn resolve_path(path: &Path, base: &Path) -> PathBuf {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in resolved.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _removed = normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}

fn rebase_reference(reference: &str, base: &Path, source_add: bool) -> String {
    let path = Path::new(reference);
    if path.is_absolute()
        || reference == "~"
        || reference.starts_with("~/")
        || reference.starts_with("~\\")
        || reference.contains("://")
    {
        return reference.to_owned();
    }
    let relative_path = reference.starts_with('.')
        || reference.contains('\\')
        || base.join(path).exists()
        || source_add && !looks_like_github_shorthand(reference);
    if relative_path {
        resolve_path(path, base).to_string_lossy().into_owned()
    } else {
        reference.to_owned()
    }
}

fn looks_like_github_shorthand(reference: &str) -> bool {
    let Some((owner, remainder)) = reference.split_once('/') else {
        return false;
    };
    let repo = remainder
        .split('/')
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default();
    valid_github_segment(owner) && valid_github_segment(repo)
}

fn valid_github_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;

    use clap::Parser;
    use serde_json::{Value, json};

    use super::{
        apply_recipe, canonical_command, default_command, looks_like_github_shorthand,
        rebase_reference, resolve_path, strict_bool, strict_i64, strict_string, strict_strings,
        validate_required,
    };
    use crate::cli::{Cli, Command, ConfigsAction, SourceAction, TargetAction};

    fn inline_recipe(payload: &Value) -> Cli {
        let argument = format!("--json={payload}");
        let mut cli = Cli::try_parse_from(["skill-manager", &argument])
            .unwrap_or_else(|error| unreachable!("{error}"));
        apply_recipe(&mut cli).unwrap_or_else(|error| unreachable!("{error}"));
        cli
    }

    #[test]
    fn file_recipe_keeps_selectors_verbatim_and_rebases_only_paths_and_locations() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let recipes = root.path().join("recipes");
        let source_path = root.path().join("source");
        std::fs::create_dir_all(&recipes).unwrap_or_else(|error| unreachable!("{error}"));
        std::fs::create_dir_all(&source_path).unwrap_or_else(|error| unreachable!("{error}"));
        let recipe_path = recipes.join("copy.json");
        std::fs::write(
            &recipe_path,
            r#"{"command":"copy","source":"../source","destination":"../destination"}"#,
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        let mut cli =
            Cli::try_parse_from(["skill-manager", "--input", &recipe_path.to_string_lossy()])
                .unwrap_or_else(|error| unreachable!("{error}"));
        apply_recipe(&mut cli).unwrap_or_else(|error| unreachable!("{error}"));
        let Some(Command::Copy(copy)) = cli.command else {
            unreachable!("copy command");
        };
        assert_eq!(copy.source, "../source");
        assert_eq!(copy.destination, root.path().join("destination"));

        std::fs::write(
            &recipe_path,
            r#"{"command":"source.update","source":"../selector","location":"../source"}"#,
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        let mut update =
            Cli::try_parse_from(["skill-manager", "--input", &recipe_path.to_string_lossy()])
                .unwrap_or_else(|error| unreachable!("{error}"));
        apply_recipe(&mut update).unwrap_or_else(|error| unreachable!("{error}"));
        let Some(Command::Source(source)) = update.command else {
            unreachable!("source command");
        };
        let SourceAction::Update(update) = source.action else {
            unreachable!("source update");
        };
        assert_eq!(update.source, "../selector");
        assert_eq!(
            update.location.as_deref(),
            Some(source_path.to_string_lossy().as_ref())
        );

        let explicit_source = root.path().join("explicit-source");
        let explicit_destination = root.path().join("explicit-destination");
        std::fs::write(
            &recipe_path,
            r#"{"command":"copy","source":"../source","destination":"../destination"}"#,
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        let mut explicit = Cli::try_parse_from([
            "skill-manager",
            "--input",
            &recipe_path.to_string_lossy(),
            "copy",
            &explicit_source.to_string_lossy(),
            &explicit_destination.to_string_lossy(),
        ])
        .unwrap_or_else(|error| unreachable!("{error}"));
        apply_recipe(&mut explicit).unwrap_or_else(|error| unreachable!("{error}"));
        let Some(Command::Copy(copy)) = explicit.command else {
            unreachable!("copy command");
        };
        assert_eq!(copy.source, explicit_source.to_string_lossy());
        assert_eq!(copy.destination, explicit_destination);
    }

    #[test]
    fn strict_recipe_types_and_carriers_reject_ambiguous_or_malformed_input() {
        assert_eq!(
            strict_string(&json!("value")).unwrap_or_else(|error| unreachable!("{error}")),
            "value"
        );
        assert!(strict_string(&json!(1)).is_err());
        assert!(strict_bool(&json!(true)).unwrap_or(false));
        assert!(strict_bool(&json!("true")).is_err());
        assert_eq!(
            strict_i64(&json!(-2)).unwrap_or_else(|error| unreachable!("{error}")),
            -2
        );
        assert!(strict_i64(&json!(0.5)).is_err());
        assert_eq!(
            strict_strings(&json!("one")).unwrap_or_else(|error| unreachable!("{error}")),
            ["one"]
        );
        assert_eq!(
            strict_strings(&json!(["one", "two"])).unwrap_or_else(|error| unreachable!("{error}")),
            ["one", "two"]
        );
        assert!(strict_strings(&json!(["one", 2])).is_err());
        assert!(strict_strings(&json!(true)).is_err());

        for payload in [
            "[]",
            r#"{"command":null}"#,
            r#"{"command":"unknown"}"#,
            r#"{"command":"status","unknown":true}"#,
        ] {
            let argument = format!("--json={payload}");
            let mut cli = Cli::try_parse_from(["skill-manager", &argument])
                .unwrap_or_else(|error| unreachable!("{error}"));
            assert!(apply_recipe(&mut cli).is_err(), "{payload}");
        }

        let mut conflicting = Cli::try_parse_from(["skill-manager", "--json={}", "--json-input"])
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(apply_recipe(&mut conflicting).is_err());

        let mismatch = "--json={\"command\":\"load\"}";
        let mut cli = Cli::try_parse_from(["skill-manager", mismatch, "status"])
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(apply_recipe(&mut cli).is_err());
    }

    #[test]
    fn sync_remove_status_and_resolve_recipes_overlay_every_selector_family() {
        let cli = inline_recipe(&json!({
            "command": "load",
            "sources": ["owner/repo", "./local"],
            "filters": "a*",
            "cd": true,
            "claude": true,
            "targets": ["disabled"],
            "dry_run": true,
            "refresh": true,
            "project": true,
            "no_input": true
        }));
        let Some(Command::Load(args)) = cli.command else {
            unreachable!("load command");
        };
        assert_eq!(args.sync.sources.len(), 2);
        assert_eq!(args.sync.filters, ["a*"]);
        assert!(args.sync.source_selection.cd);
        assert!(args.sync.targets.claude);
        assert_eq!(args.sync.targets.target_names, ["disabled"]);
        assert!(args.sync.dry_run && args.sync.refresh && cli.no_input);
        assert!(args.sync.scope.project && !args.sync.scope.global);

        let cli = inline_recipe(&json!({
            "command": "remove",
            "skills": ["one", "two"],
            "filters": ["?wo"],
            "cd_only": true,
            "shared": true,
            "all_targets": true,
            "yes": true,
            "dry_run": true,
            "refresh": true,
            "global": true
        }));
        let Some(Command::Remove(args)) = cli.command else {
            unreachable!("remove command");
        };
        assert_eq!(args.skills, ["one", "two"]);
        assert_eq!(args.filters, ["?wo"]);
        assert!(args.source_selection.cd_only);
        assert!(args.targets.shared && args.targets.all_targets);
        assert!(args.yes && args.dry_run && args.refresh);
        assert!(args.scope.global && !args.scope.project);

        let cli = inline_recipe(&json!({
            "command": "status",
            "filter": "demo",
            "no_cd": true,
            "antigravity": true,
            "refresh": true,
            "project": true
        }));
        let Some(Command::Status(args)) = cli.command else {
            unreachable!("status command");
        };
        assert_eq!(args.filters, ["demo"]);
        assert!(args.source_selection.no_cd && args.targets.antigravity && args.refresh);
        assert!(args.scope.project && !args.scope.global);

        let cli = inline_recipe(&json!({
            "command": "resolve",
            "skill": "demo",
            "prefer_source": "primary",
            "cd": true,
            "refresh": true
        }));
        let Some(Command::Resolve(args)) = cli.command else {
            unreachable!("resolve command");
        };
        assert_eq!(args.skills, ["demo"]);
        assert_eq!(args.prefer_source.as_deref(), Some("primary"));
        assert!(args.source_selection.cd && args.refresh);
    }

    #[test]
    fn source_and_target_lifecycle_recipes_build_required_commands() {
        let cli = inline_recipe(&json!({
            "command": "source.add",
            "source": "local-source",
            "name": "local",
            "label": "Local",
            "exclude": ["draft-*"],
            "mode": "single",
            "cache_ttl_hours": 0
        }));
        let Some(Command::Source(source)) = cli.command else {
            unreachable!("source command");
        };
        let SourceAction::Add(args) = source.action else {
            unreachable!("source add");
        };
        assert!(args.source.is_some());
        assert_eq!(args.source_name.as_deref(), Some("local"));
        assert_eq!(args.label.as_deref(), Some("Local"));
        assert_eq!(args.exclude, ["draft-*"]);
        assert_eq!(args.cache_ttl_hours, Some(0));

        let cli = inline_recipe(&json!({
            "command": "source.update",
            "source": "owner/repo",
            "name": "renamed",
            "label": "Renamed",
            "exclude": "old-*",
            "clear_exclude": true,
            "cache_ttl_hours": 2
        }));
        let Some(Command::Source(source)) = cli.command else {
            unreachable!("source command");
        };
        let SourceAction::Update(args) = source.action else {
            unreachable!("source update");
        };
        assert_eq!(args.source, "owner/repo");
        assert_eq!(args.name.as_deref(), Some("renamed"));
        assert_eq!(args.exclude, ["old-*"]);
        assert!(args.clear_exclude);

        for command in ["source.remove", "source.list"] {
            let payload = if command == "source.remove" {
                json!({"command": command, "source": "owner/repo"})
            } else {
                json!({"command": command})
            };
            let cli = inline_recipe(&payload);
            assert!(matches!(cli.command, Some(Command::Source(_))));
        }

        for command in [
            "target.add",
            "target.set-path",
            "target.enable",
            "target.disable",
            "target.remove",
            "target.list",
        ] {
            let payload = match command {
                "target.add" | "target.set-path" => {
                    json!({"command": command, "name": "custom", "path": "./target"})
                }
                "target.list" => json!({"command": command}),
                _ => json!({"command": command, "name": "custom"}),
            };
            let cli = inline_recipe(&payload);
            let Some(Command::Target(target)) = cli.command else {
                unreachable!("target command");
            };
            match target.action {
                TargetAction::Add(args) | TargetAction::SetPath(args) => {
                    assert_eq!(args.name, "custom");
                    assert!(!args.path.as_os_str().is_empty());
                }
                TargetAction::Enable(args)
                | TargetAction::Disable(args)
                | TargetAction::Remove(args) => assert_eq!(args.name, "custom"),
                TargetAction::List => assert_eq!(command, "target.list"),
            }
        }
    }

    #[test]
    fn config_recipes_are_strict_and_target_templates_are_not_rebased() {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let recipes = root.path().join("recipes");
        std::fs::create_dir_all(&recipes).unwrap_or_else(|error| unreachable!("{error}"));
        let recipe_path = recipes.join("target.json");
        std::fs::write(
            &recipe_path,
            r#"{"command":"target.add","name":"custom","path":"./target"}"#,
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        let mut target =
            Cli::try_parse_from(["skill-manager", "--input", &recipe_path.to_string_lossy()])
                .unwrap_or_else(|error| unreachable!("{error}"));
        apply_recipe(&mut target).unwrap_or_else(|error| unreachable!("{error}"));
        let Some(Command::Target(target)) = target.command else {
            unreachable!("target command");
        };
        let TargetAction::Add(args) = target.action else {
            unreachable!("target add");
        };
        assert_eq!(args.path, PathBuf::from("./target"));

        let reset = inline_recipe(&json!({"command": "configs.reset", "yes": true}));
        assert!(matches!(
            reset.command,
            Some(Command::Configs(crate::cli::ConfigsArgs {
                action: Some(ConfigsAction::Reset(crate::cli::ConfigsConfirmArgs {
                    yes: true
                })),
                ..
            }))
        ));
        let restore = inline_recipe(&json!({
            "command": "configs.restore",
            "backup": "20260726-reset",
            "yes": true
        }));
        let Some(Command::Configs(configs)) = restore.command else {
            unreachable!("configs command");
        };
        let Some(ConfigsAction::Restore(restore)) = configs.action else {
            unreachable!("restore action");
        };
        assert_eq!(restore.backup_id.as_deref(), Some("20260726-reset"));
        assert!(restore.yes);

        let mut no_raw = Cli::try_parse_from([
            "skill-manager",
            "--json={\"command\":\"configs\",\"raw\":true}",
        ])
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(apply_recipe(&mut no_raw).is_err());

        let mut conflicting = Cli::try_parse_from([
            "skill-manager",
            "--json={\"command\":\"load\",\"global\":true,\"project\":true}",
        ])
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(apply_recipe(&mut conflicting).is_err());

        let remove = inline_recipe(&json!({
            "command": "remove",
            "skill": "./grill-*",
            "global": true
        }));
        let Some(Command::Remove(remove)) = remove.command else {
            unreachable!("remove command");
        };
        assert_eq!(remove.skills, ["./grill-*"]);
    }

    #[test]
    fn argv_scope_wins_over_a_strictly_typed_recipe_scope_pair() {
        let mut cli = Cli::try_parse_from([
            "skill-manager",
            r#"--json={"command":"load","global":true,"project":true}"#,
            "load",
            "--global",
        ])
        .unwrap_or_else(|error| unreachable!("{error}"));
        apply_recipe(&mut cli).unwrap_or_else(|error| unreachable!("{error}"));
        let Some(Command::Load(args)) = cli.command else {
            unreachable!("load command");
        };
        assert!(args.sync.scope.global);
        assert!(!args.sync.scope.project);

        let mut invalid_type = Cli::try_parse_from([
            "skill-manager",
            r#"--json={"command":"load","project":"true"}"#,
            "load",
            "--global",
        ])
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(apply_recipe(&mut invalid_type).is_err());

        let mut recipe_conflict = Cli::try_parse_from([
            "skill-manager",
            r#"--json={"command":"load","global":true,"project":true}"#,
        ])
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(apply_recipe(&mut recipe_conflict).is_err());
    }

    #[test]
    fn required_fields_aliases_and_path_rebasing_are_strict() {
        assert_eq!(
            canonical_command("ls").unwrap_or_else(|error| unreachable!("{error}")),
            "status"
        );
        assert_eq!(
            canonical_command("list").unwrap_or_else(|error| unreachable!("{error}")),
            "status"
        );
        assert!(canonical_command("bogus").is_err());
        for command in [
            "copy",
            "source.update",
            "target.add",
            "target.set-path",
            "target.enable",
            "target.disable",
            "target.remove",
        ] {
            let value = default_command(command).unwrap_or_else(|error| unreachable!("{error}"));
            assert!(validate_required(&value).is_err(), "{command}");
        }
        assert!(default_command("unsupported").is_err());

        let base = Path::new("C:/recipe/base");
        assert_eq!(
            resolve_path(Path::new("./one/../two"), base),
            base.join("two")
        );
        assert_eq!(rebase_reference("owner/repo", base, false), "owner/repo");
        assert_eq!(
            Path::new(&rebase_reference("local", base, true)),
            resolve_path(Path::new("local"), base)
        );
        for reference in ["~", "~/skills", "https://example.test/repo"] {
            assert_eq!(rebase_reference(reference, base, true), reference);
        }
        assert!(looks_like_github_shorthand("owner/repo"));
        assert!(!looks_like_github_shorthand("single"));
        assert!(!looks_like_github_shorthand("bad owner/repo"));

        let mutually_exclusive = "--json={\"command\":\"load\",\"cd\":true,\"cd_only\":true}";
        let mut cli = Cli::try_parse_from(["skill-manager", mutually_exclusive])
            .unwrap_or_else(|error| unreachable!("{error}"));
        assert!(apply_recipe(&mut cli).is_err());
    }
}
