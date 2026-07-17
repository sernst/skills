---
name: drafting-commit-message
description:
  Helps in drafting more meaningful commit messages based on changes. Use when
  creating commit messages or users ask to "(draft|create|make) a commit
  message."
---

# Commit Guidelines

The following outlines how to draft a good commit message.

## Commit Content Sources

- Use any relevant context related to current changes.
- Analyze git-trackable, modified files in the repository.
- If any changes are staged, only consider staged changes. Otherwise, consider
  all local changes that are not ignored by git.

## Commit Structure

- Keep commit messages short. They should be under 36 characters in general and
  never exceed 50 characters.
- Use the imperative, present tense (e.g., 'change' not 'changed' nor
  'changes').
- Commit messages should be the "title" of the commit and use Title Case.
- Always add descriptions and not just messages.
- Descriptions should be bulleted lists explaining key conceptual changes and
  their brief summary/overview/motivation and not detailed lists of a
  blow-by-blow change.
- Descriptions should be formatted as:
  `- **<Change Category>** - <change overview...>`.
- Description items should serve as a changelog for all contributors and focus
  on the impacts.
- Do not include testing as separate description lines if those are just
  supporting other description lines. Only call them out if they are a large,
  independent effort in the commit.
- Consolidate related changes into the fewest description items possible.
  Changes that serve a common goal, or are purely supporting another change,
  belong in the same bullet — not separate ones.
- A description bullet is only warranted when the change has an independent,
  self-contained motivation that stands apart from every other change in the
  commit.
- Small changes should have small descriptions or omitted entirely if trivial or
  routine, e.g. a version change.
- Wrap description lines at 72 characters to follow git-commit best practices.
- CRITICAL: Each description item must explain the _motivation_ (why this was
  needed) and the _impact_ (what is different for users or the system) — not the
  mechanism. Describe the before-state and the desired outcome; never describe
  which files, functions, or lines were changed (the diff does that).

Bad Output (Describing the code):

- **Token Logic Migration** - Change the token logic to use jwt instead of the
  old session strings. Remove lines 40-55 and added a new import at the top.
  Also fixed a bug where users got logged out randomly.

Good Output (Describing the "why"):

- **Token Logic Migration** - Migrate the authentication system from stateful
  session strings to stateless JSON Web Tokens. This resolves the ongoing issue
  where users experienced random logouts during load balancer scaling events,
  and reduces server-side memory footprint.

Bad Output (Too many trivial bullets for one cohesive change):

- **Config Refactor** - Move timeout value to a constant.
- **Config Refactor** - Remove duplicate import in config module.
- **Config Refactor** - Rename variable for clarity.
- **Tests** - Update tests to reflect renamed variable.

Good Output (Consolidated into one bullet):

- **Config Refactor** - Centralize timeout configuration to eliminate scattered
  magic numbers and make tuning easier across environments.
