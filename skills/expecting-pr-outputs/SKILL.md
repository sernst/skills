---
name: expecting-pr-outputs
description:
  Produce PR chains (trains or stacks) and deployment runbooks as session
  deliverables. Use when the user requests PR outputs or asks to merge an
  existing chain. Covers linear branches, CI/CD readiness, and separately
  authorized fast-forward merges with pipeline monitoring.
---

Deliver one or more pull requests and a runbook that lets the user take them to
production. Producing PRs never implies permission to merge: merging requires
separate, explicit user authorization.

## The output contract

Group each PR into a meaningful, coherent change by theme and deployment
surface, independently understandable, reviewable, and revertable. Discover and
apply each repository's conventions per PR, including version bumps, changelogs,
local gates, and sanctioned exceptions for infra-only changes.

**Chain, train, and stack are synonyms.** PRs are cars; the final PR is the
caboose. On GitHub, use its native stack feature only when explicitly requested
or when the user calls the work a stack on GitHub. Ordinary chain/train wording
does not opt in; on other hosts, stack remains a synonym. Verify available host
capabilities before using them; do not assume automatic retargeting or PR closure.

**Green PR:** the agent considers implementation and review complete, and CI/CD
has finished successfully with no failed checks at the current pushed head.
Local checks alone are insufficient. **Green merge:** the target-branch pipeline
for the exact merged SHA has completed successfully. Absent, skipped, or
never-started pipelines are unverified, not green.

User-approved CI/CD exceptions must be explicit and recorded with their scope:
PRs/checks, heads or subsequent changes covered, target pipelines, and permitted
continuation. A PR/check waiver does not authorize advancing after a red merge.
A recovery train may explicitly authorize particular red cars and red merges,
including advance authorization. The user may also explicitly ignore CI/CD for
the whole chain, including its merge pipelines (for example, a new repo with no
CI). A general merge request grants no waiver. Label waived failures or missing
evidence as exceptions, never green; any unwaived failure stops progress.
CI/CD exemptions never waive agent completion/review, fast-forward correctness,
necessary safety gates, or merge authorization.

## Linear history and planning

Discover each repo's actual target branch and remote; never assume `main` or
`origin`. Plan one global merge order across repos from dependencies and deploy
constraints. **Choose the first PR's branch name before branching and use that
same exact name in every participating repo.**

Within each repo, base the first branch on its target and each subsequent branch
on its predecessor. PR bases may be predecessor branches where supported.
Inspect CI triggers early: ensure predecessor-based PRs can get green; do not
assume target-branch-only filters will run for them. Resolve missing triggers
within task scope, restructure preparation, or report a blocker; do not change
host configuration without authorization.

Merging must only append the identical, already-reviewed commits to the target
by fast-forward: no merge commits, squash, rebase, or rewriting during merges.
Each predecessor's head must be an ancestor of its successor, so landing a
predecessor requires no downstream restack.

If predecessors change during preparation, deliberately restack downstream
review branches before merging begins. Preserve intended version/changelog
content and regenerate lockfiles as needed; resolve conflicts and rerun local
and remote checks at every changed head. Use `--force-with-lease` only for those
review branches when necessary for deliberate restacking, never the target.
Divergence during merging is a stop, not permission to rewrite or force the target.

## The runbook

Commit one canonical Markdown runbook for the combined global order in the most
appropriate participating repo. Every PR must link to it. Use a durable git-host
link in the final response that survives worktree and branch cleanup (prefer a
committed SHA permalink). Make it executable without the session's context. Keep
exact commands, identifiers, expected results, and current evidence accurate as
PRs evolve; retain executed steps with their status.

Use these mandatory sections in this order, writing **None** if a section is
empty:

1. **Prerequisites** — Put human preparation as early as feasible: access,
   secrets, configuration, and other necessary setup. State pre-merge safety
   gates and expected results before the merge that relies on them.
2. **Merging** — Include a global merge-order table with linked PRs, repo when
   multi-repo, exact branch, applicable version, and short contents. Give steps
   the agent can execute and verify with this session's capabilities, including
   automated gates, deploys, and service rolls it can perform. Explain deploy
   dependencies, skew risks, and expected disruption where they affect order.
   Only irreducibly intermediate **MANUAL CHANGES** may interrupt merging:
   identify the exact change and instructions, the pause/resume condition, and
   why it cannot precede or follow the sequence. Human validation belongs after
   Post-merge Actions. Never blindly defer a necessary safety gate to satisfy
   this ordering: automate it, restructure the rollout, or report an unresolved
   blocker before the affected merge.
