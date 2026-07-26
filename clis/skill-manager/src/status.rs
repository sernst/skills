//! Human-readable status tables with display-width-aware alignment.

use crate::domain::SkillState;
use unicode_width::UnicodeWidthStr;

const COLUMN_GAP: &str = "  ";

/// One source shown in the status preamble.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRow {
    /// Compact key used by the skill table.
    pub name: String,
    /// Human-facing source label.
    pub label: String,
    /// Canonical local or GitHub location.
    pub location: String,
    /// Optional inactive location.
    pub alternate: Option<String>,
}

/// One skill and its state for every selected target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillRow {
    /// Portable skill name.
    pub skill: String,
    /// Compact source key, or `unknown` for a deployed-only skill.
    pub source: String,
    /// Target names and states in configured target order.
    pub targets: Vec<(String, SkillState)>,
}

/// Render the source-key legend.
#[must_use]
pub fn source_table(rows: &[SourceRow]) -> Vec<String> {
    let mut widths = [0, 0];
    for row in rows {
        widths[0] = widths[0].max(display_width(&row.name));
        widths[1] = widths[1].max(display_width(&parenthesized_label(&row.label)));
        if row.alternate.is_some() {
            widths[0] = widths[0].max(display_width("  alternate"));
            widths[1] = widths[1].max(display_width("(inactive)"));
        }
    }

    rows.iter()
        .flat_map(|row| {
            let mut rendered = vec![join_columns(&[
                padded(&row.name, widths[0]),
                padded(&parenthesized_label(&row.label), widths[1]),
                row.location.clone(),
            ])];
            if let Some(alternate) = &row.alternate {
                rendered.push(join_columns(&[
                    padded("  alternate", widths[0]),
                    padded("(inactive)", widths[1]),
                    alternate.clone(),
                ]));
            }
            rendered
        })
        .collect()
}

/// Render the skill status table.
#[must_use]
pub fn skill_table(
    rows: &[SkillRow],
    target_names: &[String],
    symbols: bool,
    color: bool,
) -> Vec<String> {
    let mut widths = Vec::with_capacity(target_names.len() + 2);
    widths.push(display_width("skill"));
    widths.push(display_width("source"));
    widths.extend(target_names.iter().map(|name| display_width(name)));

    for row in rows {
        widths[0] = widths[0].max(display_width(&row.skill));
        widths[1] = widths[1].max(display_width(&row.source));
        for (index, (_, state)) in row.targets.iter().enumerate() {
            let cell = state_cell(*state, symbols);
            widths[index + 2] = widths[index + 2].max(display_width(cell));
        }
    }

    let mut header = vec![padded("skill", widths[0]), padded("source", widths[1])];
    for (index, name) in target_names.iter().enumerate() {
        header.push(padded(name, widths[index + 2]));
    }
    let mut lines = vec![join_columns(&header), separator(&widths)];

    for row in rows {
        let mut columns = vec![
            padded(&row.skill, widths[0]),
            padded(&row.source, widths[1]),
        ];
        for (index, (_, state)) in row.targets.iter().enumerate() {
            let plain = state_cell(*state, symbols);
            let padding = " ".repeat(widths[index + 2].saturating_sub(display_width(plain)));
            columns.push(format!("{}{padding}", styled_state(plain, *state, color)));
        }
        lines.push(join_columns(&columns));
    }
    lines
}

