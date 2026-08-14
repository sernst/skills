//! Direct strict-recipe coverage for every public command shape.

#![allow(
    clippy::expect_used,
    reason = "A parser failure in a fixed JSON fixture is an unrecoverable test harness failure."
)]

use clap::Parser;
use skill_manager::cli::{Cli, Command, SourceAction, TargetAction};
use skill_manager::recipe::apply_recipe;

fn recipe(value: &serde_json::Value) -> Cli {
    let argument = format!("--json={value}");
    let mut cli =
        Cli::try_parse_from(["skill-manager", argument.as_str()]).expect("parse JSON carrier");
    apply_recipe(&mut cli).expect("apply valid recipe");
    cli
}

fn recipe_error(value: &serde_json::Value) -> String {
    let argument = format!("--json={value}");
    let mut cli =
        Cli::try_parse_from(["skill-manager", argument.as_str()]).expect("parse JSON carrier");
    apply_recipe(&mut cli)
        .expect_err("recipe must fail")
        .to_string()
}

#[test]
fn source_recipe_rebases_noncanonical_internal_paths_to_the_recipe_directory() {
    let root = tempfile::tempdir().expect("create recipe root");
    let recipe_dir = root.path().join("recipes");
    std::fs::create_dir_all(&recipe_dir).expect("create recipe directory");
    let recipe_path = recipe_dir.join("source-add.json");
    std::fs::write(
        &recipe_path,
        serde_json::json!({
            "command": "source.add",
            "source": "owner/repo/../local",
            "name": "local"
        })
        .to_string(),
    )
    .expect("write source recipe");

    let mut cli = Cli::try_parse_from([
        "skill-manager",
        "--input",
        recipe_path.to_str().expect("UTF-8 recipe path"),
    ])
    .expect("parse recipe carrier");
    apply_recipe(&mut cli).expect("apply source recipe");
    let Some(Command::Source(source)) = cli.command else {
        unreachable!("source command")
    };
    let SourceAction::Add(args) = source.action else {
        unreachable!("source add")
    };
    let expected = recipe_dir.join("owner/local");
    assert_eq!(
        args.source.as_deref().map(std::path::Path::new),
        Some(expected.as_path())
    );
    assert_eq!(args.name.as_deref(), Some("local"));
}

#[test]
fn recipe_overlay_covers_transfer_command_shapes() {
    let load = recipe(&serde_json::json!({
        "command": "load",
        "sources": ["one", "two"],
        "filters": "a*",
        "target": ["claude", "shared"],
        "cd": true,
        "dry_run": true,
        "refresh": true,
        "no_input": true
    }));
    let Some(Command::Load(args)) = load.command else {
        unreachable!("load command")
    };
    assert_eq!(args.sync.sources, ["one", "two"]);
    assert_eq!(args.sync.filters, ["a*"]);
    assert_eq!(args.sync.targets.target_names, ["claude", "shared"]);
    assert!(
        args.sync.source_selection.cd && args.sync.dry_run && args.sync.refresh && load.no_input
    );

    let update = recipe(&serde_json::json!({
        "command": "update",
        "source": "one",
        "filter": ["a*", "b*"],
        "claude": true,
        "shared": true,
        "antigravity": true,
        "all": true,
        "cd_only": true,
        "yes": true
    }));
    let Some(Command::Update(args)) = update.command else {
        unreachable!("update command")
    };
    assert_eq!(args.sync.sources, ["one"]);
    assert_eq!(args.sync.filters, ["a*", "b*"]);
    assert!(args.sync.targets.claude && args.sync.targets.shared);
    assert!(args.sync.targets.antigravity && args.sync.targets.all_targets);
    assert!(args.sync.source_selection.cd_only && args.yes);

    let copy = recipe(&serde_json::json!({
        "command": "copy",
        "source": "source",
        "destination": "destination",
        "filter": "x",
        "dry_run": true,
        "refresh": true
    }));
    let Some(Command::Copy(args)) = copy.command else {
        unreachable!("copy command")
    };
    assert_eq!(args.filters, ["x"]);
    assert!(args.dry_run && args.refresh);
    assert!(args.destination.ends_with("destination"));

    let remove = recipe(&serde_json::json!({
        "command": "remove",
        "skills": ["one", "two"],
        "filters": "o*",
        "targets": "claude",
        "no_cd": true,
        "dry_run": true,
        "refresh": true,
        "yes": true
    }));
    let Some(Command::Remove(args)) = remove.command else {
        unreachable!("remove command")
    };
    assert_eq!(args.skills, ["one", "two"]);
    assert!(args.source_selection.no_cd && args.dry_run && args.refresh && args.yes);
}

