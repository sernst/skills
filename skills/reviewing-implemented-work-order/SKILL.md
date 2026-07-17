---
name: reviewing-implemented-work-order
description:
  Perform code reviews following my engineering practices for work-order-driven
  development. Use when reviewing a work order (i.e. job) implementation.
---

# Pre Code Review

Follow these guidelines when reviewing the implementation of a work order job.
The goal is to identify potential implementation issues as well as any
implementation gaps from the work order files and to document each issue in a
markdown file alongside the job file for review and remediation.

## Review Workflow

## Step 1: Initial Setup

### 1.A Locate the Job File

BEFORE PROCEEDING make sure the targeted work order folder has been
unambiguously identified and confirmed by the user.

There are two types of work orders:

- **repo-specific** work orders are used when only one repo is impacted by an
  implementation and those work orders will be found in the
  `/path/to/ghub/github-org/repo/work-orders/` folder.
- **cross-repo** work orders, the job files will be found in the **work-orders**
  repo that sits alongside the other repos in the filesystem on a branch with
  the work order `<slug>` as the name.

The job file is named `job.<slug>.md` and lives inside the work order's folder:
`work-orders/<slug>/job.<slug>.md`. Each work order is a folder named `<slug>/`
containing files like `job.<slug>.md`, `research.<slug>.md`, and
`plan.<slug>.md`.

- If the current working directory is already named `work-orders`, look for the
  `<slug>/` folder here.
- Otherwise, look in `./work-orders/<slug>/` first before searching elsewhere.
- If the current folder name is `ghub|GitHub` then the sub folders are GitHub
  organizations and the work order will be found either in a shared
  `work orders` repo within one of those organization folders or within the
  `work orders` folder within one of the repos. There may also be a
  `work-orders` folder within the `ghub|GitHub` folder as well.
- Use filename searching to find the best matching work order file and confirm
  with the user the correct work order location has been found if there is any
  ambiguity, multiple matches, etc.

This ensures the skill works whether the user is at any of the following folder
levels:

1. Inside a `work-orders` directory (within repo or the shared work orders
   repo).
2. At the repository root where a `work-orders/` folder exists for that repo.
3. At the GitHub organization level where there's a shared `work-orders/` repo
   and a `work orders/` folder in many of the repos themselves.
4. At the GitHub root folder `ghub|GitHub` where the sub-folders are the GitHub
   organizations.

The directory structure looks something like this:

```
(ghub|GitHub)/
  work-orders/
  <GitHub ORG 1>
    foo/
      work-orders/
    work-orders/
  <GitHub ORG 2>
    bar/
      work-orders/
    work-orders/
```

If the user has not specified the `<slug>` for the work order or the work order
location explicitly, stop and ask the user to provide it before conducting this
identification process.

## Step 2: Understand Commit Changes

The implementation of a work order is one or more of the most recent commits
commonly on a branch with the work order `<slug>` as the name for each
repository where changes have been implemented. Assume all commits on the
current or specified branch should be considered unless the user specifies
otherwise.

Begin by first taking time to understand the intentions for the implementation
from the work order files, read the job file in full, and the code changes
included to adequately inform the subsequent review process. Spawn parallel
subagents to browse code changes and summarize to be efficient with context.
Identify key themes in the changes and analyze them across the changes.

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

Holistically review the code that has been changed in each of the implemented
repos. As each issue is identified, record which repo(s) it belongs to so the
Repo attribution in the output is accurate.

### Identifying Issues

Look for these issues in code changes:

- **Runtime errors**: Potential exceptions
- **Pattern drift**: New code should follow existing code patterns and
  organization
- **Performance**: Prefer vectorized operations over loops where possible
- **Side effects**: Unintended behavioral changes affecting other components
- **Security vulnerabilities**: Access control gaps, secrets exposure
- **Implementation gaps**: Areas where the implementation is incomplete compared
  to the work order file
- **Unused functionality**: Any added/modified code that isn't referenced
  anywhere or dead branches in the code that are no longer reachable as a result
  of the changes.

### Test Coverage

Every work order implementation should have appropriate test coverage:

- Scenario tests for business logic covering functional units
- Unit tests for complex public functions with parameterized outputs to properly
  exercise applicable cases.

Verify tests cover actual requirements and edge cases. Avoid excessive branching
or looping in test code.

## Step 4: Write Review Output

Create a `review.<slug>.md` file saved alongside the work order file that
contain an entry for **every** issue found — there is no limit on the number of
entries. Do not artificially cap or truncate the list. Assess the
severity/priority of each issue for reporting and review purposes.

### Issue Structure

- Each entry in the `review.<slug>.md` file should provide a clear and concise
  explanation of the issue
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
````

{% endif %}

{% if example_failing_test %} **Example Test**

```
{{ example_failing_test }}
```

{% endif %}

```

Once this file has been written summarize the results for the user including the
issue count broken down by priority for the caller to orient themselves before
looking at the review output.
```
