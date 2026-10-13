#!/usr/bin/env python3
"""Select a Maestro model route and supported efforts with TypeSafe Choice."""

from __future__ import annotations

import argparse
import http.client
import json
import math
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

API_URL = "https://api.typesafe.ai/v1/systemone"
MAX_RESPONSE = 1_048_576
MAX_INPUT = 1_048_576
ROLES = ("explorer", "executor", "judge")
ROLE_GUIDANCE = {
    "explorer": "Investigates a bounded question and reports evidence, uncertainty, and findings; include only when needed.",
    "executor": "Owns one coherent implementation or research deliverable and its focused validation; the model must plausibly complete the task.",
    "judge": "Independently reviews executor work against the request, criteria, diff, and evidence. The judge is a separate worker with an independent context and must satisfy the capability floor.",
}


class RouteError(Exception):
    """Safe, user-facing routing failure."""


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req: Any, fp: Any, code: int, msg: str,
                         headers: Any, newurl: str) -> None:
        raise RouteError("TypeSafe redirected the request; routing stopped.")


def load_input(path: str) -> dict[str, Any]:
    try:
        raw = Path(path).read_bytes()
        if len(raw) > MAX_INPUT:
            raise RouteError("Routing input exceeded the size limit.")
        data = json.loads(raw.decode("utf-8"), parse_constant=lambda _value: (_ for _ in ()).throw(ValueError()))
    except RouteError:
        raise
    except (OSError, UnicodeError, ValueError, RecursionError):
        raise RouteError("Could not read valid routing JSON input.") from None
    if not isinstance(data, dict):
        raise RouteError("Routing input must be a JSON object.")
    return data


def validate(data: dict[str, Any]) -> tuple[dict[str, dict[str, Any]], dict[str, Any], list[dict[str, Any]], set[str]]:
    api_state = data.get("api_state")
    if not isinstance(api_state, dict):
        raise RouteError("api_state must be an explicitly approved JSON object.")
    roles = data.get("roles")
    roster_items = data.get("roster")
    plans = data.get("plans")
    if not isinstance(roles, dict) or not roles or not set(roles) <= set(ROLES):
        raise RouteError("roles must contain one or more supported role tasks.")
    required = data.get("required_roles")
    if (not isinstance(required, list) or not required or any(not isinstance(r, str) or r not in roles for r in required)
            or len(set(required)) != len(required)):
        raise RouteError("required_roles must explicitly identify the roles every eligible plan must include.")
    required_set = set(required)
    plans_with_judge = isinstance(plans, list) and any(
        isinstance(plan, dict) and isinstance(plan.get("roles"), dict) and "judge" in plan["roles"]
        for plan in plans)
    if not isinstance(roster_items, list) or not roster_items:
        raise RouteError("roster must contain verified live model metadata.")
    roster: dict[str, dict[str, Any]] = {}
    for item in roster_items:
        if not isinstance(item, dict):
            raise RouteError("Invalid live roster entry.")
        mid, provider, model = item.get("id"), item.get("provider"), item.get("model")
        efforts = item.get("efforts")
        if not all(isinstance(v, str) and v for v in (mid, provider, model)) or mid in roster:
            raise RouteError("Roster IDs, providers, and model IDs must be unique valid strings.")
        if (not isinstance(efforts, list) or not efforts or len(efforts) > 255
                or any(not isinstance(e, dict) or not isinstance(e.get("id"), str) or not e["id"]
                       or not isinstance(e.get("description"), str) or not e["description"].strip() for e in efforts)
                or len({e["id"] for e in efforts}) != len(efforts)):
            raise RouteError("Each live model needs unique supported efforts with caller-provided semantic descriptions.")
        if not isinstance(item.get("capability_description"), str) or not item["capability_description"].strip():
            raise RouteError("A caller-verified public capability description is required for each model.")
        if plans_with_judge:
            if not isinstance(item.get("family"), str) or not item["family"]:
                raise RouteError("Verified model family metadata is required when a route includes a judge.")
            for field in ("generation", "capability_rank"):
                if isinstance(item.get(field), bool) or not isinstance(item.get(field), int) or item[field] < 0:
                    raise RouteError("Verified generation and capability rank integers are required for judge pairing.")
        roster[mid] = item
    for role, task in roles.items():
        if (not isinstance(task, dict) or not isinstance(task.get("task"), str) or not task["task"].strip()
                or not isinstance(task.get("difficulty"), str) or not task["difficulty"].strip()
                or not isinstance(task.get("absences"), list)
                or any(not isinstance(v, str) for v in task["absences"])):
            raise RouteError("Each role needs a concrete task, difficulty, and explicit absences list.")
        costs = task.get("costs", {})
        if not isinstance(costs, dict):
            raise RouteError("Role cost estimates must be objects keyed by live model ID.")
        for model_id, estimates in costs.items():
            effort_ids = {e["id"] for e in roster.get(model_id, {}).get("efforts", [])}
            if (not isinstance(model_id, str) or model_id not in roster or not isinstance(estimates, dict)
                    or any(e not in effort_ids or not isinstance(v, (int, float)) or isinstance(v, bool)
                           or not _finite(v) or v < 0 for e, v in estimates.items())):
                raise RouteError("Cost estimates must map live model IDs to supported efforts and nonnegative numbers.")
    if not isinstance(plans, list) or not plans or len(plans) > 255:
        raise RouteError("Provide between 1 and 255 eligible route plans.")
    seen: set[str] = set()
    for plan in plans:
        if not isinstance(plan, dict) or not isinstance(plan.get("id"), str) or not plan["id"] or plan["id"] in seen:
            raise RouteError("Route plan IDs must be unique nonempty strings.")
        seen.add(plan["id"])
        selected = plan.get("roles")
        if not isinstance(selected, dict) or not selected or not set(selected) <= set(roles) or not required_set <= set(selected):
            raise RouteError("A route plan must select only supplied role tasks.")
        if "judge" in selected and "executor" not in selected:
            raise RouteError("A judge route must also include its executor.")
        for mid in selected.values():
            if not isinstance(mid, str) or mid not in roster:
                raise RouteError("A route references a model outside the live roster.")
        if "judge" in selected:
            executor = roster[selected["executor"]]
            judge = roster[selected["judge"]]
            if judge["capability_rank"] < executor["capability_rank"]:
                raise RouteError("A route violates the judge capability floor.")
            if judge["provider"] == executor["provider"] and judge["family"] == executor["family"] and judge["generation"] < executor["generation"]:
                raise RouteError("A route uses an older same-family judge generation.")
    return roster, roles, plans, required_set