/// `install` is an alias for `load`, and `import` is its own strict shape.
#[test]
fn recipe_overlay_covers_sync_aliases_and_import_command() {
    let installed = recipe(&serde_json::json!({
        "command": "install",
        "sources": ["primary"],
        "claude": true,
        "global": true
    }));
    let Some(Command::Load(args)) = installed.command else {
        unreachable!("install must canonicalize to load")
    };
    assert_eq!(args.sync.sources, ["primary"]);
    assert!(args.sync.targets.claude && args.sync.scope.global);

    let mut argv_alias = Cli::try_parse_from([
        "skill-manager",
        "--json={\"command\":\"install\"}",
        "install",
        "--claude",
    ])
    .expect("parse install alias");
    apply_recipe(&mut argv_alias).expect("install alias matches the load command");
    assert!(matches!(argv_alias.command, Some(Command::Load(_))));

    let updated = recipe(&serde_json::json!({
        "command": "up",
        "filters": ["alpha"]
    }));
    let Some(Command::Update(args)) = updated.command else {
        unreachable!("up must canonicalize to update")
    };
    assert_eq!(args.sync.filters, ["alpha"]);

    let imported = recipe(&serde_json::json!({
        "command": "import",
        "skill": "alpha",
        "targets": ["custom"],
        "shared": true,
        "project": true,
        "dry_run": true,
        "yes": true,
        "no_input": true
    }));
    let Some(Command::Import(args)) = imported.command else {
        unreachable!("import command")
    };
    assert_eq!(args.skill, "alpha");
    assert_eq!(args.targets.target_names, ["custom"]);
    assert!(args.targets.shared && args.scope.project);
    assert!(args.dry_run && args.yes && imported.no_input);

    assert!(
        recipe_error(&serde_json::json!({"command": "import"}))
            .contains("JSON invocation requires field import.skill")
    );
    assert!(
        recipe_error(&serde_json::json!({"command": "import", "skill": "a", "refresh": true}))
            .contains("unknown JSON invocation field: refresh")
    );
}

/// clap's `conflicts_with` between `--update`/`--no-update` only guards the
/// CLI argv path; the JSON overlay applies both fields independently, so
/// `recipe.rs` re-checks the exclusivity itself. Also covers item E:
/// `update` is a tri-state in JSON even though it is two flags on the CLI,
/// so `update:false` must resolve propagation to import-only, equivalent to
/// `no_update:true`, rather than silently leaving both flags unset.
#[test]
fn import_update_and_no_update_recipe_fields_are_mutually_exclusive() {
    assert!(
        recipe_error(&serde_json::json!({
            "command": "import",
            "skill": "a",
            "update": true,
            "no_update": true
        }))
        .contains("mutually exclusive")
    );

    let import_update = recipe(&serde_json::json!({
        "command": "import",
        "skill": "a",
        "update": true
    }));
    let Some(Command::Import(args)) = import_update.command else {
        unreachable!("import command")
    };
    assert!(args.update && !args.no_update);

    let import_only = recipe(&serde_json::json!({
        "command": "import",
        "skill": "a",
        "update": false
    }));
    let Some(Command::Import(args)) = import_only.command else {
        unreachable!("import command")
    };
    assert!(
        !args.update && args.no_update,
        "update:false must mean import-only (equivalent to no_update:true)"
    );

    let consistent = recipe(&serde_json::json!({
        "command": "import",
        "skill": "a",
        "update": false,
        "no_update": true
    }));
    let Some(Command::Import(args)) = consistent.command else {
        unreachable!("import command")
    };
    assert!(
        !args.update && args.no_update,
        "update:false alongside a consistent explicit no_update:true is not a conflict"
    );
}

