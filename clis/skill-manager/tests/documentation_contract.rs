//! Source-derived contracts for user and agent documentation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[cfg(any(unix, windows))]
mod support;

#[cfg(any(unix, windows))]
use support::portable_canonicalize;

/// Resolve a scratch root to the spelling a child process observes.
///
/// Shells report the physical, long-form directory (`(Get-Location).Path` on
/// Windows, `pwd -L` falling back to `getcwd` on POSIX), while `tempfile`
/// hands back whatever `TMPDIR`/`TEMP` spelled — an 8.3 short path on Windows
/// or `/var` instead of `/private/var` on macOS.  Expected paths must be
/// derived from the canonical spelling so the two agree.
#[cfg(any(unix, windows))]
fn canonical_scratch_root(path: &Path) -> PathBuf {
    portable_canonicalize(path)
        .unwrap_or_else(|error| unreachable!("canonicalize installer test root: {error}"))
}

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

fn normalized_prose(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
        ("load", "overlay_load"),
        ("update", "overlay_update"),
        ("import", "overlay_import"),
        ("copy", "overlay_copy"),
        ("remove", "overlay_remove"),
        ("status", "overlay_status"),
        ("resolve", "overlay_resolve"),
        ("configs.copy", "overlay_configs_copy"),
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
    let target_add = allowed_fields(arm_block(
        target_block,
        "TargetAction::Add",
        Some("TargetAction::SetPath"),
    ));
    let target_path = allowed_fields(arm_block(
        target_block,
        "TargetAction::SetPath",
        Some("TargetAction::Enable"),
    ));
    let target_name = allowed_fields(arm_block(target_block, "TargetAction::Enable", None));
    recipes.insert("target.list".into(), target_list);
    recipes.insert("target.add".into(), target_add);
    recipes.insert("target.set-path".into(), target_path);
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
        || value == "plan"
        || [
            "collision.",
            "command.",
            "config.",
            "configs.copy.",
            "describe.",
            "plan.",
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

/// Top-level keys inserted into one named map variable.
///
/// Payloads assembled with `serde_json::Map` rather than `json!` need their own
/// extractor so nested maps built in the same function stay out of the family.
fn insert_fields(block: &str, variable: &str) -> BTreeSet<String> {
    let marker = format!("{variable}.insert(\"");
    block
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix(&marker)?;
            let (field, _) = rest.split_once('"')?;
            Some(field.to_owned())
        })
        .collect()
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

    let review = read(&root.join("clis/skill-manager/src/review.rs"));
    assert_eq!(
        documented.get("plan"),
        Some(&insert_fields(
            function_block(&review, "plan_event_data"),
            "data"
        )),
        "plan payload fields drifted"
    );

    let summaries = event_json_payloads(app, "summary");
    assert_eq!(summaries.len(), 9, "production summary emit-site count");
    let summary_fields = summaries
        .iter()
        .map(|payload| object_fields(payload))
        .collect::<Vec<_>>();
    for (marker, expected_count) in [
        ("summary-source-list", 1),
        ("summary-load-update", 1),
        ("summary-import", 1),
        ("summary-copy", 1),
        ("summary-remove", 1),
        ("summary-status", 1),
        ("summary-resolve", 1),
        ("summary-describe", 1),
        ("summary-configs-copy", 1),
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
        "\"action\": action,",
        "self.report_sync_summary(\"load\", changed, skipped, run.args.dry_run)",
        "self.report_sync_summary(\"update\", changed, skipped, run.args.dry_run)",
        "\"action\": \"copy\", \"copied\": copied",
        "self.report_remove_summary(0, args.dry_run)",
        "self.report_remove_summary(0, true)",
        "self.report_remove_summary(removed, args.dry_run)",
        "\"action\": \"status\", \"skills\": status_rows.len()",
        "\"action\": \"resolve\", \"resolved\": resolved_count",
        "\"action\": \"configs.copy\",",
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
    assert_eq!(diagnostics.len(), 3, "diagnostic variants");
    let diagnostic_fields = diagnostics
        .iter()
        .map(|payload| object_fields(payload))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        diagnostic_fields,
        BTreeSet::from([
            documented["diagnostic-message"].clone(),
            documented["diagnostic-pattern"].clone(),
            documented["diagnostic-ambiguous-argument-roles"].clone(),
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
        ("configs.copy.item", "configs-copy-item"),
        ("diagnostic", "diagnostic-message"),
        ("describe.skill", "describe-skill"),
        ("describe.source", "describe-source"),
        ("plan", "plan"),
        ("plan.updated", "plan"),
        ("skill.copied", "skill-action"),
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

fn tracked_markdown_files(root: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["ls-files", "-z", "--", "*.md"])
        .output()
        .unwrap_or_else(|error| unreachable!("run git ls-files for Markdown contract: {error}"));
    assert!(
        output.status.success(),
        "git ls-files failed while collecting tracked Markdown:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let files = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            let relative = std::str::from_utf8(path).unwrap_or_else(|error| {
                unreachable!("tracked Markdown path is not UTF-8: {error}")
            });
            root.join(relative)
        })
        // A tracked deletion remains in the index until it is staged. It is no
        // longer repository documentation and has no worktree content to scan.
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    assert!(
        !files.is_empty(),
        "git ls-files returned no tracked Markdown for {}",
        root.display()
    );
    files
}

