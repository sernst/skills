# Reporting skill-manager results

Present the CLI's semantics in conversational Markdown. Do not reproduce ANSI,
emoji, raw NDJSON, fixed terminal spacing, or empty/zero-count clauses unless
the user asks for raw detail.

## Read-only results

- Lead with the outcome or answer, then the evidence needed to act on it.
- Use labeled fields for one item and a compact table for two or more
  comparable items or significant destinations.
- Preserve CLI terminology, configured ordering, provenance when it
  disambiguates a result, warnings, and actionable next steps.
- Omit paths, columns, unchanged items, and summary clauses that add no value.

## Plans and confirmation

Before any operation requiring confirmation, present one complete plan. Include
the selected source and skills, targets and scope, each significant change,
warnings, and whether the command is a dry run or will commit. Never split one
decision across partial previews.

Use a compact table when skills or destinations need comparison. Otherwise use
one sentence plus labeled metadata. Ask the CLI-equivalent confirmation only
after the full plan; preserve its safe default and never treat a recommendation
as consent.

## Results

- State **Previewed** or **Completed** first and distinguish dry-run from
  committed work explicitly.
- Name each changed item and its action. Include unchanged items only when they
  materially explain selection or blast radius.
- End with a significance-gated summary containing only nonzero outcomes.
- Surface warnings next to the affected item or decision.
- If the process fails after actions, report committed actions first, then the
  exact failure and what remains incomplete. Never imply rollback.

The NDJSON stream is the evidence source, not the user-facing format. Preserve
event order while interpreting it; expose raw recipes or events only on request
and redact secrets and sensitive local paths.
