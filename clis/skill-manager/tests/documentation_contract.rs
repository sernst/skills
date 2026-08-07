//! Source-derived contracts for user and agent documentation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| unreachable!("crate must live below repository root"))
        .to_path_buf()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| unreachable!("{}: {error}", path.display()))
        .replace("\r\n", "\n")
}

fn function_block<'a>(source: &'a str, name: &str) -> &'a str {
    let marker = format!("fn {name}(");
    let start = source
        .find(&marker)
        .unwrap_or_else(|| unreachable!("missing {marker}"));
    let rest = &source[start + marker.len()..];
    let end = rest.find("\nfn ").unwrap_or(rest.len());
    &rest[..end]
}

fn arm_block<'a>(block: &'a str, marker: &str, next: Option<&str>) -> &'a str {
    let start = block
        .find(marker)
        .unwrap_or_else(|| unreachable!("missing recipe arm {marker}"));
    let rest = &block[start..];
    let end = next
        .and_then(|value| rest.find(value))
        .unwrap_or(rest.len());
    &rest[..end]
}

fn quoted_strings(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut characters = text.char_indices();
    while let Some((start, character)) = characters.next() {
        if character != '"' {
            continue;
        }
        let mut escaped = false;
        let mut end = None;
        for (index, candidate) in characters.by_ref() {
            if escaped {
                escaped = false;
            } else if candidate == '\\' {
                escaped = true;
            } else if candidate == '"' {
                end = Some(index);
                break;
            }
        }
        if let Some(end) = end {
            values.push(text[start + 1..end].to_owned());
        }
    }
    values
}

fn allowed_fields(block: &str) -> BTreeSet<String> {
    let call = block
        .find("reject_unknown")
        .unwrap_or_else(|| unreachable!("recipe overlay must reject unknown fields"));
    let rest = &block[call..];
    let start = rest
        .find("&[")
        .unwrap_or_else(|| unreachable!("reject_unknown must use an explicit field list"));
    let list = &rest[start + 2..];
    let end = list
        .find(']')
        .unwrap_or_else(|| unreachable!("unterminated reject_unknown field list"));
    quoted_strings(&list[..end]).into_iter().collect()
}

fn canonical_recipe_commands(source: &str) -> BTreeSet<String> {
    let block = function_block(source, "canonical_command");
    let mut commands = BTreeSet::new();
    for line in block.lines() {
        let Some(ok) = line.find("Ok(\"") else {
            continue;
        };
        let value = &line[ok + 4..];
        let Some(end) = value.find('"') else {
            continue;
        };
        commands.insert(value[..end].to_owned());
    }
    commands
}

fn source_recipe_fields(source: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut recipes = BTreeMap::new();
    for (name, function) in [
        ("load", "overlay_sync"),
        ("update", "overlay_update"),
        ("import", "overlay_import"),
        ("copy", "overlay_copy"),
        ("remove", "overlay_remove"),
        ("status", "overlay_status"),
        ("resolve", "overlay_resolve"),
    ] {
        recipes.insert(
            name.to_owned(),
            allowed_fields(function_block(source, function)),
        );
    }

    let source_block = function_block(source, "overlay_source");
    for (name, marker, next) in [
        (
            "source.add",
            "SourceAction::Add",
            Some("SourceAction::Remove"),
        ),
        (
            "source.remove",
            "SourceAction::Remove",
            Some("SourceAction::List"),
        ),
        (
            "source.list",
            "SourceAction::List",
            Some("SourceAction::Update"),
        ),
        (
            "source.update",
            "SourceAction::Update",
            Some("SourceAction::Locate"),
        ),
        (
            "source.locate",
            "SourceAction::Locate",
            Some("SourceAction::Alternate"),
        ),
        (
            "source.alternate",
            "SourceAction::Alternate",
            Some("SourceAction::Swap"),
        ),
        ("source.swap", "SourceAction::Swap", None),
    ] {
        recipes.insert(
            name.to_owned(),
            allowed_fields(arm_block(source_block, marker, next)),
        );
    }

    let target_block = function_block(source, "overlay_target");
    let target_list = allowed_fields(arm_block(
        target_block,
        "TargetAction::List",
        Some("TargetAction::Add"),
    ));
    let target_path = allowed_fields(arm_block(
        target_block,
        "TargetAction::Add",
        Some("TargetAction::Enable"),
    ));
    let target_name = allowed_fields(arm_block(target_block, "TargetAction::Enable", None));
    recipes.insert("target.list".into(), target_list);
    for name in ["target.add", "target.set-path"] {
        recipes.insert(name.into(), target_path.clone());
    }
    for name in ["target.enable", "target.disable", "target.remove"] {
        recipes.insert(name.into(), target_name.clone());
    }

    let configs_block = function_block(source, "overlay_configs");
    for (name, marker, next) in [
        ("configs", "None =>", Some("Some(ConfigsAction::Reset")),
        (
            "configs.reset",
            "Some(ConfigsAction::Reset",
            Some("Some(ConfigsAction::Restore"),
        ),
        ("configs.restore", "Some(ConfigsAction::Restore", None),
    ] {
        recipes.insert(
            name.into(),
            allowed_fields(arm_block(configs_block, marker, next)),
        );
    }
    recipes
}

