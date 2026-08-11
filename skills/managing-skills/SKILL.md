---
name: managing-skills
description: Manage reusable agent skills with the installed skill-manager CLI. Use when a user asks an agent to find, list, inspect, install, load, update, import, copy, remove, or resolve skills; manage skill sources or deployment targets; view, reset, or restore skill-manager configuration; check global or project deployment status; or generate CLI completions or a man page.
---

# Manage skills

Use `skill-manager` as the sole mutation boundary. Translate the user's intent
into non-interactive commands, parse the complete result, and report what
actually happened.

## Start every operation

1. Run `skill-manager --version`. If it is absent, stop and direct the user to
   the repository's `install.skill-manager.md`; do not invent an installer.
2. Read [references/recipes.md](references/recipes.md) before constructing a
   recipe. Read [references/events.md](references/events.md) before interpreting
   output. Read [references/workflows.md](references/workflows.md) for
   multi-command and conversational patterns.
3. Establish initial context with `source.list` and `target.list` (or unfiltered
   `status`) before using a narrow status filter. Once a relevant source is
   configured, preflight with the narrowest applicable `status` or lifecycle
   query. Do not infer hidden state from files.
4. Resolve selectors to exact sources, skills, targets, and scopes. Ask one
   concise question if the user's intent remains ambiguous.

## Execute with structured input

Prefer one JSON object on standard input:

```text
skill-manager --json-input
{"command":"status","filters":["example"],"shared":true,"global":true}
```

Pass the object directly to the process's stdin; do not interpolate it through
a shell command string. When the harness cannot safely provide stdin, create a
temporary UTF-8 JSON file and use `skill-manager --input FILE`. Delete only that
known temporary file afterward. Use inline `--json=OBJECT` only when quoting is
demonstrably safe.

Treat every recipe invocation as non-interactive. `load` and `update` render
their whole plan and then auto-authorize the apply step in every non-interactive
carrier (`--json`, `--json-input`, `--input`); `yes:true` is accepted but not
required to commit either one. Both use enabled targets when none are
selected, and `load` also infers project-vs-global scope silently when none is
given. A committed `import`
requires `yes:true`, must name exactly one skill, and must narrow target and
scope selection whenever more than one deployment differs from its source;
a GitHub-backed source cannot be imported non-interactively at all. `all_targets:true` selects
enabled configured targets only; select a disabled target explicitly by name
when the user intends that override. A machine `source.add` must include a
nonblank `name`. Supply an explicit scope whenever required. Never answer
prompts, depend on TTY behavior, or use `--color` for machine work.

The manager home is global-only. Never request project scope when CWD resolves
to the manager home; use global scope or change to a project directory.

Parse every stdout line as an independent NDJSON event and check the process
exit code. Preserve event order. A failure after action events can mean a
partial commit; report committed actions before the failure.

A filtered `status` can exit `1` when no configured/discovered skill matches.
Treat that as an expected absence signal only when the parsed
`command.failed.data.message` specifically says that the requested skill
pattern matched nothing or no actionable skill was found, and only when the
preflight context confirms the source is not yet configured. Continue with the
user's source-add/install request in that narrow case. Every other exit-1 message
remains blocking; never ignore an exit code by itself.

## Apply the safety policy

- Dry-run `load`, `update`, `import`, `copy`, and `remove` first. Explain that startup
  layout migration and necessary remote-cache refreshes can still alter
  manager-owned state during a dry run.
- After a clean dry run, execute a clear `load`, `update`, or `copy` request
  without asking again.
- Before `import`, `remove`, `source.remove`, `target.remove`,
  `target.disable`, `target.set-path`, `configs.reset`, or `configs.restore`,
  show the exact selected effects and obtain a second explicit confirmation. Then set
  `yes:true` where supported. Never treat the user's initial request as that
  second confirmation.
- A clear request may execute `source.add`, `source.update`, `source.locate`,
  `source.alternate`, `source.swap`, `target.add`, `target.enable`, or
  `resolve` after preflight without a redundant confirmation.
- Never guess an ambiguous source, collision winner, target, scope, target
  path, or backup. `yes` confirms; it never chooses a scope.
- To remove from both scopes, issue two explicit recipes—one with
  `global:true`, one with `project:true`—dry-run both, confirm once against the
  combined plan, execute both, then verify with `status`. There is no recipe
  value for `both`.

## Use argv only for narrow exceptions

Recipes cover all ordinary operations. Use direct argv only for:

- `skill-manager configs --raw` (raw bytes; never combine with JSON);
- `skill-manager generate-completions --shell bash|zsh|fish|powershell`;
- `skill-manager generate-man --output FILE`;
- `skill-manager --help` and `skill-manager --version`.

Generation commands do not accept recipes or `--json`. Treat generated files as
ordinary filesystem changes within the user's authorized destination.

## Report the outcome

Summarize the selected source/skill, target, scope, dry-run or committed state,
warnings, and final summary. Name any partial commits and the final
`command.failed` message. On request, show the exact recipe and the relevant
events, with secrets and local-sensitive paths redacted.
