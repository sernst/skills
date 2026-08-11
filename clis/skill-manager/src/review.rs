//! Shared plan-review surface for every mutating command.
//!
//! A mutating command must render a complete semantic plan before it asks the
//! user anything, and every rendering must hide whatever carries no information
//! for the current plan revision. This module owns that shared vocabulary so
//! `load`, `update`, `remove`, `copy`, and `import` describe their work with one
//! set of types, one renderer, and one structured plan payload builder.
//!
//! The model separates the two things a plan can contain:
//!
//! * **Answers** — [`PlannedAction`]s already decided against a [`Destination`],
//!   plus [`PlanRow::availability`], which records only that an item *exists*
//!   somewhere. Availability is deliberately not an action: `remove` must be
//!   able to show where a skill lives before anything has been chosen, and
//!   rendering that as an action would claim an operation the user never
//!   authorized.
//! * **Questions** — [`Decision`]s, each one unresolved authorization dimension
//!   with its own alternatives. A [`DecisionOption`] carries its own consequence
//!   preview, including nested destination-level previews, so `import` can show
//!   what every candidate source copy would propagate before the first prompt.
//!   Answering a dimension sets [`Decision::resolved`], and a resolved dimension
//!   is gated out of every later render.
//!
//! The model is also destination-generic. A [`Destination`] is one place a plan
//! can write: a target/scope deployment for `load`, `update`, and `remove`; an
//! arbitrary filesystem path for `copy`; and a canonical source collection for
//! `import`. Destinations that share a [`Destination::column`] become one matrix
//! column, which is why per-cell scope (`↕ both`) and the matrix layout fall out
//! of the same data rather than needing a second model.
//!
//! Rendering reuses the [`crate::status`] width helpers so plan tables, status
//! tables, and diff blocks stay aligned by Unicode display width, and reuses
//! [`crate::plan::PlanAction`] so symbols and colors remain shared product
//! language instead of per-command synonyms.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::authorize::CANCEL_TOKEN;
use crate::domain::Scope;
use crate::plan::{DiffStat, PlanAction};
use crate::status::{COLUMN_GAP, SkillLocation, display_width, join_columns, padded, separator};

/// Indentation of a decision option line.
const OPTION_INDENT: usize = 2;
/// Indentation added inside a nested preview block.
const NESTED_INDENT: usize = 2;

/// Output capability that selects the symbol or word vocabulary and color.
///
/// Significance gating is identical in both modes; only the rendered tokens
/// differ, so redirected output stays a faithful transcript of what a terminal
/// user would have reviewed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderStyle {
    /// Whether compact Unicode symbols may replace stable words.
    pub symbols: bool,
    /// Whether semantic ANSI color may be applied.
    pub color: bool,
}

impl RenderStyle {
    /// Style for a redirected, non-interactive stream.
    #[must_use]
    pub const fn plain() -> Self {
        Self {
            symbols: false,
            color: false,
        }
    }
}

/// What one planned destination actually is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DestinationKind {
    /// A target and scope pair, used by `load`, `update`, and `remove`.
    Deployment {
        /// Configured target name.
        target: String,
        /// Installation scope of this destination.
        scope: Scope,
    },
    /// An arbitrary filesystem directory, used by `copy`.
    Path,
    /// A canonical source collection, used by `import`.
    Source {
        /// Command-facing source name.
        source: String,
    },
}

impl DestinationKind {
    /// Stable machine label for the structured plan event.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Deployment { .. } => "deployment",
            Self::Path => "path",
            Self::Source { .. } => "source",
        }
    }

    /// Installation scope, when this destination has one.
    #[must_use]
    pub const fn scope(&self) -> Option<Scope> {
        match self {
            Self::Deployment { scope, .. } => Some(*scope),
            Self::Path | Self::Source { .. } => None,
        }
    }
}

/// One place a plan can write.
#[derive(Clone, Debug)]
pub struct Destination {
    /// Stable machine identity, such as `claude:global`.
    pub id: String,
    /// Matrix column this destination belongs to, such as `claude`.
    pub column: String,
    /// Human-facing label, such as `claude · global`.
    pub label: String,
    /// What the destination is.
    pub kind: DestinationKind,
    /// Resolved directory, when the destination has one.
    pub path: Option<PathBuf>,
}

/// One planned write against exactly one destination.
#[derive(Clone, Debug)]
pub struct PlannedAction {
    /// [`Destination::id`] this action writes to.
    pub destination: String,
    /// Semantic operation.
    pub action: PlanAction,
    /// Whether the destination already existed.
    pub existed: bool,
    /// Human-facing change description, such as `2 files changed, +11/-5`.
    pub description: String,
    /// Per-file changes the action would apply.
    pub stat: DiffStat,
}

/// One reviewable item, everything it will do, and everywhere it exists.
#[derive(Clone, Debug, Default)]
pub struct PlanRow {
    /// Identity column value, normally a skill name.
    pub identity: String,
    /// Optional provenance, rendered only when rows disagree.
    pub provenance: Option<String>,
    /// Optional non-diff metric, such as `remove`'s deployed file count.
    pub metric: Option<String>,
    /// Planned writes for this item.
    pub actions: Vec<PlannedAction>,
    /// [`Destination::id`]s where this item exists without a decided action.
    ///
    /// Availability is evidence, not an operation. `remove` lists where a skill
    /// currently lives so its scope options mean something; rendering those
    /// cells as actions would claim writes the user has not authorized.
    pub availability: Vec<String>,
}

impl PlanRow {
    /// Whether this row carries no information a human reviewer needs to see.
    ///
    /// A row is dormant when every planned action is a no-op
    /// [`PlanAction::Skip`] and it lists no bare availability either — the
    /// exact shape `load` gives an already-identical deployment so it can
    /// still be counted precisely (`existed`, a stable destination id, an
    /// empty diff) in the machine event, while never earning a table row, a
    /// column, or a progress line a human did not need. Significance gating
    /// hides dormant rows from rendering; it never removes them from
    /// [`ChangePlan::rows`] itself, so the structured `plan` event — which
    /// walks `rows` directly — stays complete.
    #[must_use]
    pub fn is_dormant(&self) -> bool {
        self.availability.is_empty()
            && !self.actions.is_empty()
            && self
                .actions
                .iter()
                .all(|action| action.action == PlanAction::Skip)
    }
}

/// One nested consequence line inside a preview block or option field.
#[derive(Clone, Debug, Default)]
pub struct PreviewEntry {
    /// Leading marker, such as a file-change symbol.
    pub marker: Option<String>,
    /// ANSI code for the marker.
    pub marker_color: Option<u8>,
    /// Aligned label, such as a file path or a destination label.
    pub label: String,
    /// Consequence text for this entry.
    pub value: String,
    /// ANSI code for the value.
    pub value_color: Option<u8>,
}

/// A headed block of aligned consequence lines.
#[derive(Clone, Debug, Default)]
pub struct PreviewBlock {
    /// Section heading, such as `Propagation preview`.
    pub heading: String,
    /// Optional value rendered on the heading line, such as `5 deployments`.
    pub heading_value: Option<String>,
    /// Optional unlabelled summary line rendered before the entries.
    pub lead: Option<String>,
    /// ANSI code for the lead line.
    pub lead_color: Option<u8>,
    /// Nested consequence lines.
    pub entries: Vec<PreviewEntry>,
}

/// One aligned `label  value` pair inside an option, with optional nesting.
#[derive(Clone, Debug, Default)]
pub struct PreviewField {
    /// Field label, such as `Path` or `Source`.
    pub label: String,
    /// Field value.
    pub value: String,
    /// ANSI code for the value.
    pub value_color: Option<u8>,
    /// Consequence lines belonging to this field.
    pub entries: Vec<PreviewEntry>,
}

/// One piece of an option's consequence preview.
#[derive(Clone, Debug)]
pub enum OptionDetail {
    /// Aligned `label  value` pairs sharing one label width.
    Fields(Vec<PreviewField>),
    /// A headed block of nested consequence lines.
    Block(PreviewBlock),
    /// One free-form explanatory line.
    Note(String),
}

/// Machine-visible consequences of choosing one alternative.
///
/// The rendered `detail` is command-authored prose; this is the same
/// information typed, so the structured event describes the plan the user was
/// actually shown rather than a summary of it. A command that gives an option a
/// preview MUST give it the matching consequence.
#[derive(Clone, Debug, Default)]
pub struct OptionConsequence {
    /// The uniform operation this option performs, when it has one.
    pub operation: Option<PlanAction>,
    /// Filesystem path this option identifies, such as one source copy.
    pub path: Option<PathBuf>,
    /// Per-destination writes this option would perform.
    pub actions: Vec<PlannedAction>,
    /// Named aggregate counts, such as `deployments` and `files`.
    ///
    /// These carry a blast radius too large to enumerate per destination, which
    /// is how `remove` states each scope option's cost across every skill.
    pub totals: Vec<(String, u64)>,
}

impl OptionConsequence {
    /// Whether this option carries no machine-visible consequence at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operation.is_none()
            && self.path.is_none()
            && self.actions.is_empty()
            && self.totals.is_empty()
    }
}

/// One alternative of an unresolved authorization dimension.
#[derive(Clone, Debug, Default)]
pub struct DecisionOption {
    /// Stable machine identity, such as `shared:global` or `no-update`.
    pub id: String,
    /// Single token the user types to choose this option.
    pub token: String,
    /// Human label rendered on the option line.
    pub label: String,
    /// Whether this option is guidance; a recommendation is never a default.
    pub recommended: bool,
    /// Short consequence clause on the option line.
    pub effect: Option<String>,
    /// ANSI code for the effect clause.
    pub effect_color: Option<u8>,
    /// This option's own consequence preview.
    pub detail: Vec<OptionDetail>,
    /// The same consequence, typed for the structured plan event.
    pub consequence: OptionConsequence,
}

/// One authorization dimension, answered or still open.
///
/// A dimension is rendered as part of the complete plan, never as a question
/// asked before the plan exists. Once [`Decision::resolved`] is set the whole
/// dimension is gated out of later renders and its answer becomes ordinary plan
/// metadata supplied by the command.
#[derive(Clone, Debug, Default)]
pub struct Decision {
    /// Stable dimension name, such as `removal_scope` or `source_copy`.
    pub id: String,
    /// Section heading used while this dimension is the one being asked.
    pub heading: Option<String>,
    /// Section heading used while this dimension is still deferred.
    ///
    /// A dimension the user can see but cannot yet answer needs to say so, and
    /// it usually says it differently from the same dimension once it is live.
    pub deferred_heading: Option<String>,
    /// Optional line rendered between the heading and the options.
    pub preamble: Option<String>,
    /// Prompt stem, such as `Select removal scope`.
    pub prompt: String,
    /// Mutually exclusive alternatives in least-destructive-first order.
    pub options: Vec<DecisionOption>,
    /// The chosen [`DecisionOption::id`], once this dimension is answered.
    pub resolved: Option<String>,
}

impl Decision {
    /// The chosen option, when this dimension has been answered.
    #[must_use]
    pub fn selected(&self) -> Option<&DecisionOption> {
        let id = self.resolved.as_deref()?;
        self.options.iter().find(|option| option.id == id)
    }
}

/// A complete semantic plan revision, rendered before any prompt.
#[derive(Clone, Debug)]
pub struct ChangePlan {
    /// Command name used by the structured plan event.
    pub command: String,
    /// Stable plan identity shared by every revision.
    pub plan_id: String,
    /// Cyan-bold section heading, such as `Update plan`.
    pub heading: String,
    /// Labelled metadata lines rendered once instead of per row.
    pub metadata: Vec<(String, String)>,
    /// Every destination in configured order.
    pub destinations: Vec<Destination>,
    /// Optional heading above the matrix, such as `Available deployments`.
    pub body_heading: Option<String>,
    /// Header for the optional non-diff metric column.
    pub metric_header: Option<String>,
    /// Heading of the indented per-destination detail block.
    pub detail_heading: String,
    /// Connector used by the degenerate one-item sentence, such as `->`.
    pub connector: Option<String>,
    /// Reviewable items.
    pub rows: Vec<PlanRow>,
    /// Resolved consequence blocks rendered below the matrix.
    pub blocks: Vec<PreviewBlock>,
    /// Authorization dimensions in prompt order.
    pub decisions: Vec<Decision>,
    /// Whether a prompt follows this revision, which is what earns a cancel line.
    pub prompting: bool,
    /// Whether the structured `summary` buckets nonzero action counts by
    /// `new`/`overwrite` (from [`PlannedAction::existed`]) instead of by the
    /// rendered action word.
    ///
    /// `load` and `copy` render two different cell words for a "new" write
    /// (`load`/`copy`) but share `update`'s word for an overwrite, because the
    /// cell vocabulary is about *what a destination looked like before*, not
    /// which command is running. The summary should report the plan's own
    /// footer categories (`new`/`overwrite`) rather than an ambiguous cell
    /// word that would collide with the unrelated `update` command's summary
    /// key. `update`, `import`, and `remove` keep one operation meaning per
    /// action word, so they leave this `false` and bucket by the action word
    /// as before. A [`PlanAction::Skip`] entry always buckets as `skip`
    /// regardless of this flag — it is neither a new write nor an overwrite.
    pub distinguishes_overwrites: bool,
}

