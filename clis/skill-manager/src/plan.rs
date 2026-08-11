//! Shared change plans rendered before `import` and `update` mutate anything.
//!
//! Both commands answer the same question for a human reviewer: which skill
//! directories change, in which direction, and by how much. The renderer
//! therefore follows the [`crate::status`] conventions exactly, including
//! display-width alignment, optional compact symbols, and color that is scoped
//! to the semantic cells only.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::{Result, SkillManagerError};
use crate::skills::directory_files;
use crate::status::{display_width, join_columns, padded, separator};

/// Disposition of one file inside a planned directory replacement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileChange {
    /// The file exists only in the incoming tree.
    Added,
    /// The file exists in both trees with different content.
    Modified,
    /// The file exists only in the existing tree.
    Deleted,
}

impl FileChange {
    /// Stable plain-text label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }

    /// Compact single-character marker.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Added => "+",
            Self::Modified => "~",
            Self::Deleted => "-",
        }
    }

    const fn color(self) -> u8 {
        match self {
            Self::Added => 32,
            Self::Modified => 33,
            Self::Deleted => 31,
        }
    }
}

/// One changed file within a planned directory replacement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileDelta {
    /// Slash-separated path relative to the skill directory.
    pub path: String,
    /// Disposition of the file.
    pub change: FileChange,
    /// Added text lines; always zero for binary content.
    pub insertions: usize,
    /// Removed text lines; always zero for binary content.
    pub deletions: usize,
    /// Whether either side of the comparison is non-UTF-8 content.
    pub binary: bool,
    /// Signed byte delta, reported instead of line counts for binary content.
    pub bytes: i64,
}

/// Aggregate per-file changes for one planned directory replacement.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiffStat {
    /// Changed files ordered by their relative path.
    pub files: Vec<FileDelta>,
}

impl DiffStat {
    /// Whether the two directories already have identical content.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Number of changed files.
    #[must_use]
    pub fn files_changed(&self) -> usize {
        self.files.len()
    }

    /// Total added text lines.
    #[must_use]
    pub fn insertions(&self) -> usize {
        self.files.iter().map(|file| file.insertions).sum()
    }

    /// Total removed text lines.
    #[must_use]
    pub fn deletions(&self) -> usize {
        self.files.iter().map(|file| file.deletions).sum()
    }

    /// Number of changed files whose content is not UTF-8 text.
    #[must_use]
    pub fn binary_files(&self) -> usize {
        self.files.iter().filter(|file| file.binary).count()
    }
}

/// Compute the change plan for replacing `existing` content with `incoming`.
///
/// A missing directory is treated as empty, so a first-time deployment reports
/// every incoming file as added. Comparison uses relative regular-file paths
/// and exact content, matching [`crate::skills::directories_equal`].
///
/// # Errors
///
/// Returns an error for unsafe tree entries or filesystem reads.
pub fn diff_directories(existing: &Path, incoming: &Path) -> Result<DiffStat> {
    let before = directory_files(existing)?;
    let after = directory_files(incoming)?;
    let mut files = Vec::new();
    for (relative, path) in &before {
        let old = std::fs::read(path).map_err(|error| SkillManagerError::io(path, error))?;
        match after.get(relative) {
            None => files.push(deleted_delta(relative, &old)),
            Some(new_path) => {
                let new = std::fs::read(new_path)
                    .map_err(|error| SkillManagerError::io(new_path, error))?;
                if old != new {
                    files.push(modified_delta(relative, &old, &new));
                }
            }
        }
    }
    for (relative, path) in &after {
        if before.contains_key(relative) {
            continue;
        }
        let new = std::fs::read(path).map_err(|error| SkillManagerError::io(path, error))?;
        files.push(added_delta(relative, &new));
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(DiffStat { files })
}

fn added_delta(path: &str, new: &[u8]) -> FileDelta {
    let lines = text(new);
    FileDelta {
        path: path.to_owned(),
        change: FileChange::Added,
        insertions: lines.as_ref().map_or(0, Vec::len),
        deletions: 0,
        binary: lines.is_none(),
        bytes: byte_delta(0, new.len()),
    }
}

fn deleted_delta(path: &str, old: &[u8]) -> FileDelta {
    let lines = text(old);
    FileDelta {
        path: path.to_owned(),
        change: FileChange::Deleted,
        insertions: 0,
        deletions: lines.as_ref().map_or(0, Vec::len),
        binary: lines.is_none(),
        bytes: byte_delta(old.len(), 0),
    }
}

fn modified_delta(path: &str, old: &[u8], new: &[u8]) -> FileDelta {
    let bytes = byte_delta(old.len(), new.len());
    match (text(old), text(new)) {
        (Some(before), Some(after)) => {
            let (insertions, deletions) = line_delta(&before, &after);
            FileDelta {
                path: path.to_owned(),
                change: FileChange::Modified,
                insertions,
                deletions,
                binary: false,
                bytes,
            }
        }
        _ => FileDelta {
            path: path.to_owned(),
            change: FileChange::Modified,
            insertions: 0,
            deletions: 0,
            binary: true,
            bytes,
        },
    }
}

/// Split content into lines, or report binary content as `None`.
///
/// A NUL byte marks binary content even when the bytes decode as UTF-8, which
/// matches the conventional text/binary split used by diff tools.
fn text(bytes: &[u8]) -> Option<Vec<&str>> {
    if bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes)
        .ok()
        .map(|value| value.lines().collect())
}