def _post(payload: dict[str, Any], key: str, timeout: float) -> dict[str, Any]:
    if not isinstance(key, str) or not key or any(ord(ch) < 33 or ord(ch) > 126 for ch in key):
        raise RouteError("API key contains unsupported header characters; routing stopped.")
    try:
        body = json.dumps(payload, separators=(",", ":"), allow_nan=False).encode("utf-8")
    except (TypeError, ValueError, RecursionError):
        raise RouteError("Could not prepare bounded TypeSafe request; routing stopped.") from None
    req = urllib.request.Request(API_URL, data=body, method="POST", headers={
        "Authorization": f"Bearer {key}", "Content-Type": "application/json",
    })
    try:
        opener = urllib.request.build_opener(NoRedirect)
        with opener.open(req, timeout=timeout) as response:
            if response.status != 200:
                raise RouteError("TypeSafe returned an unsuccessful response; routing stopped.")
            raw = response.read(MAX_RESPONSE + 1)
    except RouteError:
        raise
    except (urllib.error.URLError, TimeoutError, OSError, http.client.HTTPException):
        raise RouteError("TypeSafe request failed; routing stopped.") from None
    if len(raw) > MAX_RESPONSE:
        raise RouteError("TypeSafe response exceeded the size limit; routing stopped.")
    try:
        result = json.loads(raw, parse_constant=lambda _value: (_ for _ in ()).throw(ValueError()))
    except (UnicodeError, ValueError, RecursionError):
        raise RouteError("TypeSafe returned malformed JSON; routing stopped.") from None
    usage = result.get("usage") if isinstance(result, dict) else None
    if (not isinstance(result, dict) or not isinstance(result.get("model"), str) or not result["model"]
            or not isinstance(usage, dict)
            or any(isinstance(usage.get(k), bool) or not isinstance(usage.get(k), int) or usage[k] < 0
                   for k in ("input_tokens", "output_tokens"))):
        raise RouteError("TypeSafe response omitted required model or usage metadata.")
    return result