fn documented_recipe_fields(markdown: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut recipes = BTreeMap::new();
    for line in markdown.lines() {
        let Some(marker) = line.strip_prefix("<!-- recipe-command: ") else {
            continue;
        };
        let Some((name, fields)) = marker.split_once(" fields: ") else {
            unreachable!("malformed recipe marker: {line}");
        };
        let Some(fields) = fields.strip_suffix(" -->") else {
            unreachable!("unterminated recipe marker: {line}");
        };
        let values = if fields.is_empty() {
            BTreeSet::new()
        } else {
            fields.split(',').map(ToOwned::to_owned).collect()
        };
        assert!(recipes.insert(name.to_owned(), values).is_none());
    }
    recipes
}

fn is_event_name(value: &str) -> bool {
    value == "summary"
        || value == "diagnostic"
        || [
            "collision.",
            "command.",
            "config.",
            "skill.",
            "source.",
            "status.",
            "target.",
        ]
        .iter()
        .any(|prefix| value.starts_with(prefix))
}

fn emitted_events(source: &str) -> BTreeSet<String> {
    let lines = source.lines().collect::<Vec<_>>();
    let mut events = BTreeSet::new();
    for (index, line) in lines.iter().enumerate() {
        let relevant = line.contains(".event(")
            || line.contains("emit_target_change(")
            || line.contains("let event = if");
        if !relevant {
            continue;
        }
        let end = (index + 8).min(lines.len());
        for value in quoted_strings(&lines[index..end].join("\n")) {
            if is_event_name(&value) {
                events.insert(value);
            }
        }
    }
    events
}

fn documented_events(markdown: &str) -> BTreeSet<String> {
    markdown
        .lines()
        .filter_map(|line| {
            line.strip_prefix("<!-- event: ")
                .and_then(|value| value.strip_suffix(" -->"))
                .map(ToOwned::to_owned)
        })
        .collect()
}

fn object_fields(text: &str) -> BTreeSet<String> {
    let mut fields = BTreeSet::new();
    for line in text.lines() {
        let mut remainder = line;
        while let Some(start) = remainder.find('"') {
            let rest = &remainder[start + 1..];
            let Some(end) = rest.find('"') else {
                break;
            };
            if rest[end + 1..].trim_start().starts_with(':') {
                fields.insert(rest[..end].to_owned());
            }
            remainder = &rest[end + 1..];
        }
        if let Some(start) = line.find(".insert(\"") {
            let rest = &line[start + ".insert(\"".len()..];
            if let Some(end) = rest.find('"') {
                fields.insert(rest[..end].to_owned());
            }
        }
    }
    fields
}

fn json_object_after(source: &str, start: usize) -> &str {
    let rest = &source[start..];
    let json = rest
        .find("json!({")
        .unwrap_or_else(|| unreachable!("event payload must use json object"));
    let object_start = start + json + "json!(".len();
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut escaped = false;
    for (offset, character) in source[object_start..].char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
            continue;
        }
        if character == '"' {
            quoted = true;
        } else if character == '{' {
            depth += 1;
        } else if character == '}' {
            depth -= 1;
            if depth == 0 {
                return &source[object_start..=object_start + offset];
            }
        }
    }
    unreachable!("unterminated event JSON object")
}

