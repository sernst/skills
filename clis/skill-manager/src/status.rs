//! Human-readable status tables with display-width-aware alignment.

use std::path::PathBuf;

use crate::domain::{Scope, SkillState};
use unicode_width::UnicodeWidthStr;

/// Two ASCII spaces separate every rendered column.
pub(crate) const COLUMN_GAP: &str = "  ";

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

/// Aggregate placement of a skill across all selected targets.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillLocation {
    /// The skill is installed only globally.
    Global,
    /// The skill is installed only in the current project.
    Project,
    /// The skill is installed in both scopes.
    Both,
    /// The skill is not installed in either scope.
    None,
}

impl SkillLocation {
    /// Stable machine and redirected-human label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
            Self::Both => "both",
            Self::None => "none",
        }
    }
}

/// One target/scope observation for a status row.
///
/// Callers must provide records ordered first by configured target order and
/// then global before project.  Keeping the full detail separate from the
/// effective [`SkillRow::targets`] map lets machine consumers understand a
/// shadowed global deployment without making the human table wider.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct DeploymentDetail {
    /// Stable target name.
    pub target: String,
    /// Location of this target deployment.
    pub scope: Scope,
    /// Fully resolved directory for the target in this scope.
    pub path: PathBuf,
    /// Whether the skill directory is installed at this target and scope.
    pub installed: bool,
    /// Source-relative state of this specific deployment.
    pub state: SkillState,
    /// Whether this scope supplies the effective deployment for this target.
    pub effective: bool,
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
    /// Aggregate location across selected targets and both scopes.
    pub location: SkillLocation,
    /// Whether installed targets disagree about their non-empty scope sets.
    pub mixed: bool,
    /// Whether a shadowed global copy differs from its effective project copy.
    pub shadowed_global_divergent: bool,
    /// Deterministically ordered target/scope observations for machine output.
    pub deployments: Vec<DeploymentDetail>,
}

impl SkillRow {
    /// Construct a row before scope-aware deployment details have been added.
    ///
    /// Command code should populate the public scope fields directly for new
    /// status reports; this constructor keeps legacy callers explicit about the
    /// temporary `none` placement they represent.
    #[must_use]
    pub fn without_deployments(
        skill: String,
        source: String,
        targets: Vec<(String, SkillState)>,
    ) -> Self {
        Self {
            skill,
            source,
            targets,
            location: SkillLocation::None,
            mixed: false,
            shadowed_global_divergent: false,
            deployments: Vec::new(),
        }
    }
}

/// Aggregate counts displayed below a status report.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StatusSummaryCounts {
    /// Number of target state cells that are current.
    pub up_to_date: usize,
    /// Number of target state cells that differ from their source.
    pub needs_update: usize,
    /// Number of target state cells with no deployment.
    pub not_loaded: usize,
    /// Number of target state cells without a source.
    pub no_connection: usize,
    /// Number of skills installed only globally.
    pub global: usize,
    /// Number of skills installed only at project scope.
    pub project: usize,
    /// Number of skills installed at both scopes.
    pub both: usize,
    /// Number of skills not installed at either scope.
    pub none: usize,
    /// Number of skills whose installed targets have differing scope sets.
    pub mixed: usize,
    /// Number of rows with a divergent global copy shadowed by project scope.
    pub shadowed_global_divergent: usize,
}