impl ChangePlan {
    /// Compute the significance-gated view of this plan revision.
    #[must_use]
    pub fn view(&self) -> PlanView<'_> {
        let by_id = self
            .destinations
            .iter()
            .map(|destination| (destination.id.as_str(), destination))
            .collect::<BTreeMap<_, _>>();
        let mut referenced = Vec::new();
        let mut scopes = BTreeSet::new();
        let mut actions = 0_usize;
        let mut availability = 0_usize;
        for row in &self.rows {
            // Totals stay complete over every row so the structured event and
            // `is_empty` reflect the whole plan; only the rendering-facing
            // column/scope set below is restricted to what a human actually
            // sees, because a dormant row's destination must not keep an
            // otherwise-all-none column alive in the table.
            actions += row.actions.len();
            availability += row.availability.len();
            if row.is_dormant() {
                continue;
            }
            // A `Skip` action is a no-op: it must not keep an otherwise
            // all-none column alive, or `uniform_scope` uniform just because
            // one row happened to also touch it with real work elsewhere.
            let action_ids = row
                .actions
                .iter()
                .filter(|action| action.action != PlanAction::Skip)
                .map(|action| action.destination.as_str());
            let availability_ids = row.availability.iter().map(String::as_str);
            for id in action_ids.chain(availability_ids) {
                let Some(destination) = by_id.get(id) else {
                    continue;
                };
                if !referenced.contains(&destination.column) {
                    referenced.push(destination.column.clone());
                }
                if let Some(scope) = destination.kind.scope() {
                    scopes.insert(scope);
                }
            }
        }
        // Columns must keep configured destination order rather than the order
        // in which rows happened to mention them.
        let columns = self
            .destinations
            .iter()
            .map(|destination| destination.column.clone())
            .filter(|column| referenced.contains(column))
            .fold(Vec::new(), |mut ordered: Vec<String>, column| {
                if !ordered.contains(&column) {
                    ordered.push(column);
                }
                ordered
            });
        let uniform_scope = (scopes.len() == 1)
            .then(|| scopes.iter().next().copied())
            .flatten();
        PlanView {
            plan: self,
            by_id,
            columns,
            uniform_scope,
            actions,
            availability,
        }
    }
}

/// One plan revision after significance gating.
#[derive(Debug)]
pub struct PlanView<'a> {
    plan: &'a ChangePlan,
    by_id: BTreeMap<&'a str, &'a Destination>,
    columns: Vec<String>,
    uniform_scope: Option<Scope>,
    actions: usize,
    availability: usize,
}

impl PlanView<'_> {
    /// Surviving destination columns in configured order.
    #[must_use]
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// The single scope shared by every referenced destination, when there is one.
    #[must_use]
    pub const fn uniform_scope(&self) -> Option<Scope> {
        self.uniform_scope
    }

    /// Total planned writes, which is the plan's blast radius.
    #[must_use]
    pub const fn actions(&self) -> usize {
        self.actions
    }

    /// Dimensions still awaiting an answer, in prompt order.
    #[must_use]
    pub fn decisions(&self) -> Vec<&Decision> {
        self.plan
            .decisions
            .iter()
            .filter(|decision| decision.resolved.is_none())
            .collect()
    }

    /// Whether this revision has anything to review.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.actions == 0 && self.availability == 0 && self.decisions().is_empty()
    }

    /// Whether per-cell location must be rendered.
    ///
    /// A uniform single scope is either hoisted to one metadata line or already
    /// stated by an explicit flag, so repeating it in every cell would consume
    /// space without adding information. `↕ both` is never uniform in this
    /// sense because it doubles the deployment count.
    #[must_use]
    pub const fn show_locations(&self) -> bool {
        self.uniform_scope.is_none()
    }

    /// Rows a human reviewer needs to see, in plan order.
    ///
    /// A dormant row (every action a no-op skip, no bare availability) is
    /// evidence for the machine event only — [`plan_event_data`] still walks
    /// [`ChangePlan::rows`] directly and reports it precisely — but it earns
    /// no table row, no column, and no progress line here.
    #[must_use]
    pub fn visible_rows(&self) -> Vec<&PlanRow> {
        self.plan
            .rows
            .iter()
            .filter(|row| !row.is_dormant())
            .collect()
    }

    /// Rows whose destinations disagree about their change description.
    ///
    /// A [`PlanAction::Skip`] carries no description and is never itself a
    /// divergence — a no-op destination sitting alongside a real change must
    /// not force detail-row expansion, or every dormant destination that
    /// happens to share a row with a genuine action would spuriously earn a
    /// "N destination-specific changes" grouping.
    #[must_use]
    pub fn detail_rows(&self) -> Vec<&PlanRow> {
        self.visible_rows()
            .into_iter()
            .filter(|row| Self::descriptions(row).len() > 1)
            .collect()
    }

    fn descriptions(row: &PlanRow) -> BTreeSet<&str> {
        row.actions
            .iter()
            .filter(|action| action.action != PlanAction::Skip)
            .map(|action| action.description.as_str())
            .collect()
    }

    fn change_cell(row: &PlanRow) -> String {
        let descriptions = Self::descriptions(row);
        match descriptions.len() {
            0 => String::new(),
            1 => descriptions
                .into_iter()
                .next()
                .unwrap_or_default()
                .to_owned(),
            count => format!("{count} destination-specific changes"),
        }
    }

    fn cell_actions<'r>(&self, row: &'r PlanRow, column: &str) -> Vec<&'r PlannedAction> {
        self.plan
            .destinations
            .iter()
            .filter(|destination| destination.column == column)
            .filter_map(|destination| {
                row.actions
                    .iter()
                    .find(|action| action.destination == destination.id)
            })
            .collect()
    }

    fn cell_availability(&self, row: &PlanRow, column: &str) -> BTreeSet<Scope> {
        self.plan
            .destinations
            .iter()
            .filter(|destination| destination.column == column)
            .filter(|destination| row.availability.contains(&destination.id))
            .filter_map(|destination| destination.kind.scope())
            .collect()
    }

    fn cell(&self, row: &PlanRow, column: &str, style: RenderStyle) -> RenderedCell {
        let actions = self.cell_actions(row, column);
        if actions.is_empty() {
            // An availability-only cell is pure evidence: its whole content is
            // where the item exists, so the location is never elided here.
            let scopes = self.cell_availability(row, column);
            let text = location_of(&scopes).map_or_else(
                || none_text(style.symbols).to_owned(),
                |location| location_text(location, style.symbols).to_owned(),
            );
            return RenderedCell {
                plain: text.clone(),
                styled: text,
            };
        }
        let mut groups: Vec<(PlanAction, BTreeSet<Scope>)> = Vec::new();
        for action in actions {
            let scope = self
                .by_id
                .get(action.destination.as_str())
                .and_then(|destination| destination.kind.scope());
            if let Some(group) = groups
                .iter_mut()
                .find(|(operation, _)| *operation == action.action)
            {
                group.1.extend(scope);
            } else {
                groups.push((action.action, scope.into_iter().collect()));
            }
        }
        let mut plain = Vec::with_capacity(groups.len());
        let mut styled = Vec::with_capacity(groups.len());
        for (operation, scopes) in groups {
            let mut text = action_text(operation, style.symbols).to_owned();
            if self.show_locations()
                && let Some(location) = location_of(&scopes)
            {
                text.push(' ');
                text.push_str(location_text(location, style.symbols));
            }
            styled.push(colored(&text, operation.color_code(), style.color));
            plain.push(text);
        }
        RenderedCell {
            plain: plain.join(" / "),
            styled: styled.join(" / "),
        }
    }
}

struct RenderedCell {
    plain: String,
    styled: String,
}

/// Render one complete plan revision with recursive significance gating.
///
/// The caller supplies the footer because footer grammar is command-specific;
/// [`result_footer`] and [`destination_label`] keep that copy consistent.
#[must_use]
pub fn render_plan(view: &PlanView<'_>, style: RenderStyle) -> Vec<String> {
    let plan = view.plan;
    let mut chunks = vec![vec![heading(&plan.heading, style.color)]];
    if !plan.metadata.is_empty() {
        let width = plan
            .metadata
            .iter()
            .map(|(label, _)| display_width(label))
            .max()
            .unwrap_or(0);
        chunks.push(
            plan.metadata
                .iter()
                .map(|(label, value)| join_columns(&[padded(label, width), value.clone()]))
                .collect(),
        );
    }
    if !view.visible_rows().is_empty() {
        if let Some(body_heading) = &plan.body_heading {
            chunks.push(vec![heading(body_heading, style.color)]);
        }
        chunks.push(render_body(view, style));
        chunks.push(render_details(view));
    }
    for block in &plan.blocks {
        chunks.push(render_block(block, 0, style));
    }
    for (index, decision) in view.decisions().into_iter().enumerate() {
        let active = plan.prompting && index == 0;
        chunks.extend(render_decision(decision, active, style));
    }

    let mut lines = Vec::new();
    for chunk in chunks {
        if chunk.is_empty() {
            continue;
        }
        lines.extend(chunk);
        lines.push(String::new());
    }
    lines
}

/// Render the table, or the degenerate sentence form when a table would not
/// earn its header.
///
/// A table needs at least two rows or at least two significant destinations;
/// one item at one destination is a sentence.
fn render_body(view: &PlanView<'_>, style: RenderStyle) -> Vec<String> {
    let rows = view.visible_rows();
    let visible_actions = rows.iter().map(|row| row.actions.len()).sum::<usize>();
    let visible_availability = rows.iter().map(|row| row.availability.len()).sum::<usize>();
    if rows.len() == 1 && visible_actions == 1 && visible_availability == 0 {
        return rows
            .into_iter()
            .flat_map(|row| render_sentence(view, row, style))
            .collect();
    }
    render_table(view, style)
}

fn render_sentence(view: &PlanView<'_>, row: &PlanRow, style: RenderStyle) -> Vec<String> {
    let Some(action) = row.actions.first() else {
        return Vec::new();
    };
    let marker = colored(
        action_text(action.action, style.symbols),
        action.action.color_code(),
        style.color,
    );
    let destination = view
        .by_id
        .get(action.destination.as_str())
        .map(|destination| destination.column.clone())
        .unwrap_or_default();
    let connector = view
        .plan
        .connector
        .as_ref()
        .map_or_else(String::new, |word| format!(" {word} {destination}"));
    let change = row
        .metric
        .clone()
        .unwrap_or_else(|| action.description.clone());
    vec![format!("{marker} {}{connector}: {change}", row.identity)]
}