def _choice(result: dict[str, Any], question_id: str, choices: dict[str, Any]) -> tuple[str, float]:
    answers = result.get("answers")
    answer = answers.get(question_id) if isinstance(answers, dict) else None
    if not isinstance(answer, dict) or answer.get("type") != "choice":
        raise RouteError("TypeSafe returned an invalid choice answer; routing stopped.")
    selected = answer.get("choice")
    confidence = answer.get("confidence")
    probabilities = answer.get("probabilities")
    if not isinstance(selected, str) or selected not in choices or isinstance(confidence, bool) or not isinstance(confidence, (int, float)) or not _finite(confidence) or not 0 <= confidence <= 1:
        raise RouteError("TypeSafe selected an unsupported choice or invalid confidence; routing stopped.")
    if not isinstance(probabilities, dict) or set(probabilities) != set(choices):
        raise RouteError("TypeSafe returned an invalid probability distribution; routing stopped.")
    values = list(probabilities.values())
    if any(isinstance(v, bool) or not isinstance(v, (int, float)) or not _finite(v) or not 0 <= v <= 1 for v in values) or abs(sum(values) - 1) > 0.001:
        raise RouteError("TypeSafe returned an invalid probability distribution; routing stopped.")
    if probabilities[selected] != max(probabilities.values()):
        raise RouteError("TypeSafe choice does not match its highest probability; routing stopped.")
    return selected, float(confidence)


def _finite(value: int | float) -> bool:
    try:
        return math.isfinite(value)
    except OverflowError:
        return False