#[derive(Debug)]
struct MarkdownFence {
    marker: char,
    length: usize,
    language: String,
}

fn fence_candidate(line: &str) -> Option<&str> {
    let indentation = line
        .chars()
        .take_while(|character| *character == ' ')
        .count();
    (indentation <= 3).then(|| &line[indentation..])
}

fn opening_fence(line: &str) -> Option<MarkdownFence> {
    let candidate = fence_candidate(line)?;
    let marker = candidate.chars().next()?;
    if !['`', '~'].contains(&marker) {
        return None;
    }
    let length = candidate
        .chars()
        .take_while(|character| *character == marker)
        .count();
    if length < 3 {
        return None;
    }
    let info = &candidate[length..];
    if marker == '`' && info.contains('`') {
        return None;
    }
    let language = info
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    Some(MarkdownFence {
        marker,
        length,
        language,
    })
}

fn closes_fence(line: &str, fence: &MarkdownFence) -> bool {
    let Some(candidate) = fence_candidate(line) else {
        return false;
    };
    let length = candidate
        .chars()
        .take_while(|character| *character == fence.marker)
        .count();
    length >= fence.length && candidate[length..].trim().is_empty()
}

fn command_fence_violations(markdown: &str) -> Vec<(usize, &'static str)> {
    let mut violations = Vec::new();
    let mut fence: Option<MarkdownFence> = None;
    for (index, line) in markdown.lines().enumerate() {
        if let Some(active) = &fence {
            if closes_fence(line, active) {
                fence = None;
                continue;
            }
            let command_fence = [
                "console",
                "shell",
                "sh",
                "bash",
                "zsh",
                "powershell",
                "pwsh",
            ]
            .contains(&active.language.as_str());
            if command_fence && line.trim_start().starts_with("$ ") {
                violations.push((index + 1, "leading `$ ` prompt marker"));
            }
            if ["powershell", "pwsh"].contains(&active.language.as_str()) {
                let lowercase = line.trim_start().to_ascii_lowercase();
                if [
                    "powershell -c ",
                    "powershell -command ",
                    "powershell.exe -c ",
                    "powershell.exe -command ",
                    "pwsh -c ",
                    "pwsh -command ",
                    "pwsh.exe -c ",
                    "pwsh.exe -command ",
                ]
                .iter()
                .any(|prefix| lowercase.starts_with(prefix))
                {
                    violations.push((index + 1, "redundant PowerShell command wrapper"));
                }
            }
        } else {
            fence = opening_fence(line);
        }
    }
    violations
}

#[test]
fn markdown_command_examples_are_directly_pasteable() {
    let root = repository_root();
    for path in tracked_markdown_files(&root) {
        if let Some((line, violation)) = command_fence_violations(&read(&path)).first() {
            unreachable!("{violation} at {}:{line}", path.display());
        }
    }
}

#[test]
fn markdown_command_fence_scanner_handles_commonmark_delimiters() {
    let markdown = r#"
````console copy=true
```
$ prefixed
`````

~~~~powershell title=setup
pwsh -Command "Get-Thing"
~~~~~

    ```console
    $ indented-code-block-is-not-a-fence
    ```
"#;
    assert_eq!(
        command_fence_violations(markdown),
        vec![
            (4, "leading `$ ` prompt marker"),
            (8, "redundant PowerShell command wrapper"),
        ]
    );
}