fn render_table(view: &PlanView<'_>, style: RenderStyle) -> Vec<String> {
    let plan = view.plan;
    let visible = view.visible_rows();
    let provenance = visible
        .iter()
        .filter_map(|row| row.provenance.clone())
        .collect::<BTreeSet<_>>();
    let show_provenance = provenance.len() > 1;
    let show_metric =
        plan.metric_header.is_some() && visible.iter().any(|row| row.metric.is_some());
    let changes = visible
        .iter()
        .map(|row| PlanView::change_cell(row))
        .collect::<Vec<_>>();
    let show_change = changes.iter().any(|change| !change.is_empty());

    let mut headers = vec!["skill".to_owned()];
    if show_provenance {
        headers.push("source".to_owned());
    }
    if let Some(header) = plan.metric_header.clone()
        && show_metric
    {
        headers.push(header);
    }
    if show_change {
        headers.push("change".to_owned());
    }
    headers.extend(view.columns.iter().cloned());

    let mut rows = Vec::with_capacity(visible.len());
    for (row, change) in visible.iter().zip(changes) {
        let mut measured = vec![row.identity.clone()];
        let mut rendered = vec![row.identity.clone()];
        if show_provenance {
            let value = row.provenance.clone().unwrap_or_default();
            measured.push(value.clone());
            rendered.push(value);
        }
        if show_metric {
            let value = row.metric.clone().unwrap_or_default();
            measured.push(value.clone());
            rendered.push(value);
        }
        if show_change {
            measured.push(change.clone());
            rendered.push(change);
        }
        for column in &view.columns {
            let cell = view.cell(row, column, style);
            measured.push(cell.plain);
            rendered.push(cell.styled);
        }
        rows.push((measured, rendered));
    }

    let mut widths = headers
        .iter()
        .map(|header| display_width(header))
        .collect::<Vec<_>>();
    for (measured, _) in &rows {
        for (index, cell) in measured.iter().enumerate() {
            widths[index] = widths[index].max(display_width(cell));
        }
    }
    let header = headers
        .iter()
        .enumerate()
        .map(|(index, value)| padded(value, widths[index]))
        .collect::<Vec<_>>();
    let mut lines = vec![join_columns(&header), separator(&widths)];
    for (measured, rendered) in rows {
        let columns = measured
            .iter()
            .zip(rendered)
            .enumerate()
            .map(|(index, (raw, styled))| {
                let padding = " ".repeat(widths[index].saturating_sub(display_width(raw)));
                format!("{styled}{padding}")
            })
            .collect::<Vec<_>>();
        lines.push(join_columns(&columns));
    }
    lines
}

fn render_details(view: &PlanView<'_>) -> Vec<String> {
    let detail_rows = view.detail_rows();
    if detail_rows.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![view.plan.detail_heading.clone()];
    for row in detail_rows {
        lines.push(format!("  {}", row.identity));
        let entries = view
            .plan
            .destinations
            .iter()
            .filter_map(|destination| {
                row.actions
                    .iter()
                    .find(|action| {
                        action.destination == destination.id && action.action != PlanAction::Skip
                    })
                    .map(|action| {
                        let label = if view.show_locations() {
                            destination.label.clone()
                        } else {
                            destination.column.clone()
                        };
                        (label, action.description.clone())
                    })
            })
            .collect::<Vec<_>>();
        let width = entries
            .iter()
            .map(|(label, _)| display_width(label))
            .max()
            .unwrap_or(0);
        for (label, description) in entries {
            lines.push(format!(
                "    {}",
                join_columns(&[padded(&label, width), description])
            ));
        }
    }
    lines
}

/// Render aligned consequence lines, such as a file list or a propagation preview.
fn render_entries(entries: &[PreviewEntry], indent: usize, style: RenderStyle) -> Vec<String> {
    let marker_width = entries
        .iter()
        .filter_map(|entry| entry.marker.as_deref())
        .map(display_width)
        .max()
        .unwrap_or(0);
    let label_width = entries
        .iter()
        .map(|entry| display_width(&entry.label))
        .max()
        .unwrap_or(0);
    let margin = " ".repeat(indent);
    entries
        .iter()
        .map(|entry| {
            let mut columns = Vec::new();
            if marker_width > 0 {
                let marker = entry.marker.clone().unwrap_or_default();
                let padding = " ".repeat(marker_width.saturating_sub(display_width(&marker)));
                columns.push(format!(
                    "{}{padding}",
                    colored(&marker, entry.marker_color, style.color)
                ));
            }
            let padding = " ".repeat(label_width.saturating_sub(display_width(&entry.label)));
            columns.push(format!("{}{padding}", entry.label));
            columns.push(colored(&entry.value, entry.value_color, style.color));
            format!("{margin}{}", join_columns(&columns))
        })
        .collect()
}

/// Render one headed consequence block and its nested entries.
fn render_block(block: &PreviewBlock, indent: usize, style: RenderStyle) -> Vec<String> {
    let margin = " ".repeat(indent);
    let title = heading(&block.heading, style.color);
    let mut lines = vec![match &block.heading_value {
        Some(value) => format!("{margin}{}", join_columns(&[title, value.clone()])),
        None => format!("{margin}{title}"),
    }];
    if let Some(lead) = &block.lead {
        lines.push(format!(
            "{margin}{}{}",
            " ".repeat(NESTED_INDENT),
            colored(lead, block.lead_color, style.color)
        ));
    }
    lines.extend(render_entries(
        &block.entries,
        indent + NESTED_INDENT,
        style,
    ));
    lines
}

/// Render one option's own consequence preview.
fn render_option_detail(detail: &[OptionDetail], indent: usize, style: RenderStyle) -> Vec<String> {
    let margin = " ".repeat(indent);
    let mut lines = Vec::new();
    for item in detail {
        match item {
            OptionDetail::Note(text) => lines.push(format!("{margin}{text}")),
            OptionDetail::Block(block) => lines.extend(render_block(block, indent, style)),
            OptionDetail::Fields(fields) => {
                let width = fields
                    .iter()
                    .map(|field| display_width(&field.label))
                    .max()
                    .unwrap_or(0);
                for field in fields {
                    lines.push(format!(
                        "{margin}{}",
                        join_columns(&[
                            padded(&field.label, width),
                            colored(&field.value, field.value_color, style.color),
                        ])
                    ));
                    lines.extend(render_entries(
                        &field.entries,
                        indent + NESTED_INDENT,
                        style,
                    ));
                }
            }
        }
    }
    lines
}

/// Render one unresolved dimension as blank-line-separated chunks.
///
/// Options that carry a consequence preview each become their own chunk so the
/// previews stay readable; bare options stay in one compact list. The cancel
/// line exists only when this dimension is the one about to be prompted, which
/// is why `--dry-run` enumerates alternatives without offering to cancel.
fn render_decision(decision: &Decision, active: bool, style: RenderStyle) -> Vec<Vec<String>> {
    let mut chunks = Vec::new();
    let title = if active {
        decision.heading.as_ref()
    } else {
        decision.deferred_heading.as_ref()
    };
    if let Some(title) = title {
        chunks.push(vec![heading(title, style.color)]);
    }
    if let Some(preamble) = &decision.preamble {
        chunks.push(vec![preamble.clone()]);
    }
    let token_width = decision
        .options
        .iter()
        .map(|option| display_width(&option.token))
        .chain(active.then_some(display_width(CANCEL_TOKEN)))
        .max()
        .unwrap_or(0);
    let labels = decision
        .options
        .iter()
        .map(|option| {
            if option.recommended {
                format!("{}  (recommended)", option.label)
            } else {
                option.label.clone()
            }
        })
        .collect::<Vec<_>>();
    let label_width = labels
        .iter()
        .map(|label| display_width(label))
        .max()
        .unwrap_or(0);
    let detailed = decision
        .options
        .iter()
        .any(|option| !option.detail.is_empty());
    let indent = " ".repeat(OPTION_INDENT);
    let detail_indent = OPTION_INDENT + token_width + COLUMN_GAP.len();

    let mut compact = Vec::new();
    for (option, label) in decision.options.iter().zip(labels) {
        let mut cells = vec![
            padded(&option.token, token_width),
            padded(&label, label_width),
        ];
        if let Some(effect) = &option.effect {
            cells.push(colored(effect, option.effect_color, style.color));
        }
        let mut lines = vec![format!("{indent}{}", join_columns(&cells))];
        lines.extend(render_option_detail(&option.detail, detail_indent, style));
        if detailed {
            chunks.push(lines);
        } else {
            compact.extend(lines);
        }
    }
    if active {
        let cancel = format!(
            "{indent}{}",
            join_columns(&[padded(CANCEL_TOKEN, token_width), "Cancel".to_owned()])
        );
        if detailed {
            chunks.push(vec![cancel]);
        } else {
            compact.push(cancel);
        }
    }
    if !compact.is_empty() {
        chunks.push(compact);
    }
    chunks
}

/// Marker vocabulary shared by every post-apply result footer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResultMarker {
    /// Work that completed successfully.
    Completed,
    /// Work that was deliberately left untouched.
    Unchanged,
}

impl ResultMarker {
    const fn symbol(self) -> &'static str {
        match self {
            Self::Completed => "✓",
            Self::Unchanged => "—",
        }
    }

    const fn word(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Unchanged => "unchanged",
        }
    }

    const fn color(self) -> Option<u8> {
        match self {
            Self::Completed => Some(32),
            Self::Unchanged => None,
        }
    }
}

/// One nonzero category of a result footer.
#[derive(Clone, Debug)]
pub struct ResultEntry {
    /// Semantic marker for this category.
    pub marker: ResultMarker,
    /// Count of affected items; a zero count omits the whole entry.
    pub count: usize,
    /// Description such as `deployments updated`.
    pub description: String,
}