def route(data: dict[str, Any], key: str, timeout: float) -> dict[str, Any]:
    roster, roles, plans, required = validate(data)
    state = {"approved_task_state": data["api_state"], "role_meanings": ROLE_GUIDANCE,
             "required_roles": sorted(required),
             "judge_floor": "judge capability rank >= executor; for same provider and family, judge generation >= executor generation",
             "role_tasks": {r: {"task": v["task"], "difficulty": v["difficulty"],
                               "absences": v["absences"], "constraints": v.get("constraints", [])}
                           for r, v in roles.items()}}
    plan_options: dict[str, str] = {}
    for plan in plans:
        parts = []
        for role, mid in plan["roles"].items():
            model = roster[mid]
            costs = roles[role].get("costs", {}).get(mid, {})
            effort_info = [{"id": e["id"], "description": e["description"],
                            "cost_estimate": costs.get(e["id"], "unknown")} for e in model["efforts"]]
            parts.append(f"{role}: {model['provider']} / {model['model']}; capability={model['capability_description']}; effort options and role-specific costs={effort_info}")
        plan_options[plan["id"]] = " | ".join(parts)
    route_question = {
        "type": "choice",
        "instructions": f"Role meanings: {ROLE_GUIDANCE}. Select a complete eligible route including all required roles in state. Compare expected whole-route work, review/retry costs, and supplied per-model/per-effort estimates together. The executor must plausibly complete its task; any judge independently reviews that executor and satisfies the capability floor. Prefer the least expensive route plausibly sufficient, not the strongest by default. Security constraints alone do not require a frontier model or maximum effort. Return only the best eligible plan.",
        "criteria": plan_options,
    }
    first = _post({"model": "jev-latest", "state": state, "questions": {"route": route_question}}, key, timeout)
    plan_id, route_confidence = _choice(first, "route", plan_options)
    selected_plan = next(p for p in plans if p["id"] == plan_id)
    second_state = {"approved_task_state": data["api_state"], "role_meanings": ROLE_GUIDANCE,
                    "required_roles": sorted(required), "judge_floor": state["judge_floor"],
                    "selected_route": {role: {"provider": roster[mid]["provider"], "model": roster[mid]["model"],
                                              "capability": roster[mid]["capability_description"], "supported_efforts": roster[mid]["efforts"]}
                                      for role, mid in selected_plan["roles"].items()},
                    "role_tasks": {r: {"task": roles[r]["task"], "difficulty": roles[r]["difficulty"],
                                       "absences": roles[r]["absences"], "constraints": roles[r].get("constraints", [])}
                                   for r in selected_plan["roles"]},
                    "cost_estimates": {r: roles[r].get("costs", {}).get(mid, {})
                                       for r, mid in selected_plan["roles"].items()}}
    # Independent non-judge choices can be asked together. Judge effort follows
    # because its state must include the executor's actual selected effort.
    early_roles = [r for r in selected_plan["roles"] if r != "judge"]
    effort_questions: dict[str, Any] = {}
    effort_criteria: dict[str, dict[str, str]] = {}
    for role in early_roles:
        mid = selected_plan["roles"][role]
        qid = f"{role}_effort"
        costs = roles[role].get("costs", {}).get(mid, {})
        effort_criteria[qid] = {e["id"]: f"{e['description']}; supplied cost estimate={costs.get(e['id'], 'unknown')}" for e in roster[mid]["efforts"]}
        effort_questions[qid] = {"type": "choice",
                                 "instructions": f"Role meanings: {ROLE_GUIDANCE}. Choose a supported effort for {role}. Task: {roles[role]['task']}; difficulty: {roles[role]['difficulty']}; absences: {roles[role]['absences']}; constraints: {roles[role].get('constraints', [])}. Consider expected review/retries and supplied role-specific cost estimates. Choose higher effort only for a concrete difficulty likely to defeat lower settings. Security constraints alone do not imply highest effort.",
                                 "criteria": effort_criteria[qid]}
    if effort_questions:
        second = _post({"model": "jev-latest", "state": second_state, "questions": effort_questions}, key, timeout)
    else:
        second = {"model": "jev-latest", "answers": {}, "usage": {"input_tokens": 0, "output_tokens": 0}}
    efforts: dict[str, dict[str, Any]] = {}
    for role in early_roles:
        mid = selected_plan["roles"][role]
        qid = f"{role}_effort"
        effort, confidence = _choice(second, qid, effort_criteria[qid])
        efforts[role] = {"provider": roster[mid]["provider"], "model": roster[mid]["model"], "effort": effort,
                         "confidence": confidence, "criterion": effort_criteria[qid][effort]}
    if "judge" in selected_plan["roles"]:
        role, mid = "judge", selected_plan["roles"]["judge"]
        executor_id = selected_plan["roles"]["executor"]
        qid = "judge_effort"
        costs = roles[role].get("costs", {}).get(mid, {})
        criteria = {e["id"]: f"{e['description']}; supplied cost estimate={costs.get(e['id'], 'unknown')}" for e in roster[mid]["efforts"]}
        judge_state = {**second_state, "selected_executor_effort": efforts["executor"],
                       "executor_model_profile": roster[executor_id]["capability_description"],
                       "judge_cost_estimates": costs}
        question = {qid: {"type": "choice",
                          "instructions": f"Role meanings: {ROLE_GUIDANCE}. Choose a supported judge effort for this independent review. Task: {roles[role]['task']}; difficulty: {roles[role]['difficulty']}; absences: {roles[role]['absences']}; constraints: {roles[role].get('constraints', [])}. State includes the actual executor model and effort; consider executor/judge economics together. Size effort for review risk and ambiguity. Higher effort requires concrete difficulty likely to defeat lower settings. Security constraints alone do not imply highest effort.",
                          "criteria": criteria}}
        third = _post({"model": "jev-latest", "state": judge_state, "questions": question}, key, timeout)
        effort, confidence = _choice(third, qid, criteria)
        efforts[role] = {"provider": roster[mid]["provider"], "model": roster[mid]["model"], "effort": effort,
                         "confidence": confidence, "criterion": criteria[effort]}
        judge_response = third
    stages = {"route": first, "efforts": second}
    if "judge" in selected_plan["roles"]:
        stages["judge_effort"] = judge_response
    return {"plan": plan_id, "route_confidence": route_confidence,
            "route_criterion": plan_options[plan_id], "roles": efforts,
            "jev_stages": {stage: {"model": result["model"], "usage": result["usage"]}
                           for stage, result in stages.items()}}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", help="local JSON input with explicit approved api_state")
    parser.add_argument("--key-env", default="TYPESAFE_API_KEY", help="environment variable containing the API key")
    parser.add_argument("--key-file", help="runtime file containing the API key")
    parser.add_argument("--timeout", type=float, default=20.0, help="per-request timeout in seconds (1..120)")
    args = parser.parse_args(argv)
    try:
        if args.key_file and args.key_env != "TYPESAFE_API_KEY":
            raise RouteError("Choose either --key-file or --key-env.")
        if not 1 <= args.timeout <= 120:
            raise RouteError("Timeout must be between 1 and 120 seconds.")
        if args.key_file:
            try:
                key = Path(args.key_file).read_text(encoding="utf-8").strip()
            except (OSError, UnicodeError):
                raise RouteError("Could not read the runtime API key file.") from None
        else:
            key = os.environ.get(args.key_env, "")
        if not key:
            raise RouteError("API key is unavailable; set the named environment variable or provide --key-file.")
        result = route(load_input(args.input), key, args.timeout)
    except RouteError as exc:
        print(f"jev_route: {exc}", file=sys.stderr)
        return 2
    print(json.dumps(result, ensure_ascii=False, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