3. **Post-merge Actions** — Required human actions that the agent cannot
   automate, with exact instructions and dependencies. Service rolls are not
   automatically human-owned; automate them when authorized and supported.
4. **Validation** — A slim human checklist after all post-merge actions. Give
   each check a short, distinct, referenceable title, the simplest steps, and
   expected results; focus on the behavior and failures motivating the work.
   Record relevant non-blocking follow-ups here, clearly separate from checks.

Place deployment explanations, expected disruption/duration, and known follow-ups
in the relevant section without repeating them. Resolve capability gaps during
preparation so the Merging section is executable; explicitly report any gap
that remains rather than presenting the runbook as ready.

## Building and delivering the chain

1. Plan chunks, the shared first-branch name, global order, and runbook. Discover
   conventions, host behavior, CI triggers, and session capabilities early.
2. Implement and run each repo's full local gate at every tip. Complete review
   to the session's quality bar, fixing and re-verifying findings.
3. Finish preparatory restacking, push branches, and open PRs with appropriate
   bases and descriptions explaining the change, relevant review fixes, and
   deploy notes consistent with the runbook, including its canonical link.
4. Watch every PR's CI/CD to completion at its current head. Fix root causes and
   recheck changed heads. The default is a fully green chain; identify any
   explicit exceptions and blockers instead of claiming readiness without proof.
5. Before session completion, remove only task-created worktrees. First preserve
   dirty or unpushed work safely; never discard it to satisfy cleanup. Keep
   committed, pushed review branches accessible from the main worktree and
   leave unrelated worktrees alone. Report any unresolved preservation blocker.
   A main-worktree-only workflow requires no extra worktree.
6. The final response must include a table in global merge order: linked PR,
   repository when multi-repo, exact branch name, and short description. Link
   the runbook immediately below the table. State current head/check evidence,
   readiness versus authorized exceptions, blockers, and limits honestly.

## Merging the chain — only if explicitly directed

**Preflight the entire chain before the first merge:**

- Fetch the discovered remotes. Verify clean working trees for merge operations
  and local targets match their remote targets; preserve unrelated dirty work.
- Within each repo, verify the target tip is an ancestor of the first PR head,
  each head is an ancestor of its successor, and the appended ranges contain
  zero merge commits. Record exact heads and the global merge order.
- Verify every PR's implementation/review is complete, its current head matches
  expectations, and CI/CD is green at that head, except explicitly scoped
  waivers. Record exceptions separately; unresolved unwaived gates block merging.
- Complete prerequisites and confirm intermediate manual changes, automation,
  and any necessary safety gates are feasible before starting the sequence.

**For each PR, strictly in global order:**

1. Verify it is open, its current head still matches the reviewed and checked
   SHA, and the target has not diverged. Refresh current-head check evidence;
   changed heads require fresh review/checks and chain preflight.
2. Determine whether the host requires retargeting to the actual target branch
   for this merge. Verify behavior rather than assuming automatic retargeting
   or closure. If retargeting starts checks, wait for successful completion or
   an applicable explicit waiver before proceeding.
3. Execute the runbook's automated gates and any irreducibly intermediate
   MANUAL CHANGES at their specified points. An unavailable safety gate blocks
   the affected merge unless an equivalent mechanism verifiably preserves it.
4. On the verified local target, run `git merge --ff-only <head-sha>`, then a
   plain push to the discovered remote/target. Verify the remote target equals
   that SHA. On divergence or push rejection, stop and report; never force.
5. Find the target pipeline for exactly that SHA and watch it to completion.
   **Every merge must be green before the next by default.** An absent, skipped,
   or never-started pipeline is unverified and blocks continuation unless
   explicitly exempted. On failure capture the failing job's evidence and stop,
   unless explicit red-merge continuation covers this merge. A waived red car
   alone is insufficient. Verify conclusions directly after watcher/API errors.
6. Verify and report the host's PR state; do not assume reachability closes PRs
   on every host. If completion cannot be verified, report the blocker before
   advancing. Record merge/deploy outcomes and any applied waiver in the runbook.

After the caboose successfully merges, clean up merged branches as appropriate
(verify any automatic deletion), prune, and confirm each remote target's final
SHA and zero merge commits in the appended range. Preserve branches still needed
for unresolved work. Remove remaining task-created worktrees with the same
preservation rules. Deliver the mandatory table and runbook link, per-PR pipeline
results, final SHAs, and outstanding Post-merge Actions and Validation checks.

On an unwaived failure, stop at the exact affected car and report partial
progress and evidence. Do not improvise production recovery or revert without
user authorization. State what merged, deployed, failed, was waived, or remains
unverified; an authorized exception never turns a red or unverified result green.
