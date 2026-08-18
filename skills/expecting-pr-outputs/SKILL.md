---
name: expecting-pr-outputs
description:
  Conventions and expectations around PR session outputs. Use when the user asks
  for a "PR chain output", says the session's deliverable should be one or more
  PRs, or asks to merge a previously produced PR chain. Covers chunking work
  into PRs, stacked branches under enforced linear history, CI/CD readiness
  gates, deployment runbooks, and — only when explicitly directed — conducting
  fast-forward merges with pipeline monitoring between each merge.
---

The deliverable of this session is one or more pull requests, plus a runbook
that lets the user take them to production. Everything below defines what "done"
means for that deliverable and how to produce it. Merging is a separate,
explicitly-gated activity covered at the end — producing the chain never implies
permission to merge it.

## The output contract

**One or more PRs, each a large, meaningful chunk of work.** Group by theme and
deployment surface — a bug-fix PR, a CLI-feature PR, a behavior/doctrine PR, an
infra-only PR — not by file count or convenience. A PR should be independently
understandable, reviewable as one coherent change, and revertable as one unit.
Do not shred one concern across several small PRs, and do not staple unrelated
risk together just to save a branch.

**A PR is not ready until CI/CD passes on the pushed branch.** Local gates (the
repo's full check/lint/test task) are necessary but never sufficient — CI runs
checks the local environment cannot (image builds, platform differences, missing
system binaries, structure checks). Push, watch the checks to completion, fix
failures, and only then present the PR as ready. Expect the gap: a locally-green
branch failing CI usually means an environment assumption (a binary the runner
lacks, a version-bump rule, a formatting check) — fix the root cause, never
weaken the gate.

**Every repo convention binds every PR.** Version bumps, changelog entries in
the same commit as the work, wire-compatibility test rules, commit-message style
— discover the target repo's conventions before the first commit and apply them
per PR. Infra-only PRs often have their own convention (for example, a changelog
note without a version bump, enabled by an explicit skip marker) — find the
sanctioned mechanism rather than inventing one.

## Linear history rules

All repos this skill applies to enforce linear git history: **no merge commits,
no squashing, no history rewrites of shared branches.** PRs are merged by
fast-forwarding `main` onto branch head commits. This shapes how the chain must
be built:

- **Stack dependent branches.** Each PR's branch is based on its predecessor's
  branch; the first is based on `main`. Open each PR with its base set to the
  predecessor branch (the platform retargets to `main` as bases merge). Merge
  order is therefore fixed and must be documented.
- **Restack when predecessors move.** When an earlier branch gains commits
  (review fixes), rebase every later branch onto the new tip, in order. Expected
  conflicts are mechanical and have standard resolutions: version files keep the
  branch's own version; changelogs keep every section in reverse-chronological
  order; lockfiles take the branch's side and are regenerated with the package
  manager, verifying only version metadata changed. Re-run the full local gate
  at every restacked tip.
- **Independent work still stacks cleanly.** Even when PRs don't logically
  depend on each other, a stacked chain keeps the eventual fast-forward merges
  trivial. Truly independent single PRs may base on `main` directly.
- **Never force-push shared branches.** The only exception is
  `--force-with-lease` on a branch you just rebased as part of deliberate
  restacking, before or between reviews — never on `main`, never to recover from
  a mistake without the user's direction.

## The runbook

Produce a markdown runbook alongside the chain — it is part of the deliverable,
not an afterthought. It must let the user execute the deployment without this
session's context. Include:

- **A merge-order table**: position, PR link, version, one-line contents.
- **Per-phase steps** in execution order: what to merge, what CI will do
  automatically on merge (deploys, applies), and every **manual action** with
  exact commands — populating secrets, service restarts/redeploys, configuration
  steps.
- **Pre-merge gates**, called out loudly: any verification that must happen
  BEFORE a merge whose CI auto-applies changes (for example, verifying a
  container image actually contains a required binary before terraform creates a
  service from it). State plainly: "if this fails, do not merge."
- **Deploy-order constraints and why**: which component must be current before
  which (for example, consumers of a widened wire contract before its producer),
  what breaks during the skew window, whether it self-heals, and how wide the
  exposure is.
- **Expected disruption**: anything that restarts, goes briefly down, or behaves
  oddly mid-roll, with rough durations.
- **A verification checklist per phase**: concrete actions with expected
  results, biased toward re-testing the exact failures that motivated the work.
- **A non-blocking punchlist**: known deferrals and follow-ups, so they are
  recorded rather than lost.

Keep the runbook current as the chain evolves — when review cycles change
behavior or new PRs join the chain, update it and re-deliver. If earlier phases
have already been executed, mark their status rather than deleting them.

## Building the chain, end to end

1. Plan the chunks and their order (dependencies first, then risk: production
   fixes before features before infra, unless the user directs otherwise).
2. Implement each PR on its stacked branch; run the repo's full local gate to
   green at every tip.
3. Get each PR reviewed to the session's quality bar before it is presented (if
   operating under an orchestration skill with judges, judge-approve each PR;
   findings fixed and re-verified).