fn event_json_payloads<'a>(source: &'a str, event: &str) -> Vec<&'a str> {
    let marker = format!("\"{event}\"");
    let mut payloads = Vec::new();
    let mut offset = 0_usize;
    while let Some(relative) = source[offset..].find(&marker) {
        let start = offset + relative;
        let tail = &source[start..];
        let next_event = tail.find(".event(").filter(|value| *value > 0);
        let next_json = tail.find("json!({");
        if next_json.is_some() && next_event.is_none_or(|event| next_json < Some(event)) {
            payloads.push(json_object_after(source, start));
        }
        offset = start + marker.len();
    }
    payloads
}

fn struct_fields(source: &str, name: &str) -> BTreeSet<String> {
    let marker = format!("pub struct {name}");
    let start = source
        .find(&marker)
        .unwrap_or_else(|| unreachable!("missing struct {name}"));
    let rest = &source[start..];
    let end = rest
        .find("\n}")
        .unwrap_or_else(|| unreachable!("unterminated struct {name}"));
    rest[..end]
        .lines()
        .filter_map(|line| {
            let value = line.trim().strip_prefix("pub ")?;
            let (field, _) = value.split_once(':')?;
            Some(field.to_owned())
        })
        .collect()
}

fn documented_payload_fields(markdown: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut payloads = BTreeMap::new();
    for line in markdown.lines() {
        let Some(marker) = line.strip_prefix("<!-- payload: ") else {
            continue;
        };
        let Some((name, fields)) = marker.split_once(" fields: ") else {
            unreachable!("malformed payload marker: {line}");
        };
        let Some(fields) = fields.strip_suffix(" -->") else {
            unreachable!("unterminated payload marker: {line}");
        };
        let values = fields.split(',').map(ToOwned::to_owned).collect();
        assert!(payloads.insert(name.to_owned(), values).is_none());
    }
    payloads
}

#[test]
fn recipe_reference_matches_production_commands_and_allowed_fields() {
    let root = repository_root();
    let recipe_source = read(&root.join("clis/skill-manager/src/recipe.rs"));
    let reference = read(&root.join("skills/managing-skills/references/recipes.md"));
    let documented = documented_recipe_fields(&reference);
    assert_eq!(
        documented.keys().cloned().collect::<BTreeSet<_>>(),
        canonical_recipe_commands(&recipe_source)
    );
    assert_eq!(documented, source_recipe_fields(&recipe_source));
}

#[test]
fn event_reference_matches_production_emit_sites() {
    let root = repository_root();
    let mut production = emitted_events(&read(&root.join("clis/skill-manager/src/app.rs")));
    production.extend(emitted_events(&read(
        &root.join("clis/skill-manager/src/main.rs"),
    )));
    let documented = documented_events(&read(
        &root.join("skills/managing-skills/references/events.md"),
    ));
    assert_eq!(documented, production);
}