#[test]
fn recipe_overlay_covers_query_command_shapes_and_explicit_false() {
    let explicit_false = recipe(&serde_json::json!({
        "command": "load",
        "claude": false,
        "all": false,
        "cd": false,
        "dry_run": false,
        "refresh": false,
        "no_input": false
    }));
    let Some(Command::Load(args)) = explicit_false.command else {
        unreachable!("load command")
    };
    assert!(!args.sync.targets.claude);
    assert!(!args.sync.targets.all_targets);
    assert!(!args.sync.source_selection.cd);
    assert!(!args.sync.dry_run);
    assert!(!args.sync.refresh);
    assert!(!explicit_false.no_input);

    let status = recipe(&serde_json::json!({
        "command": "list",
        "filters": ["one"],
        "target": "shared",
        "refresh": true
    }));
    let Some(Command::Status(args)) = status.command else {
        unreachable!("status command")
    };
    assert_eq!(args.filters, ["one"]);
    assert!(args.refresh);

    let resolve = recipe(&serde_json::json!({
        "command": "resolve",
        "skill": "common",
        "prefer_source": "second",
        "cd_only": true,
        "refresh": true
    }));
    let Some(Command::Resolve(args)) = resolve.command else {
        unreachable!("resolve command")
    };
    assert_eq!(args.skills, ["common"]);
    assert_eq!(args.prefer_source.as_deref(), Some("second"));
}

#[test]
fn recipe_overlay_covers_source_and_target_lifecycle_shapes() {
    let source_add = recipe(&serde_json::json!({
        "command": "source.add",
        "source": ".",
        "name": "local",
        "label": "Local",
        "exclude": ["draft-*"],
        "mode": "single",
        "cache_ttl_hours": 0
    }));
    let Some(Command::Source(source)) = source_add.command else {
        unreachable!("source command")
    };
    let SourceAction::Add(args) = source.action else {
        unreachable!("source add")
    };
    assert_eq!(args.name.as_deref(), Some("local"));
    assert_eq!(args.exclude, ["draft-*"]);
    assert_eq!(args.cache_ttl_hours, Some(0));

    let source_remove = recipe(&serde_json::json!({
        "command": "source.remove",
        "directory": "."
    }));
    assert!(matches!(
        source_remove.command,
        Some(Command::Source(source)) if matches!(source.action, SourceAction::Remove(_))
    ));

    let source_update = recipe(&serde_json::json!({
        "command": "source.update",
        "source": "local",
        "location": "owner/repo",
        "name": "renamed",
        "label": "Renamed",
        "exclude": "private-*",
        "clear_exclude": true,
        "cache_ttl_hours": 4
    }));
    let Some(Command::Source(source)) = source_update.command else {
        unreachable!("source command")
    };
    let SourceAction::Update(args) = source.action else {
        unreachable!("source update")
    };
    assert_eq!(args.source, "local");
    assert_eq!(args.location.as_deref(), Some("owner/repo"));
    assert!(args.clear_exclude);
    assert_eq!(args.cache_ttl_hours, Some(4));

    let source_list = recipe(&serde_json::json!({"command": "source.list"}));
    assert!(matches!(
        source_list.command,
        Some(Command::Source(source)) if matches!(source.action, SourceAction::List)
    ));

    let target_add = recipe(&serde_json::json!({
        "command": "target.add",
        "name": "custom",
        "path": "target"
    }));
    let Some(Command::Target(target)) = target_add.command else {
        unreachable!("target command")
    };
    let TargetAction::Add(args) = target.action else {
        unreachable!("target add")
    };
    assert_eq!(args.name.as_deref(), Some("custom"));
    assert_eq!(args.first, "target");
    assert!(args.second.is_none());
    let target_set = recipe(&serde_json::json!({
        "command": "target.set-path",
        "name": "custom",
        "path": "changed"
    }));
    assert!(matches!(
        target_set.command,
        Some(Command::Target(target)) if matches!(target.action, TargetAction::SetPath(_))
    ));
    for command in ["target.enable", "target.disable", "target.remove"] {
        let target = recipe(&serde_json::json!({"command": command, "name": "custom"}));
        assert!(matches!(target.command, Some(Command::Target(_))));
    }
    let target_list = recipe(&serde_json::json!({"command": "target.list"}));
    assert!(matches!(
        target_list.command,
        Some(Command::Target(target)) if matches!(target.action, TargetAction::List)
    ));
}

