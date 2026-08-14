# UX guidelines for mutating commands

## Governing principle

**Use of the CLI should teach the user how to use it.** Before an operation
runs, the user MUST be able to tell what was selected, where it will act, and
how large or risky the effect is. After it runs, the user MUST be able to tell
what happened, what did not happen, and whether further action is needed.
Output is part of the command's interface, not incidental logging.

Every rule below is a consequence of that principle, not an arbitrary style
choice. A new command should optimize for confidence, not mere terseness:
infer safe defaults without interrogating the user, disclose consequential
inference before acting, request authorization against a concrete plan, and
report a specific result afterward. When a situation this document does not
enumerate comes up, re-derive the answer from the principle rather than from
the nearest example.

The key words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative.
Section headings below separate **invariants** (never violated, changeable
only as a deliberate CLI-wide policy revision) from **defaults** (the right
choice absent a specific documented reason — see
[Defaults and the departure test](#defaults-and-the-departure-test)).

## Authorization model (invariant)

A mutating command MUST compute and render a complete semantic plan before
asking the user anything. Prompting before showing anything is the defect
this document exists to eliminate — it must never recur.

A command MAY then use more than one prompt, but only when genuine
independent dimensions remain unresolved. Each prompt resolves **exactly one**
dimension, answered with **one single token** copied from a rendered option
line. Compound answers, matrix coordinates, delimiter-separated lists, or any
other multi-token input are forbidden — a prompt is either a yes/no question
or a numbered single-choice list, never both at once and never two questions
merged into one line.

Before every prompt after the first, the command MUST re-render the plan,
narrowed by every answer already given. The final prompt's answer authorizes
and immediately applies the fully resolved plan; it is a selection, not a
confirmation, so **no trailing `[y/N]` follows it**.

Two prompts is the normal ceiling. A third prompt requires a documented
safety need; needing more than three means the command's decision model is
wrong — split it into subcommands, require explicit flags, or redesign it,
rather than continuing to interrogate the user.

```text
> skill-manager import importing-meeting-notes
Import plan
                                              <- complete plan renders first
  1  claude · project        ...
  2  shared · global         ...
  3  antigravity · global    ...
Select source copy [1-3, c to cancel]: 2

Import plan — source copy 2 selected         <- re-rendered, narrowed
  1  Import + update  (recommended)
  2  Import only
Select propagation [1-2, c to cancel]: 1

Imported importing-meeting-notes from shared · global into personal (source).
...                                           <- final answer applies immediately
```

`import` is the one command whose decision model has two genuine dimensions
— source copy, then propagation mode — because a multi-copy import cannot
safely infer either. Every other command in this design resolves in at most
one prompt: a new command should default to one and treat two as the
exception it is for `import`, not the norm.

## Inference versus selection (invariant, with a guard rule)

Use the least burdensome decision shape that remains safe:

1. **Infer and review.** When a documented, deterministic rule can choose
   without risking unique data or violating reasonable user intent, infer it
   silently, show the consequential result once in the plan, and use one
   yes/no confirmation to authorize. Teach the flags that would have changed
   the inference **only on cancel**, so the plan itself stays clean.
2. **Enumerate and select.** Reserve a selection prompt for a genuine branch
   point: materially different outcomes exist, no safety-preserving policy
   can choose between them, and the choice affects authoritative content,
   deletion scope, or another consequential result. Render every option in
   the plan and ask for one token from one option line; that answer is final.
3. **Progressively narrow.** When multiple genuine dimensions remain, expose
   every dimension and its consequences in the initial complete plan, then
   ask one dimension at a time, re-rendering before each subsequent question.

**Guard rule: a branch is not "genuine" merely because a flag could alter a
safely inferred preference.** If a documented policy already picks the right
answer (default scope, default target set, recommended propagation mode), a
configuration flag existing for that preference does not turn it into a menu
question. Do not convert every configurable default into a selection prompt —
that reintroduces the interrogation this design eliminates. A dimension
earns a prompt only when its answer materially changes what gets written and
cannot be safely inferred; presentation preferences and facts already fixed
by explicit flags never earn prompts.

```text
Bad: turning an inferable default into a menu
  Select scope:
    1  global
    2  project
  (global was always going to be correctly inferred here — this is
   interrogation for its own sake.)

Good: infer, show, and only explain on cancel
  Scope  🌐 global (inferred)
  ...
  Apply this load plan to 3 enabled targets? [Y/n] n
  Cancelled.
  Hint: targets and scope were inferred. Re-run with --claude, --shared,
  --antigravity, --all, or --target NAME, and --global or --project, to
  change this plan.
```

### Ambiguous add operands

`source add` and `target add` accept a path/location and a name. When two
bare operands leave their roles genuinely unresolved, the command MUST not
guess from legacy argument order. `source add` first recognizes an explicit
GitHub URL or GitHub shorthand through the canonical source parser, before
testing either operand as a folder location. Otherwise, exactly one existing
folder determines the path.
Identical operands are the documented exception: one is used as the location
and one as the name.

When both operands are folders, or neither is a folder after the explicit
source-reference rule, the command MUST show an ambiguity warning and a
complete two-alternative add plan before one numbered selection prompt. The
two mappings and `c` to cancel are the only choices; there is no default and
the chosen mapping immediately applies. This is a genuine branch, not a
preference that legacy ordering can safely infer.

```text
Ambiguous source add operands: `alpha` and `beta` could each be the name or location.

Source add plan
  1  Name alpha; location beta
  2  Name beta; location alpha
Select mapping [1-2, c to cancel]:
```

`--yes`, `--no-input`, JSON output, and recipe carriers cannot make that
choice. They MUST fail before mutation and name the canonical unambiguous
form: one location operand plus `--name NAME`. Recipes remain explicit fields
and never prompt or infer roles. Ambiguity is a structured `diagnostic`
warning in NDJSON, carrying both mappings and the explicit-form resolution;
it MUST NOT synthesize a `plan` event outside the stable plan schema.

## Read-only result rendering

Read-only commands do not need a plan or authorization prompt, but their
output is still a user interface and MUST follow significance gating. A
multi-result inspection uses a clear separator before every result after the
first, a cyan-bold title, labeled metadata, and blank lines only where they
separate distinct information. Semantic labels and states may be colored;
embedded user-authored Markdown and code MUST remain verbatim and uncolored.

`describe` is the reference shape. A skill result shows its name, trigger
text, and a bounded source excerpt. A source result shows its name,
line-by-line configuration, an optional bounded README excerpt, then the
source's skills and their trigger text. A missing README falls back to the
first 20 raw `SKILL.md` lines; a README is limited to 100 logical lines. A
truncated excerpt has one separate dimmed notice. Human output MUST make
resolver status (effective, excluded, or shadowed) visible when it is
relevant, while ordinary unqualified inspection stays focused on effective
skills.

Unmatched selectors are warnings when other results survive. A wholly empty
result is a nonzero, actionable error rather than a blank success. NDJSON
uses structured warning/result/summary events and never contains ANSI codes.

## Significance gating (invariant — considered essential)

Only significant information is displayed. Columns, legend/key entries,
footer counts, and whole sections are elided when they carry no information
for the current plan revision. Significance is evaluated independently for
every rendered semantic state (an initial plan, each narrowed re-render, or a
read-only result) and is recomputed after every answer.

Precise rules:

1. **A destination column is dropped only when every cell is the semantic
   none-value** (`—`/`None`/no action), never merely because every surviving
   cell happens to share the same value. `+` in every `claude` cell still
   means every item will be written to `claude` — dropping it would hide the
   blast radius from the user. The none-only test is intentionally strict.

   ```text
   Correct: identical-but-present values stay, because they are real writes
   skill        change                           claude  shared  antigravity
   -----------  -------------------------------  ------  ------  -----------
   teach        new deployment, 1 file, +46/-0   +       +       +

   Wrong: dropping a uniform-but-actionable column hides blast radius
   skill        change                           shared  antigravity
   -----------  -------------------------------  ------  -----------
   teach        new deployment, 1 file, +46/-0   +       +
   (claude silently disappeared even though it is being written to)
   ```

2. **The inferred/explicit asymmetry is a rule, not a coincidence.** A
   uniform scope that was **inferred** is hoisted to one
   `Scope  {location} (inferred)` line instead of repeating per cell,
   because the user never stated it and the plan is the only place they can
   learn what the command decided on their behalf. A uniform scope that was
   **explicitly selected** (e.g. via `--global`) is omitted entirely — the
   user who typed the flag does not need it read back to them. Same
   uniform value, opposite rendering, because the two cases differ in
   exactly the fact that matters: whether the user already knows it. Per-cell
   scope remains whenever scopes differ by destination or row, regardless of
   how the scope was chosen. `↕ both` (global and project) is never omitted,
   even when uniform, because it doubles the deployment count.

   ```text
   Inferred, uniform: hoisted, because the user must be told what was chosen
   Scope  🌐 global (inferred)

   skill  change                           claude
   -----  -------------------------------  ------
   teach  new deployment, 1 file, +46/-0   +

   Explicit, uniform (--global --claude): no Scope line, flag already said it
   skill  change                           claude
   -----  -------------------------------  ------
   teach  new deployment, 1 file, +46/-0   +
   ```
3. Runtime legends/keys contain exactly the markers and locations present in
   surviving rows or nonzero footer clauses. The plan footer is the compact
   legend; no separate legend line prints unless a rendered symbol would
   otherwise be ambiguous.
4. **Counts are nonzero-only.** `0 changes` or `: 0` never render; the whole
   clause is omitted instead. This applies recursively to plan footers,
   result footers, `ls`/status summaries, and nested machine summaries meant
   for human rendering. Optional sections (destination-specific detail,
   file-change lists, unmatched notices, unchanged counts, cancel hints) are
   entirely absent when empty.
5. Hint lines name only the decisions that were actually inferred, and only
   the flags that can change those specific decisions. Explicit choices are
   never re-explained.
6. **Gating applies identically to interactive and redirected output.**
   Redirected human output is still dynamic human presentation; keeping dead
   columns "for scripts" would make the primary interface worse without
   giving scripts anything, because the actual stable machine contract is
   the NDJSON stream (see
   [The structured NDJSON contract](#the-structured-ndjson-contract)), not
   redirected text.

```text
Rejected: a static legend exposing unused keys
2 changes across 1 selected target: + 2 new, ↑ 0 overwrite, ✓ 0 already identical
Legend: + new, ↑ overwrite, ✓ already identical, 🌐 global, 📁 project, ↕ both

Chosen: only what this invocation actually uses
2 changes across 1 selected target: + 2 new
```

## Degenerate rendering (invariant)

A table requires at least two rows **or** at least two significant
destinations. One row across multiple destinations still renders as a table,
because comparing across destinations is the point. But when gating would
leave a table with only an identity column and nothing to compare, there is
no action left to review — collapse to a sentence instead of rendering a
table that carries no information.

```text
Rejected: a table with nothing left to show
skill
-----
teach

Chosen: the equivalent sentence
teach is already identical at every selected destination.
```

```text
One item, one destination — chosen sentence
Remove plan

− managing-skills from claude: 3 files

1 deployment removal across 1 selected target: − 1 remove; 1 skill, 3 files
```

```text
One item, multiple significant destinations — chosen table (comparison is the point)
Scope  📁 project (inferred)

skill              change                   claude  shared
-----------------  -----------------------  ------  ------
reviewing-my-code  2 files changed, +11/-5  ↑       ↑
```

## Column grammar and table layout

**Invariants:**

- Headers are lowercase. Exactly two ASCII spaces separate columns; no
  trailing spaces. The dashed separator matches each surviving column's
  Unicode display width, computed with the `unicode-width` crate (see
  `status::display_width`, which wraps `UnicodeWidthStr::width`). Some
  symbols are double-width (`🌐`, `📁`) and some are single-width (`↕ ↑ − ✓
  —`); alignment MUST account for this rather than assuming one
  character-cell per glyph.
- The identity column (normally `skill`) is never elided while a table
  exists.
- An action cell is `{action} {location-symbol} {location-word}` only when
  location is significant per the gating rules above; otherwise it is just
  `{action}`. `both` renders as `↕ both`. If global and project genuinely
  differ in action for one target, render both explicitly, e.g.
  `+ 🌐 global / ↑ 📁 project` — never collapse unlike actions into one
  symbol.
- A single non-tabular destination (one `copy` destination, one `import`
  source) is shown once as labeled metadata (`Destination`, `From`/`Into`)
  rather than repeated per row.
- Width is content-driven and never truncates. The approved renderer
  performs **no terminal-width detection**; it prints the full significant
  table and lets the terminal wrap. This applies identically to TTY and
  redirected output (see
  [Non-goals](#non-goals--explicitly-deferred)).

**Defaults:**

- Matrix layout (identity + optional provenance + optional `change` +
  destination columns in configured order) for multiple destinations.
- Configured destination order (the order targets are defined in, not the
  order rows happen to mention them).
- Two-space column gaps, cyan-bold section headings.

```text
skill        change                           claude  shared  antigravity
-----------  -------------------------------  ------  ------  -----------
teach        2 destination-specific changes   +       +       ↑
in-my-voice  new deployment, 2 files, +86/-0  +       —       +
```

## Symbols and colors (reference table)

This is the exhaustive specification. A runtime plan or result renders only
the entries actually used by that invocation — this table itself is design
reference, never printed as terminal output.

| Context | Symbol/text | Meaning | TTY color |
| --- | --- | --- | --- |
| state | `✓` | up-to-date | green |
| state | `↑` | needs-update | yellow |
| state | `✗` | not-loaded | uncolored |
| state | `~` | no-connection / unsourced deployed | cyan |
| location | `🌐 global` | global scope only | uncolored |
| location | `📁 project` | project scope only | uncolored |
| location | `↕ both` | global and project scopes | uncolored |
| location | `— none` | no deployment | uncolored |
| location | `⚠ mixed` | targets use differing non-empty scope sets | uncolored |
| plan action | `+` | create a deployment or copy | green |
| plan action | `↑` | overwrite/update a deployment or copy | yellow |
| plan action | `←` | replace source content from a deployment | yellow |
| plan action | `−` | remove a deployment | red |
| plan action | `✓` | already identical / skipped | uncolored |
| plan empty cell | `—` | no action at this destination | uncolored |
| file | `+` | added file | green |
| file | `~` | modified file | yellow |
| file | `-` | deleted file | red |
| result footer | `✓` | completed successfully | green |
| result footer | `—` | unchanged/skipped | uncolored |

Raw ANSI codes: green `32`, yellow `33`, red `31`, cyan `36`, bold-cyan
heading `1;36`, reset `0`. Only semantic cells/fragments are colored — never
whole lines, borders, or decorative emoji. `status::styled_state` and
`plan::PlanAction::color_code` are the canonical implementations; reuse them
rather than inventing a synonym symbol or a new color for an existing
meaning (see [Non-goals](#non-goals--explicitly-deferred) for the invariant
this table backs: *semantic styling is consistent*).

**Invariant: interactive symbol mode degrades to word mode when output is
redirected.** ANSI escapes, emoji, and compact symbols are a TTY-only
convenience; a non-TTY stream (piped or redirected) uses stable words so the
output stays legible and greppable without a terminal.

```text
TTY:        claude  shared  antigravity
            ------  ------  -----------
            🌐 global  📁 project  ↕ both

Redirected: claude       shared       antigravity
            -----------  -----------  -----------
            global       project      both
```

Action words for redirected output: `load`, `update`, `remove`, `copy`,
`import`, `unchanged`. Result-footer `✓` becomes `completed`.

## Confirmation copy and defaults

**Invariant: the default follows destructiveness, not the command name.**
Additive, reversible, or easily regenerated work defaults to yes (`[Y/n]`);
deletion, authoritative overwrite, adoption of external state, or
difficult-to-reverse work defaults to no (`[y/N]`). If reasonable users could
lose unique work, default no. When **every** option in a selection prompt is
destructive or authoritative, there is **no preselected default** — pressing
Enter must reprompt, never silently pick the first (typically
"recommended") option.

```text
load / update (regenerable, defaults yes)
Apply this load plan to 3 enabled targets? [Y/n]

remove (destructive, defaults no)
Remove these 2 deployments from 1 selected target? [y/N]

import propagation (both options overwrite canonical source content — no default)
Select propagation [1-2, c to cancel]:
```

A recommended option (`(recommended)`) is guidance printed with the plan,
never consent — it must never become an automation default either (see
`--yes`/`--no-input` in
[Alternate execution modes](#alternate-execution-modes)).

**Defaults:** one short binary question naming the reviewed operation and
significant blast radius for a single complete outcome; one selection
question per unresolved dimension, enumerated from the plan; never introduce
facts in the prompt text itself — the rendered plan is the evidence.

## Cancellation

**Invariants:**

- Cancellation is available at every prompt (a yes/no decline, or an
  explicit `c` selection token).
- It always exits `0`.
- It performs no writes.
- It prints `Cancelled.` — calm, not styled as an error.
- A hint teaching relevant flags follows **only** when the plan being
  cancelled contained safely-inferred decisions; it names only the flags
  that would change **those specific** inferred decisions. Cancelling an
  explicit selection, or a plan that was already fully explicit, adds no
  hint — there is nothing to teach.

```text
Apply this load plan to 3 enabled targets? [Y/n] n
Cancelled.
Hint: targets and scope were inferred. Re-run with --claude, --shared,
--antigravity, --all, or --target NAME, and --global or --project, to
change this plan.
```

```text
Select removal scope [1-3, c to cancel]: c
Cancelled.
(no hint: the user cancelled an explicit selection)
```

Generic, non-specific help text ("run with --help for more options") is
forbidden as a cancellation hint.

## Result and footer grammar

**Invariants:**

- A plan footer is one grammatical line: total actionable work, significant
  destination/blast-radius context, then only nonzero action categories.
  Example shape: `{N} changes across {target label}: + {N} new[, ↑ {N}
  overwrite][, ✓ {N} already identical]`. Per command: `load`: `{N} changes
  across {target label}: + {N} new[, ↑ {N} overwrite][, ✓ {N} already
  identical]`; `update`: `{N} updates across {target label}`; `remove`, one
  resolved plan: `{N} deployment removals across {target label}: − {N}
  remove; {N} skill(s), {N} files`; `remove`, unresolved branch: one nonzero
  base clause plus one effect line per alternative; `copy`: `{N} changes to 1
  destination: + {N} new[, ↑ {N} overwrite]`; `import`, resolved import-only:
  `1 source replacement from {TARGET} · {SCOPE}[; {N} deployment(s) left out
  of date]` (the staleness clause is omitted when propagation would leave
  nothing out of date); `import`, resolved import + update: `1 source
  replacement; {N} deployments synchronized (1 source copy, {N} updated[, {N}
  already identical])` (the "already identical" clause covers any deployment
  that already matched the resolved copy without being the copy itself). An
  explicit `--update`/`update:true` on a degenerate plan (nothing left to
  synchronize) still records `import-update` in the machine stream honestly,
  but the human footer falls back to the import-only form regardless — the
  import+update form has no way to omit its `{N} updated` clause, so it is
  never used when that count is zero.
- `import`'s pre-apply preview reframes to match whichever propagation mode
  is actually resolved: only when import + update is genuinely pending or
  chosen does the resolved copy's nested preview read as `Propagation
  preview` with `↑ {N} file(s) changed` (a pending write). Once import-only
  is resolved and something would genuinely be left behind, the same
  destinations render instead as `Left out of date` with `{N} file(s)
  behind, +{ins}/-{del}` — never the update marker — since nothing will
  actually be written. A destination that is already synchronized (or is
  the resolved copy's own identity) carries nothing under the staleness
  framing and is dropped, the same recursive none-value rule as elsewhere.
- A result footer after apply uses the same nonzero-only, comma-separated
  grammar: `✓: {N} {result description}[, —: {N} unchanged]`. The complete
  entry is green in a TTY.
- Per-item apply progress is one line per applied item (e.g. `Loaded teach ->
  claude`, `Overwrote teach -> antigravity`), naming the specific action
  taken — not a generic "processed" line — with scope included only when the
  plan spans more than one scope.
- Zero counts are omitted rather than rendered as `: 0` (recursive; same rule
  as significance gating above).
- `Dry run — no changes were made.` follows the plan footer after one blank
  line, and replaces the confirmation prompt and per-item apply lines
  entirely — never a per-item `(dry-run)` echo appended to normal output.

```text
5 changes across 3 enabled targets: + 4 new, ↑ 1 overwrite, ✓ 1 already identical

Loaded teach -> claude
Loaded teach -> shared
Overwrote teach -> antigravity
Loaded in-my-voice -> claude
Loaded in-my-voice -> antigravity

✓: 5 deployments changed (4 loaded, 1 overwritten), —: 1 unchanged
```

**A no-op or miss names the specific item and reason and renders no empty
plan or confirmation** — this is an invariant, not a nicety, because an empty
table would violate significance gating twice over (an all-none-value table
that should not exist). A syntactically valid positional pattern that
matches nothing is the one documented exception: it retains the existing
`NotFound`/nonzero-exit contract for backward compatibility rather than
becoming a plain zero-count message.

```text
teach is up to date across 3 enabled targets.
wait-what is not deployed to any enabled target in global or project scope.
```

## The structured NDJSON contract

A mutating command MUST emit an initial `plan` event, matching the complete
human plan, before any prompt or write. Progressive narrowing emits one
`plan.updated` event — sharing the same `plan_id`, with `revision` increasing
monotonically — after each nonfinal answer and before the corresponding
narrowed re-render/next prompt. `resolved` and `pending` fields make prompt
order explicit so a consumer can reconstruct exactly what the human reviewer
saw at each step. No extra plan revision follows the final answer: apply
begins immediately, and subsequent action/summary events carry the fully
resolved choices.

**Invariant: gating applies to human output; the machine stream stays stable
and complete.** The NDJSON payload is significance-gated the same way the
rendered plan is (zero metrics, empty diffs, and inapplicable fields are
omitted rather than emitted as zero/null/empty), but it is never truncated
for brevity the way a human table can collapse to a sentence — it is the one
place scripts and tests get a complete, stable structural record regardless
of how minimal the human-facing render became.

```json
{"version":1,"event":"plan","level":"info","data":{"plan_id":"load:teach","revision":0,"command":"load","dry_run":false,"authorization":{"kind":"binary","mode":"yes","default":true},"selection":{"targets":{"mode":"inferred","names":["claude","shared","antigravity"]},"scope":{"mode":"inferred","value":"global"}},"destinations":[{"id":"claude:global","kind":"deployment","label":"claude · global","target":"claude","scope":"global"}],"entries":[{"skill":"teach","actions":[{"operation":"load","destination":"claude:global","existed":false,"diff":{"files_changed":1,"insertions":46}}]}],"summary":{"skills":1,"actions":1,"new":1}}}
```

Under `--json`/`--no-input`, an unresolved dimension emits its revision and
then fails with a disambiguating error naming the flags needed — it never
guesses. `--dry-run` emits only the complete, unnarrowed revision.

See [json.md](json.md) for the full event-envelope and recipe contract this
plan event lives inside.

## Alternate execution modes

**Invariants:**

- `--dry-run` renders the same plan — or, when multiple alternatives are
  still unresolved, every alternative — and makes no writes. It never
  prompts, because it cannot mutate; it ends with the `Dry run — …`
  conclusion instead.
- `--yes` renders and applies a fully resolved plan without prompting. It
  MUST fail rather than silently choose an unresolved branch. A
  `(recommended)` option is never implied by `--yes`.
- `--no-input` requires `--yes` to authorize a write; inferred defaults
  still compute and render, but nothing is written without explicit
  authorization.
- Ambiguity under noninteractive flags fails **before writing**, with an
  error naming exactly which flags would resolve it, e.g.: `Error: import
  requires a source copy and propagation mode before --yes; choose exactly
  one target and scope, then pass --update or --no-update.`

## Defaults and the departure test

The following are strong **defaults**, not universal invariants: matrix
layout for multiple destinations; sentence layout for one item at one
destination; configured destination order; one line of progress per applied
item; cyan-bold section headings; two-space table gaps.

A command MAY depart from a default only when its author can answer **yes**
to all four of these (the departure test, quoted from the design source):

1. Does the command's data shape or risk make the default materially less
   clear, less safe, or less legible?
2. Does the alternative still let a first-time user predict the effect
   before apply and verify the result afterward without external
   documentation?
3. Does it preserve significance gating, semantic color/symbol meanings,
   noninteractive safety, and the structured machine contract?
4. Can the reason and behavior be expressed as a testable rule rather than a
   one-off aesthetic preference?

If any answer is no, use the default. Invariants (as opposed to defaults)
may be changed only as a deliberate CLI-wide policy revision — never locally,
for one command, to make one situation look nicer.

## Non-goals / explicitly deferred

**Responsive terminal-width stacking is explicitly out of scope.** A
significant table renders at full content width; the terminal wraps it if
needed. There is no terminal-column detection, no injectable reporter width,
and no stacked-topology renderer. Do not "helpfully" add this — it was
considered and deliberately deferred because it would require a
cross-platform width source (Windows console plus Unix/`COLUMNS` fallback),
an injectable reporter width for deterministic tests, a second rendering
topology, and wide/narrow Unicode snapshot tests, for a benefit (avoiding
terminal wrap) that significance gating already achieves for the common
case. If a future need reopens this, it must ship as a deliberate, separately
justified follow-up, evaluated against the departure test above — not folded
quietly into an unrelated command change.

Skill names are never ellipsized or truncated, in either TTY or redirected
output, regardless of terminal width.

