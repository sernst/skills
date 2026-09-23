# GPT/Codex maestro adapter

Use this adapter only for the GPT/Codex profile named in the main skill. The
collaboration tool's current schema and roster are authoritative; this file
deliberately stores no model names, tier ladder, prices, or generation claims.

## Route from the live interface

Before the first worker creation, inspect the models and reasoning efforts the
collaboration tool currently exposes. Reuse that roster until the harness
reports a change; do not browse or rebuild it before every dispatch.

Choose the model and reasoning effort together for the actual role, risk,
ambiguity, context, tool work, and likely verification and retry cost. Prefer
the least expensive pairing plausibly sufficient for the whole route. For a
mapped, repeatable task, consider a lighter model or lower effort first; for
ambiguous or cross-cutting work, prioritize enough depth to resolve the
uncertainty. Highest effort requires a specific unsolved difficulty, not task
importance alone. These are judgment factors, not a fixed model mapping.

When the tool exposes model and effort controls for worker creation, pass both
explicitly. If either control is unavailable, acknowledge the limitation and do
not invent an argument. Do not infer capability or cost from a remembered name,
model recency, or generation; if cost is unavailable, say it is unknown. Do not
use benchmark rankings as routing defaults. If a continued worker cannot accept
a new model or effort, retain its existing pairing deliberately or create a
fresh worker with the new pairing.

Fork only the context the worker needs. Under the current collaboration schema,
a full-history fork inherits the parent's model and effort and cannot override
them. Therefore use a fresh fork or a limited-turn fork when setting an explicit
pairing, and put the durable facts, requirements, paths, and return contract in
the brief. Do not use a full-history fork merely to evade deliberate routing.
Use one only when the inherited pairing is itself the deliberate choice or the
needed context cannot be transferred reliably in a bounded brief. If the tool
schema changes, follow the schema exposed in the session rather than this
description.

## Own coherent work

Give one executor ownership of a coherent edit and its immediate focused
validation. Do not automatically split implementation, tests, and documentation
into different workers merely to shorten lifetimes. Split when the work is
independently parallel, requires a distinct specialty, or accumulated context
is materially impairing progress.

For a localized, reversible, low-risk change, the maestro may make the edit and
run focused validation directly when delegation would cost more than the work.
This path does not require an independent judge. It still requires inspection
of the resulting diff and any repository gate that applies to the change.

Before broad implementation, perform a brief design challenge when the work
changes a security boundary, authorization, durable data, public contract,
cross-system workflow, or has ambiguous product or noncoding intent that would
be expensive to redo. Ask for the simpler viable alternative and the key
assumption whose failure would invalidate the proposed approach. Resolve the
material concern; reuse a documented challenge from the grill or design session
while its assumptions remain current instead of repeating it. Then ask the
executor for a representative end-to-end slice
before scaling the pattern. Use an independent challenger when the risk warrants
the extra perspective; this does not force a separate agent for every piece of
public-facing text.

Preserve security and high-risk review requirements. When a judge is warranted,
preserve the shared capability floor and independence rules. Give the judge:

- the user's original requirements and subsequent corrections;
- derived acceptance criteria, clearly labeled as derived;
- the diff or artifact scope and deterministic validation evidence; and
- earlier findings, decisions, and anything still unverified.

The judge may identify a mismatch between the derived criteria and the user's
intent. Criteria organize the review; they do not replace the source request.
Required build, test, CI, or release gates remain required regardless of a
worker or judge verdict.

For user-facing work, validate the artifact or workflow users will actually
experience. Inspect the rendered document, exercise the interaction, run the
real command against a disposable fixture, or capture equivalent artifact-level
evidence as appropriate. Source inspection and a worker's summary alone do not
establish user-facing correctness.

## Correct course without preserving stale assumptions

Treat a user correction as new authoritative input. Identify which assumptions,
acceptance criteria, designs, and downstream decisions it invalidates before
continuing. Update affected briefs and evidence packs; do not preserve a prior
decision merely because work already implements it.

Use a fresh worker when the current executor is anchored to the rejected
approach, repeatedly reframes the correction into the old design, or carries so
much stale context that a clean brief is cheaper and clearer. Otherwise a
focused continuation is reasonable. Preserve durable work that remains valid.

After one failed small repair, pause patching and diagnose the failure mode,
including whether the brief, design, environment, or capability pairing is the
problem. Then choose a corrected retry, reassignment, direct resolution, or user
escalation. This can stop retries earlier; it does not extend the shared maximum
execute/judge attempt ceiling.

## Track what is observable

Track factual elapsed time, tool-reported usage or cost when available, retry
count, and time spent waiting on external systems. Label estimates as estimates.
Do not claim a universal runtime or cost-growth formula, and do not count an
external wait as model work unless telemetry does. Use the observed record to
decide whether to continue, narrow, reassign, or stop a worker.

## Scope of these refinements

For this profile, the direct small-change path refines the general delegation
and judging defaults; coherent executor ownership refines automatic phase
splitting; the fork rule refines generic lifetime advice; correction
invalidation refines continuation choice; factual tracking replaces any
universal cost-growth claim; and diagnosis after one failed small repair may
stop the retry loop early. Live-roster selection and the ban on inferring price
from recency replace the shared claim that newer generations are usually
cheaper. The original-intent mismatch rule refines the shared rule for
acceptance criteria: the user's intent remains authoritative when derived
review criteria disagree with it.

These refinements do not change the judge capability floor when a judge is
warranted, the shared maximum retry ceiling, security or high-risk review,
required repository gates, sandboxing, dispatch return contracts, parallelism
for genuinely independent work, or any other harness behavior.