#[test]
fn recipe_overlay_covers_source_location_switching_shapes() {
    let source_locate = recipe(&serde_json::json!({
        "command": "source.locate",
        "source": "local",
        "location": "owner/repo"
    }));
    assert!(matches!(
        source_locate.command,
        Some(Command::Source(source))
            if matches!(source.action, SourceAction::Locate(ref args)
                if args.source == "local" && args.location == "owner/repo")
    ));

    let source_alternate = recipe(&serde_json::json!({
        "command": "source.alternate",
        "source": "local",
        "location": "owner/repo"
    }));
    assert!(matches!(
        source_alternate.command,
        Some(Command::Source(source))
            if matches!(source.action, SourceAction::Alternate(ref args)
                if args.location.as_deref() == Some("owner/repo") && !args.clear)
    ));
    let source_alternate_clear = recipe(&serde_json::json!({
        "command": "source.alternate",
        "source": "local",
        "clear": true
    }));
    assert!(matches!(
        source_alternate_clear.command,
        Some(Command::Source(source))
            if matches!(source.action, SourceAction::Alternate(ref args)
                if args.location.is_none() && args.clear)
    ));
    let source_swap = recipe(&serde_json::json!({
        "command": "source.swap",
        "source": "local"
    }));
    assert!(matches!(
        source_swap.command,
        Some(Command::Source(source))
            if matches!(source.action, SourceAction::Swap(ref args) if args.source == "local")
    ));
}

#[test]
fn recipe_strictness_rejects_all_invalid_carrier_shapes() {
    let failures = [
        (
            serde_json::json!({"command": "status", "unknown": true}),
            "unknown JSON invocation field",
        ),
        (
            serde_json::json!({"command": "status", "refresh": "yes"}),
            "must be a boolean",
        ),
        (
            serde_json::json!({"command": "status", "filter": [1]}),
            "must be a string",
        ),
        (
            serde_json::json!({"command": "status", "filter": null}),
            "does not accept null",
        ),
        (
            serde_json::json!({"command": "status", "cd": true, "no_cd": true}),
            "mutually exclusive",
        ),
        (
            serde_json::json!({"command": "copy", "destination": "out"}),
            "requires field copy.source",
        ),
        (
            serde_json::json!({"command": "copy", "source": "in"}),
            "requires field copy.destination",
        ),
        (
            serde_json::json!({"command": "target.add", "name": "custom"}),
            "requires field target.path",
        ),
        (
            serde_json::json!({"command": "source.add", "name": "local"}),
            "requires field source.add.source",
        ),
        (
            serde_json::json!({"command": "source.add", "source": "."}),
            "requires field source.add.name",
        ),
        (
            serde_json::json!({"command": "target.enable"}),
            "requires field target.name",
        ),
        (
            serde_json::json!({"command": "source.update"}),
            "requires field source.update.source",
        ),
        (
            serde_json::json!({"command": "source.add", "mode": "many"}),
            "unsupported source mode",
        ),
        (
            serde_json::json!({"command": "source.add", "cache_ttl_hours": 1.5}),
            "must be an integer",
        ),
        (
            serde_json::json!({
                "command": "source.add",
                "source": ".",
                "name": "local",
                "yes": true
            }),
            "unknown JSON invocation field",
        ),
        (
            serde_json::json!({
                "command": "target.add",
                "name": "custom",
                "path": "target",
                "yes": true
            }),
            "unknown JSON invocation field",
        ),
        (
            serde_json::json!({"command": "unknown"}),
            "unknown recipe command",
        ),
    ];
    for (value, expected) in failures {
        assert!(
            recipe_error(&value).contains(expected),
            "failure must mention {expected}"
        );
    }

    let mut mismatch = Cli::try_parse_from([
        "skill-manager",
        r#"--json={"command":"update"}"#,
        "load",
        "--all",
    ])
    .expect("parse mismatch fixture");
    assert!(
        apply_recipe(&mut mismatch)
            .expect_err("command mismatch")
            .to_string()
            .contains("does not match")
    );
}