#[test]
fn onboarding_markdown_links_resolve() {
    let root = repository_root();
    for relative in [
        "README.md",
        "cheatsheet.skill-manager.md",
        "docs/agent-usage.md",
        "clis/skill-manager/README.md",
        "skills/managing-skills/SKILL.md",
        "skills/managing-skills/references/recipes.md",
        "skills/managing-skills/references/events.md",
        "skills/managing-skills/references/reporting.md",
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

fn clear_installer_environment(command: &mut Command) {
    for name in [
        "SKILL_MANAGER_VERSION",
        "SKILL_MANAGER_INSTALL_DIR",
        "SKILL_MANAGER_INSTALL_YES",
        "SKILL_MANAGER_INSTALL_FORCE",
        "SKILL_MANAGER_NO_MODIFY_PATH",
        "SKILL_MANAGER_TEST_RESOLVE_DIR",
        "SKILL_MANAGER_TEST_FORCE_INTERACTIVE",
        "SKILL_MANAGER_TEST_PATH_ENTRY",
    ] {
        command.env_remove(name);
    }
}

fn run_resolver(mut command: Command, stdin: Option<&str>) -> Output {
    if let Some(input) = stdin {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| unreachable!("start installer resolver process: {error}"));
        child
            .stdin
            .take()
            .unwrap_or_else(|| unreachable!("resolver stdin must be piped"))
            .write_all(input.as_bytes())
            .unwrap_or_else(|error| unreachable!("write resolver stdin: {error}"));
        child
            .wait_with_output()
            .unwrap_or_else(|error| unreachable!("wait for installer resolver process: {error}"))
    } else {
        command
            .output()
            .unwrap_or_else(|error| unreachable!("run installer resolver process: {error}"))
    }
}

fn assert_resolver_output(command: Command, expected: &Path) {
    let output = run_resolver(command, None);
    assert!(
        output.status.success(),
        "resolver failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|error| unreachable!("resolver output is UTF-8: {error}"));
    let normalized_stdout = stdout.replace("\r\n", "\n");
    assert_eq!(
        normalized_stdout,
        format!("{}\n", expected.display()),
        "resolver mode must print only the absolute destination"
    );
    assert!(
        output.stderr.is_empty(),
        "resolver mode wrote stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_resolver_rejects_empty_directory(command: Command, conflicting: &Path) {
    let output = run_resolver(command, None);
    assert!(
        !output.status.success(),
        "resolver accepted an explicitly empty install directory"
    );
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    assert!(
        !stdout.lines().any(|line| Path::new(line).is_absolute()),
        "resolver emitted a resolved destination: {stdout:?}"
    );
    assert!(
        !stdout.contains(conflicting.to_string_lossy().as_ref()),
        "resolver emitted the conflicting environment destination: {stdout:?}",
    );
    assert!(
        format!("{stdout}{stderr}").contains("install directory must not be empty"),
        "resolver did not report the empty-directory error: stdout={stdout:?}, stderr={stderr:?}"
    );
}

fn assert_prompted_resolver_output(command: Command, stdin: &str, expected: &Path) {
    let output = run_resolver(command, Some(stdin));
    assert!(
        output.status.success(),
        "prompted resolver failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|error| unreachable!("resolver output is UTF-8: {error}"))
        .replace("\r\n", "\n");
    assert!(
        stdout.ends_with(&format!("{}\n", expected.display())),
        "prompted resolver did not end with destination {}: {stdout:?}",
        expected.display()
    );
    let transcript = format!("{stdout}{}", String::from_utf8_lossy(&output.stderr));
    let echoed_input = stdout
        .lines()
        .next()
        .is_some_and(|line| line == stdin.trim_end_matches(['\r', '\n']));
    assert!(
        transcript.contains("Install directory [") || echoed_input,
        "prompted resolver neither rendered its prompt nor consumed stdin through the host prompt reader: {transcript:?}"
    );
}

#[cfg(unix)]
fn assert_resolver_text(command: Command, expected: &str) {
    let output = run_resolver(command, None);
    assert!(
        output.status.success(),
        "resolver failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

#[cfg(unix)]
fn posix_resolver_command(
    script: &Path,
    cwd: &Path,
    home: &Path,
    local_app_data: &Path,
) -> Command {
    let mut command = Command::new("sh");
    command
        .arg(script)
        .current_dir(cwd)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("LOCALAPPDATA", local_app_data)
        .env("TMPDIR", cwd.join("tmp"))
        .env("SKILL_MANAGER_TEST_RESOLVE_DIR", "1");
    clear_installer_environment(&mut command);
    command.env("SKILL_MANAGER_TEST_RESOLVE_DIR", "1");
    command
}

#[cfg(unix)]
#[test]
fn posix_installer_resolves_destination_spellings_without_side_effects() {
    if !Command::new("sh")
        .arg("-c")
        .arg("exit 0")
        .status()
        .is_ok_and(|status| status.success())
    {
        eprintln!("skipping: sh is unavailable");
        return;
    }

    let root = repository_root();
    let scratch = tempfile::tempdir()
        .unwrap_or_else(|error| unreachable!("isolated installer test root: {error}"));
    let scratch_root = canonical_scratch_root(scratch.path());
    let cwd = scratch_root.join("cwd");
    let home = scratch_root.join("home");
    let local_app_data = scratch_root.join("local");
    fs::create_dir_all(&cwd)
        .unwrap_or_else(|error| unreachable!("create invocation directory: {error}"));
    fs::create_dir_all(&home)
        .unwrap_or_else(|error| unreachable!("create synthetic home: {error}"));
    fs::create_dir_all(cwd.join("tmp"))
        .unwrap_or_else(|error| unreachable!("create synthetic temporary directory: {error}"));
    let script = root.join("clis/skill-manager/install.sh");

    let mut dot = posix_resolver_command(&script, &cwd, &home, &local_app_data);
    dot.args(["--dir", "."]);
    assert_resolver_output(dot, &cwd);

    let mut nested = posix_resolver_command(&script, &cwd, &home, &local_app_data);
    nested.args(["--dir", "nested/./missing/../leaf"]);
    assert_resolver_output(nested, &cwd.join("nested/leaf"));

    let mut home_exact = posix_resolver_command(&script, &cwd, &home, &local_app_data);
    home_exact.env("SKILL_MANAGER_INSTALL_DIR", "~");
    assert_resolver_output(home_exact, &home);

    let default_dest = home.join(".local/bin");
    assert_resolver_output(
        posix_resolver_command(&script, &cwd, &home, &local_app_data),
        &default_dest,
    );

    let mut prompted = posix_resolver_command(&script, &cwd, &home, &local_app_data);
    prompted.env("SKILL_MANAGER_TEST_FORCE_INTERACTIVE", "1");
    assert_prompted_resolver_output(prompted, "~/tools/../bin\n", &home.join("bin"));

    let mut empty_prompt = posix_resolver_command(&script, &cwd, &home, &local_app_data);
    empty_prompt.env("SKILL_MANAGER_TEST_FORCE_INTERACTIVE", "1");
    assert_prompted_resolver_output(empty_prompt, "\n", &default_dest);

    let dirty_absolute = format!("{}/absolute//./deep/../leaf", cwd.display());
    let mut dirty = posix_resolver_command(&script, &cwd, &home, &local_app_data);
    dirty.env("SKILL_MANAGER_INSTALL_DIR", dirty_absolute);
    assert_resolver_output(dirty, &cwd.join("absolute/leaf"));

    let mut precedence = posix_resolver_command(&script, &cwd, &home, &local_app_data);
    precedence
        .args(["--dir", "argument/../winner"])
        .env("SKILL_MANAGER_INSTALL_DIR", "environment")
        .env("SKILL_MANAGER_TEST_FORCE_INTERACTIVE", "1");
    assert_resolver_output(precedence, &cwd.join("winner"));

    let conflicting = cwd.join("environment");
    let mut empty_argument = posix_resolver_command(&script, &cwd, &home, &local_app_data);
    empty_argument
        .args(["--dir", ""])
        .env("SKILL_MANAGER_INSTALL_DIR", &conflicting);
    assert_resolver_rejects_empty_directory(empty_argument, &conflicting);

    let mut environment_wins = posix_resolver_command(&script, &cwd, &home, &local_app_data);
    environment_wins
        .env("SKILL_MANAGER_INSTALL_DIR", "environment/../winner")
        .env("SKILL_MANAGER_TEST_FORCE_INTERACTIVE", "1");
    assert_resolver_output(environment_wins, &cwd.join("winner"));

    let mut literal_tilde = posix_resolver_command(&script, &cwd, &home, &local_app_data);
    literal_tilde.env("SKILL_MANAGER_TEST_PATH_ENTRY", "~/.local/bin");
    assert_resolver_text(literal_tilde, "no-match\n");

    let mut lexical_match = posix_resolver_command(&script, &cwd, &home, &local_app_data);
    lexical_match.args(["--dir", "missing/bin"]).env(
        "SKILL_MANAGER_TEST_PATH_ENTRY",
        format!("{}/missing/./tools/../bin", cwd.display()),
    );
    assert_resolver_text(lexical_match, "match\n");

    let physical = scratch_root.join("physical/target");
    fs::create_dir_all(&physical)
        .unwrap_or_else(|error| unreachable!("create symlink target: {error}"));
    let alias = cwd.join("alias");
    if std::os::unix::fs::symlink(&physical, &alias).is_ok() {
        let mut lexical = posix_resolver_command(&script, &cwd, &home, &local_app_data);
        lexical.args(["--dir", "alias/../lexical"]);
        assert_resolver_output(lexical, &cwd.join("lexical"));

        if Command::new("realpath")
            .arg(&alias)
            .status()
            .is_ok_and(|status| status.success())
        {
            let mut canonical = posix_resolver_command(&script, &cwd, &home, &local_app_data);
            canonical
                .args(["--dir", physical.to_string_lossy().as_ref()])
                .env("SKILL_MANAGER_TEST_PATH_ENTRY", &alias);
            assert_resolver_text(canonical, "match\n");
        }
    }
}

#[cfg(windows)]
fn windows_resolver_command(
    script: &Path,
    cwd: &Path,
    home: &Path,
    local_app_data: &Path,
) -> Command {
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .current_dir(cwd)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("LOCALAPPDATA", local_app_data)
        .env("TEMP", cwd.join("temp"))
        .env("TMP", cwd.join("temp"));
    clear_installer_environment(&mut command);
    command.env("SKILL_MANAGER_TEST_RESOLVE_DIR", "1");
    command
}

#[cfg(windows)]
#[test]
fn windows_installer_resolves_destination_spellings_without_side_effects() {
    let root = repository_root();
    let scratch = tempfile::tempdir()
        .unwrap_or_else(|error| unreachable!("isolated installer test root: {error}"));
    let scratch_root = canonical_scratch_root(scratch.path());
    let cwd = scratch_root.join("cwd");
    let home = scratch_root.join("home");
    let local_app_data = scratch_root.join("local");
    fs::create_dir_all(cwd.join("temp"))
        .unwrap_or_else(|error| unreachable!("create invocation and temporary directory: {error}"));
    fs::create_dir_all(&home)
        .unwrap_or_else(|error| unreachable!("create synthetic home: {error}"));
    let script = root.join("clis/skill-manager/install.ps1");

    let mut dot = windows_resolver_command(&script, &cwd, &home, &local_app_data);
    dot.args(["-Dir", "."]);
    assert_resolver_output(dot, &cwd);

    let mut nested = windows_resolver_command(&script, &cwd, &home, &local_app_data);
    nested.args(["-Dir", r"nested\.\missing\..\leaf"]);
    assert_resolver_output(nested, &cwd.join(r"nested\leaf"));

    let mut home_exact = windows_resolver_command(&script, &cwd, &home, &local_app_data);
    home_exact.env("SKILL_MANAGER_INSTALL_DIR", "~");
    assert_resolver_output(home_exact, &home);

    let default_dest = local_app_data.join(r"Programs\skill-manager");
    assert_resolver_output(
        windows_resolver_command(&script, &cwd, &home, &local_app_data),
        &default_dest,
    );

    let mut prompted = windows_resolver_command(&script, &cwd, &home, &local_app_data);
    prompted.env("SKILL_MANAGER_TEST_FORCE_INTERACTIVE", "1");
    assert_prompted_resolver_output(prompted, "~/tools/../bin\n", &home.join("bin"));

    let mut empty_prompt = windows_resolver_command(&script, &cwd, &home, &local_app_data);
    empty_prompt.env("SKILL_MANAGER_TEST_FORCE_INTERACTIVE", "1");
    assert_prompted_resolver_output(empty_prompt, "\n", &default_dest);

    let dirty_absolute = format!(r"{}\absolute\\.\deep\..\leaf", cwd.display());
    let mut dirty = windows_resolver_command(&script, &cwd, &home, &local_app_data);
    dirty.env("SKILL_MANAGER_INSTALL_DIR", dirty_absolute);
    assert_resolver_output(dirty, &cwd.join(r"absolute\leaf"));

    let mut unc = windows_resolver_command(&script, &cwd, &home, &local_app_data);
    unc.args(["-Dir", r"\\server\share\tools\..\bin"]);
    assert_resolver_output(unc, Path::new(r"\\server\share\bin"));

    let mut precedence = windows_resolver_command(&script, &cwd, &home, &local_app_data);
    precedence
        .args(["-Dir", r"argument\..\winner"])
        .env("SKILL_MANAGER_INSTALL_DIR", "environment")
        .env("SKILL_MANAGER_TEST_FORCE_INTERACTIVE", "1");
    assert_resolver_output(precedence, &cwd.join("winner"));

    let conflicting = cwd.join("environment");
    let mut empty_argument = windows_resolver_command(&script, &cwd, &home, &local_app_data);
    empty_argument
        .args(["-Dir", ""])
        .env("SKILL_MANAGER_INSTALL_DIR", &conflicting);
    assert_resolver_rejects_empty_directory(empty_argument, &conflicting);

    let mut environment_wins = windows_resolver_command(&script, &cwd, &home, &local_app_data);
    environment_wins
        .env("SKILL_MANAGER_INSTALL_DIR", r"environment\..\winner")
        .env("SKILL_MANAGER_TEST_FORCE_INTERACTIVE", "1");
    assert_resolver_output(environment_wins, &cwd.join("winner"));

    let drive = cwd
        .to_string_lossy()
        .get(..2)
        .unwrap_or_else(|| unreachable!("Windows temporary path must have a drive"))
        .to_owned();
    let mut root_relative = windows_resolver_command(&script, &cwd, &home, &local_app_data);
    root_relative.args(["-Dir", r"\tools\bin"]);
    assert_resolver_output(root_relative, &PathBuf::from(format!(r"{drive}\tools\bin")));

    let drive_relative_input = format!(r"{drive}tools\bin");
    let mut drive_relative = windows_resolver_command(&script, &cwd, &home, &local_app_data);
    drive_relative.args(["-Dir", &drive_relative_input]);
    assert_resolver_output(drive_relative, &cwd.join(r"tools\bin"));

    let mut stale_process_cwd = Command::new("powershell.exe");
    stale_process_cwd
        .args([
            "-NoLogo",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "Set-Location -LiteralPath $env:SKILL_MANAGER_TEST_INVOCATION_CWD; & $env:SKILL_MANAGER_TEST_SCRIPT -Dir $env:SKILL_MANAGER_TEST_DRIVE_RELATIVE",
        ])
        .current_dir(&home)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("LOCALAPPDATA", &local_app_data)
        .env("TEMP", cwd.join("temp"))
        .env("TMP", cwd.join("temp"));
    clear_installer_environment(&mut stale_process_cwd);
    stale_process_cwd
        .env("SKILL_MANAGER_TEST_RESOLVE_DIR", "1")
        .env("SKILL_MANAGER_TEST_INVOCATION_CWD", &cwd)
        .env("SKILL_MANAGER_TEST_SCRIPT", &script)
        .env("SKILL_MANAGER_TEST_DRIVE_RELATIVE", &drive_relative_input);
    assert_resolver_output(stale_process_cwd, &cwd.join(r"tools\bin"));

    let physical = scratch_root.join(r"physical\target");
    fs::create_dir_all(&physical)
        .unwrap_or_else(|error| unreachable!("create symlink target: {error}"));
    let alias = cwd.join("alias");
    if std::os::windows::fs::symlink_dir(&physical, &alias).is_ok() {
        let mut lexical = windows_resolver_command(&script, &cwd, &home, &local_app_data);
        lexical.args(["-Dir", r"alias\..\lexical"]);
        assert_resolver_output(lexical, &cwd.join("lexical"));
    }
}

#[test]
fn managing_skill_has_required_metadata_and_current_storage_claims() {
    let root = repository_root();
    let skill = read(&root.join("skills/managing-skills/SKILL.md"));
    let agent_guide = read(&root.join("docs/agent-usage.md"));
    let canonical_source = "https://github.com/sernst/skills/tree/main/skills";
    assert!(skill.starts_with("---\nname: managing-skills\ndescription: "));
    assert!(!skill.contains("TODO"));
    for bootstrap_guardrail in [
        "Run `skill-manager --version`.",
        "If it is absent, stop.",
        "Do not run an installer, choose an install directory, or modify persistent",
        "If verification fails, stop and report the exact failure.",
    ] {
        assert!(
            skill.contains(bootstrap_guardrail),
            "missing bootstrap/verify guardrail: {bootstrap_guardrail}"
        );
    }
    assert!(!root.join("install.skill-manager.md").exists());
    assert!(
        !root
            .join("skills/managing-skills/references/install.skill-manager.md")
            .exists()
    );
    assert!(!skill.contains("install.skill-manager.md"));
    assert!(skill.contains("explicitly asks to \"manage skills\""));
    assert!(skill.contains("invokes `$managing-skills`"));
    assert!(skill.contains("without framing it as skill management"));
    assert!(skill.contains("[references/reporting.md](references/reporting.md)"));
    assert!(
        skill.contains("https://github.com/sernst/skills/blob/main/docs/agent-usage.md"),
        "the deployed skill must link to the portable human setup guide"
    );
    assert!(agent_guide.contains(canonical_source));
    let source_add = agent_guide
        .find("skill-manager source add")
        .unwrap_or_else(|| unreachable!("agent guide must register its source first"));
    let preview = agent_guide
        .find("--shared --global --dry-run")
        .unwrap_or_else(|| unreachable!("agent guide must preview the deployment"));
    let apply = agent_guide
        .find("--shared --global\n")
        .unwrap_or_else(|| unreachable!("agent guide must apply the deployment"));
    let status = agent_guide
        .find("skill-manager status managing-skills")
        .unwrap_or_else(|| unreachable!("agent guide must verify the deployment"));
    assert!(
        source_add < preview && preview < apply && apply < status,
        "agent quickstart must register, preview, apply, then verify"
    );
    for failure_guardrail in [
        "Treat that as an expected absence signal only when the parsed",
        "Every other exit-1 message",
    ] {
        assert!(
            skill.contains(failure_guardrail),
            "missing failure guardrail: {failure_guardrail}"
        );
    }
    let metadata = read(&root.join("skills/managing-skills/agents/openai.yaml"));
    for required in [
        "display_name: \"Manage Skills\"",
        "short_description: \"Manage agent skills with skill-manager\"",
        "default_prompt: \"Use $managing-skills to manage my installed agent skills.\"",
        "allow_implicit_invocation: true",
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

fn assert_forward_activation_cases(activation: &[serde_json::Value], description: &str) {
    let positives = activation
        .iter()
        .filter(|case| case["expected_activation"] == true)
        .count();
    let near_misses = activation
        .iter()
        .filter(|case| case["expected_activation"] == false)
        .count();
    assert!(positives >= 3, "activation cases need positive coverage");
    assert!(near_misses >= 3, "activation cases need near-miss coverage");
    for case in activation {
        let prompt = case["prompt"]
            .as_str()
            .unwrap_or_else(|| unreachable!("activation prompt must be a string"));
        assert!(
            !prompt.trim().is_empty(),
            "activation prompt must not be empty"
        );
        assert!(
            case["expected_activation"].is_boolean(),
            "activation case {prompt:?} must declare its expected decision"
        );
        let anchor = case["instruction_anchor"]
            .as_str()
            .unwrap_or_else(|| unreachable!("activation case must cite its skill instruction"));
        assert!(
            description.contains(&normalized_prose(anchor)),
            "activation case {prompt:?} cites missing frontmatter guidance: {anchor:?}"
        );
    }
}

fn assert_forward_reporting_cases(reporting_cases: &[serde_json::Value], reporting: &str) {
    let scenario_ids = reporting_cases
        .iter()
        .filter_map(|case| case["id"].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        scenario_ids,
        BTreeSet::from([
            "committed-mutation-with-warning",
            "dry-run-plan",
            "partial-failure",
            "read-only-comparison",
        ]),
        "forward review must retain inspection, preview, commit/warning, and partial-failure scenarios"
    );
    for case in reporting_cases {
        let id = case["id"].as_str().unwrap_or("<missing id>");
        for required in ["prompt", "evidence", "pass_if", "fail_if"] {
            let populated = match &case[required] {
                serde_json::Value::String(value) => !value.trim().is_empty(),
                serde_json::Value::Array(values) => !values.is_empty(),
                _ => false,
            };
            assert!(populated, "forward-review scenario {id} needs {required}");
        }
        let anchors = case["guidance_anchors"]
            .as_array()
            .unwrap_or_else(|| unreachable!("scenario {id} must cite reporting guidance"));
        assert!(
            !anchors.is_empty(),
            "scenario {id} must cite reporting guidance"
        );
        for anchor in anchors {
            let anchor = anchor
                .as_str()
                .unwrap_or_else(|| unreachable!("scenario {id} has a non-string anchor"));
            assert!(
                reporting.contains(&normalized_prose(anchor)),
                "scenario {id} cites missing reporting guidance: {anchor:?}"
            );
        }
    }
}

#[test]
fn managing_skill_forward_review_fixture_is_instruction_aligned() {
    let root = repository_root();
    let fixture_path = root.join("skills/managing-skills/evals/forward-review.json");
    let fixture: serde_json::Value = serde_json::from_str(&read(&fixture_path))
        .unwrap_or_else(|error| unreachable!("{}: {error}", fixture_path.display()));
    assert!(
        fixture["purpose"]
            .as_str()
            .is_some_and(|purpose| purpose.contains("does not claim to execute an agent")),
        "the fixture must state that CI does not execute these behavioral reviews"
    );
    assert!(
        fixture["review_protocol"]
            .as_array()
            .is_some_and(|steps| steps.len() >= 4),
        "the fixture must retain its independent forward-review protocol"
    );

    let skill = read(&root.join("skills/managing-skills/SKILL.md"));
    let description = skill
        .lines()
        .find_map(|line| line.strip_prefix("description: "))
        .map_or_else(
            || unreachable!("managing-skills needs a frontmatter description"),
            normalized_prose,
        );
    let reporting = normalized_prose(&read(
        &root.join("skills/managing-skills/references/reporting.md"),
    ));
    let activation = fixture["activation"]
        .as_array()
        .unwrap_or_else(|| unreachable!("activation cases must be an array"));
    let reporting_cases = fixture["reporting"]
        .as_array()
        .unwrap_or_else(|| unreachable!("reporting cases must be an array"));
    assert_forward_activation_cases(activation, &description);
    assert_forward_reporting_cases(reporting_cases, &reporting);
}

#[test]
fn machine_requirements_and_all_target_semantics_match_production() {
    let root = repository_root();
    let app = read(&root.join("clis/skill-manager/src/app.rs"));
    for production_contract in [
        "selection.all_targets && target.target.enabled",
        "source name is required in noninteractive mode; pass SOURCE --name=NAME",
        "arguments are ambiguous in noninteractive mode; pass {location} --name=NAME",
        "target add requires NAME and PATH, or PATH --name=NAME",
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
        "Without explicit scope or target selection,\n`load` infers project-vs-global scope and enabled targets silently",
        "Machine/non-interactive use\nrequires an explicit nonblank `name`",
        "`SOURCE --name=NAME`",
        "`PATH --name=NAME`",
        "`all_targets:true` selects enabled\n  configured targets only",
        "A disabled target requires explicit selection",
    ] {
        assert!(
            recipes.contains(required),
            "recipe machine requirement is missing: {required}"
        );
    }
    for required in [
        "`load` and `update` render\ntheir whole plan and then auto-authorize the apply step",
        "Both use enabled targets when none are\nselected, and `load` also infers project-vs-global scope silently",
        "`all_targets:true` selects\nenabled configured targets only",
        "machine use should pass\n`SOURCE --name=NAME`",
        "`PATH --name=NAME` to `target.add`",
    ] {
        assert!(
            skill.contains(required),
            "managing skill machine requirement is missing: {required}"
        );
    }
    for required in [
        "`load` in\nnon-interactive mode infers enabled targets silently, exactly like `update`",
        "`SOURCE --name NAME`",
        "`PATH --name NAME`",
        "`--all` never opts into a disabled\ntarget",
    ] {
        assert!(
            cheatsheet.contains(required),
            "cheatsheet machine requirement is missing: {required}"
        );
    }
}

fn values_after_marker(source: &str, marker: &str, terminator: char) -> BTreeSet<String> {
    source
        .lines()
        .filter_map(|line| {
            let start = line.find(marker)?;
            let value = &line[start + marker.len()..];
            let end = value.find(terminator)?;
            Some(value[..end].to_owned())
        })
        .collect()
}

fn assert_installer_targets_are_released(
    installer: &str,
    architectures: &BTreeSet<String>,
    platforms: &BTreeSet<String>,
    released_targets: &BTreeSet<String>,
) {
    assert!(
        !architectures.is_empty(),
        "{installer} installer must map at least one architecture"
    );
    assert!(
        !platforms.is_empty(),
        "{installer} installer must map at least one platform"
    );
    for architecture in architectures {
        for platform in platforms {
            let target = format!("{architecture}-{platform}");
            assert!(
                released_targets.contains(&target),
                "{installer} installer references release target absent from the canonical matrix: {target}"
            );
        }
    }
}

fn assert_posix_installer_release_contract(source: &str, released_targets: &BTreeSet<String>) {
    let architectures = values_after_marker(source, "arch=\"", '"');
    let platforms = values_after_marker(source, "platform=\"", '"');
    assert_eq!(
        architectures,
        BTreeSet::from(["aarch64".to_owned(), "x86_64".to_owned()]),
        "POSIX installer must map both released architectures"
    );
    assert_installer_targets_are_released("POSIX", &architectures, &platforms, released_targets);
    assert_eq!(
        platforms,
        BTreeSet::from(["apple-darwin".to_owned(), "unknown-linux-musl".to_owned(),]),
        "POSIX installer must map its supported operating systems to release platforms"
    );
    assert_eq!(
        values_after_marker(source, "archive_ext=\"", '"'),
        BTreeSet::from(["tar.gz".to_owned()]),
        "POSIX installer must request tar.gz release archives"
    );
    assert!(
        source
            .lines()
            .any(|line| line.trim_start().starts_with("asset=") && line.contains("archive_ext")),
        "POSIX asset name must use its archive extension mapping"
    );
    assert!(
        source.contains("SHA256SUMS")
            && (source.contains("sha256sum") || source.contains("shasum"))
            && source.contains("checksum mismatch"),
        "POSIX installer must verify downloads against SHA256SUMS"
    );
}

fn assert_windows_installer_release_contract(source: &str, released_targets: &BTreeSet<String>) {
    let architectures = values_after_marker(source, "$arch = '", '\'');
    let platforms = values_after_marker(source, "$target = \"$arch-", '"');
    assert_eq!(
        architectures,
        BTreeSet::from(["aarch64".to_owned(), "x86_64".to_owned()]),
        "Windows installer must map both released architectures"
    );
    assert_installer_targets_are_released("Windows", &architectures, &platforms, released_targets);
    assert_eq!(
        platforms,
        BTreeSet::from(["pc-windows-msvc".to_owned()]),
        "Windows installer must map Windows to its released platform"
    );
    let assets = values_after_marker(source, "$asset = \"", '"');
    assert!(
        !assets.is_empty()
            && assets.iter().all(|asset| {
                Path::new(asset)
                    .extension()
                    .is_some_and(|extension| extension == "zip")
            }),
        "Windows installer must request zip release archives: {assets:?}"
    );
    assert!(
        source.contains("SHA256SUMS")
            && source.contains("Get-FileHash")
            && source.contains("checksum mismatch"),
        "Windows installer must verify downloads against SHA256SUMS"
    );
}

#[test]
fn installers_map_release_assets_to_the_canonical_matrix() {
    let root = repository_root();
    let matrix = read(&root.join("tools/build-matrix-contract.ps1"));
    let marker = "function Get-CanonicalFullBuildTargets";
    let start = matrix
        .find(marker)
        .unwrap_or_else(|| unreachable!("canonical build matrix function"));
    let remainder = &matrix[start + marker.len()..];
    let end = remainder.find("\nfunction ").unwrap_or(remainder.len());
    let released_targets = values_after_marker(&remainder[..end], "target='", '\'');
    assert_eq!(
        released_targets.len(),
        8,
        "canonical release matrix target count"
    );
    assert_posix_installer_release_contract(
        &read(&root.join("clis/skill-manager/install.sh")),
        &released_targets,
    );
    assert_windows_installer_release_contract(
        &read(&root.join("clis/skill-manager/install.ps1")),
        &released_targets,
    );
}
