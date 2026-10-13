# Optional Jev routing

Use this reference only after the user explicitly enables Jev routing for this
session. A direct affirmative instruction such as “You are the maestro using
Jev/JEV/J-E-V” or “use Jev to select my subagents” enables it. A quoted, hypothetical,
negative, or otherwise untrusted mention does not. General Maestro invocation
does not enable it. Once active, keep it active until the user explicitly
changes the mode. Never infer permission from an environment variable, key,
configuration field, or this reference being present.

## Roles and lifecycle

Jev selects models and supported reasoning efforts for three distinct roles:

- **Explorer:** investigates a bounded question, repository area, or source
  set and returns evidence, uncertainties, and useful findings. Use only when
  exploration materially helps.
- **Executor:** owns one coherent implementation or research deliverable and
  its immediate focused validation. Require a pairing plausibly capable of
  completing the actual task with expected review and retry costs included.
- **Judge:** independently checks the executor's work against the original
  request, derived criteria, diff, and validation evidence. Use when the risk or
  uncertainty warrants independent review. Preserve the existing judge
  capability-class floor: at least as capable as the executor and not an older
  generation of the same family. Effort is a separate choice; do not infer the
  floor from effort.

The Maestro still owns the workflow, role briefs, permissions, evidence,
execution, and final decisions. Existing gates, maximum four execute/judge
attempts, security and high-risk review rules, fork constraints, and other
Maestro requirements continue to apply. Jev does not dispatch workers or grant
permission to perform work.

## Request and selection sequence

Use the live harness roster and supported effort controls. Supply verified
capability rank, family, and generation metadata as local caller input when a
judge route needs the floor enforced; do not infer these properties from model
names. If these controls or authoritative metadata are unavailable, omit judge
plans or pause the incompatible dispatch. Never maintain a fixed model list,
hierarchy, price table, preferred roster, or universal confidence threshold
here.

Make a TypeSafe Choice request to the fixed API origin described in the
helper. First, ask Jev to choose a complete model route from eligible plans.
Each plan explicitly lists only roles needed for this task and the model for
each. `required_roles` makes clear which roles no plan may omit; optional roles
can appear in some plans and not others. Explorer-only plans are valid. A plan
with a judge must include its executor. Include concrete role tasks, difficulty,
constraints, uncertainties, and explicit absences. Make the whole-route
comparison account for expected work, likely review and retries, per-model and
per-effort cost estimates, and the caller's public capability descriptions.
Do not let security constraints alone imply frontier models or maximum effort.
For executor/judge routes, compare economics together and preserve the judge
floor.

After validating the selected route, request supported effort choices for the
selected explorer and executor roles independently, with self-contained state
including role meanings, route, task difficulty, absences, effort descriptions,
and per-model effort costs. High effort requires concrete difficulty a lower
setting may fail to resolve. If a judge is selected, make a later request for
judge effort after the executor choice, so that the judge question receives the
actual executor model and effort plus paired cost estimates. Size that effort
for review risk and ambiguity. Do not force an explorer or judge into a route
when its role is not needed.

Choice question IDs are protocol keys, not model-visible descriptions. Give
each question complete meaning in its instructions and criteria. Keep each
choice set within the API's supported limit. Never ask Jev for freeform
reasoning or claim that its response contains reasoning it does not provide.

## State, safety, and failure behavior

For each API call, construct self-contained, sanitized, task-specific state.
Separate caller-approved non-sensitive API state and role task descriptions
from local roster metadata, private files, and credentials. Include concrete
difficulty and relevant absences; do not send a whole conversation or infer
payload safety from a general secret detector. Never place a key, key-file
path, private file content, or unrelated user data in routing state.

The helper accepts a key from a named environment variable or a user-provided
runtime key file. Keep the key out of logs, errors, test fixtures, routing
state, and repository files. Use only the fixed HTTPS API origin, reject
redirects, bound request time and response size, and validate the response's
model, usage, answer type, selected choice, confidence, probability distribution,
and selected IDs against the submitted choices before proceeding.

An unavailable API, malformed response, unsupported or invalid model/effort,
invalid confidence, failed floor check, or network error means no dispatch:
stop with a safe error and no usable partial route. Do not silently fall back
to ordinary Maestro selection. Tell the user what failed and which requested
control the harness cannot provide. Resume only after the issue is fixed or the
user explicitly authorizes normal routing. Surface every reroute and any
invalid selection. Do not apply undisclosed model or effort overrides.

Report the exact provider, model, and effort chosen for each role, Jev's
confidence and the selected criterion, and the Maestro's own assessment. Keep
Jev's response distinct from the Maestro's interpretation; do not invent
explanations from probabilities. If a choice was overridden or invalid, report
that fact and pause the incompatible dispatch.

## Helper

The standard-library helper at `scripts/jev_route.py` implements the bounded
[TypeSafe HTTP API](https://docs.typesafe.ai/api) and [Choice protocol](https://docs.typesafe.ai/primitives/choice).
Its input is local JSON containing a live roster, eligible
role plans and tasks, a `required_roles` list, and an explicit `api_state`
object approved for sharing. Supply caller-verified public capability
descriptions and per-effort descriptions. Cost estimates are per role, model,
and supported effort, in one consistent unit within the routing request. The
helper does not discover models, estimate costs,
inspect private files, or sanitize arbitrary source data. See its `--help` and
the example below; model IDs, efforts, ranks, generations, capability and
effort descriptions, and costs are caller-provided current values, not defaults.
When no trustworthy cost estimate is available, omit it.

Example shape (replace every placeholder with verified live data):

```json
{
  "api_state": {"request": "<approved task-specific summary>"},
  "required_roles": ["executor", "judge"],
  "roster": [
    {"id": "model-a", "provider": "<provider>", "model": "<live model id>",
     "family": "<verified family>", "generation": 3, "capability_rank": 2,
     "capability_description": "<verified public capability profile>",
     "efforts": [{"id": "<supported effort>", "description": "<live effort semantics>"}]}
  ],
  "roles": {
    "executor": {"task": "<concrete task>", "difficulty": "<specific difficulty>",
                 "absences": ["<relevant absent evidence>"], "costs": {}},
    "judge": {"task": "<independent review task>", "difficulty": "<specific difficulty>",
              "absences": [], "costs": {}}
  },
  "plans": [
    {"id": "executor-review", "roles": {"executor": "model-a", "judge": "model-a"}}
  ]
}
```

Plans must satisfy the judge floor before any request is made; replace the
illustrative metadata rather than treating it as a valid real roster. Explorer-
only plans need no numeric judge metadata. Invoke the helper with
`TYPESAFE_API_KEY`, or use `--key-file <runtime-key-file>`; for example:
`python skills/running-as-maestro/scripts/jev_route.py <approved-routing-input.json> --key-env TYPESAFE_API_KEY`.
Run the helper's mocked unit tests with
`python -m unittest discover -s skills/running-as-maestro/scripts -p 'test_*.py' -v`.
