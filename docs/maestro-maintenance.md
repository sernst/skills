# Maintaining GPT/Codex maestro guidance

Use this playbook to keep the GPT/Codex profile in
[`running-as-maestro`](../skills/running-as-maestro/SKILL.md) current without
changing the Claude or other harness profiles. It is an agent-session entrypoint,
not a PR, deployment, or automation runbook.

## When to review

Start a review when the available model/control surface changes, a practical
quality or cost regression is reported, or a maintainer asks for one. An
occasional review about every six to eight weeks is useful, but a person must
invoke or schedule it separately. This playbook does not itself authorize
recurring automation, new CI, or separately billed benchmark/evaluation runs.

No change is a successful result. A mechanical content difference is an alert,
not a conclusion. The maintenance agent may close a no-change triage; obtain an
independent semantic review only for a substantive proposed guidance change.

## Evidence and authority

Read the last reviewed baseline first. Maintain one small
`docs/maestro-guidance-review.md` record (create it when establishing a verified
baseline, not for a timestamp-only update). Each entry should name the scoped
files and profile, sources checked and their retrieval/version evidence, the
baseline compared, availability gaps, semantic decision and rationale, scenarios
run, commands and outcomes, and the resulting change/no-change commit or diff.

Use this authority order for current controls: the live harness tool schema and
reported available controls; then official product documentation. Do not infer a
control, model, tier, default, or pricing rule from names, old prose, or a
benchmark. Check billing information only when the requested decision involves
billing or spend.

Bound source checks to the GPT/Codex profile and these primary references:

- [OpenAI model guidance](https://developers.openai.com/api/docs/guides/latest-model)
  for current model behavior and prompting guidance.
- [Codex subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents)
  for available agent configuration and inheritance behavior.
- [Rethinking skills and prompts for GPT-6 Astra](https://developers.openai.com/blog/rethinking-skills-and-prompts-for-gpt-6-astra)
  as dated design rationale, not enduring authority.
- [ChatGPT pricing](https://learn.chatgpt.com/docs/pricing) when subscription
  billing is in scope.
- [OpenAI API pricing](https://developers.openai.com/api/docs/pricing) when API
  billing is in scope.

If a required source cannot be retrieved or does not establish the relevant
claim, record it as unavailable or inconclusive. Do not call that result green,
and do not fill the gap from memory.

## Benchmark discipline

Keep the generated benchmark input and its checked-in snapshot workflow intact.
Inspect the relevant benchmark snapshot and selection reference only when model
selection is genuinely at issue. Do not refresh the snapshot for instruction
maintenance. Never hand-edit fixtures, source data, or generated benchmark
output to make a result fit. Benchmark changes may motivate review, but they do
not select a model or rewrite a profile by themselves.

## Review workflow

1. Compare the current GPT/Codex profile and the last review record with the
   requested trigger and known baseline.
2. Inspect the live tool schema or roster once; capture only controls actually
   exposed in this environment. Then make bounded checks of the sources above.
3. Diff the relevant skill, references, and benchmark evidence. Separate
   mechanical source/content changes from changed operational meaning.
4. For a substantive proposed change, ask an independent semantic reviewer to
   decide whether it is warranted, preserving the profile boundary and recording
   uncertainty. A no-change triage needs no second reviewer.
5. If needed, make the smallest GPT/Codex-profile edit. Preserve byte-for-byte
   Claude and other harness sections outside that profile unless separately
   authorized.
6. Validate the actual proposed guidance against realistic scenarios, including
   a tiny edit, risky small change, large design, correction, unavailable model
   or knob, and a Claude-profile regression check. Distinguish actual evidence
   from dry-run or reasoning-only evidence.
7. Produce a short cited change/no-change record. Include source gaps and any
   scenarios not run. Prepare a PR only when authorized; never merge, deploy,
   schedule automation, or start separately billed evaluation runs from this
   playbook alone.

## Copy-paste agent prompt

```text
Review and, only if warranted, update the GPT/Codex profile of
skills/running-as-maestro/SKILL.md. This is a bounded maintenance session, not
a deployment or automation task.

First read docs/maestro-guidance-review.md if it exists and compare its last
verified baseline with the current GPT/Codex profile, relevant references, and
the reported trigger. Inspect the live harness tool schema/available controls
once. It is the authority for what can be selected or passed today; official
documentation is next. Do not hardcode remembered model names, tiers, defaults,
or pricing. Check pricing only if this request requires a billing decision.

Use bounded official checks: OpenAI's latest-model guide,
learn.chatgpt.com/docs/agent-configuration/subagents, and the dated
developers.openai.com blog "rethinking-skills-and-prompts-for-gpt-6-astra" as
rationale only. If a source or control is unavailable, record the gap as
inconclusive; do not claim a green verification or invent a replacement.

Check ChatGPT subscription pricing only if subscription billing is in scope;
check OpenAI API pricing only if API billing is in scope.

Keep the Claude and every other harness profile byte-for-byte unchanged outside
the GPT/Codex profile. Preserve benchmark generated input and workflow: inspect
the existing selection reference/snapshot when relevant, but do not refresh it
for instruction maintenance and never manually edit generated benchmark data or
output. A mechanical content difference is only an alert. Obtain an independent
semantic review only for a substantive proposed guidance change; a no-change
triage needs no second reviewer.

Validate the final guidance with these scenarios: tiny edit; risky small
change; large design; user correction; unavailable model or control; and a
Claude regression. Clearly label observed actual behavior versus dry-run or
reasoning-only evidence. No change is a valid success.

Write or update one concise docs/maestro-guidance-review.md entry only when it
adds substantive evidence or a decision; avoid timestamps-only churn. Report a
short cited change/no-change record with baseline, source evidence, gaps,
decision, validation, and exact files changed. Do not create scheduled work,
new CI, separately billed evaluation runs, merge, or deploy. If the user
authorizes PR preparation, use expecting-pr-outputs for that PR; do not merge
or deploy.
```

## Review output shape

Keep the final record short: trigger and baseline; sources with links and
availability; semantic decision; changed files or explicit no-change; scenario
evidence labeled actual/dry-run; and follow-ups. Avoid routine date-only edits.
