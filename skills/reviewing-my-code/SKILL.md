---
name: reviewing-my-code
description:
  Perform code reviews following my engineering practices. Use when reviewing
  changes in a branch.
---

# Pre Code Review

Follow these guidelines for reviewing code. Act as a pre-review assistant. The
goal is to identify issues and draft comments for the human reviewer to
evaluate. Do not finalize reviews; provide the "raw material" for the human
review and help the human focus on the areas where their time would be best
spent within the review process.

## Review Workflow

## Step 1: Understand Commit Changes

The scope of the review will most commonly be the most recent commit on the
current branch unless explicitly specified otherwise.

Begin by first taking time to understand the intentions for the implementation
from the the code changes included to adequately inform the subsequent review
process. Spawn parallel subagents to browse code changes and summarize to be
efficient with context. Identify key themes in the changes and analyze them
across the changes.

### Step 2.A: Report on Key Themes

Summarize the key themes for the implementation changes in a prioritized format
like this:

```markdown
# Identified Themes

1. {{ theme_1_name }}: {{ theme_1_description }}
2. {{ theme_2_name }}: {{ theme_2_description }} ...
```

Limit to a maximum of 5 themes. For smaller implementations, a single theme is
fine.

## Step 3: Conduct Review

Holistically review the code that has been changed.

### Identifying Issues

Look for these issues in code changes:

- **Runtime errors**: Potential exceptions
- **Pattern drift**: New code should follow existing code patterns and
  organization
- **Performance**: Prefer vectorized operations over loops where possible
- **Side effects**: Unintended behavioral changes affecting other components
- **Security vulnerabilities**: Access control gaps, secrets exposure
- **Implementation gaps**: Areas where the implementation is deemed incomplete,
  especially where an agent may have given up during the implementation process
  and ended early.
- **Unused functionality**: Any added/modified code that isn't referenced
  anywhere or dead branches in the code that are no longer reachable as a result
  of the changes.

### Test Coverage

Every implementation should have appropriate test coverage:

- Scenario tests for business logic covering functional units
- Unit tests for complex public functions with parameterized outputs to properly
  exercise applicable cases.

Verify tests cover actual requirements and edge cases. Avoid excessive branching
or looping in test code.

## Step 4: Review Output

Create a `review.<short-commit-sha>.local.md` file at the top of the repo that
contains an entry for **every** issue found — there is no limit on the number of
entries. Do not artificially cap or truncate the list. Assess the
severity/priority of each issue for reporting and review purposes.

### Issue Structure

- Each entry in the review file should provide a clear and concise explanation
  of the issue
- Where applicable, propose explicit code solutions
- Where applicable, create and include an example failing test case that
  illustrates the issue/bug/concern and provide a clear explanation for the test
  in its docstrings.
- Assign a **High**, **Medium**, or **Low** priority to each issue.
- Assign a **Repo** to each issue identifying which repo(s) the issue belongs
  to. Use the short repo name (e.g., `<github-org>/<repo-name>`). For cross-repo
  issues, list all impacted repos.
- All file/location references in issue explanations must use fully-qualified
  paths in the form `` `<repo-name>/path/to/file.ext:L<line>` `` to avoid
  ambiguity when the same filename appears in multiple repos.

Always use this exact structure for review comments where `<N>` if the ordered
number of the issue, `<Priority>` is the priority assigned to the issue and
`<Bried Issue Header>` gives the issue a unique title for easier reference like
this:

````markdown
# Issue <N>: (<Priority (High|Medium|Low)>) <Brief Issue Header>

**Repo(s):** `<repo-name>`

<!-- or `<repo-a>`, `<repo-b>` for cross-repo issues -->

{{ explanation of issue — all file references as `<repo>/path/file.ext:L<line>` }}

{% if explicit_code_suggestion %} **Suggestion**

```suggestion
{{ explicit_code_suggestion }}
```

{% endif %}

{% if example_failing_test %} **Example Test**

```
{{ example_failing_test }}
```

{% endif %}
````

Once this file has been written summarize the results for the user including the
issue count broken down by priority for the caller to orient themselves before
looking at the review output.