#[test]
fn event_payload_reference_matches_production_field_shapes_and_meanings() {
    let root = repository_root();
    let app = read(&root.join("clis/skill-manager/src/app.rs"));
    let status = read(&root.join("clis/skill-manager/src/status.rs"));
    let reference = read(&root.join("skills/managing-skills/references/events.md"));
    let documented = documented_payload_fields(&reference);

    let source = object_fields(function_block(&app, "source_data"));
    let target = object_fields(function_block(&app, "target_data"));
    let mut action = source.clone();
    action.extend(object_fields(function_block(&app, "skill_action_data")));
    let production = app.split("#[cfg(test)]").next().unwrap_or(&app);
    let removed = object_fields(event_json_payloads(production, "skill.removed")[0]);
    let status_row = object_fields(event_json_payloads(production, "status.row")[0]);
    let deployment = struct_fields(&status, "DeploymentDetail");

    assert_eq!(documented.get("source"), Some(&source));
    assert_eq!(documented.get("target"), Some(&target));
    assert_eq!(documented.get("skill-action"), Some(&action));
    assert_eq!(documented.get("skill-removed"), Some(&removed));
    assert_eq!(documented.get("status-row"), Some(&status_row));
    assert_eq!(documented.get("status-deployment"), Some(&deployment));

    for production_meaning in [
        "object.insert(\"path\".into(), json!(candidate.path))",
        "object.insert(\"destination\".into(), json!(destination))",
        "\"path\": destination",
        "path: target.target.path.join(&name)",
        "let source = candidate.map(|value| source_data(&value.source.entry))",
    ] {
        assert!(
            app.contains(production_meaning) || status.contains(production_meaning),
            "production payload meaning changed: {production_meaning}"
        );
    }
    for documented_meaning in [
        "source identity is not nested",
        "`path` is the discovered/materialized source skill",
        "and `destination` is\nthe destination skill directory",
        "For removal only, `path` is the destination skill directory",
        "deployment `path` is the resolved\ndeployed skill directory",
        "row-level `source` is the exact flattened source\nobject",
    ] {
        assert!(
            reference.contains(documented_meaning),
            "payload meaning is undocumented: {documented_meaning}"
        );
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "One source-derived contract keeps config and summary field schemas and meanings auditable together."
)]
fn config_and_summary_payload_references_match_production_emit_sites() {
    let root = repository_root();
    let app_source = read(&root.join("clis/skill-manager/src/app.rs"));
    let app = app_source
        .split("#[cfg(test)]")
        .next()
        .unwrap_or(&app_source);
    let reference = read(&root.join("skills/managing-skills/references/events.md"));
    let documented = documented_payload_fields(&reference);

    for (event, marker) in [
        ("config.shown", "config-shown"),
        ("config.migrated", "config-migrated"),
        ("config.reset", "config-reset"),
        ("config.restored", "config-restored"),
    ] {
        let payloads = event_json_payloads(app, event);
        assert_eq!(
            payloads.len(),
            1,
            "expected one production {event} emit site"
        );
        assert_eq!(
            documented.get(marker),
            Some(&object_fields(payloads[0])),
            "{event} payload fields drifted"
        );
    }
    let config_targets = app
        .find("let targets = global")
        .unwrap_or_else(|| unreachable!("config target construction"));
    assert_eq!(
        documented.get("config-target"),
        Some(&object_fields(json_object_after(app, config_targets))),
        "config.shown resolved-target fields drifted"
    );
    assert_eq!(
        documented.get("config-backup"),
        Some(&object_fields(function_block(app, "backup_data"))),
        "config.shown backup fields drifted"
    );

    let summaries = event_json_payloads(app, "summary");
    assert_eq!(summaries.len(), 8, "production summary emit-site count");
    let summary_fields = summaries
        .iter()
        .map(|payload| object_fields(payload))
        .collect::<Vec<_>>();
    for (marker, expected_count) in [
        ("summary-source-list", 1),
        ("summary-load-update", 1),
        ("summary-import", 1),
        ("summary-copy", 1),
        ("summary-remove", 2),
        ("summary-status", 1),
        ("summary-resolve", 1),
    ] {
        let expected = documented
            .get(marker)
            .unwrap_or_else(|| unreachable!("missing payload marker {marker}"));
        assert_eq!(
            summary_fields
                .iter()
                .filter(|fields| *fields == expected)
                .count(),
            expected_count,
            "{marker} exact field schema drifted"
        );
    }

    for production_meaning in [
        "\"path\": self.repository.config_path()",
        "\"storage_root\": self.repository.storage_root()",
        "\"home\": self.home",
        "\"project_root\": project_root",
        "\"persisted\": persisted",
        "\"config\": config",
        "\"targets\": targets",
        "\"backups\": backup_values",
        "\"template\": global_target.template",
        "\"global_path\": global_target.target.path",
        "\"project_path\": project_target.target.path",
        "\"raw_path\": backup.raw_path",
        "\"component\": item.component",
        "\"from\": item.from",
        "\"to\": item.to",
        "\"backup_id\": backup.metadata.id",
        "\"backup_path\": backup.raw_path",
        "\"displaced_backup_id\": outcome.displaced.metadata.id",
        "\"displaced_backup_path\": outcome.displaced.raw_path",
        "\"present\": outcome.restored.metadata.present",
        "\"sources\": config.sources.len()",
        "\"action\": if update_only { \"update\" } else { \"load\" }",
        "\"action\": \"copy\", \"copied\": copied",
        "\"action\": \"remove\", \"removed\": 0",
        "\"action\": \"remove\", \"removed\": removed",
        "\"action\": \"status\", \"skills\": status_rows.len()",
        "\"action\": \"resolve\", \"resolved\": resolved_count",
    ] {
        assert!(
            app.contains(production_meaning),
            "production config/summary meaning changed: {production_meaning}"
        );
    }
    for documented_meaning in [
        "`home` is the manager's global-scope root",
        "and `project_root` is the exact\ncurrent working directory",
        "`path` is the active configuration path",
        "`backup_path` or `displaced_backup_path` is the\nraw archived-byte path",
        "Other source, target, and configuration lifecycle commands finish",
    ] {
        assert!(
            reference.contains(documented_meaning),
            "config/summary meaning is undocumented: {documented_meaning}"
        );
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "One completeness contract maps every production event to its exact source-derived payload family."
)]
fn every_production_event_has_a_source_derived_payload_family() {
    let root = repository_root();
    let app_source = read(&root.join("clis/skill-manager/src/app.rs"));
    let app = app_source
        .split("#[cfg(test)]")
        .next()
        .unwrap_or(&app_source);
    let main = read(&root.join("clis/skill-manager/src/main.rs"));
    let reference = read(&root.join("skills/managing-skills/references/events.md"));
    let documented = documented_payload_fields(&reference);

    let source = object_fields(function_block(app, "source_data"));
    assert_eq!(
        documented.get("source-location"),
        Some(&object_fields(function_block(app, "location_data")))
    );
    assert_eq!(
        documented.get("source-previous"),
        Some(&object_fields(function_block(app, "source_snapshot")))
    );
    let mut source_change = source;
    source_change.extend(object_fields(function_block(app, "source_change_data")));
    assert_eq!(documented.get("source-change"), Some(&source_change));

    for (event, marker, count) in [
        ("target.removed", "target-removed", 1),
        ("collision.detected", "collision-detected", 1),
        ("collision.resolved", "collision-resolved", 1),
        ("command.cancelled", "command-cancelled", 1),
    ] {
        let payloads = event_json_payloads(app, event);
        assert_eq!(payloads.len(), count, "{event} emit-site count");
        for payload in payloads {
            assert_eq!(
                documented.get(marker),
                Some(&object_fields(payload)),
                "{event} payload drifted"
            );
        }
    }
    let diagnostics = event_json_payloads(app, "diagnostic");
    assert_eq!(diagnostics.len(), 2, "diagnostic variants");
    let diagnostic_fields = diagnostics
        .iter()
        .map(|payload| object_fields(payload))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        diagnostic_fields,
        BTreeSet::from([
            documented["diagnostic-message"].clone(),
            documented["diagnostic-pattern"].clone(),
        ])
    );
    let failures = event_json_payloads(&main, "command.failed");
    assert_eq!(failures.len(), 1);
    assert_eq!(
        documented.get("command-failed"),
        Some(&object_fields(failures[0]))
    );

    let family_by_event = BTreeMap::from([
        ("collision.detected", "collision-detected"),
        ("collision.resolved", "collision-resolved"),
        ("command.cancelled", "command-cancelled"),
        ("command.failed", "command-failed"),
        ("config.migrated", "config-migrated"),
        ("config.reset", "config-reset"),
        ("config.restored", "config-restored"),
        ("config.shown", "config-shown"),
        ("diagnostic", "diagnostic-message"),
        ("skill.copied", "skill-action"),
        ("skill.import-planned", "skill-import"),
        ("skill.import-skipped", "skill-import-skipped"),
        ("skill.imported", "skill-import"),
        ("skill.loaded", "skill-action"),
        ("skill.removed", "skill-removed"),
        ("skill.skipped", "skill-action"),
        ("skill.updated", "skill-action"),
        ("source.added", "source"),
        ("source.alternate-cleared", "source-change"),
        ("source.alternate-set", "source-change"),
        ("source.listed", "source"),
        ("source.location-set", "source-change"),
        ("source.locations-swapped", "source-change"),
        ("source.removed", "source"),
        ("source.updated", "source-change"),
        ("status.row", "status-row"),
        ("summary", "summary-load-update"),
        ("target.added", "target"),
        ("target.disabled", "target"),
        ("target.enabled", "target"),
        ("target.listed", "target"),
        ("target.path-set", "target"),
        ("target.removed", "target-removed"),
    ]);
    let mut production = emitted_events(app);
    production.extend(emitted_events(&main));
    assert_eq!(
        family_by_event.keys().copied().collect::<BTreeSet<_>>(),
        production
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        "every production event must be assigned a validated payload family"
    );
    for family in family_by_event.values() {
        assert!(
            documented.contains_key(*family),
            "payload family marker missing: {family}"
        );
    }
}