/// Count state cells and aggregate locations from rendered status rows.
#[must_use]
pub fn status_summary_counts(rows: &[SkillRow]) -> StatusSummaryCounts {
    let mut counts = StatusSummaryCounts::default();
    for row in rows {
        for (_, state) in &row.targets {
            match state {
                SkillState::UpToDate => counts.up_to_date += 1,
                SkillState::NeedsUpdate => counts.needs_update += 1,
                SkillState::NotLoaded => counts.not_loaded += 1,
                SkillState::NoConnection => counts.no_connection += 1,
            }
        }
        match row.location {
            SkillLocation::Global => counts.global += 1,
            SkillLocation::Project => counts.project += 1,
            SkillLocation::Both => counts.both += 1,
            SkillLocation::None => counts.none += 1,
        }
        if row.mixed {
            counts.mixed += 1;
        }
        if row.shadowed_global_divergent {
            counts.shadowed_global_divergent += 1;
        }
    }
    counts
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
    let mut widths = Vec::with_capacity(target_names.len() + 3);
    widths.push(display_width("skill"));
    widths.push(display_width("source"));
    widths.push(display_width("location"));
    widths.extend(target_names.iter().map(|name| display_width(name)));

    for row in rows {
        widths[0] = widths[0].max(display_width(&row.skill));
        widths[1] = widths[1].max(display_width(&row.source));
        widths[2] = widths[2].max(display_width(&location_cell(row, symbols)));
        for (index, (_, state)) in row.targets.iter().enumerate() {
            let cell = state_cell(*state, symbols);
            widths[index + 3] = widths[index + 3].max(display_width(cell));
        }
    }

    let mut header = vec![
        padded("skill", widths[0]),
        padded("source", widths[1]),
        padded("location", widths[2]),
    ];
    for (index, name) in target_names.iter().enumerate() {
        header.push(padded(name, widths[index + 3]));
    }
    let mut lines = vec![join_columns(&header), separator(&widths)];

    for row in rows {
        let mut columns = vec![
            padded(&row.skill, widths[0]),
            padded(&row.source, widths[1]),
            padded(&location_cell(row, symbols), widths[2]),
        ];
        for (index, (_, state)) in row.targets.iter().enumerate() {
            let plain = state_cell(*state, symbols);
            let padding = " ".repeat(widths[index + 3].saturating_sub(display_width(plain)));
            columns.push(format!("{}{padding}", styled_state(plain, *state, color)));
        }
        lines.push(join_columns(&columns));
    }
    lines
}

fn location_cell(row: &SkillRow, symbols: bool) -> String {
    let location = if symbols {
        match row.location {
            SkillLocation::Global => "🌐",
            SkillLocation::Project => "📁",
            SkillLocation::Both => "↕",
            SkillLocation::None => "—",
        }
    } else {
        row.location.as_str()
    };
    if row.mixed && row.location != SkillLocation::None {
        if symbols {
            format!("{location} ⚠")
        } else {
            format!("{location} (mixed)")
        }
    } else {
        location.into()
    }
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
    state_summary(
        &StatusSummaryCounts {
            up_to_date,
            needs_update,
            not_loaded,
            no_connection,
            ..StatusSummaryCounts::default()
        },
        symbols,
        color,
    )
}

/// Render the aggregate status legend and placement summary.
#[must_use]
pub fn status_summary_with_counts(
    counts: &StatusSummaryCounts,
    symbols: bool,
    color: bool,
) -> String {
    let mut rendered = state_summary_entries(counts, symbols, color);

    let locations = [
        (SkillLocation::Global, counts.global),
        (SkillLocation::Project, counts.project),
        (SkillLocation::Both, counts.both),
        (SkillLocation::None, counts.none),
    ];
    rendered.extend(
        locations
            .iter()
            .filter(|(_, count)| *count > 0)
            .map(|(location, count)| {
                let label = if symbols {
                    match location {
                        SkillLocation::Global => "🌐 global",
                        SkillLocation::Project => "📁 project",
                        SkillLocation::Both => "↕ both",
                        SkillLocation::None => "— not installed",
                    }
                } else {
                    match location {
                        SkillLocation::None => "none",
                        _ => location.as_str(),
                    }
                };
                format!("{label}: {count}")
            }),
    );
    if counts.mixed > 0 {
        rendered.push(if symbols {
            format!("⚠ mixed placement: {}", counts.mixed)
        } else {
            format!("mixed placement: {}", counts.mixed)
        });
    }
    if counts.shadowed_global_divergent > 0 {
        rendered.push(format!(
            "shadowed global divergence: {}",
            counts.shadowed_global_divergent
        ));
    }
    rendered.join(", ")
}

fn state_summary(counts: &StatusSummaryCounts, symbols: bool, color: bool) -> String {
    state_summary_entries(counts, symbols, color).join(", ")
}

