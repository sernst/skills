# AI Agent Skills

My personal collection of AI Agent Skills. Mix of ones I've developed myself and
others I've adapted from others.

## Command-line tools

Native executable tools live in [`clis`](./clis). The first is
[`skill-manager`](./clis/skill-manager), a Rust CLI for discovering, resolving,
and deploying skills. See [CONTRIBUTING.md](./CONTRIBUTING.md) for the Just-based
quality workflow and [RELEASES.md](./RELEASES.md) for tagged release procedures.

# Manifest

## [drafting-commit-message](./skills/drafting-commit-message/SKILL.md)

Helps draft meaningful commit messages from the current changes, whether staged
or all local unstaged edits if nothing is staged. It enforces a short, imperative
Title Case title (under 50 characters) plus a bulleted description in the form
`- **<Change Category>** - <overview>`, consolidating related edits into the
fewest bullets possible rather than listing a blow-by-blow diff. Each bullet must
explain the motivation and impact of a change — the before-state and outcome —
never the mechanism of which files or lines were touched. Use it when drafting a
commit message or when asked to "draft/create/make a commit message."

## [grill-me](./skills/grill-me/SKILL.md)

An adapted version of the popular skill from
[Matt Pocock](https://github.com/mattpocock) designed to work on coding and
non-coding tasks equally. It first determines whether the topic at hand is a
coding task (architecture, implementation, code changes) or a non-coding task
(strategy, process, communication), then explores the codebase or project files
to resolve what it can on its own before asking anything. It then interviews me
relentlessly, one question at a time, walking down each branch of the decision
tree — covering things like architecture and edge cases for coding tasks, or
goals and constraints for non-coding ones — and offers a recommended answer for
every question until we reach shared understanding.

## [reviewing-implemented-work-order](./skills/reviewing-implemented-work-order/SKILL.md)

Performs a structured code review of a work order (job) implementation,
following my engineering practices for work-order-driven development. It locates
the relevant job file across repo-specific or cross-repo work-order layouts,
then reviews the commits implementing it against the job, research, and plan
files to surface issues like runtime errors, pattern drift, security gaps, and
missing test coverage. Every issue found is written to a `review.<slug>.md` file
alongside the work order, complete with priority, repo attribution, and
fully-qualified file references, with no cap on how many issues get recorded.
Use it when reviewing the implementation of a work order/job rather than an ad
hoc branch.

## [reviewing-my-code](./skills/reviewing-my-code/SKILL.md)

Acts as a pre-review assistant that reviews code changes on the current branch —
typically the most recent commit — following my engineering practices. It spawns
parallel subagents to summarize the changes, identifies up to five key themes,
and then reviews the code for runtime errors, pattern drift, performance issues,
security vulnerabilities, implementation gaps, and missing or inadequate test
coverage. Rather than finalizing a review, it drafts the raw material — flagged
issues and prioritized themes — so I can focus my own review time on the areas
that matter most. Use it when reviewing changes in a branch outside of the
formal work-order process.

## [running-as-maestro](./skills/running-as-maestro/SKILL.md)

Switches me into an orchestrator role for the rest of the session, delegating
work to subagents instead of doing it directly, and remains in effect across
every subsequent turn until explicitly cancelled. As maestro it judges which
lighter-weight model tier fits each sub-task, favors `auto` routing in Copilot
(or the Sol/Terra/Luna tiers outside it), and dispatches independent subagent
work in parallel rather than sequentially. It stays accountable for verifying
subagent output — even when the verification itself is delegated — and
communicates the quality-versus-cost tradeoffs it's making along the way. Use it
when told to act as "the maestro" or to oversee work through subagents rather
than implement it myself.
