# PR output runbooks

This is the canonical runbook for the documentation change in `sernst/skills`.
The change is [PR #42](https://github.com/sernst/skills/pull/42). The user has not
authorized merging; no CI/CD exemptions have been granted. Link this runbook
from the PR and final delivery using a committed-SHA permalink that survives
branch and worktree cleanup.

## Prerequisites

- User: review the proposed skill behavior and explicitly instruct the agent to
  merge when ready. PR production alone does not authorize these merge steps.
- Agent: verify implementation/review is complete, local `just check` passed,
  and PR CI succeeded at the final pushed head. Resolve the exact reviewed SHA
  from final delivery evidence or the PR's head and matching review/check
  evidence at merge time; it need not be committed into this file.
  Git and authenticated GitHub CLI access
  with permission to fetch, push the target, and inspect checks must be available.
- Use a clean main worktree; preserve any dirty work first. Confirm `origin`
  points to `git@github.com:sernst/skills.git` and the PR targets `main`. This is
  one ordinary PR; no native GitHub stack feature or host configuration change
  is needed. If target protections prevent the fast-forward push, stop and report.

## Merging

| Order | PR | Repository | Exact branch | Description |
| --- | --- | --- | --- | --- |
| 1 (caboose) | [#42](https://github.com/sernst/skills/pull/42) | `sernst/skills` | `codex/pr-output-runbooks` | Clarify runbook ownership, CI exceptions, and fast-forward delivery rules. |

No version bump, runtime deployment, intermediate manual change, or service
disruption is expected. Repository documentation changes do not automatically
refresh an installed skill. Workflow inspection shows:

- `pr.yml` (Pull request quality) runs on PRs targeting `main`, with no path
  filter; its `PR required gate` requires the PR jobs to succeed.
- `build.yml` (Main and manual builds) runs on pushes to `main`, with no path
  filter, and performs quality checks and packaging.
- `security-and-live.yml` (Weekly advisory and live smoke) also runs on pushes
  to `main`, with no path filter. Both push workflows must succeed.
- `release.yml` runs on version tags; this change requires no tag or release.

After explicit merge authorization, run these PowerShell steps in the main
worktree. Stop on any nonzero command exit, failed assertion, changed head,
unverified gate, or divergence; do not continue to the next block. Never use a
GitHub squash/merge button, rebase, merge commit, or force push.

1. Set the reviewed SHA resolved from the delivery or PR evidence above, fetch,
   and verify the full one-car chain. Replace the command placeholder at
   execution time; do not infer approval from a fresh branch tip.

   ```powershell
   Set-Location C:/Users/swern/ghub/sernst/skills
   $reviewedSha = '<FINAL_REVIEWED_HEAD_SHA>'
   git remote get-url origin
   git status --porcelain
   git fetch origin
   git rev-parse origin/codex/pr-output-runbooks
   git rev-parse main
   git rev-parse origin/main
   git merge-base --is-ancestor origin/main $reviewedSha
   git rev-list --merges "origin/main..$reviewedSha"
   gh pr view codex/pr-output-runbooks --repo sernst/skills --json url,state,headRefOid,baseRefName
   gh pr checks codex/pr-output-runbooks --repo sernst/skills --watch
   gh pr checks codex/pr-output-runbooks --repo sernst/skills --json name,state,bucket,link
   ```

   Expect empty status and merge-commit output; local/remote `main` SHAs must
   match. The fetched branch and open PR must both equal `$reviewedSha`, the
   base must be `main`, and all checks must finish successfully, including
   `PR required gate`. Missing/skipped checks are unverified, not green. Record
   check links and the pre-merge target SHA; a waiver would require separate,
   explicit scope and must not be reported as green.

2. Recheck the PR head and target immediately before the fast-forward. If
   either changed, stop and repeat preparation and preflight as needed.

   ```powershell
   git fetch origin
   gh pr view codex/pr-output-runbooks --repo sernst/skills --json state,headRefOid,baseRefName
   git rev-parse main origin/main origin/codex/pr-output-runbooks
   git switch main
   git merge --ff-only $reviewedSha
   git push origin main:main
   git ls-remote origin refs/heads/main
   ```

   Expect the remote target to equal `$reviewedSha`. A rejected push is a stop;
   never force or rewrite to recover.

3. Find both push runs for that exact SHA; wait for each to finish. Requery
   while GitHub schedules runs. If a run never starts or evidence is unavailable,
   report the merge as unverified and blocked, not successful.

   ```powershell
   gh run list --repo sernst/skills --workflow build.yml --branch main --event push --commit $reviewedSha --json databaseId,headSha,status,conclusion,url
   gh run list --repo sernst/skills --workflow security-and-live.yml --branch main --event push --commit $reviewedSha --json databaseId,headSha,status,conclusion,url
   ```

   For each matching run, set `$runId` to its returned `databaseId` and run:

   ```powershell
   gh run watch $runId --repo sernst/skills --exit-status
   gh run view $runId --repo sernst/skills --json headSha,headBranch,event,status,conclusion,jobs,url
   ```

   Expect `headSha` equal to `$reviewedSha`, branch `main`, event `push`, status
   `completed`, conclusion `success`, and no failed jobs. Capture failure logs
   with `gh run view $runId --repo sernst/skills --log-failed` and stop on failure.
   A PR-check waiver alone would not authorize accepting a red merge.

4. Verify the PR reports merged using `gh pr view codex/pr-output-runbooks
   --repo sernst/skills --json state,url`, and confirm the remote target still
   equals the reviewed SHA. Record both pipeline URLs/conclusions and final SHA
   in the delivery report. If GitHub has not marked the PR merged, report the
   discrepancy. Preserve review branches until completion is verified; clean up
   only merged task branches and task-created worktrees safely. No extra worktree
   is required. Deliver the PR table and a committed-SHA GitHub permalink to this
   runbook so later branch/worktree cleanup cannot break the link.

## Post-merge Actions

None.

## Validation

- **Section ownership:** In a fresh session, explicitly load the merged
  repository's `skills/expecting-pr-outputs/SKILL.md` and ask for a hypothetical
  runbook with a secret prerequisite and a service roll the agent can automate.
  Expect the four sections in order, early human preparation, the automated roll
  under Merging, and human validation last, with `None` for empty sections.
- **Exception boundaries:** Ask the same session to simulate a failed PR check
  followed by a failed target pipeline. Grant only a PR-check waiver. Expect a
  stop at the red merge; then explicitly authorize that red merge's continuation
  and expect scoped exception reporting, never a green claim. Request simulation
  only, with no repository or production mutations.

Optional follow-up: refresh the installed skill through the user's existing
distribution workflow if desired. Installation is outside this documentation
change; direct loading of the merged repository copy supports these checks.