fn byte_delta(before: usize, after: usize) -> i64 {
    i64::try_from(after).unwrap_or(i64::MAX) - i64::try_from(before).unwrap_or(i64::MAX)
}

/// Count changed lines after trimming the common prefix and suffix.
///
/// This deliberately avoids a full longest-common-subsequence pass. It is exact
/// for appended, prepended, and contiguous edits, and reports a conservative
/// upper bound for scattered edits, which is enough for a review decision.
fn line_delta(before: &[&str], after: &[&str]) -> (usize, usize) {
    let mut start = 0;
    while start < before.len() && start < after.len() && before[start] == after[start] {
        start += 1;
    }
    let mut end = 0;
    while end < before.len() - start
        && end < after.len() - start
        && before[before.len() - 1 - end] == after[after.len() - 1 - end]
    {
        end += 1;
    }
    (after.len() - start - end, before.len() - start - end)
}

/// Planned action for one skill, target, and scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanAction {
    /// Replace source content with a deployed copy.
    Import,
    /// Replace an existing deployment with source content.
    Update,
    /// Create a deployment that does not exist yet.
    Load,
    /// Delete an existing deployment.
    Remove,
    /// Leave identical content untouched.
    Skip,
}

impl PlanAction {
    /// Stable plain-text label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Update => "update",
            Self::Load => "load",
            Self::Remove => "remove",
            Self::Skip => "skip",
        }
    }

    /// Compact single-character marker.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Import => "←",
            Self::Update => "↑",
            Self::Load => "+",
            Self::Remove => "−",
            Self::Skip => "✓",
        }
    }

    /// Semantic ANSI color code, when this action has one.
    #[must_use]
    pub const fn color_code(self) -> Option<u8> {
        self.color()
    }

    const fn color(self) -> Option<u8> {
        match self {
            Self::Import | Self::Update => Some(33),
            Self::Load => Some(32),
            Self::Remove => Some(31),
            Self::Skip => None,
        }
    }
}

/// One reviewable row of a change plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanEntry {
    /// Planned action for this row.
    pub action: PlanAction,
    /// Portable skill name.
    pub skill: String,
    /// Human-facing target and scope, such as `claude (global)`.
    pub location: String,
    /// Per-file changes the action would apply.
    pub stat: DiffStat,
}

/// One changed skill in a grouped update plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupedUpdateEntry {
    /// Portable skill name.
    pub skill: String,
    /// Human-readable aggregate change description.
    pub change: String,
    /// One cell per selected target, containing the affected scope label.
    pub target_scopes: Vec<Option<String>>,
}