/// Render the comma-separated, nonzero-only result footer.
///
/// The redirected form drops the description for the unchanged marker because
/// its word already says `unchanged`, matching the `status` summary grammar.
#[must_use]
pub fn result_footer(entries: &[ResultEntry], style: RenderStyle) -> String {
    entries
        .iter()
        .filter(|entry| entry.count > 0)
        .map(|entry| {
            let text = if style.symbols {
                format!(
                    "{}: {} {}",
                    entry.marker.symbol(),
                    entry.count,
                    entry.description
                )
            } else if entry.marker == ResultMarker::Unchanged {
                format!("{}: {}", entry.marker.word(), entry.count)
            } else {
                format!(
                    "{}: {} {}",
                    entry.marker.word(),
                    entry.count,
                    entry.description
                )
            };
            colored(&text, entry.marker.color(), style.color)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render the destination/blast-radius phrase used by footers and prompts.
///
/// The qualifier survives only while every inferred or selected destination
/// does; once significance gating drops one, the phrase degrades to a bare
/// count so it never overstates what the plan touches.
#[must_use]
pub fn destination_label(surviving: usize, total: usize, explicit: bool, noun: &str) -> String {
    let qualifier = if surviving == total && total > 0 {
        if explicit { "selected " } else { "enabled " }
    } else {
        ""
    };
    let plural = if surviving == 1 { "" } else { "s" };
    format!("{surviving} {qualifier}{noun}{plural}")
}

/// How the user is asked to authorize a rendered plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanAuthorization {
    /// Decision shape, such as `binary` or `progressive`.
    pub kind: &'static str,
    /// How authorization is obtained for this invocation.
    pub mode: &'static str,
    /// Default answer of a binary confirmation, when it has one.
    pub default: Option<bool>,
}

/// Which selection dimensions were inferred rather than stated.
///
/// These are the *resolved* selections, not the surviving rendered columns.
/// Significance gating is a property of human rendering; the machine stream
/// stays complete so automation can tell "selected but had no work" apart from
/// "never selected".
#[derive(Clone, Debug, Default)]
pub struct PlanSelection {
    /// Every selected target name in configured order.
    pub targets: Vec<String>,
    /// Whether targets were explicitly selected.
    pub targets_explicit: bool,
    /// Uniform scope, when the plan has one.
    pub scope: Option<Scope>,
    /// Whether the scope was explicitly selected.
    pub scope_explicit: bool,
}

/// Name the structured event carrying one plan revision.
///
/// Revision `0` is the initial complete plan; every narrowed re-render emits a
/// correlated `plan.updated` sharing the same `plan_id`.
#[must_use]
pub const fn plan_event_name(revision: u64) -> &'static str {
    if revision == 0 {
        "plan"
    } else {
        "plan.updated"
    }
}

/// Build the structured plan payload mirroring one rendered plan revision.
///
/// The payload describes the complete reviewed plan, including every decision
/// dimension and every alternative's typed consequence. Significance gating is
/// a property of human rendering only; the payload omits values that are
/// genuinely absent (a zero metric, an empty diff) and nothing else.
#[must_use]
pub fn plan_event_data(
    view: &PlanView<'_>,
    revision: u64,
    dry_run: bool,
    authorization: PlanAuthorization,
    selection: &PlanSelection,
) -> Value {
    let plan = view.plan;
    let mut authorization_value = Map::new();
    authorization_value.insert("kind".into(), json!(authorization.kind));
    authorization_value.insert("mode".into(), json!(authorization.mode));
    if let Some(default) = authorization.default {
        authorization_value.insert("default".into(), json!(default));
    }
    if !plan.decisions.is_empty() {
        let sequence = plan
            .decisions
            .iter()
            .map(|decision| decision.id.clone())
            .collect::<Vec<_>>();
        let mut resolved = Map::new();
        let mut pending = Vec::new();
        for decision in &plan.decisions {
            match &decision.resolved {
                Some(answer) => {
                    resolved.insert(decision.id.clone(), json!(answer));
                }
                None => pending.push(decision.id.clone()),
            }
        }
        authorization_value.insert("sequence".into(), json!(sequence));
        authorization_value.insert("resolved".into(), Value::Object(resolved));
        authorization_value.insert("pending".into(), json!(pending));
        if plan.prompting
            && let Some(next) = view.decisions().first()
        {
            // The full alternatives live in `decisions`; duplicating them here
            // would give the same fact two representations that can drift.
            authorization_value.insert("prompt".into(), json!({ "dimension": next.id }));
        }
    }

    let mut selection_value = Map::new();
    selection_value.insert(
        "targets".into(),
        json!({
            "mode": mode_label(selection.targets_explicit),
            "names": selection.targets,
        }),
    );
    let mut scope_value = Map::new();
    scope_value.insert("mode".into(), json!(mode_label(selection.scope_explicit)));
    if let Some(scope) = selection.scope {
        scope_value.insert("value".into(), json!(scope));
    }
    selection_value.insert("scope".into(), Value::Object(scope_value));

    let destinations = plan
        .destinations
        .iter()
        .filter(|destination| plan_references(plan, &destination.id))
        .map(destination_value)
        .collect::<Vec<_>>();
    let entries = plan
        .rows
        .iter()
        .map(|row| row_value(view, row))
        .collect::<Vec<_>>();

    let mut data = Map::new();
    data.insert("plan_id".into(), json!(plan.plan_id));
    data.insert("revision".into(), json!(revision));
    data.insert("command".into(), json!(plan.command));
    data.insert("dry_run".into(), json!(dry_run));
    data.insert("authorization".into(), Value::Object(authorization_value));
    data.insert("selection".into(), Value::Object(selection_value));
    data.insert("destinations".into(), json!(destinations));
    data.insert("entries".into(), json!(entries));
    if !plan.decisions.is_empty() {
        let decisions = plan
            .decisions
            .iter()
            .map(decision_value)
            .collect::<Vec<_>>();
        data.insert("decisions".into(), json!(decisions));
    }
    data.insert("summary".into(), plan_totals(view));
    Value::Object(data)
}

/// Whether any row or decision alternative mentions this destination.
///
/// A destination reached only by an unresolved alternative is still part of the
/// reviewed plan, which is why decisions are searched alongside rows.
fn plan_references(plan: &ChangePlan, id: &str) -> bool {
    let in_rows = plan.rows.iter().any(|row| {
        row.actions.iter().any(|action| action.destination == id)
            || row.availability.iter().any(|value| value == id)
    });
    in_rows
        || plan.decisions.iter().any(|decision| {
            decision.options.iter().any(|option| {
                option
                    .consequence
                    .actions
                    .iter()
                    .any(|action| action.destination == id)
            })
        })
}

fn decision_value(decision: &Decision) -> Value {
    let mut value = Map::new();
    value.insert("id".into(), json!(decision.id));
    value.insert("prompt".into(), json!(decision.prompt));
    value.insert(
        "state".into(),
        json!(if decision.resolved.is_some() {
            "resolved"
        } else {
            "pending"
        }),
    );
    if let Some(resolved) = &decision.resolved {
        value.insert("resolved".into(), json!(resolved));
    }
    value.insert(
        "options".into(),
        json!(
            decision
                .options
                .iter()
                .map(option_value)
                .collect::<Vec<_>>()
        ),
    );
    Value::Object(value)
}

fn option_value(option: &DecisionOption) -> Value {
    let mut value = Map::new();
    value.insert("id".into(), json!(option.id));
    value.insert("token".into(), json!(option.token));
    value.insert("label".into(), json!(option.label));
    if option.recommended {
        value.insert("recommended".into(), json!(true));
    }
    if let Some(effect) = &option.effect {
        value.insert("effect".into(), json!(effect));
    }
    if !option.consequence.is_empty() {
        value.insert("consequence".into(), consequence_value(&option.consequence));
    }
    Value::Object(value)
}

fn consequence_value(consequence: &OptionConsequence) -> Value {
    let mut value = Map::new();
    if let Some(operation) = consequence.operation {
        value.insert("operation".into(), json!(operation.as_str()));
    }
    if let Some(path) = &consequence.path {
        value.insert("path".into(), json!(path));
    }
    if !consequence.actions.is_empty() {
        value.insert(
            "actions".into(),
            json!(
                consequence
                    .actions
                    .iter()
                    .map(action_value)
                    .collect::<Vec<_>>()
            ),
        );
    }
    if !consequence.totals.is_empty() {
        let totals = consequence
            .totals
            .iter()
            .map(|(name, count)| (name.clone(), json!(count)))
            .collect::<Map<_, _>>();
        value.insert("totals".into(), Value::Object(totals));
    }
    Value::Object(value)
}

fn plan_totals(view: &PlanView<'_>) -> Value {
    let mut totals = Map::new();
    totals.insert("skills".into(), json!(view.plan.rows.len()));
    totals.insert("actions".into(), json!(view.actions));
    if view.availability > 0 {
        totals.insert("available".into(), json!(view.availability));
    }
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for row in &view.plan.rows {
        for action in &row.actions {
            let bucket = if view.plan.distinguishes_overwrites && action.action != PlanAction::Skip
            {
                if action.existed { "overwrite" } else { "new" }
            } else {
                action.action.as_str()
            };
            *counts.entry(bucket).or_default() += 1;
        }
    }
    for (name, count) in counts {
        if count > 0 {
            totals.insert(name.into(), json!(count));
        }
    }
    Value::Object(totals)
}

fn destination_value(destination: &Destination) -> Value {
    let mut value = Map::new();
    value.insert("id".into(), json!(destination.id));
    value.insert("kind".into(), json!(destination.kind.as_str()));
    value.insert("label".into(), json!(destination.label));
    match &destination.kind {
        DestinationKind::Deployment { target, scope } => {
            value.insert("target".into(), json!(target));
            value.insert("scope".into(), json!(scope));
        }
        DestinationKind::Source { source } => {
            value.insert("source".into(), json!(source));
        }
        DestinationKind::Path => {}
    }
    if let Some(path) = &destination.path {
        value.insert("path".into(), json!(path));
    }
    Value::Object(value)
}

fn row_value(view: &PlanView<'_>, row: &PlanRow) -> Value {
    let mut value = Map::new();
    value.insert("skill".into(), json!(row.identity));
    if let Some(provenance) = &row.provenance {
        value.insert("source".into(), json!(provenance));
    }
    let actions = view
        .plan
        .destinations
        .iter()
        .filter_map(|destination| {
            row.actions
                .iter()
                .find(|action| action.destination == destination.id)
        })
        .map(action_value)
        .collect::<Vec<_>>();
    value.insert("actions".into(), json!(actions));
    if !row.availability.is_empty() {
        let available = view
            .plan
            .destinations
            .iter()
            .filter(|destination| row.availability.contains(&destination.id))
            .map(|destination| destination.id.clone())
            .collect::<Vec<_>>();
        value.insert("available".into(), json!(available));
    }
    Value::Object(value)
}

fn action_value(action: &PlannedAction) -> Value {
    let mut value = Map::new();
    value.insert("operation".into(), json!(action.action.as_str()));
    value.insert("destination".into(), json!(action.destination));
    value.insert("existed".into(), json!(action.existed));
    if !action.stat.is_empty() {
        value.insert("diff".into(), diff_value(&action.stat));
    }
    Value::Object(value)
}

fn diff_value(stat: &DiffStat) -> Value {
    let mut value = Map::new();
    value.insert("files_changed".into(), json!(stat.files_changed()));
    if stat.insertions() > 0 {
        value.insert("insertions".into(), json!(stat.insertions()));
    }
    if stat.deletions() > 0 {
        value.insert("deletions".into(), json!(stat.deletions()));
    }
    let files = stat
        .files
        .iter()
        .map(|file| {
            let mut entry = Map::new();
            entry.insert("path".into(), json!(file.path));
            entry.insert("change".into(), json!(file.change.as_str()));
            if file.insertions > 0 {
                entry.insert("insertions".into(), json!(file.insertions));
            }
            if file.deletions > 0 {
                entry.insert("deletions".into(), json!(file.deletions));
            }
            if file.binary {
                entry.insert("binary".into(), json!(true));
            }
            if file.bytes != 0 {
                entry.insert("bytes".into(), json!(file.bytes));
            }
            Value::Object(entry)
        })
        .collect::<Vec<_>>();
    value.insert("files".into(), json!(files));
    Value::Object(value)
}

const fn mode_label(explicit: bool) -> &'static str {
    if explicit { "explicit" } else { "inferred" }
}

/// Aggregate a scope set into the shared location vocabulary.
#[must_use]
pub fn location_of(scopes: &BTreeSet<Scope>) -> Option<SkillLocation> {
    match (
        scopes.contains(&Scope::Global),
        scopes.contains(&Scope::Project),
    ) {
        (true, true) => Some(SkillLocation::Both),
        (true, false) => Some(SkillLocation::Global),
        (false, true) => Some(SkillLocation::Project),
        (false, false) => None,
    }
}

/// Render one location in the symbol or word vocabulary.
#[must_use]
pub const fn location_text(location: SkillLocation, symbols: bool) -> &'static str {
    if symbols {
        match location {
            SkillLocation::Global => "🌐 global",
            SkillLocation::Project => "📁 project",
            SkillLocation::Both => "↕ both",
            SkillLocation::None => "—",
        }
    } else {
        match location {
            SkillLocation::None => "none",
            other => other.as_str(),
        }
    }
}

/// Render one plan action in the symbol or word vocabulary.
#[must_use]
pub const fn action_text(action: PlanAction, symbols: bool) -> &'static str {
    if symbols {
        action.symbol()
    } else {
        match action {
            PlanAction::Skip => "unchanged",
            other => other.as_str(),
        }
    }
}

/// Render the empty-cell marker in the symbol or word vocabulary.
#[must_use]
pub const fn none_text(symbols: bool) -> &'static str {
    if symbols { "—" } else { "none" }
}

/// Render one cyan-bold section heading.
#[must_use]
pub fn heading(text: &str, color: bool) -> String {
    if color {
        format!("\u{1b}[1;36m{text}\u{1b}[0m")
    } else {
        text.to_owned()
    }
}