fn markdown_links(markdown: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut remainder = markdown;
    while let Some(start) = remainder.find("](") {
        remainder = &remainder[start + 2..];
        let Some(end) = remainder.find(')') else {
            break;
        };
        links.push(remainder[..end].trim_matches(['<', '>']).to_owned());
        remainder = &remainder[end + 1..];
    }
    links
}

#[test]
fn onboarding_markdown_links_resolve() {
    let root = repository_root();
    for relative in [
        "README.md",
        "cheatsheet.skill-manager.md",
        "install.skill-manager.md",
        "clis/skill-manager/README.md",
        "skills/managing-skills/SKILL.md",
        "skills/managing-skills/references/recipes.md",
        "skills/managing-skills/references/events.md",
        "skills/managing-skills/references/workflows.md",
    ] {
        let path = root.join(relative);
        let parent = path
            .parent()
            .unwrap_or_else(|| unreachable!("markdown path must have a parent"));
        for link in markdown_links(&read(&path)) {
            let local = link.split('#').next().unwrap_or_default();
            if local.is_empty()
                || local.starts_with("http://")
                || local.starts_with("https://")
                || local.starts_with("mailto:")
            {
                continue;
            }
            assert!(
                parent.join(local).exists(),
                "broken link {link:?} in {}",
                path.display()
            );
        }
    }
}