/// Render an update plan with one row per changed skill and one column per target.
#[must_use]
pub fn grouped_update_table(
    entries: &[GroupedUpdateEntry],
    target_names: &[String],
    color: bool,
) -> Vec<String> {
    let mut headers = vec!["skill".to_owned(), "change".to_owned()];
    headers.extend(target_names.iter().cloned());
    let mut widths = headers
        .iter()
        .map(|header| display_width(header))
        .collect::<Vec<_>>();
    let cells = entries
        .iter()
        .map(|entry| {
            let mut row = vec![entry.skill.clone(), entry.change.clone()];
            row.extend(entry.target_scopes.iter().map(|scope| {
                scope
                    .as_ref()
                    .map_or_else(|| "—".to_owned(), |scope| format!("↑ {scope}"))
            }));
            row
        })
        .collect::<Vec<_>>();
    for row in &cells {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(display_width(cell));
        }
    }
    let header = headers
        .iter()
        .enumerate()
        .map(|(index, value)| padded(value, widths[index]))
        .collect::<Vec<_>>();
    let mut lines = vec![join_columns(&header), separator(&widths)];
    for row in cells {
        let columns = row
            .iter()
            .enumerate()
            .map(|(index, cell)| {
                let padding = " ".repeat(widths[index].saturating_sub(display_width(cell)));
                if index >= 2 && cell.starts_with('↑') {
                    format!("{}{padding}", styled(cell, Some(33), color))
                } else {
                    format!("{cell}{padding}")
                }
            })
            .collect::<Vec<_>>();
        lines.push(join_columns(&columns));
    }
    lines
}

/// Render the shared action table used by `import` and `update`.
#[must_use]
pub fn plan_table(entries: &[PlanEntry], symbols: bool, color: bool) -> Vec<String> {
    let headers = ["action", "skill", "target", "change"];
    let mut widths = headers.map(str::len);
    let cells = entries
        .iter()
        .map(|entry| {
            [
                action_cell(entry.action, symbols).to_owned(),
                entry.skill.clone(),
                entry.location.clone(),
                change_cell(entry),
            ]
        })
        .collect::<Vec<_>>();
    for row in &cells {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(display_width(cell));
        }
    }

    let header = headers
        .iter()
        .enumerate()
        .map(|(index, name)| padded(name, widths[index]))
        .collect::<Vec<_>>();
    let mut lines = vec![join_columns(&header), separator(&widths)];
    for (entry, row) in entries.iter().zip(cells) {
        let mut columns = Vec::with_capacity(row.len());
        for (index, cell) in row.iter().enumerate() {
            if index == 0 {
                let padding = " ".repeat(widths[0].saturating_sub(display_width(cell)));
                columns.push(format!(
                    "{}{padding}",
                    styled(cell, entry.action.color(), color)
                ));
            } else {
                columns.push(padded(cell, widths[index]));
            }
        }
        lines.push(join_columns(&columns));
    }
    lines
}

/// Render one indented line per changed file, git-diff-stat style.
#[must_use]
pub fn file_change_lines(stat: &DiffStat, symbols: bool, color: bool) -> Vec<String> {
    let markers = stat
        .files
        .iter()
        .map(|file| {
            if symbols {
                file.change.symbol().to_owned()
            } else {
                file.change.as_str().to_owned()
            }
        })
        .collect::<Vec<_>>();
    let marker_width = markers
        .iter()
        .map(|marker| display_width(marker))
        .max()
        .unwrap_or(0);
    let path_width = stat
        .files
        .iter()
        .map(|file| display_width(&file.path))
        .max()
        .unwrap_or(0);

    stat.files
        .iter()
        .zip(markers)
        .map(|(file, marker)| {
            let padding = " ".repeat(marker_width.saturating_sub(display_width(&marker)));
            let marker = format!(
                "{}{padding}",
                styled(&marker, Some(file.change.color()), color)
            );
            format!(
                "  {}",
                join_columns(&[marker, padded(&file.path, path_width), delta_cell(file)])
            )
        })
        .collect()
}

/// Render the git-style totals line for one planned replacement.
#[must_use]
pub fn totals_line(stat: &DiffStat) -> String {
    if stat.is_empty() {
        return "no file changes".into();
    }
    let mut rendered = format!(
        "{} changed, +{}/-{}",
        pluralized(stat.files_changed(), "file"),
        stat.insertions(),
        stat.deletions()
    );
    if stat.binary_files() > 0 {
        let _written = write!(rendered, ", {} binary", stat.binary_files());
    }
    rendered
}

/// Render the first-write description, such as `new deployment, 2 files, +86/-0`.
///
/// `load` and `copy` differ only in the noun, so the shared plan renderer keeps
/// one grammar instead of two nearly identical sentences.
#[must_use]
pub fn creation_line(noun: &str, stat: &DiffStat) -> String {
    format!(
        "new {noun}, {}, +{}/-{}",
        pluralized(stat.files_changed(), "file"),
        stat.insertions(),
        stat.deletions()
    )
}