#[test]
fn source_location_recipes_reject_missing_conflicting_alias_and_unknown_fields() {
    let failures = [
        (
            serde_json::json!({"command": "source.locate", "source": "one"}),
            "requires field source.locate.location",
        ),
        (
            serde_json::json!({"command": "source.alternate", "source": "one"}),
            "requires exactly one",
        ),
        (
            serde_json::json!({
                "command": "source.alternate",
                "source": "one",
                "location": "owner/repo",
                "clear": true
            }),
            "requires exactly one",
        ),
        (
            serde_json::json!({"command": "source.swap"}),
            "requires field source.swap.source",
        ),
        (
            serde_json::json!({
                "command": "source.alternate",
                "source": "one",
                "clear": "yes"
            }),
            "must be a boolean",
        ),
        (
            serde_json::json!({
                "command": "source.locate",
                "source": "one",
                "unknown": true
            }),
            "unknown JSON invocation field",
        ),
        (
            serde_json::json!({
                "command": "source.relocate",
                "source": "one",
                "location": "owner/repo"
            }),
            "unknown recipe command",
        ),
    ];
    for (value, expected) in failures {
        assert!(
            recipe_error(&value).contains(expected),
            "failure must mention {expected}"
        );
    }
}

#[test]
fn alternate_cli_values_suppress_the_opposite_recipe_value() {
    let mut explicit_location = Cli::try_parse_from([
        "skill-manager",
        r#"--json={"command":"source.alternate","source":"recipe","clear":true}"#,
        "source",
        "alternate",
        "cli-selector",
        "owner/cli",
    ])
    .expect("parse explicit location");
    apply_recipe(&mut explicit_location).expect("overlay explicit location");
    assert!(matches!(
        explicit_location.command,
        Some(Command::Source(source))
            if matches!(source.action, SourceAction::Alternate(ref args)
                if args.source == "cli-selector"
                    && args.location.as_deref() == Some("owner/cli")
                    && !args.clear)
    ));

    let mut explicit_clear = Cli::try_parse_from([
        "skill-manager",
        r#"--json={"command":"source.alternate","source":"recipe","location":"owner/recipe"}"#,
        "source",
        "alternate",
        "cli-selector",
        "--clear",
    ])
    .expect("parse explicit clear");
    apply_recipe(&mut explicit_clear).expect("overlay explicit clear");
    assert!(matches!(
        explicit_clear.command,
        Some(Command::Source(source))
            if matches!(source.action, SourceAction::Alternate(ref args)
                if args.source == "cli-selector" && args.location.is_none() && args.clear)
    ));

    let mut malformed_suppressed_clear = Cli::try_parse_from([
        "skill-manager",
        r#"--json={"command":"source.alternate","source":"recipe","clear":"malformed"}"#,
        "source",
        "alternate",
        "cli-selector",
        "owner/cli",
    ])
    .expect("parse location suppressing malformed clear");
    apply_recipe(&mut malformed_suppressed_clear)
        .expect("explicit location suppresses recipe clear before type parsing");

    let mut malformed_suppressed_location = Cli::try_parse_from([
        "skill-manager",
        r#"--json={"command":"source.alternate","source":"recipe","location":false}"#,
        "source",
        "alternate",
        "cli-selector",
        "--clear",
    ])
    .expect("parse clear suppressing malformed location");
    apply_recipe(&mut malformed_suppressed_location)
        .expect("explicit clear suppresses recipe location before type parsing");
}