#[test]
fn root_catalog_covers_every_skill_directory() {
    let root = repository_root();
    let readme = read(&root.join("README.md"));
    let mut directories = fs::read_dir(root.join("skills"))
        .unwrap_or_else(|error| unreachable!("skills directory: {error}"))
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    directories.sort();
    for name in directories {
        let link = format!("(./skills/{name}/SKILL.md)");
        assert!(readme.contains(&link), "README catalog is missing {name}");
    }
}

#[test]
fn managing_skill_has_required_metadata_and_current_storage_claims() {
    let root = repository_root();
    let skill = read(&root.join("skills/managing-skills/SKILL.md"));
    assert!(skill.starts_with("---\nname: managing-skills\ndescription: "));
    assert!(!skill.contains("TODO"));
    for bootstrap_guardrail in [
        "Establish initial context with `source.list` and `target.list`",
        "Treat that as an expected absence signal only when the parsed",
        "Every other exit-1 message",
    ] {
        assert!(
            skill.contains(bootstrap_guardrail),
            "missing bootstrap failure guardrail: {bootstrap_guardrail}"
        );
    }
    let metadata = read(&root.join("skills/managing-skills/agents/openai.yaml"));
    for required in [
        "display_name: \"Manage Skills\"",
        "short_description: \"Manage agent skills with skill-manager\"",
        "default_prompt: \"Use $managing-skills to manage my installed agent skills.\"",
    ] {
        assert!(
            metadata.contains(required),
            "missing OpenAI metadata: {required}"
        );
    }

    let cli_readme = read(&root.join("clis/skill-manager/README.md"));
    assert!(cli_readme.contains("`~/.skill-manager/`"));
    for stale_claim in [
        "The active configuration file is `~/.skill-manager.config.json`",
        "Remote cache content is under `~/.skill-manager-cache`",
        "without changing configuration, cache, targets, backups, or lock state",
    ] {
        assert!(
            !cli_readme.contains(stale_claim),
            "stale CLI README claim remains: {stale_claim}"
        );
    }
}

#[test]
fn machine_requirements_and_all_target_semantics_match_production() {
    let root = repository_root();
    let app = read(&root.join("clis/skill-manager/src/app.rs"));
    for production_contract in [
        "target selection is required in noninteractive mode; pass --all or --target",
        "selection.all_targets && target.target.enabled",
        "source name is required in noninteractive mode; pass NAME or --name",
        "target '{requested}' is disabled; use --target {requested} to override",
    ] {
        assert!(
            app.contains(production_contract),
            "production machine requirement changed: {production_contract}"
        );
    }

    let recipes = read(&root.join("skills/managing-skills/references/recipes.md"));
    let cheatsheet = read(&root.join("cheatsheet.skill-manager.md"));
    let skill = read(&root.join("skills/managing-skills/SKILL.md"));
    for required in [
        "A committed non-interactive call must also explicitly select at\nleast one target",
        "Machine/non-interactive use\nrequires an explicit nonblank `name`",
        "`all_targets:true` selects enabled\n  configured targets only",
        "A disabled target requires explicit selection",
    ] {
        assert!(
            recipes.contains(required),
            "recipe machine requirement is missing: {required}"
        );
    }
    for required in [
        "A committed `load` or\n`update` must explicitly select at least one target",
        "`all_targets:true` selects\nenabled configured targets only",
        "A machine `source.add` must include a\nnonblank `name`",
    ] {
        assert!(
            skill.contains(required),
            "managing skill machine requirement is missing: {required}"
        );
    }
    for required in [
        "committed non-interactive `load` or `update` must explicitly select targets",
        "Machine/non-interactive `source add` requires an explicit nonblank",
        "`--all` never opts into a disabled\ntarget",
    ] {
        assert!(
            cheatsheet.contains(required),
            "cheatsheet machine requirement is missing: {required}"
        );
    }
}

