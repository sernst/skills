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
    assert_eq!(args.sources, ["one", "two"]);
    assert_eq!(args.filters, ["a*"]);
    assert_eq!(args.targets.target_names, ["claude", "shared"]);
    assert!(args.source_selection.cd && args.dry_run && args.refresh && load.no_input);

    let update = recipe(&serde_json::json!({
        "command": "update",
        "source": "one",
        "filter": ["a*", "b*"],
        "claude": true,
        "shared": true,
        "antigravity": true,
        "all": true,
        "cd_only": true
    }));
    let Some(Command::Update(args)) = update.command else {
        unreachable!("update command")
    };
    assert_eq!(args.sources, ["one"]);
    assert_eq!(args.filters, ["a*", "b*"]);
    assert!(args.targets.claude && args.targets.shared && args.targets.antigravity);
    assert!(args.targets.all_targets && args.source_selection.cd_only);

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
    assert!(!args.targets.claude);
    assert!(!args.targets.all_targets);
    assert!(!args.source_selection.cd);
    assert!(!args.dry_run);
    assert!(!args.refresh);
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
    assert_eq!(args.source_name.as_deref(), Some("local"));
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
    assert!(matches!(
        target_add.command,
        Some(Command::Target(target)) if matches!(target.action, TargetAction::Add(_))
    ));
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