/// Render the one-line count summary below a plan table.
#[must_use]
pub fn plan_summary(entries: &[PlanEntry]) -> String {
    let counts = [
        (PlanAction::Import, "to import"),
        (PlanAction::Update, "to update"),
        (PlanAction::Load, "to load"),
        (PlanAction::Remove, "to remove"),
        (PlanAction::Skip, "unchanged"),
    ];
    let rendered = counts
        .iter()
        .filter_map(|(action, label)| {
            let count = entries
                .iter()
                .filter(|entry| entry.action == *action)
                .count();
            (count > 0).then(|| format!("{count} {label}"))
        })
        .collect::<Vec<_>>();
    if rendered.is_empty() {
        "nothing to do".into()
    } else {
        rendered.join(", ")
    }
}

fn action_cell(action: PlanAction, symbols: bool) -> &'static str {
    if symbols {
        action.symbol()
    } else {
        action.as_str()
    }
}

fn change_cell(entry: &PlanEntry) -> String {
    match entry.action {
        PlanAction::Skip => "up to date".into(),
        PlanAction::Remove => "remove deployment".into(),
        PlanAction::Load => format!(
            "new deployment, {}, +{}/-{}",
            pluralized(entry.stat.files_changed(), "file"),
            entry.stat.insertions(),
            entry.stat.deletions()
        ),
        PlanAction::Import | PlanAction::Update => totals_line(&entry.stat),
    }
}

fn delta_cell(file: &FileDelta) -> String {
    if file.binary {
        let sign = if file.bytes < 0 { "-" } else { "+" };
        return format!("bin {sign}{} bytes", file.bytes.unsigned_abs());
    }
    format!("+{}/-{}", file.insertions, file.deletions)
}

