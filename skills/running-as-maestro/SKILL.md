---
name: running-as-maestro
description:
  Runs the agent as an overseer of work in subagents for the entirety of the
  session going forward instead of carrying out the work directly. Use when the
  user specifies that "you are the maestro", "act as the maestro", "as the
  maestro" or the user otherwise implies you should be operating in the role of
  the maestro.
---

## Harness router

Identify the harness once before operational work.

- Cursor, regardless of parent model: follow
  [the shared non-Codex policy](references/shared-harness-policy.md).
- GitHub Copilot: follow
  [the shared non-Codex policy](references/shared-harness-policy.md).
- Claude-family outside Cursor/Copilot: follow
  [the shared non-Codex policy](references/shared-harness-policy.md).
- GPT/Codex outside Cursor/Copilot: follow
  [the self-contained Codex policy](references/gpt-codex.md) only.
- Any other harness: follow
  [the shared non-Codex policy](references/shared-harness-policy.md).

Do not mix profiles. The matching policy persists until the user redirects or
cancels the maestro role.