4. Restack, push every branch, open PRs in order with correct bases and bodies
   that include: a bullet summary, notable review findings fixed, and the
   PR-specific deploy notes that also appear in the runbook.
5. Watch CI on every PR to completion. Fix and re-push until all are green.
6. Deliver: PR links in merge order + the runbook + honest notes on anything
   unverified (things only production or a real deploy can prove).

## Merging the chain — only if explicitly directed

Never merge, and never treat chain-production as implicit permission to merge.
When — and only when — the user explicitly directs you to merge, the procedure
is a supervised fast-forward walk where **CI gates every step**:

**Pre-flight (once):**

- Fetch. Verify the working tree is clean and `main` matches origin.
- Verify strict linearity: origin/main's tip is an ancestor of the first branch
  tip; each branch tip is an ancestor of the next; zero merge commits anywhere
  in the range. Any mismatch: stop and report — never "fix" with a rebase
  mid-merge.
- Verify every PR in the chain is currently green and its head matches the
  expected commit.

**Per PR, strictly in order:**

1. Confirm the PR is OPEN and its head sha matches expectations.
2. If its base is not `main`, retarget it to `main` now (the platform marks a PR
   merged only when its head becomes reachable from its base ref — a stacked PR
   left based on a sibling branch will not close). If retargeting re-triggers
   checks, wait for green again.
3. Execute any pre-merge gates from the runbook for this PR now — before the
   merge whose CI will auto-apply.
4. On local `main`: `git merge --ff-only <head-sha>`, then a plain push. If the
   push is rejected for any reason, stop and report verbatim — never retry with
   force.
5. Find the main-branch pipeline run for exactly that sha and watch it to
   completion. Any failure: capture the failing job's logs, STOP the chain — no
   further merges — and report. Do not revert, do not improvise recovery on
   production-affecting pipelines without the user.
6. Confirm the PR now reports merged. (Trust but verify tooling: if a watcher
   exits with an API blip, confirm the run's conclusion directly before
   proceeding.)

**After the final PR:** delete the merged branches (many repos auto-delete on
merge — verify rather than assume), prune, confirm the full range contains zero
merge commits and origin/main sits at the final head, and report: per-PR
pipeline results, final sha, and which runbook manual actions remain for the
user (post-merge service rolls stay theirs unless explicitly delegated).

## Failure discipline

- The first red pipeline stops the chain. Partial progress is fine — report
  exactly where it stopped, why, with verbatim evidence.
- A blocked or unavailable tool for a runbook gate is a stop, not a substitution
  — unless an equivalent mechanism verifiably preserves the gate's substance, in
  which case state the substitution and its residual risk explicitly when
  reporting.
- Report outcomes faithfully: what merged, what deployed, what was skipped, what
  remains. The user should never discover state you didn't mention.