#[test]
fn installer_asset_mapping_covers_the_release_matrix() {
    let root = repository_root();
    let matrix = read(&root.join("tools/build-matrix-contract.ps1"));
    let marker = "function Get-CanonicalFullBuildTargets";
    let start = matrix
        .find(marker)
        .unwrap_or_else(|| unreachable!("canonical build matrix function"));
    let remainder = &matrix[start + marker.len()..];
    let end = remainder.find("\nfunction ").unwrap_or(remainder.len());
    let block = &remainder[..end];
    let targets = block
        .lines()
        .filter_map(|line| {
            let marker = "target='";
            let start = line.find(marker)?;
            let value = &line[start + marker.len()..];
            let end = value.find('\'')?;
            Some(value[..end].to_owned())
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(targets.len(), 8, "canonical release matrix target count");

    let installer = read(&root.join("install.skill-manager.md"));
    for target in &targets {
        assert!(
            installer.contains(target),
            "installer does not map release target {target}"
        );
    }
    for required in [
        "skill-manager-v$($Latest.Text)-$Target.zip",
        "asset_name=\"skill-manager-v${latest}-${target}.tar.gz\"",
        "SHA256SUMS must contain exactly one checksum",
        "%LOCALAPPDATA%\\skill-manager\\bin",
        "$HOME/.local/share/skill-manager/bin",
        "'schema_version', 'repository', 'tag', 'version', 'asset', 'sha256', 'installed_at'",
        "\"schema_version\", \"repository\", \"tag\", \"version\",\n    \"asset\", \"sha256\", \"installed_at\"",
        "Assert-ExactObjectFields $Event @('version', 'event', 'level', 'data')",
        "set(event) != {\"version\", \"event\", \"level\", \"data\"}",
        "Join-Path ([string]$Deployment[0].path) 'SKILL.md'",
        "pathlib.Path(matches[0][\"path\"]) / \"SKILL.md\"",
        "require_command python3",
        "installed_at must be an RFC 3339 timestamp with an offset",
        "installed_at must be RFC 3339 with an offset",
        "$ManagedBinaryItem = Get-ManagedPathItem $ManagedBinary",
        "Assert-OrdinaryFile $ManagedBinaryItem 'Managed binary'",
        "($Item.Attributes -band [IO.FileAttributes]::ReparsePoint)",
        "[ -f \"$1\" ] && [ ! -L \"$1\" ]",
        "path_present \"$managed_binary\" && binary_present=true",
        "targets = $ExpectedTargets",
        "\\\"targets\\\":$targets_json",
        "Explicit name selection deliberately installs to a disabled built-in target",
        "Reset-InstallerFilePair $StagedBinary $StagedProvenance",
        "Reset-InstallerFilePair $RollbackBinary $RollbackProvenance",
        "reset_file_pair \"$staged_binary\" \"$staged_provenance\"",
        "reset_file_pair \"$rollback_binary\" \"$rollback_provenance\"",
        "A system-level skill-manager precedes User PATH",
        "source.locations-swapped",
        "action-event set does not equal",
    ] {
        assert!(
            installer.contains(required),
            "installer is missing release/install contract: {required}"
        );
    }
    for forbidden in [
        "installed_at_utc",
        "Join-Path ([string]$Deployment[0].path) 'managing-skills",
        "pathlib.Path(matches[0][\"path\"]) / \"managing-skills\"",
        "$TargetFields",
        "$targets,\"global\"",
    ] {
        assert!(
            !installer.contains(forbidden),
            "installer contains stale or reconstructed-path logic: {forbidden}"
        );
    }
}