fn state_summary_entries(counts: &StatusSummaryCounts, symbols: bool, color: bool) -> Vec<String> {
    let entries = [
        (SkillState::UpToDate, counts.up_to_date, "up-to-date"),
        (SkillState::NeedsUpdate, counts.needs_update, "needs-update"),
        (SkillState::NotLoaded, counts.not_loaded, "not-loaded"),
        (
            SkillState::NoConnection,
            counts.no_connection,
            "unsourced deployed",
        ),
    ];

    if symbols {
        entries
            .iter()
            .filter(|(_, count, _)| *count > 0)
            .map(|(state, count, description)| {
                let text = format!("{}: {count} {description}", status_symbol(*state));
                styled_state(&text, *state, color)
            })
            .collect::<Vec<_>>()
    } else {
        entries
            .iter()
            .filter(|(_, count, _)| *count > 0)
            .map(|(state, count, _)| format!("{}: {count}", state.as_str()))
            .collect::<Vec<_>>()
    }
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

/// Pad a cell to a display width shared with other plan/status renderers.
pub(crate) fn padded(value: &str, width: usize) -> String {
    let padding = width.saturating_sub(display_width(value));
    format!("{value}{}", " ".repeat(padding))
}

/// Join rendered cells with the shared column gap.
pub(crate) fn join_columns(columns: &[String]) -> String {
    columns.join(COLUMN_GAP).trim_end().into()
}

/// Render the dashed header separator for a set of column widths.
pub(crate) fn separator(widths: &[usize]) -> String {
    join_columns(
        &widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>(),
    )
}

/// Measure the terminal display width of a cell.
pub(crate) fn display_width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}

#[cfg(test)]
mod tests {
    use super::{
        DeploymentDetail, SkillLocation, SkillRow, SourceRow, StatusSummaryCounts, skill_table,
        source_table, status_summary, status_summary_counts, status_summary_with_counts,
    };
    use crate::domain::{Scope, SkillState};

