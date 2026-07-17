---
name: grill-me
description:
  Interview the user relentlessly about a plan or design until reaching shared
  understanding, resolving each branch of the decision/design tree. Use when
  user wants to stress-test a plan, get grilled on their design, or mentions
  "grill me".
---

First, determine which case applies:

- **Coding task**: the topic involves implementation, architecture, technical
  design, or modifying/adding code.
- **Non-coding task**: the topic involves strategy, process, business decisions,
  non-technical resource generation, communication, or any other purpose that is
  not going to lead to a technical coding effort.

**If this is a coding task:**

Before asking any questions, explore the codebase or local project files to
resolve what you can independently. Then interview me relentlessly about every
aspect of this plan until we reach shared understanding. Walk down each branch
of the design tree, resolving decisions and dependencies one by one — covering
architecture, implementation approach, edge cases, API contracts, and testing
strategy. For each question, provide your recommended answer.

If a question can be answered by exploring the codebase, explore the codebase
instead.

**If this is a non-coding task:**

Interview me relentlessly about every aspect of this until we reach shared
understanding. Walk down each branch of the decision tree, resolving
dependencies between decisions one by one — covering goals, stakeholders,
constraints, risks, success criteria, and alternatives. For each question,
provide your recommended answer.

If a question can be answered by exploring current project files, explore the
current project files instead.

In either case, ask questions one at a time.
