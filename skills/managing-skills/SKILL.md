---
name: managing-skills
description: Manage reusable agent skills with the installed skill-manager CLI. Use when a user asks an agent to find, list, inspect, install, load, update, import, copy, remove, or resolve skills; manage skill sources or deployment targets; view, reset, or restore skill-manager configuration; check global or project deployment status; or generate CLI completions or a man page.
---

# Manage skills

Use `skill-manager` as the sole skill-management mutation boundary. Translate
the user's intent into non-interactive commands, parse the complete result, and
report what actually happened.

## Bootstrap

1. Run `skill-manager --version`.
2. If it is absent, read [references/install.skill-manager.md](references/install.skill-manager.md)
   in full. Detect the operating system, run its documented non-interactive
   installer with an explicit user-writable directory and PATH modification
   suppressed, and record the installed binary's absolute path (for example,
   `C:\\...\\skill-manager.exe` or `/.../skill-manager`). Invoke that exact path
   for every remaining `skill-manager` call in this operation; agent shell calls
   can be separate processes, so do not rely on a one-off PATH change. When the
   environment supports a persistent process PATH update, it may also prepend
   the binary's directory, but never modify persistent shell PATH. Verify the
   recorded executable with `--version`, then establish the needed source and
   target context and continue with this workflow.
3. If installation or verification fails, stop and report the exact failure.

## Start every operation

1. Read [references/recipes.md](references/recipes.md) before constructing a
   recipe. Read [references/events.md](references/events.md) before interpreting
   output. Read [references/workflows.md](references/workflows.md) for
   multi-command and conversational patterns.
2. Start inspection with direct `skill-manager describe` calls: use exact or
   qualified selectors for a known skill, `--source NAME` to scope a source,
   and `--installed`/`--outdated`/`--not-installed` for deployment state.
   Use `describe --skills` or `describe --sources` for type-wide discovery.
   Inspect qualified excluded/shadowed copies only when the user identifies the
   source. Use `source.list`, `target.list`, or `status` only when their
   configuration/deployment matrix is specifically needed. Do not infer hidden
   state from files.
3. Resolve selectors to exact sources, skills, targets, and scopes. Ask one
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
given. `copy` renders and auto-authorizes the same way. `remove` renders its
whole plan identically but never auto-authorizes in any non-interactive mode:
`yes:true` is always required to commit, because removal is destructive and
irreversible, and an ambiguous scope (a skill deployed in both global and
project) must be resolved with `global:true`, `project:true`, or the
remove-only `both:true` — `yes:true` alone never picks one. A committed `import`
requires `yes:true`, must name exactly one skill, and must narrow target and
scope selection whenever more than one deployment still differs from the
source after that; set `update:true` (import + update, recommended) or
`no_update:true`/`update:false` to resolve the propagation dimension when it
is genuinely ambiguous — neither is implied by `yes:true`. Propagation
resolves silently with no flag needed whenever the resolved source copy
would leave nothing else out of date (nothing to synchronize either way), so
a single-deployment import commits with only `yes:true`. A GitHub-backed source
needs a configured local alternate location — without one, import fails
outright, interactively or not; with one, it imports into that alternate like
any other destination. `all_targets:true` selects
enabled configured targets only; select a disabled target explicitly by name
when the user intends that override. For direct argv, machine use should pass
`SOURCE --name=NAME` to `source.add` and `PATH --name=NAME` to `target.add`;
recipes use their explicit named fields. Supply an explicit scope whenever
required. Never answer prompts, depend on TTY behavior, or use `--color` for
machine work.

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
- To remove from both scopes in one recipe, set `both:true` (mutually
  exclusive with `global`/`project`); dry-run first, then set `yes:true` to
  commit, and verify with `status` afterward.

## Use argv only for narrow exceptions

Recipes cover all ordinary operations. Use direct argv only for:

- `skill-manager describe [SELECTOR...] [--json]` (read-only inspection; see
  [references/workflows.md](references/workflows.md#discover-and-inspect));
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