    fn row(skill: &str, source: &str, targets: Vec<(String, SkillState)>) -> SkillRow {
        SkillRow::without_deployments(skill.into(), source.into(), targets)
    }

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
            &[row(
                "a-long-skill",
                "primary",
                vec![
                    ("claude".into(), SkillState::UpToDate),
                    ("shared".into(), SkillState::NeedsUpdate),
                ],
            )],
            &["claude".into(), "shared".into()],
            true,
            true,
        );

        assert!(!lines[0].contains('\u{1b}'));
        assert_eq!(lines[1], "------------  -------  --------  ------  ------");
        assert!(lines[2].starts_with("a-long-skill  primary  "));
        assert!(lines[2].contains("\u{1b}[32m✓\u{1b}[0m"));
        assert!(lines[2].contains("\u{1b}[33m↑\u{1b}[0m"));
        assert!(!lines[2].contains("up-to-date"));
        assert!(!lines[2].contains("needs-update"));
    }

    #[test]
    fn interactive_symbols_do_not_depend_on_color() {
        let lines = skill_table(
            &[row(
                "demo",
                "primary",
                vec![("claude".into(), SkillState::UpToDate)],
            )],
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
            &[row(
                "demo",
                "primary",
                vec![(target.clone(), SkillState::NotLoaded)],
            )],
            &[target],
            true,
            false,
        );

        assert_eq!(lines[0], "skill  source   location  a-custom-target");
        assert_eq!(lines[1], "-----  -------  --------  ---------------");
        assert_eq!(lines[2], "demo   primary  —         ✗");
    }

    #[test]
    fn redirected_status_is_plain_and_aligned() {
        let lines = skill_table(
            &[
                row(
                    "a",
                    "source-with-a-long-name",
                    vec![("target".into(), SkillState::NotLoaded)],
                ),
                row(
                    "long-skill",
                    "unknown",
                    vec![("target".into(), SkillState::NoConnection)],
                ),
            ],
            &["target".into()],
            false,
            false,
        );

        assert_eq!(
            lines,
            [
                "skill       source                   location  target",
                "----------  -----------------------  --------  -------------",
                "a           source-with-a-long-name  none      not-loaded",
                "long-skill  unknown                  none      no-connection",
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
            [
                "skill  source  location  claude",
                "-----  ------  --------  ------"
            ]
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

    #[test]
    fn location_column_uses_compact_symbols_and_marks_mixed_rows() {
        let mut row = row(
            "demo",
            "primary",
            vec![("claude".into(), SkillState::UpToDate)],
        );
        row.location = SkillLocation::Both;
        row.mixed = true;

        let lines = skill_table(&[row], &["claude".into()], true, false);

        assert_eq!(lines[0], "skill  source   location  claude");
        assert!(lines[2].contains("↕ ⚠"));
        assert!(lines[2].contains('✓'));
    }

    #[test]
    fn redirected_location_column_uses_plain_labels_and_mixed_text() {
        let mut row = row(
            "demo",
            "primary",
            vec![("claude".into(), SkillState::UpToDate)],
        );
        row.location = SkillLocation::Both;
        row.mixed = true;

        let lines = skill_table(&[row], &["claude".into()], false, false);

        assert!(lines[2].contains("both (mixed)"));
        assert!(!lines.iter().any(|line| line.contains('↕')));
        assert!(!lines.iter().any(|line| line.contains('\u{1b}')));
    }

    #[test]
    fn summary_counts_all_locations_mixed_and_shadowed_deployments() {
        let mut global = row(
            "global",
            "source",
            vec![("claude".into(), SkillState::UpToDate)],
        );
        global.location = SkillLocation::Global;
        global.deployments = vec![DeploymentDetail {
            target: "claude".into(),
            scope: Scope::Global,
            path: "/global/.claude/skills".into(),
            installed: true,
            state: SkillState::UpToDate,
            effective: true,
        }];
        let mut project = row(
            "project",
            "source",
            vec![("claude".into(), SkillState::NeedsUpdate)],
        );
        project.location = SkillLocation::Project;
        let mut both = row(
            "both",
            "source",
            vec![("claude".into(), SkillState::UpToDate)],
        );
        both.location = SkillLocation::Both;
        both.mixed = true;
        both.shadowed_global_divergent = true;
        let none = row(
            "none",
            "source",
            vec![("claude".into(), SkillState::NotLoaded)],
        );

        let counts = status_summary_counts(&[global, project, both, none]);
        assert_eq!(
            counts,
            StatusSummaryCounts {
                up_to_date: 2,
                needs_update: 1,
                not_loaded: 1,
                no_connection: 0,
                global: 1,
                project: 1,
                both: 1,
                none: 1,
                mixed: 1,
                shadowed_global_divergent: 1,
            }
        );

        assert_eq!(
            status_summary_with_counts(&counts, false, false),
            "up-to-date: 2, needs-update: 1, not-loaded: 1, global: 1, project: 1, both: 1, none: 1, mixed placement: 1, shadowed global divergence: 1"
        );
        assert!(status_summary_with_counts(&counts, true, false).contains("— not installed: 1"));
    }

    #[test]
    fn placement_summary_omits_all_zero_location_categories() {
        let counts = StatusSummaryCounts {
            up_to_date: 27,
            not_loaded: 18,
            global: 9,
            none: 6,
            ..StatusSummaryCounts::default()
        };

        assert_eq!(
            status_summary_with_counts(&counts, true, false),
            "✓: 27 up-to-date, ✗: 18 not-loaded, 🌐 global: 9, — not installed: 6"
        );
        assert_eq!(
            status_summary_with_counts(&counts, false, false),
            "up-to-date: 27, not-loaded: 18, global: 9, none: 6"
        );
    }

    #[test]
    fn placement_summary_includes_each_nonzero_location_category_in_order() {
        let cases = [
            (
                StatusSummaryCounts {
                    global: 1,
                    ..StatusSummaryCounts::default()
                },
                "🌐 global: 1",
            ),
            (
                StatusSummaryCounts {
                    project: 1,
                    ..StatusSummaryCounts::default()
                },
                "📁 project: 1",
            ),
            (
                StatusSummaryCounts {
                    both: 1,
                    ..StatusSummaryCounts::default()
                },
                "↕ both: 1",
            ),
            (
                StatusSummaryCounts {
                    none: 1,
                    ..StatusSummaryCounts::default()
                },
                "— not installed: 1",
            ),
        ];

        for (counts, expected) in cases {
            assert_eq!(status_summary_with_counts(&counts, true, false), expected);
        }
    }
}
