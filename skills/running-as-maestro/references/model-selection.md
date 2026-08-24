# Selecting subagent model class and effort

The maestro owns every model-and-effort choice. Treat benchmark results as one
input to task-specific judgment, never as a routing table, capability taxonomy,
or default. The harness's current model roster, the exact task, tool access,
blast radius, iteration cost, and session billing remain authoritative.

## When to consult the snapshot

Read [benchmark-snapshot.md](benchmark-snapshot.md) when choosing an executor
for substantive autonomous coding and multiple pairings are plausible, or when
cost materially affects session ROI. It compares each published pairing's
average cost per benchmark task with its benchmark score and marks the
point-estimate Pareto frontier.

Do not load the snapshot for every dispatch. For a researcher or judge, consult
it only when role-specific reasoning leaves a genuine tie. Re-read it when the
session roster or task shape changes enough to invalidate an earlier choice.

## Interpret the evidence

- Compare rows only within one benchmark and its current task-set version.
  Harnesses, prompts, tools, scoring, pricing, task distributions, and effort
  labels differ. Benchmark cost is not necessarily the session user's billed
  cost.
- Prefer the cost/performance tradeoff relevant to the task over the highest
  score or cheapest row. A Pareto marker says only that no published row has
  both an equal-or-better point score and equal-or-lower point cost.
- Treat small score differences and overlapping uncertainty as ties. Do not
  manufacture a composite ROI score or cross-benchmark ranking.
- Preserve source model, effort, harness, and configuration labels. Map them to
  the current roster at decision time; do not infer static tier equivalences.
- Benchmarks are averages over a broad task set. Task-specific evidence can and
  should override them. The maestro remains accountable for explaining the
  pairing when the choice materially affects ROI.

## Weight by subagent role

**Executors/implementers:** these autonomous multi-file coding benchmarks are
most relevant here. Use their cost/performance frontier to sharpen a hypothesis
about the least expensive pairing likely to clear the task's quality bar, then
adjust for the actual language, repository, risk, tools, context, and expected
review/retry cost.

**Researchers/explorers:** benchmark evidence is a weak tie-breaker only.
Search strategy, repository comprehension, source quality, uncertainty handling,
and synthesis dominate successful-edit averages.

**Reviewers/judges:** benchmark evidence is also a weak tie-breaker. Defect
recall and precision, calibration, adversarial scrutiny, and evidence quality
matter more than patch success. The judge floor in the main skill is absolute:
the judge's model class must remain equal to or stronger than the executor and
may not be an older generation of the same family. Size judge effort separately
for review risk, evidence quality, and ambiguity; there is no equal-effort floor,
but never lower effort merely because the model-class floor is met. Never trade
the class floor for a cheaper benchmark row.

The snapshot is generated from allowlisted public sources and intentionally
contains no external narrative. Follow its provenance and caveats; if it is
stale, missing, or conflicts with current harness facts, rely on current facts
and task judgment instead.