pub(crate) fn colored(text: &str, code: Option<u8>, color: bool) -> String {
    match (color, code) {
        (true, Some(code)) => format!("\u{1b}[{code}m{text}\u{1b}[0m"),
        _ => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChangePlan, Decision, DecisionOption, Destination, DestinationKind, OptionConsequence,
        OptionDetail, PlanAuthorization, PlanRow, PlanSelection, PlannedAction, PreviewBlock,
        PreviewEntry, PreviewField, RenderStyle, ResultEntry, ResultMarker, destination_label,
        plan_event_data, render_plan, result_footer,
    };
    use crate::domain::Scope;
    use crate::plan::{DiffStat, FileChange, FileDelta, PlanAction};
    use serde_json::{Map, Value, json};
    use std::path::PathBuf;

    fn deployment(target: &str, scope: Scope) -> Destination {
        Destination {
            id: format!("{target}:{}", scope.as_str()),
            column: target.to_owned(),
            label: format!("{target} · {}", scope.as_str()),
            kind: DestinationKind::Deployment {
                target: target.to_owned(),
                scope,
            },
            path: None,
        }
    }

    fn action(destination: &str, description: &str) -> PlannedAction {
        PlannedAction {
            destination: destination.to_owned(),
            action: PlanAction::Update,
            existed: true,
            description: description.to_owned(),
            stat: DiffStat::default(),
        }
    }

    fn plan(rows: Vec<PlanRow>) -> ChangePlan {
        ChangePlan {
            command: "update".into(),
            plan_id: "update:test".into(),
            heading: "Update plan".into(),
            metadata: Vec::new(),
            destinations: vec![
                deployment("claude", Scope::Global),
                deployment("claude", Scope::Project),
                deployment("shared", Scope::Global),
                deployment("shared", Scope::Project),
                deployment("antigravity", Scope::Global),
            ],
            metric_header: None,
            detail_heading: "Destination-specific changes".into(),
            connector: Some("->".into()),
            body_heading: None,
            rows,
            blocks: Vec::new(),
            decisions: Vec::new(),
            prompting: false,
            distinguishes_overwrites: false,
        }
    }

    #[test]
    fn all_none_destination_columns_are_dropped_before_widths_are_measured() {
        let plan = plan(vec![
            PlanRow {
                identity: "alpha".into(),
                provenance: None,
                metric: None,
                availability: Vec::new(),
                actions: vec![
                    action("claude:global", "1 file changed, +1/-0"),
                    action("shared:global", "1 file changed, +1/-0"),
                ],
            },
            PlanRow {
                identity: "beta".into(),
                provenance: None,
                metric: None,
                availability: Vec::new(),
                actions: vec![action("claude:global", "2 files changed, +4/-1")],
            },
        ]);
        let view = plan.view();
        assert_eq!(view.columns(), ["claude", "shared"]);
        assert_eq!(view.uniform_scope(), Some(Scope::Global));
        let lines = render_plan(
            &view,
            RenderStyle {
                symbols: true,
                color: false,
            },
        );
        let rendered = lines.join("\n");
        assert!(!rendered.contains("antigravity"), "{rendered}");
        assert!(
            rendered.contains("skill  change                  claude  shared"),
            "{rendered}"
        );
        assert!(
            rendered.contains("beta   2 files changed, +4/-1  ↑       —"),
            "{rendered}"
        );
    }

    #[test]
    fn mixed_scopes_keep_per_cell_locations_and_detail_sections() {
        let plan = plan(vec![PlanRow {
            identity: "alpha".into(),
            provenance: None,
            metric: None,
            availability: Vec::new(),
            actions: vec![
                action("claude:global", "1 file changed, +1/-0"),
                action("claude:project", "1 file changed, +1/-0"),
                action("shared:global", "3 files changed, +9/-4"),
            ],
        }]);
        let view = plan.view();
        assert!(view.show_locations());
        let rendered = render_plan(
            &view,
            RenderStyle {
                symbols: true,
                color: false,
            },
        )
        .join("\n");
        assert!(rendered.contains("↑ ↕ both"), "{rendered}");
        assert!(rendered.contains("↑ 🌐 global"), "{rendered}");
        assert!(
            rendered.contains("2 destination-specific changes"),
            "{rendered}"
        );
        assert!(
            rendered.contains("    claude · global   1 file changed, +1/-0"),
            "{rendered}"
        );
    }

    #[test]
    fn a_single_item_at_a_single_destination_degrades_to_a_sentence() {
        let plan = plan(vec![PlanRow {
            identity: "alpha".into(),
            provenance: None,
            metric: None,
            availability: Vec::new(),
            actions: vec![action("claude:global", "1 file changed, +1/-0")],
        }]);
        let rendered = render_plan(
            &plan.view(),
            RenderStyle {
                symbols: true,
                color: false,
            },
        )
        .join("\n");
        assert!(
            rendered.contains("↑ alpha -> claude: 1 file changed, +1/-0"),
            "{rendered}"
        );
        assert!(!rendered.contains("skill  change"), "{rendered}");
    }

    #[test]
    fn redirected_output_uses_words_for_actions_locations_and_empty_cells() {
        let plan = plan(vec![
            PlanRow {
                identity: "alpha".into(),
                provenance: None,
                metric: None,
                availability: Vec::new(),
                actions: vec![
                    action("claude:global", "3 files changed, +9/-4"),
                    action("claude:project", "3 files changed, +9/-4"),
                ],
            },
            PlanRow {
                identity: "beta".into(),
                provenance: None,
                metric: None,
                availability: Vec::new(),
                actions: vec![action("shared:global", "1 file changed, +1/-0")],
            },
        ]);
        let rendered = render_plan(&plan.view(), RenderStyle::plain()).join("\n");
        assert!(rendered.contains("update both"), "{rendered}");
        assert!(rendered.contains("update global"), "{rendered}");
        assert!(rendered.contains("none"), "{rendered}");
    }

    #[test]
    fn footers_and_labels_omit_zero_categories_and_degraded_qualifiers() {
        let style = RenderStyle {
            symbols: true,
            color: false,
        };
        assert_eq!(
            result_footer(
                &[
                    ResultEntry {
                        marker: ResultMarker::Completed,
                        count: 2,
                        description: "deployments updated".into(),
                    },
                    ResultEntry {
                        marker: ResultMarker::Unchanged,
                        count: 0,
                        description: "unchanged".into(),
                    },
                ],
                style
            ),
            "✓: 2 deployments updated"
        );
        assert_eq!(
            result_footer(
                &[ResultEntry {
                    marker: ResultMarker::Unchanged,
                    count: 3,
                    description: "unchanged".into(),
                }],
                RenderStyle::plain()
            ),
            "unchanged: 3"
        );
        assert_eq!(
            destination_label(3, 3, false, "target"),
            "3 enabled targets"
        );
        assert_eq!(destination_label(1, 1, true, "target"), "1 selected target");
        assert_eq!(destination_label(2, 3, false, "target"), "2 targets");
    }

    #[test]
    fn the_plan_event_mirrors_surviving_columns_and_omits_zero_metrics() {
        let plan = plan(vec![PlanRow {
            identity: "alpha".into(),
            provenance: Some("primary".into()),
            metric: None,
            availability: Vec::new(),
            actions: vec![action("claude:global", "1 file changed, +1/-0")],
        }]);
        let view = plan.view();
        let data = plan_event_data(
            &view,
            0,
            false,
            PlanAuthorization {
                kind: "binary",
                mode: "prompt",
                default: Some(true),
            },
            &PlanSelection {
                targets: vec!["claude".into()],
                targets_explicit: true,
                scope: Some(Scope::Global),
                scope_explicit: false,
            },
        );
        assert_eq!(data["command"], "update");
        assert_eq!(data["revision"], 0);
        assert_eq!(data["destinations"].as_array().map(Vec::len), Some(1));
        assert_eq!(data["entries"][0]["actions"][0]["operation"], "update");
        assert!(data["entries"][0]["actions"][0].get("diff").is_none());
        assert_eq!(data["summary"]["actions"], 1);
        assert_eq!(data["selection"]["scope"]["mode"], "inferred");
        assert_eq!(data["authorization"]["default"], true);
    }

    // ---------------------------------------------------------------------
    // Stage 2-5 shape proofs.
    //
    // These construct the approved `remove` and `import` plans directly and
    // assert the rendered output. They exist so the abstraction is shown to
    // carry the later commands rather than asserted to; the commands
    // themselves are deliberately not migrated yet.
    // ---------------------------------------------------------------------

    const SYMBOLS: RenderStyle = RenderStyle {
        symbols: true,
        color: false,
    };

    fn remove_destinations() -> Vec<Destination> {
        ["claude", "shared", "antigravity"]
            .into_iter()
            .flat_map(|target| {
                [Scope::Global, Scope::Project]
                    .into_iter()
                    .map(move |scope| deployment(target, scope))
            })
            .collect()
    }

    /// Build one availability row from a per-target `b`/`g`/`p`/`-` sketch.
    fn availability_row(identity: &str, metric: &str, sketch: [&str; 3]) -> PlanRow {
        let mut availability = Vec::new();
        for (target, spec) in ["claude", "shared", "antigravity"].into_iter().zip(sketch) {
            for scope in [Scope::Global, Scope::Project] {
                let wanted = match scope {
                    Scope::Global => spec == "b" || spec == "g",
                    Scope::Project => spec == "b" || spec == "p",
                };
                if wanted {
                    availability.push(format!("{target}:{}", scope.as_str()));
                }
            }
        }
        PlanRow {
            identity: identity.to_owned(),
            metric: Some(metric.to_owned()),
            availability,
            ..PlanRow::default()
        }
    }

    /// Build one removal alternative whose blast radius is too wide to
    /// enumerate per destination, so it travels as typed aggregate totals.
    fn scope_option(
        id: &str,
        token: &str,
        label: &str,
        deployments: u64,
        files: u64,
    ) -> DecisionOption {
        let mut totals = vec![("deployments".to_owned(), deployments)];
        let effect = if files > 0 {
            totals.push(("files".to_owned(), files));
            format!("− {deployments} deployments, {files} files")
        } else {
            format!("− {deployments} deployments")
        };
        DecisionOption {
            id: id.to_owned(),
            token: token.to_owned(),
            label: label.to_owned(),
            effect: Some(effect),
            effect_color: PlanAction::Remove.color_code(),
            consequence: OptionConsequence {
                operation: Some(PlanAction::Remove),
                totals,
                ..OptionConsequence::default()
            },
            ..DecisionOption::default()
        }
    }

    fn remove_plan(rows: Vec<PlanRow>, decision: Decision, prompting: bool) -> ChangePlan {
        ChangePlan {
            command: "remove".into(),
            plan_id: "remove:test".into(),
            heading: "Remove plan".into(),
            metadata: Vec::new(),
            destinations: remove_destinations(),
            body_heading: Some("Available deployments".into()),
            metric_header: Some("files/deploy".into()),
            detail_heading: "Destination-specific changes".into(),
            connector: Some("from".into()),
            rows,
            blocks: Vec::new(),
            decisions: vec![decision],
            prompting,
            distinguishes_overwrites: false,
        }
    }

    #[test]
    fn the_remove_scope_branch_renders_availability_and_three_unresolved_alternatives() {
        let rows = vec![
            availability_row("converting-board-decks", "14", ["b", "b", "b"]),
            availability_row("drafting-commit-message", "3", ["b", "b", "g"]),
            availability_row("grill-me", "2", ["g", "g", "g"]),
            availability_row("handoff", "5", ["b", "b", "b"]),
            availability_row("importing-meeting-notes", "9", ["b", "b", "b"]),
            availability_row("in-my-voice", "4", ["p", "p", "p"]),
            availability_row("knowing-camber-me", "6", ["b", "b", "b"]),
            availability_row("managing-camber-skills", "8", ["g", "g", "g"]),
            availability_row("managing-skills", "3", ["g", "g", "g"]),
            availability_row("reviewing-implemented-work-order", "7", ["p", "p", "-"]),
            availability_row("reviewing-my-code", "5", ["b", "b", "p"]),
            availability_row("running-as-maestro", "4", ["g", "g", "g"]),
            availability_row("slack-to-todoist", "3", ["g", "g", "-"]),
            availability_row("teach", "1", ["b", "b", "b"]),
            availability_row("to-questionnaire", "6", ["g", "g", "-"]),
            availability_row("todoist-cli", "11", ["b", "b", "b"]),
            availability_row("wait-what", "2", ["g", "g", "-"]),
            availability_row("writing-for-agents", "8", ["b", "b", "b"]),
        ];
        let decision = Decision {
            id: "removal_scope".into(),
            preamble: Some("25 unambiguous deployments are removed in every option.".into()),
            prompt: "Select removal scope".into(),
            options: vec![
                scope_option(
                    "project",
                    "1",
                    "Remove project copies where both exist",
                    50,
                    285,
                ),
                scope_option(
                    "global",
                    "2",
                    "Remove global copies where both exist",
                    50,
                    285,
                ),
                scope_option("both", "3", "Remove both copies where both exist", 75, 463),
            ],
            ..Decision::default()
        };
        let plan = remove_plan(rows, decision, true);
        let view = plan.view();
        assert_eq!(view.actions(), 0, "availability is evidence, not an action");
        assert_eq!(view.decisions().len(), 1);
        assert_eq!(
            render_plan(&view, SYMBOLS),
            [
                "Remove plan",
                "",
                "Available deployments",
                "",
                "skill                             files/deploy  claude      shared      antigravity",
                "--------------------------------  ------------  ----------  ----------  -----------",
                "converting-board-decks            14            ↕ both      ↕ both      ↕ both",
                "drafting-commit-message           3             ↕ both      ↕ both      🌐 global",
                "grill-me                          2             🌐 global   🌐 global   🌐 global",
                "handoff                           5             ↕ both      ↕ both      ↕ both",
                "importing-meeting-notes           9             ↕ both      ↕ both      ↕ both",
                "in-my-voice                       4             📁 project  📁 project  📁 project",
                "knowing-camber-me                 6             ↕ both      ↕ both      ↕ both",
                "managing-camber-skills            8             🌐 global   🌐 global   🌐 global",
                "managing-skills                   3             🌐 global   🌐 global   🌐 global",
                "reviewing-implemented-work-order  7             📁 project  📁 project  —",
                "reviewing-my-code                 5             ↕ both      ↕ both      📁 project",
                "running-as-maestro                4             🌐 global   🌐 global   🌐 global",
                "slack-to-todoist                  3             🌐 global   🌐 global   —",
                "teach                             1             ↕ both      ↕ both      ↕ both",
                "to-questionnaire                  6             🌐 global   🌐 global   —",
                "todoist-cli                       11            ↕ both      ↕ both      ↕ both",
                "wait-what                         2             🌐 global   🌐 global   —",
                "writing-for-agents                8             ↕ both      ↕ both      ↕ both",
                "",
                "25 unambiguous deployments are removed in every option.",
                "",
                "  1  Remove project copies where both exist  − 50 deployments, 285 files",
                "  2  Remove global copies where both exist   − 50 deployments, 285 files",
                "  3  Remove both copies where both exist     − 75 deployments, 463 files",
                "  c  Cancel",
                "",
            ]
        );
    }

    #[test]
    fn a_dry_run_remove_enumerates_alternatives_without_offering_to_cancel() {
        let decision = Decision {
            id: "removal_scope".into(),
            prompt: "Select removal scope".into(),
            options: vec![
                scope_option("project", "1", "Remove project copies", 3, 3),
                scope_option("global", "2", "Remove global copies", 3, 3),
                scope_option("both", "3", "Remove both copies", 6, 6),
            ],
            ..Decision::default()
        };
        let plan = remove_plan(
            vec![availability_row("teach", "1", ["b", "b", "b"])],
            decision,
            false,
        );
        assert_eq!(
            render_plan(&plan.view(), SYMBOLS),
            [
                "Remove plan",
                "",
                "Available deployments",
                "",
                "skill  files/deploy  claude  shared  antigravity",
                "-----  ------------  ------  ------  -----------",
                "teach  1             ↕ both  ↕ both  ↕ both",
                "",
                "  1  Remove project copies  − 3 deployments, 3 files",
                "  2  Remove global copies   − 3 deployments, 3 files",
                "  3  Remove both copies     − 6 deployments, 6 files",
                "",
            ]
        );
    }

    #[test]
    fn an_explicit_remove_scope_collapses_the_branch_to_a_plain_action_table() {
        let row = |identity: &str, metric: &str| PlanRow {
            identity: identity.to_owned(),
            metric: Some(metric.to_owned()),
            actions: vec![PlannedAction {
                destination: "claude:global".into(),
                action: PlanAction::Remove,
                existed: true,
                description: String::new(),
                stat: DiffStat::default(),
            }],
            ..PlanRow::default()
        };
        let plan = ChangePlan {
            body_heading: None,
            decisions: Vec::new(),
            ..remove_plan(
                vec![row("managing-skills", "3"), row("running-as-maestro", "4")],
                Decision::default(),
                true,
            )
        };
        let view = plan.view();
        assert_eq!(view.columns(), ["claude"]);
        assert_eq!(view.uniform_scope(), Some(Scope::Global));
        assert_eq!(
            render_plan(&view, SYMBOLS),
            [
                "Remove plan",
                "",
                "skill               files/deploy  claude",
                "------------------  ------------  ------",
                "managing-skills     3             −",
                "running-as-maestro  4             −",
                "",
            ]
        );
    }

    #[test]
    fn symbol_and_color_modes_render_identical_layout_with_ansi_only_on_semantics() {
        let plan = remove_plan(
            vec![
                availability_row("teach", "1", ["b", "-", "p"]),
                PlanRow {
                    identity: "handoff".into(),
                    metric: Some("5".into()),
                    actions: vec![
                        PlannedAction {
                            destination: "claude:global".into(),
                            action: PlanAction::Remove,
                            existed: true,
                            description: String::new(),
                            stat: DiffStat::default(),
                        },
                        PlannedAction {
                            destination: "antigravity:project".into(),
                            action: PlanAction::Remove,
                            existed: true,
                            description: String::new(),
                            stat: DiffStat::default(),
                        },
                    ],
                    ..PlanRow::default()
                },
            ],
            Decision {
                id: "removal_scope".into(),
                prompt: "Select removal scope".into(),
                options: vec![
                    scope_option("project", "1", "Remove project copies", 2, 0),
                    scope_option("both", "2", "Remove both copies", 4, 0),
                ],
                ..Decision::default()
            },
            true,
        );
        let view = plan.view();

        // Double-width 🌐/📁 must consume two cells while ↕/−/— consume one, so
        // every column stays aligned in the symbol vocabulary.
        assert_eq!(
            render_plan(&view, SYMBOLS),
            [
                "Remove plan",
                "",
                "Available deployments",
                "",
                "skill    files/deploy  claude       antigravity",
                "-------  ------------  -----------  ------------",
                "teach    1             ↕ both       📁 project",
                "handoff  5             − 🌐 global  − 📁 project",
                "",
                "  1  Remove project copies  − 2 deployments",
                "  2  Remove both copies     − 4 deployments",
                "  c  Cancel",
                "",
            ]
        );

        // Color mode pads on measured width, never on escape-inflated length,
        // and paints only action and effect semantics.
        assert_eq!(
            render_plan(
                &view,
                RenderStyle {
                    symbols: true,
                    color: true,
                }
            ),
            [
                "\u{1b}[1;36mRemove plan\u{1b}[0m",
                "",
                "\u{1b}[1;36mAvailable deployments\u{1b}[0m",
                "",
                "skill    files/deploy  claude       antigravity",
                "-------  ------------  -----------  ------------",
                "teach    1             ↕ both       📁 project",
                "handoff  5             \u{1b}[31m− 🌐 global\u{1b}[0m  \u{1b}[31m− 📁 project\u{1b}[0m",
                "",
                "  1  Remove project copies  \u{1b}[31m− 2 deployments\u{1b}[0m",
                "  2  Remove both copies     \u{1b}[31m− 4 deployments\u{1b}[0m",
                "  c  Cancel",
                "",
            ]
        );

        // The word vocabulary keeps the identical structure and gating.
        assert_eq!(
            render_plan(&view, RenderStyle::plain()),
            [
                "Remove plan",
                "",
                "Available deployments",
                "",
                "skill    files/deploy  claude         antigravity",
                "-------  ------------  -------------  --------------",
                "teach    1             both           project",
                "handoff  5             remove global  remove project",
                "",
                "  1  Remove project copies  − 2 deployments",
                "  2  Remove both copies     − 4 deployments",
                "  c  Cancel",
                "",
            ]
        );
    }

    /// One file of a source copy's own diff: marker, path, insertions, deletions.
    type SourceFile = (&'static str, &'static str, usize, usize);

    /// One propagation target: destination id, files changed, insertions,
    /// deletions. Zero files marks the source copy itself, which is skipped.
    type Propagation = (&'static str, usize, usize, usize);

    /// The source a copy would be imported into.
    fn source_destination() -> Destination {
        Destination {
            id: "personal:source".into(),
            column: "personal".into(),
            label: "personal (source)".into(),
            kind: DestinationKind::Source {
                source: "personal".into(),
            },
            path: None,
        }
    }

    fn import_destinations() -> Vec<Destination> {
        let mut destinations = vec![source_destination()];
        destinations.extend(remove_destinations());
        destinations
    }

    fn destination_label_of(id: &str) -> String {
        import_destinations()
            .into_iter()
            .find(|destination| destination.id == id)
            .map_or_else(|| id.to_owned(), |destination| destination.label)
    }

    fn source_entries(files: &[SourceFile]) -> Vec<PreviewEntry> {
        files
            .iter()
            .map(|&(marker, path, insertions, deletions)| PreviewEntry {
                marker: Some(marker.to_owned()),
                marker_color: Some(match marker {
                    "+" => 32,
                    "-" => 31,
                    _ => 33,
                }),
                label: path.to_owned(),
                value: format!("+{insertions}/-{deletions}"),
                value_color: None,
            })
            .collect()
    }

    fn source_diff(files: &[SourceFile]) -> DiffStat {
        DiffStat {
            files: files
                .iter()
                .map(|&(marker, path, insertions, deletions)| FileDelta {
                    path: path.to_owned(),
                    change: match marker {
                        "+" => FileChange::Added,
                        "-" => FileChange::Deleted,
                        _ => FileChange::Modified,
                    },
                    insertions,
                    deletions,
                    binary: false,
                    bytes: 0,
                })
                .collect(),
        }
    }

    /// A per-file breakdown whose aggregate matches a rendered diff summary.
    ///
    /// Production diffs arrive already itemized; the mocks only publish the
    /// aggregate, so the fixture reconstructs one breakdown that adds up.
    fn spread(files: usize, insertions: usize, deletions: usize) -> DiffStat {
        DiffStat {
            files: (0..files)
                .map(|index| FileDelta {
                    path: format!("file-{}.md", index + 1),
                    change: FileChange::Modified,
                    insertions: if index == 0 { insertions } else { 0 },
                    deletions: if index == 0 { deletions } else { 0 },
                    binary: false,
                    bytes: 0,
                })
                .collect(),
        }
    }

    /// Render and type one propagation table from a single source of truth, so
    /// the preview a user reads and the event a machine reads cannot drift.
    fn propagation(rows: &[Propagation]) -> (Vec<PreviewEntry>, Vec<PlannedAction>) {
        let entries = rows
            .iter()
            .map(|&(id, files, insertions, deletions)| PreviewEntry {
                marker: None,
                marker_color: None,
                label: destination_label_of(id),
                value: if files == 0 {
                    "✓ source copy; synchronized, no file changes".to_owned()
                } else {
                    format!("↑ {files} files changed, +{insertions}/-{deletions}")
                },
                value_color: (files > 0).then_some(33),
            })
            .collect();
        let actions = rows
            .iter()
            .map(|&(id, files, insertions, deletions)| PlannedAction {
                destination: id.to_owned(),
                action: if files == 0 {
                    PlanAction::Skip
                } else {
                    PlanAction::Update
                },
                existed: true,
                description: String::new(),
                stat: spread(files, insertions, deletions),
            })
            .collect();
        (entries, actions)
    }

    fn source_copy(
        id: &str,
        token: &str,
        label: &str,
        path: &str,
        files: &[SourceFile],
        rows: &[Propagation],
    ) -> DecisionOption {
        let diff = source_diff(files);
        let summary = format!(
            "← {} files changed, +{}/-{}",
            diff.files_changed(),
            diff.insertions(),
            diff.deletions()
        );
        let (entries, mut actions) = propagation(rows);
        actions.insert(
            0,
            PlannedAction {
                destination: "personal:source".into(),
                action: PlanAction::Import,
                existed: true,
                description: String::new(),
                stat: diff,
            },
        );
        DecisionOption {
            id: id.to_owned(),
            token: token.to_owned(),
            label: label.to_owned(),
            detail: vec![
                OptionDetail::Fields(vec![
                    PreviewField {
                        label: "Path".into(),
                        value: path.to_owned(),
                        ..PreviewField::default()
                    },
                    PreviewField {
                        label: "Source".into(),
                        value: summary,
                        value_color: Some(33),
                        entries: source_entries(files),
                    },
                ]),
                OptionDetail::Block(PreviewBlock {
                    heading: "Propagation with import + update".into(),
                    heading_value: Some(format!("{} deployments", rows.len())),
                    entries,
                    ..PreviewBlock::default()
                }),
            ],
            consequence: OptionConsequence {
                operation: Some(PlanAction::Import),
                path: Some(PathBuf::from(path)),
                actions,
                totals: vec![("deployments".to_owned(), rows.len() as u64)],
            },
            ..DecisionOption::default()
        }
    }

    fn propagation_modes(update_note: &str, only_note: &str) -> Vec<DecisionOption> {
        vec![
            DecisionOption {
                id: "import-update".into(),
                token: "1".into(),
                label: "Import + update".into(),
                recommended: true,
                detail: vec![OptionDetail::Note(update_note.to_owned())],
                consequence: OptionConsequence {
                    operation: Some(PlanAction::Update),
                    totals: vec![
                        ("deployments".to_owned(), 5),
                        ("updated".to_owned(), 4),
                        ("skipped".to_owned(), 1),
                    ],
                    ..OptionConsequence::default()
                },
                ..DecisionOption::default()
            },
            DecisionOption {
                id: "import-only".into(),
                token: "2".into(),
                label: "Import only".into(),
                detail: vec![OptionDetail::Note(only_note.to_owned())],
                consequence: OptionConsequence {
                    operation: Some(PlanAction::Import),
                    totals: vec![("stale".to_owned(), 4)],
                    ..OptionConsequence::default()
                },
                ..DecisionOption::default()
            },
        ]
    }

    fn import_plan(
        heading: &str,
        metadata: Vec<(String, String)>,
        blocks: Vec<PreviewBlock>,
        decisions: Vec<Decision>,
    ) -> ChangePlan {
        ChangePlan {
            command: "import".into(),
            plan_id: "import:importing-meeting-notes".into(),
            heading: heading.to_owned(),
            metadata,
            destinations: import_destinations(),
            body_heading: None,
            metric_header: None,
            detail_heading: "Destination-specific changes".into(),
            connector: None,
            rows: Vec::new(),
            blocks,
            decisions,
            prompting: true,
            distinguishes_overwrites: false,
        }
    }

    const CLAUDE_PROJECT_FILES: [SourceFile; 3] = [
        ("~", "SKILL.md", 12, 4),
        ("+", "references.md", 6, 0),
        ("-", "examples/old.md", 0, 1),
    ];
    const CLAUDE_PROJECT_PROPAGATION: [Propagation; 5] = [
        ("claude:global", 3, 18, 5),
        ("claude:project", 0, 0, 0),
        ("shared:global", 4, 21, 9),
        ("shared:project", 3, 18, 5),
        ("antigravity:global", 2, 20, 59),
    ];
    const SHARED_GLOBAL_FILES: [SourceFile; 2] =
        [("~", "SKILL.md", 4, 7), ("+", "examples/new.md", 5, 0)];
    const SHARED_GLOBAL_PROPAGATION: [Propagation; 5] = [
        ("claude:global", 2, 9, 7),
        ("claude:project", 4, 14, 20),
        ("shared:global", 0, 0, 0),
        ("shared:project", 2, 9, 7),
        ("antigravity:global", 3, 11, 61),
    ];
    const ANTIGRAVITY_GLOBAL_FILES: [SourceFile; 2] =
        [("~", "SKILL.md", 2, 15), ("-", "references.md", 0, 44)];
    const ANTIGRAVITY_GLOBAL_PROPAGATION: [Propagation; 5] = [
        ("claude:global", 2, 2, 59),
        ("claude:project", 4, 7, 72),
        ("shared:global", 3, 6, 68),
        ("shared:project", 2, 2, 59),
        ("antigravity:global", 0, 0, 0),
    ];

    fn import_source_decision(resolved: Option<&str>) -> Decision {
        Decision {
            id: "source_copy".into(),
            heading: Some("Available source copies".into()),
            deferred_heading: None,
            preamble: None,
            prompt: "Select source copy".into(),
            options: vec![
                source_copy(
                    "claude:project",
                    "1",
                    "claude · project",
                    r"C:\Users\swern\ghub\sernst\skills\.claude\skills\importing-meeting-notes",
                    &CLAUDE_PROJECT_FILES,
                    &CLAUDE_PROJECT_PROPAGATION,
                ),
                source_copy(
                    "shared:global",
                    "2",
                    "shared · global",
                    r"C:\Users\swern\.agents\skills\importing-meeting-notes",
                    &SHARED_GLOBAL_FILES,
                    &SHARED_GLOBAL_PROPAGATION,
                ),
                source_copy(
                    "antigravity:global",
                    "3",
                    "antigravity · global",
                    r"C:\Users\swern\.gemini\antigravity\skills\importing-meeting-notes",
                    &ANTIGRAVITY_GLOBAL_FILES,
                    &ANTIGRAVITY_GLOBAL_PROPAGATION,
                ),
            ],
            resolved: resolved.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn the_import_plan_renders_nested_per_option_previews_before_the_first_question() {
        let plan = import_plan(
            "Import plan",
            vec![("Into".into(), "personal (source)".into())],
            Vec::new(),
            vec![
                import_source_decision(None),
                Decision {
                    id: "propagation".into(),
                    deferred_heading: Some(
                        "Propagation modes (chosen after the source copy)".into(),
                    ),
                    prompt: "Select propagation".into(),
                    options: propagation_modes(
                        "Replace the source, then synchronize every deployment shown for that copy.",
                        "Replace the source; write no deployments and leave the other 4 out of date.",
                    ),
                    ..Decision::default()
                },
            ],
        );
        let view = plan.view();
        assert_eq!(view.decisions().len(), 2, "both dimensions are still open");
        assert_eq!(
            render_plan(&view, SYMBOLS),
            [
                "Import plan",
                "",
                "Into  personal (source)",
                "",
                "Available source copies",
                "",
                "  1  claude · project",
                r"     Path    C:\Users\swern\ghub\sernst\skills\.claude\skills\importing-meeting-notes",
                "     Source  ← 3 files changed, +18/-5",
                "       ~  SKILL.md         +12/-4",
                "       +  references.md    +6/-0",
                "       -  examples/old.md  +0/-1",
                "     Propagation with import + update  5 deployments",
                "       claude · global       ↑ 3 files changed, +18/-5",
                "       claude · project      ✓ source copy; synchronized, no file changes",
                "       shared · global       ↑ 4 files changed, +21/-9",
                "       shared · project      ↑ 3 files changed, +18/-5",
                "       antigravity · global  ↑ 2 files changed, +20/-59",
                "",
                "  2  shared · global",
                r"     Path    C:\Users\swern\.agents\skills\importing-meeting-notes",
                "     Source  ← 2 files changed, +9/-7",
                "       ~  SKILL.md         +4/-7",
                "       +  examples/new.md  +5/-0",
                "     Propagation with import + update  5 deployments",
                "       claude · global       ↑ 2 files changed, +9/-7",
                "       claude · project      ↑ 4 files changed, +14/-20",
                "       shared · global       ✓ source copy; synchronized, no file changes",
                "       shared · project      ↑ 2 files changed, +9/-7",
                "       antigravity · global  ↑ 3 files changed, +11/-61",
                "",
                "  3  antigravity · global",
                r"     Path    C:\Users\swern\.gemini\antigravity\skills\importing-meeting-notes",
                "     Source  ← 2 files changed, +2/-59",
                "       ~  SKILL.md       +2/-15",
                "       -  references.md  +0/-44",
                "     Propagation with import + update  5 deployments",
                "       claude · global       ↑ 2 files changed, +2/-59",
                "       claude · project      ↑ 4 files changed, +7/-72",
                "       shared · global       ↑ 3 files changed, +6/-68",
                "       shared · project      ↑ 2 files changed, +2/-59",
                "       antigravity · global  ✓ source copy; synchronized, no file changes",
                "",
                "  c  Cancel",
                "",
                "Propagation modes (chosen after the source copy)",
                "",
                "  1  Import + update  (recommended)",
                "     Replace the source, then synchronize every deployment shown for that copy.",
                "",
                "  2  Import only",
                "     Replace the source; write no deployments and leave the other 4 out of date.",
                "",
            ]
        );
    }

    #[test]
    fn resolving_the_first_import_dimension_gates_it_out_of_the_narrowed_re_render() {
        let plan = import_plan(
            "Import plan — source copy 2 selected",
            vec![
                ("From".into(), "shared · global".into()),
                (
                    "Path".into(),
                    r"C:\Users\swern\.agents\skills\importing-meeting-notes".into(),
                ),
                ("Into".into(), "personal (source)".into()),
            ],
            vec![
                PreviewBlock {
                    heading: "Source replacement".into(),
                    heading_value: None,
                    lead: Some("← 2 files changed, +9/-7".into()),
                    lead_color: Some(33),
                    entries: source_entries(&SHARED_GLOBAL_FILES),
                },
                PreviewBlock {
                    heading: "Propagation preview".into(),
                    entries: propagation(&SHARED_GLOBAL_PROPAGATION).0,
                    ..PreviewBlock::default()
                },
            ],
            vec![
                import_source_decision(Some("shared:global")),
                Decision {
                    id: "propagation".into(),
                    deferred_heading: Some(
                        "Propagation modes (chosen after the source copy)".into(),
                    ),
                    prompt: "Select propagation".into(),
                    options: propagation_modes(
                        "Replace the source and synchronize 5 deployments (1 source copy, 4 updated).",
                        "Replace the source; write no deployments and leave 4 out of date.",
                    ),
                    ..Decision::default()
                },
            ],
        );
        let view = plan.view();
        assert_eq!(
            view.decisions().len(),
            1,
            "the answered dimension is gated out"
        );
        assert_eq!(view.decisions()[0].id, "propagation");
        assert_eq!(
            render_plan(&view, SYMBOLS),
            [
                "Import plan — source copy 2 selected",
                "",
                "From  shared · global",
                r"Path  C:\Users\swern\.agents\skills\importing-meeting-notes",
                "Into  personal (source)",
                "",
                "Source replacement",
                "  ← 2 files changed, +9/-7",
                "  ~  SKILL.md         +4/-7",
                "  +  examples/new.md  +5/-0",
                "",
                "Propagation preview",
                "  claude · global       ↑ 2 files changed, +9/-7",
                "  claude · project      ↑ 4 files changed, +14/-20",
                "  shared · global       ✓ source copy; synchronized, no file changes",
                "  shared · project      ↑ 2 files changed, +9/-7",
                "  antigravity · global  ↑ 3 files changed, +11/-61",
                "",
                "  1  Import + update  (recommended)",
                "     Replace the source and synchronize 5 deployments (1 source copy, 4 updated).",
                "",
                "  2  Import only",
                "     Replace the source; write no deployments and leave 4 out of date.",
                "",
                "  c  Cancel",
                "",
            ]
        );
    }

    // -- machine contract -------------------------------------------------
    //
    // Gating is a property of human rendering, so these assertions compare
    // whole event values: the payload must describe the complete plan the user
    // reviewed, including alternatives nobody has chosen yet.

    fn diff_json(files: usize, insertions: usize, deletions: usize) -> Value {
        let entries = (0..files)
            .map(|index| {
                let mut file = Map::new();
                file.insert("path".into(), json!(format!("file-{}.md", index + 1)));
                file.insert("change".into(), json!("modified"));
                if index == 0 && insertions > 0 {
                    file.insert("insertions".into(), json!(insertions));
                }
                if index == 0 && deletions > 0 {
                    file.insert("deletions".into(), json!(deletions));
                }
                Value::Object(file)
            })
            .collect::<Vec<_>>();
        let mut value = Map::new();
        value.insert("files_changed".into(), json!(files));
        if insertions > 0 {
            value.insert("insertions".into(), json!(insertions));
        }
        if deletions > 0 {
            value.insert("deletions".into(), json!(deletions));
        }
        value.insert("files".into(), json!(entries));
        Value::Object(value)
    }

    fn source_diff_json(files: &[SourceFile]) -> Value {
        let entries = files
            .iter()
            .map(|&(marker, path, insertions, deletions)| {
                let mut file = Map::new();
                file.insert("path".into(), json!(path));
                file.insert(
                    "change".into(),
                    json!(match marker {
                        "+" => "added",
                        "-" => "deleted",
                        _ => "modified",
                    }),
                );
                if insertions > 0 {
                    file.insert("insertions".into(), json!(insertions));
                }
                if deletions > 0 {
                    file.insert("deletions".into(), json!(deletions));
                }
                Value::Object(file)
            })
            .collect::<Vec<_>>();
        let insertions = files.iter().map(|file| file.2).sum::<usize>();
        let deletions = files.iter().map(|file| file.3).sum::<usize>();
        let mut value = Map::new();
        value.insert("files_changed".into(), json!(files.len()));
        if insertions > 0 {
            value.insert("insertions".into(), json!(insertions));
        }
        if deletions > 0 {
            value.insert("deletions".into(), json!(deletions));
        }
        value.insert("files".into(), json!(entries));
        Value::Object(value)
    }

    fn source_copy_json(
        id: &str,
        token: &str,
        label: &str,
        path: &str,
        files: &[SourceFile],
        rows: &[Propagation],
    ) -> Value {
        let mut actions = vec![json!({
            "operation": "import",
            "destination": "personal:source",
            "existed": true,
            "diff": source_diff_json(files),
        })];
        for &(destination, changed, insertions, deletions) in rows {
            if changed == 0 {
                actions.push(json!({
                    "operation": "skip",
                    "destination": destination,
                    "existed": true,
                }));
            } else {
                actions.push(json!({
                    "operation": "update",
                    "destination": destination,
                    "existed": true,
                    "diff": diff_json(changed, insertions, deletions),
                }));
            }
        }
        json!({
            "id": id,
            "token": token,
            "label": label,
            "consequence": {
                "operation": "import",
                "path": path,
                "actions": actions,
                "totals": { "deployments": rows.len() },
            },
        })
    }

    fn import_source_options_json() -> Value {
        json!([
            source_copy_json(
                "claude:project",
                "1",
                "claude · project",
                r"C:\Users\swern\ghub\sernst\skills\.claude\skills\importing-meeting-notes",
                &CLAUDE_PROJECT_FILES,
                &CLAUDE_PROJECT_PROPAGATION,
            ),
            source_copy_json(
                "shared:global",
                "2",
                "shared · global",
                r"C:\Users\swern\.agents\skills\importing-meeting-notes",
                &SHARED_GLOBAL_FILES,
                &SHARED_GLOBAL_PROPAGATION,
            ),
            source_copy_json(
                "antigravity:global",
                "3",
                "antigravity · global",
                r"C:\Users\swern\.gemini\antigravity\skills\importing-meeting-notes",
                &ANTIGRAVITY_GLOBAL_FILES,
                &ANTIGRAVITY_GLOBAL_PROPAGATION,
            ),
        ])
    }

    fn propagation_options_json() -> Value {
        json!([
            {
                "id": "import-update",
                "token": "1",
                "label": "Import + update",
                "recommended": true,
                "consequence": {
                    "operation": "update",
                    "totals": { "deployments": 5, "updated": 4, "skipped": 1 },
                },
            },
            {
                "id": "import-only",
                "token": "2",
                "label": "Import only",
                "consequence": {
                    "operation": "import",
                    "totals": { "stale": 4 },
                },
            },
        ])
    }

    fn import_destinations_json() -> Value {
        json!([
            {
                "id": "personal:source",
                "kind": "source",
                "label": "personal (source)",
                "source": "personal",
            },
            {
                "id": "claude:global",
                "kind": "deployment",
                "label": "claude · global",
                "target": "claude",
                "scope": "global",
            },
            {
                "id": "claude:project",
                "kind": "deployment",
                "label": "claude · project",
                "target": "claude",
                "scope": "project",
            },
            {
                "id": "shared:global",
                "kind": "deployment",
                "label": "shared · global",
                "target": "shared",
                "scope": "global",
            },
            {
                "id": "shared:project",
                "kind": "deployment",
                "label": "shared · project",
                "target": "shared",
                "scope": "project",
            },
            {
                "id": "antigravity:global",
                "kind": "deployment",
                "label": "antigravity · global",
                "target": "antigravity",
                "scope": "global",
            },
        ])
    }

    fn remove_scope_options_json() -> Value {
        json!([
            {
                "id": "project",
                "token": "1",
                "label": "Remove project copies where both exist",
                "effect": "− 50 deployments, 285 files",
                "consequence": {
                    "operation": "remove",
                    "totals": { "deployments": 50, "files": 285 },
                },
            },
            {
                "id": "global",
                "token": "2",
                "label": "Remove global copies where both exist",
                "effect": "− 50 deployments, 285 files",
                "consequence": {
                    "operation": "remove",
                    "totals": { "deployments": 50, "files": 285 },
                },
            },
            {
                "id": "both",
                "token": "3",
                "label": "Remove both copies where both exist",
                "effect": "− 75 deployments, 463 files",
                "consequence": {
                    "operation": "remove",
                    "totals": { "deployments": 75, "files": 463 },
                },
            },
        ])
    }

    fn remove_scope_decision() -> Decision {
        Decision {
            id: "removal_scope".into(),
            preamble: Some("25 unambiguous deployments are removed in every option.".into()),
            prompt: "Select removal scope".into(),
            options: vec![
                scope_option(
                    "project",
                    "1",
                    "Remove project copies where both exist",
                    50,
                    285,
                ),
                scope_option(
                    "global",
                    "2",
                    "Remove global copies where both exist",
                    50,
                    285,
                ),
                scope_option("both", "3", "Remove both copies where both exist", 75, 463),
            ],
            ..Decision::default()
        }
    }

    #[test]
    fn the_remove_plan_event_carries_each_alternatives_blast_radius() {
        let plan = remove_plan(
            vec![availability_row("teach", "1", ["b", "-", "-"])],
            remove_scope_decision(),
            true,
        );
        let view = plan.view();
        let data = plan_event_data(
            &view,
            0,
            false,
            PlanAuthorization {
                kind: "selection",
                mode: "prompt",
                default: None,
            },
            &PlanSelection {
                targets: vec!["claude".into(), "shared".into(), "antigravity".into()],
                targets_explicit: false,
                scope: None,
                scope_explicit: false,
            },
        );
        assert_eq!(
            data,
            json!({
                "plan_id": "remove:test",
                "revision": 0,
                "command": "remove",
                "dry_run": false,
                "authorization": {
                    "kind": "selection",
                    "mode": "prompt",
                    "sequence": ["removal_scope"],
                    "resolved": {},
                    "pending": ["removal_scope"],
                    "prompt": { "dimension": "removal_scope" },
                },
                "selection": {
                    "targets": {
                        "mode": "inferred",
                        "names": ["claude", "shared", "antigravity"],
                    },
                    "scope": { "mode": "inferred" },
                },
                "destinations": [
                    {
                        "id": "claude:global",
                        "kind": "deployment",
                        "label": "claude · global",
                        "target": "claude",
                        "scope": "global",
                    },
                    {
                        "id": "claude:project",
                        "kind": "deployment",
                        "label": "claude · project",
                        "target": "claude",
                        "scope": "project",
                    },
                ],
                "entries": [
                    {
                        "skill": "teach",
                        "actions": [],
                        "available": ["claude:global", "claude:project"],
                    },
                ],
                "decisions": [
                    {
                        "id": "removal_scope",
                        "prompt": "Select removal scope",
                        "state": "pending",
                        "options": remove_scope_options_json(),
                    },
                ],
                "summary": { "skills": 1, "actions": 0, "available": 2 },
            })
        );
    }

    #[test]
    fn import_revision_zero_serializes_every_source_option_and_its_propagation() {
        let plan = import_plan(
            "Import plan",
            vec![("Into".into(), "personal (source)".into())],
            Vec::new(),
            vec![
                import_source_decision(None),
                Decision {
                    id: "propagation".into(),
                    deferred_heading: Some(
                        "Propagation modes (chosen after the source copy)".into(),
                    ),
                    prompt: "Select propagation".into(),
                    options: propagation_modes(
                        "Replace the source, then synchronize every deployment shown for that copy.",
                        "Replace the source; write no deployments and leave the other 4 out of date.",
                    ),
                    ..Decision::default()
                },
            ],
        );
        let view = plan.view();
        let data = plan_event_data(
            &view,
            0,
            false,
            PlanAuthorization {
                kind: "progressive",
                mode: "prompt",
                default: None,
            },
            &PlanSelection::default(),
        );
        assert_eq!(super::plan_event_name(0), "plan");
        assert_eq!(
            data,
            json!({
                "plan_id": "import:importing-meeting-notes",
                "revision": 0,
                "command": "import",
                "dry_run": false,
                "authorization": {
                    "kind": "progressive",
                    "mode": "prompt",
                    "sequence": ["source_copy", "propagation"],
                    "resolved": {},
                    "pending": ["source_copy", "propagation"],
                    "prompt": { "dimension": "source_copy" },
                },
                "selection": {
                    "targets": { "mode": "inferred", "names": [] },
                    "scope": { "mode": "inferred" },
                },
                "destinations": import_destinations_json(),
                "entries": [],
                "decisions": [
                    {
                        "id": "source_copy",
                        "prompt": "Select source copy",
                        "state": "pending",
                        "options": import_source_options_json(),
                    },
                    {
                        "id": "propagation",
                        "prompt": "Select propagation",
                        "state": "pending",
                        "options": propagation_options_json(),
                    },
                ],
                "summary": { "skills": 0, "actions": 0 },
            })
        );
    }

    #[test]
    fn import_revision_one_records_the_resolved_source_and_keeps_both_dimensions() {
        let plan = import_plan(
            "Import plan — source copy 2 selected",
            Vec::new(),
            Vec::new(),
            vec![
                import_source_decision(Some("shared:global")),
                Decision {
                    id: "propagation".into(),
                    prompt: "Select propagation".into(),
                    options: propagation_modes(
                        "Replace the source and synchronize 5 deployments (1 source copy, 4 updated).",
                        "Replace the source; write no deployments and leave 4 out of date.",
                    ),
                    ..Decision::default()
                },
            ],
        );
        let view = plan.view();
        let data = plan_event_data(
            &view,
            1,
            false,
            PlanAuthorization {
                kind: "progressive",
                mode: "prompt",
                default: None,
            },
            &PlanSelection::default(),
        );
        assert_eq!(super::plan_event_name(1), "plan.updated");
        assert_eq!(
            data,
            json!({
                "plan_id": "import:importing-meeting-notes",
                "revision": 1,
                "command": "import",
                "dry_run": false,
                "authorization": {
                    "kind": "progressive",
                    "mode": "prompt",
                    "sequence": ["source_copy", "propagation"],
                    "resolved": { "source_copy": "shared:global" },
                    "pending": ["propagation"],
                    "prompt": { "dimension": "propagation" },
                },
                "selection": {
                    "targets": { "mode": "inferred", "names": [] },
                    "scope": { "mode": "inferred" },
                },
                "destinations": import_destinations_json(),
                "entries": [],
                "decisions": [
                    {
                        "id": "source_copy",
                        "prompt": "Select source copy",
                        "state": "resolved",
                        "resolved": "shared:global",
                        "options": import_source_options_json(),
                    },
                    {
                        "id": "propagation",
                        "prompt": "Select propagation",
                        "state": "pending",
                        "options": propagation_options_json(),
                    },
                ],
                "summary": { "skills": 0, "actions": 0 },
            })
        );
    }

    #[test]
    fn availability_reaches_the_machine_stream_without_becoming_an_action() {
        let plan = remove_plan(
            vec![availability_row("teach", "1", ["b", "-", "-"])],
            Decision::default(),
            false,
        );
        let view = plan.view();
        let data = plan_event_data(
            &view,
            0,
            true,
            PlanAuthorization {
                kind: "selection",
                mode: "dry-run",
                default: None,
            },
            &PlanSelection {
                targets: vec!["claude".into(), "shared".into(), "antigravity".into()],
                targets_explicit: false,
                scope: None,
                scope_explicit: false,
            },
        );
        assert_eq!(
            data["entries"][0]["actions"].as_array().map(Vec::len),
            Some(0)
        );
        assert_eq!(
            data["entries"][0]["available"],
            serde_json::json!(["claude:global", "claude:project"])
        );
        assert_eq!(
            data["summary"]["actions"], 0,
            "the headline count stays present and honest"
        );
        assert_eq!(data["summary"]["available"], 2);
        assert_eq!(
            data["summary"].get("remove"),
            None,
            "zero per-operation counts are omitted"
        );
        assert_eq!(
            data["selection"]["targets"]["names"],
            serde_json::json!(["claude", "shared", "antigravity"]),
            "gating must not reach the machine stream"
        );
    }
}