fn pluralized(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn styled(text: &str, code: Option<u8>, color: bool) -> String {
    match (color, code) {
        (true, Some(code)) => format!("\u{1b}[{code}m{text}\u{1b}[0m"),
        _ => text.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        DiffStat, FileChange, PlanAction, PlanEntry, diff_directories, file_change_lines,
        plan_summary, plan_table, totals_line,
    };

    fn write(root: &std::path::Path, relative: &str, body: &[u8]) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| unreachable!("{error}"));
        }
        fs::write(path, body).unwrap_or_else(|error| unreachable!("{error}"));
    }

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap_or_else(|error| unreachable!("{error}"));
        let existing = root.path().join("existing");
        let incoming = root.path().join("incoming");
        write(&existing, "SKILL.md", b"one\ntwo\nthree\n");
        write(&existing, "reference/stale.md", b"gone\nalso gone\n");
        write(&existing, "logo.png", &[0, 159, 146, 150]);
        write(&incoming, "SKILL.md", b"one\nchanged\nthree\n");
        write(&incoming, "reference/new.md", b"fresh\ncontent\n");
        write(&incoming, "logo.png", &[0, 159, 146, 150, 7, 7]);
        (root, existing, incoming)
    }

    #[test]
    fn diff_reports_added_modified_deleted_and_binary_files() {
        let (_root, existing, incoming) = fixture();
        let stat =
            diff_directories(&existing, &incoming).unwrap_or_else(|error| unreachable!("{error}"));

        assert_eq!(stat.files_changed(), 4);
        assert_eq!(stat.insertions(), 3);
        assert_eq!(stat.deletions(), 3);
        assert_eq!(stat.binary_files(), 1);
        assert_eq!(
            stat.files
                .iter()
                .map(|file| (file.path.as_str(), file.change))
                .collect::<Vec<_>>(),
            [
                ("SKILL.md", FileChange::Modified),
                ("logo.png", FileChange::Modified),
                ("reference/new.md", FileChange::Added),
                ("reference/stale.md", FileChange::Deleted),
            ]
        );
        assert_eq!(totals_line(&stat), "4 files changed, +3/-3, 1 binary");
    }

    #[test]
    fn diff_treats_a_missing_directory_as_empty_and_equal_trees_as_no_change() {
        let (root, existing, _incoming) = fixture();
        let missing = root.path().join("missing");
        let fresh =
            diff_directories(&missing, &existing).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(fresh.files_changed(), 3);
        assert_eq!(fresh.deletions(), 0);
        assert_eq!(fresh.insertions(), 5);

        let unchanged =
            diff_directories(&existing, &existing).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(unchanged.is_empty());
        assert_eq!(totals_line(&unchanged), "no file changes");
    }

    #[test]
    fn binary_and_single_file_rendering_stays_readable_in_both_modes() {
        let (_root, existing, incoming) = fixture();
        let stat =
            diff_directories(&existing, &incoming).unwrap_or_else(|error| unreachable!("{error}"));

        let symbols = file_change_lines(&stat, true, false);
        assert_eq!(
            symbols,
            [
                "  ~  SKILL.md            +1/-1",
                "  ~  logo.png            bin +2 bytes",
                "  +  reference/new.md    +2/-0",
                "  -  reference/stale.md  +0/-2",
            ]
        );

        let plain = file_change_lines(&stat, false, false);
        assert!(plain[0].starts_with("  modified  SKILL.md"));
        assert!(plain[3].starts_with("  deleted   reference/stale.md"));
        assert!(plain.iter().all(|line| !line.contains('\u{1b}')));

        let colored = file_change_lines(&stat, true, true);
        assert!(colored[0].contains("\u{1b}[33m~\u{1b}[0m"));
        assert!(colored[2].contains("\u{1b}[32m+\u{1b}[0m"));
        assert!(colored[3].contains("\u{1b}[31m-\u{1b}[0m"));
    }

    #[test]
    fn plan_table_aligns_actions_and_scopes_color_to_the_action_cell() {
        let entries = vec![
            PlanEntry {
                action: PlanAction::Update,
                skill: "alpha".into(),
                location: "claude (global)".into(),
                stat: DiffStat {
                    files: vec![super::FileDelta {
                        path: "SKILL.md".into(),
                        change: FileChange::Modified,
                        insertions: 3,
                        deletions: 1,
                        binary: false,
                        bytes: 4,
                    }],
                },
            },
            PlanEntry {
                action: PlanAction::Skip,
                skill: "beta".into(),
                location: "shared (project)".into(),
                stat: DiffStat::default(),
            },
            PlanEntry {
                action: PlanAction::Load,
                skill: "gamma".into(),
                location: "shared (global)".into(),
                stat: DiffStat {
                    files: vec![super::FileDelta {
                        path: "SKILL.md".into(),
                        change: FileChange::Added,
                        insertions: 9,
                        deletions: 0,
                        binary: false,
                        bytes: 40,
                    }],
                },
            },
        ];

        let plain = plan_table(&entries, false, false);
        assert_eq!(
            plain,
            [
                "action  skill  target            change",
                "------  -----  ----------------  -----------------------------",
                "update  alpha  claude (global)   1 file changed, +3/-1",
                "skip    beta   shared (project)  up to date",
                "load    gamma  shared (global)   new deployment, 1 file, +9/-0",
            ]
        );
        assert_eq!(
            plan_summary(&entries),
            "1 to update, 1 to load, 1 unchanged"
        );
        assert_eq!(plan_summary(&[]), "nothing to do");

        let decorated = plan_table(&entries, true, true);
        assert!(!decorated[0].contains('\u{1b}'));
        assert!(decorated[2].contains("\u{1b}[33m↑\u{1b}[0m"));
        assert!(decorated[3].contains('✓') && !decorated[3].contains('\u{1b}'));
        assert!(decorated[4].contains("\u{1b}[32m+\u{1b}[0m"));
    }

    #[test]
    fn import_rows_reuse_the_shared_table_and_totals() {
        let entries = vec![PlanEntry {
            action: PlanAction::Import,
            skill: "alpha".into(),
            location: "claude (global)".into(),
            stat: DiffStat {
                files: vec![super::FileDelta {
                    path: "SKILL.md".into(),
                    change: FileChange::Modified,
                    insertions: 2,
                    deletions: 2,
                    binary: false,
                    bytes: 0,
                }],
            },
        }];
        let lines = plan_table(&entries, true, false);
        assert!(lines[2].starts_with('←'));
        assert!(lines[2].ends_with("1 file changed, +2/-2"));
        assert_eq!(plan_summary(&entries), "1 to import");
    }
}