/// Render the aggregate status legend at the bottom of a human status report.
#[must_use]
pub fn status_summary(
    up_to_date: usize,
    needs_update: usize,
    not_loaded: usize,
    no_connection: usize,
    symbols: bool,
    color: bool,
) -> String {
    let entries = [
        (SkillState::UpToDate, up_to_date, "up-to-date"),
        (SkillState::NeedsUpdate, needs_update, "needs-update"),
        (SkillState::NotLoaded, not_loaded, "not-loaded"),
        (
            SkillState::NoConnection,
            no_connection,
            "unsourced deployed",
        ),
    ];

    if !symbols {
        return entries
            .iter()
            .filter(|(_, count, _)| *count > 0)
            .map(|(state, count, _)| format!("{}: {count}", state.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
    }

    entries
        .iter()
        .filter(|(_, count, _)| *count > 0)
        .map(|(state, count, description)| {
            let text = format!("{}: {count} {description}", status_symbol(*state));
            styled_state(&text, *state, color)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn parenthesized_label(label: &str) -> String {
    format!("({label})")
}

fn state_cell(state: SkillState, symbols: bool) -> &'static str {
    if symbols {
        status_symbol(state)
    } else {
        state.as_str()
    }
}

fn status_symbol(state: SkillState) -> &'static str {
    match state {
        SkillState::UpToDate => "✓",
        SkillState::NeedsUpdate => "↑",
        SkillState::NotLoaded => "✗",
        SkillState::NoConnection => "~",
    }
}

fn styled_state(text: &str, state: SkillState, color: bool) -> String {
    if !color {
        return text.into();
    }
    let code = match state {
        SkillState::UpToDate => Some(32),
        SkillState::NeedsUpdate => Some(33),
        SkillState::NotLoaded => None,
        SkillState::NoConnection => Some(36),
    };
    code.map_or_else(
        || text.into(),
        |code| format!("\u{1b}[{code}m{text}\u{1b}[0m"),
    )
}

fn padded(value: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(value));
    format!("{value}{}", " ".repeat(padding))
}

fn join_columns(columns: &[String]) -> String {
    columns.join(COLUMN_GAP).trim_end().into()
}

fn separator(widths: &[usize]) -> String {
    join_columns(
        &widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>(),
    )
}

fn display_width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}

#[cfg(test)]
mod tests {
    use super::{SkillRow, SourceRow, skill_table, source_table, status_summary};
    use crate::domain::SkillState;

    #[test]
    fn source_columns_align_by_unicode_display_width() {
        let lines = source_table(&[
            SourceRow {
                name: "短い".into(),
                label: "Wide".into(),
                location: "/a path/with spaces".into(),
                alternate: None,
            },
            SourceRow {
                name: "long-source".into(),
                label: "Longer label".into(),
                location: "owner/repo:main/skills".into(),
                alternate: None,
            },
        ]);

        assert_eq!(
            lines,
            [
                "短い         (Wide)          /a path/with spaces",
                "long-source  (Longer label)  owner/repo:main/skills",
            ]
        );
    }

    #[test]
    fn source_alternate_rows_align_by_unicode_display_width() {
        let lines = source_table(&[
            SourceRow {
                name: "短い".into(),
                label: "技能".into(),
                location: "/active".into(),
                alternate: Some("所有者/倉庫".into()),
            },
            SourceRow {
                name: "long-source".into(),
                label: "Label".into(),
                location: "owner/repo".into(),
                alternate: None,
            },
        ]);

        assert_eq!(
            lines,
            [
                "短い         (技能)      /active",
                "  alternate  (inactive)  所有者/倉庫",
                "long-source  (Label)     owner/repo",
            ]
        );
    }

    #[test]
    fn status_color_is_scoped_to_state_cells() {
        let lines = skill_table(
            &[SkillRow {
                skill: "a-long-skill".into(),
                source: "primary".into(),
                targets: vec![
                    ("claude".into(), SkillState::UpToDate),
                    ("shared".into(), SkillState::NeedsUpdate),
                ],
            }],
            &["claude".into(), "shared".into()],
            true,
            true,
        );

        assert!(!lines[0].contains('\u{1b}'));
        assert_eq!(lines[1], "------------  -------  ------  ------");
        assert!(lines[2].starts_with("a-long-skill  primary  "));
        assert!(lines[2].contains("\u{1b}[32m✓\u{1b}[0m"));
        assert!(lines[2].contains("\u{1b}[33m↑\u{1b}[0m"));
        assert!(!lines[2].contains("up-to-date"));
        assert!(!lines[2].contains("needs-update"));
    }

    #[test]
    fn interactive_symbols_do_not_depend_on_color() {
        let lines = skill_table(
            &[SkillRow {
                skill: "demo".into(),
                source: "primary".into(),
                targets: vec![("claude".into(), SkillState::UpToDate)],
            }],
            &["claude".into()],
            true,
            false,
        );

        assert!(lines[2].contains('✓'));
        assert!(!lines[2].contains("up-to-date"));
        assert!(!lines[2].contains('\u{1b}'));
    }

    #[test]
    fn custom_target_headers_stay_readable_while_cells_stay_one_symbol_wide() {
        let target = "a-custom-target".to_owned();
        let lines = skill_table(
            &[SkillRow {
                skill: "demo".into(),
                source: "primary".into(),
                targets: vec![(target.clone(), SkillState::NotLoaded)],
            }],
            &[target],
            true,
            false,
        );

        assert_eq!(lines[0], "skill  source   a-custom-target");
        assert_eq!(lines[1], "-----  -------  ---------------");
        assert_eq!(lines[2], "demo   primary  ✗");
    }

    #[test]
    fn redirected_status_is_plain_and_aligned() {
        let lines = skill_table(
            &[
                SkillRow {
                    skill: "a".into(),
                    source: "source-with-a-long-name".into(),
                    targets: vec![("target".into(), SkillState::NotLoaded)],
                },
                SkillRow {
                    skill: "long-skill".into(),
                    source: "unknown".into(),
                    targets: vec![("target".into(), SkillState::NoConnection)],
                },
            ],
            &["target".into()],
            false,
            false,
        );

        assert_eq!(
            lines,
            [
                "skill       source                   target",
                "----------  -----------------------  -------------",
                "a           source-with-a-long-name  not-loaded",
                "long-skill  unknown                  no-connection",
            ]
        );
        assert!(lines.iter().all(|line| !line.contains('\u{1b}')));
        assert!(lines.iter().all(|line| !line.contains('✓')));
    }

    #[test]
    fn empty_skill_table_still_has_a_stable_header_and_separator() {
        assert!(source_table(&[]).is_empty());
        assert_eq!(
            skill_table(&[], &["claude".into()], false, false),
            ["skill  source  claude", "-----  ------  ------"]
        );
    }

    #[test]
    fn summary_uses_only_nonzero_states_and_matches_cell_colors() {
        let summary = status_summary(23, 1, 9, 3, true, true);
        assert_eq!(
            summary,
            "\u{1b}[32m✓: 23 up-to-date\u{1b}[0m, \
             \u{1b}[33m↑: 1 needs-update\u{1b}[0m, \
             ✗: 9 not-loaded, \
             \u{1b}[36m~: 3 unsourced deployed\u{1b}[0m"
        );
        assert_eq!(status_summary(0, 0, 1, 0, true, false), "✗: 1 not-loaded");
        assert!(status_summary(0, 0, 0, 0, true, false).is_empty());
    }

    #[test]
    fn no_color_keeps_interactive_symbols_without_ansi() {
        let summary = status_summary(1, 1, 1, 1, true, false);
        assert_eq!(
            summary,
            "✓: 1 up-to-date, ↑: 1 needs-update, ✗: 1 not-loaded, ~: 1 unsourced deployed"
        );
        assert!(!summary.contains('\u{1b}'));
    }
}
