# Maestro guidance review record

This is the single substantive review record for GPT/Codex maestro guidance.
Update it only for a semantic change or a new verified baseline. A no-op review
reports its check date outside the repository; do not create timestamp-only
churn here.

## Policy isolation and prescriptive routing — 2026-09-18

**Decision.** Codex now routes only to the self-contained `gpt-codex.md`; the
existing non-Codex policy moved to `shared-harness-policy.md`. The non-Codex
four-attempt judged loop is preserved. Codex fixes Luna < Terra < Sol, prohibits
Astra workers, and permits two judged attempts per deliverable.

**Evidence and validation actually run.** The local collaboration schema exposed
explicit `model` and `reasoning_effort` with fresh/limited forks; it controls
dispatch. The Codex subagents page was fetched this session. Terra/high
independently accepted seven reasoning-only scenario dry-runs and verified
normalized non-Codex preservation (header, relative link, GPT removal, and
profile-count repair only), including the Claude profile and four-attempt loop.
Local Markdown-link and `git diff --check` passed. The existing snapshot
provenance was checked: semantic retrieval 2026-09-11, DeepSWE 1.1, and
CursorBench 4.0. Generated data, `model-selection.md`, CLI files, and changelog
files were not modified.

**Gaps.** No live comparative ROI trial or external pricing verification was
run; no measured cost claim is made.

## Baseline — 2026-09-16

**Scope and decision.** Reviewed the GPT/Codex profile in
[`running-as-maestro`](../skills/running-as-maestro/SKILL.md). The guidance
adopts dynamic routing from the live available controls, risk-proportional
delegation and review, and correction invalidation. It does not hardcode model
names or tier choices. Claude-profile text is preserved.

**Verified evidence.** The following official pages were fetched and read in
this session:

- [OpenAI model guidance](https://developers.openai.com/api/docs/guides/latest-model)
  documents instruction sensitivity, calibration of testing for small changes,
  and prompting for subagent delegation.
- [Codex subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents)
  documents inherited model and reasoning effort, explicit controls, and the
  latency/token cost of increased reasoning effort.
- [Rethinking skills and prompts for GPT-6 Astra](https://developers.openai.com/blog/rethinking-skills-and-prompts-for-gpt-6-astra),
  dated 2026-09-11, recommends progressive disclosure and removing
  overconstraint. It is design rationale, not enduring control authority.

**Local harness observation.** The collaboration spawn schema presently exposes
`model` and `reasoning_effort`. `fork_turns: "all"` inherits and disallows an
override; `"none"` or a positive integer permits one. This is a dated local
observation, not a universal harness guarantee.

**Benchmark baseline.** The generated snapshot was refreshed in `origin/main`
at `dde5395`. It is existing evidence and was not modified by this review.

**Observed validation.** Skill frontmatter validation, local Markdown link
resolution, and `git diff --check` passed. A comparison against `dde5395`
confirmed every part of `SKILL.md` outside the GPT/Codex section is unchanged
after newline normalization. The generated benchmark snapshot and selection
reference are unchanged. Repository-wide gate evidence belongs in the PR.

**Independent scenario dry-run.** A separate reviewer read the actual proposed
instructions and found no blocking contradiction. Its action plans covered:

- Tiny CLI label edit: direct edit, focused check, required gates; no obligatory
  worker or judge.
- Small authorization fix: positive/negative tests and independent review;
  small size does not reduce security scrutiny.
- Cross-system feature after grilling: reuse current design reasoning, validate
  a representative end-to-end slice, then scale.
- Rejected interaction model: invalidate the old assumptions and derived
  criteria; validate the corrected user experience.
- Renamed models/missing effort control: use the live roster, pass only exposed
  controls, and disclose the missing control.
- Claude routing: ignore the GPT adapter and follow the unchanged profile and
  shared rules.
- Unchanged maintenance evidence: report no change without another reviewer or
  timestamp-only edits.

The review prompted an explicit instruction to reuse a still-current design
challenge from grilling. These are reasoning-only scenarios, not live execution
trials. No live Claude session or comparative cost/latency trial was run; no ROI
improvement is claimed as measured. Future usage should compare completion time,
external waiting, repair cycles, user interventions, and post-completion defects
on a small correction, a substantial feature, and a noncoding deliverable.
