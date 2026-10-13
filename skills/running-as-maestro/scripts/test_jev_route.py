import json
import tempfile
import unittest
from unittest import mock

import jev_route


def sample():
    return {
        "api_state": {"request": "approved summary"},
        "roster": [
            {"id": "exec", "provider": "p", "model": "model-x", "family": "f", "generation": 2,
             "capability_rank": 2, "capability_description": "verified coding capability",
             "efforts": [{"id": "low", "description": "lower reasoning setting"}, {"id": "high", "description": "higher reasoning setting"}]},
            {"id": "judge", "provider": "p", "model": "model-y", "family": "f", "generation": 3,
             "capability_rank": 3, "capability_description": "verified stronger review capability",
             "efforts": [{"id": "low", "description": "lower reasoning setting"}, {"id": "high", "description": "higher reasoning setting"}]},
        ],
        "required_roles": ["executor", "judge"],
        "roles": {
            "executor": {"task": "implement feature", "difficulty": "cross-file change", "absences": ["no benchmark needed"],
                         "constraints": ["security review"], "costs": {"exec": {"low": 1, "high": 3}}},
            "judge": {"task": "independently review", "difficulty": "security-sensitive", "absences": [],
                      "costs": {"judge": {"low": 1, "high": 2}}},
        },
        "plans": [{"id": "pair", "roles": {"executor": "exec", "judge": "judge"}}],
    }


def answer(choice, probabilities, confidence=0.8):
    return {"type": "choice", "choice": choice, "confidence": confidence, "probabilities": probabilities}


def response(answers, model="jev-latest", input_tokens=1):
    return {"model": model, "usage": {"input_tokens": input_tokens, "output_tokens": 1}, "answers": answers}


class RoutingTests(unittest.TestCase):
    def test_three_stage_pair_selection_and_self_contained_state(self):
        data = sample()
        first = response({"route": answer("pair", {"pair": 1})}, model="jev-route-version", input_tokens=11)
        second = response({"executor_effort": answer("low", {"low": 0.8, "high": 0.2})}, model="jev-effort-version", input_tokens=22)
        third = response({"judge_effort": answer("high", {"low": 0.2, "high": 0.8})}, model="jev-judge-version", input_tokens=33)
        with mock.patch.object(jev_route, "_post", side_effect=[first, second, third]) as post:
            result = jev_route.route(data, "secret", 3)
        self.assertEqual(result["roles"]["executor"]["effort"], "low")
        self.assertEqual(result["roles"]["judge"]["effort"], "high")
        self.assertEqual(result["jev_stages"]["route"]["model"], "jev-route-version")
        self.assertEqual(result["jev_stages"]["efforts"]["model"], "jev-effort-version")
        self.assertEqual(result["jev_stages"]["judge_effort"]["model"], "jev-judge-version")
        self.assertEqual(result["jev_stages"]["judge_effort"]["usage"]["input_tokens"], 33)
        self.assertEqual(post.call_count, 3)
        req1, req2, req3 = (call.args[0] for call in post.call_args_list)
        self.assertIn("difficulty", req1["state"]["role_tasks"]["executor"])
        self.assertIn("absences", req1["state"]["role_tasks"]["executor"])
        self.assertIn("capability=verified", str(req1["questions"]))
        self.assertIn("independently reviews", req1["questions"]["route"]["instructions"])
        self.assertIn("selected_route", req2["state"])
        self.assertNotIn("judge_effort", req2["questions"])
        self.assertEqual(req3["state"]["selected_executor_effort"]["effort"], "low")
        self.assertNotIn("secret", json.dumps(req1))

    def test_explorer_only_is_eligible(self):
        data = sample()
        data["roles"] = {"explorer": {"task": "inspect API", "difficulty": "unknown docs", "absences": []}}
        data["required_roles"] = ["explorer"]
        data["roster"] = data["roster"][:1]
        data["plans"] = [{"id": "explore", "roles": {"explorer": "exec"}}]
        with mock.patch.object(jev_route, "_post", side_effect=[
            response({"route": answer("explore", {"explore": 1})}),
            response({"explorer_effort": answer("low", {"low": 1, "high": 0})}),
        ]):
            self.assertIn("explorer", jev_route.route(data, "k", 3)["roles"])

    def test_rejects_missing_metadata_and_judge_floor_before_network(self):
        cases = [sample(), sample()]
        cases[0]["roster"][0].pop("capability_rank")
        cases[1]["roster"][1]["generation"] = 1
        for data in cases:
            with self.subTest(data=data), mock.patch.object(jev_route, "_post") as post:
                with self.assertRaises(jev_route.RouteError):
                    jev_route.route(data, "k", 3)
                post.assert_not_called()

    def test_rejects_unsupported_effort_malformed_confidence_and_distribution(self):
        data = sample()
        invalid_answers = [
            {"executor_effort": answer("ultra", {"low": 0, "high": 1})},
            {"executor_effort": answer("low", {"low": 0.8, "high": 0.2}, 2)},
            {"executor_effort": answer("low", {"low": 0.1, "high": 0.2})},
        ]
        for answers in invalid_answers:
            with mock.patch.object(jev_route, "_post", side_effect=[
                response({"route": answer("pair", {"pair": 1})}), response(answers),
            ]):
                with self.assertRaises(jev_route.RouteError):
                    jev_route.route(data, "k", 3)

    def test_no_plan_may_drop_a_required_role_and_explorer_only_needs_no_executor(self):
        data = sample()
        data["plans"] = [{"id": "executor-only", "roles": {"executor": "exec"}}]
        with self.assertRaises(jev_route.RouteError):
            jev_route.validate(data)
        data["roles"] = {"explorer": {"task": "inspect", "difficulty": "unknown", "absences": []}}
        data["required_roles"] = ["explorer"]
        data["roster"] = data["roster"][:1]
        data["plans"] = [{"id": "explorer-only", "roles": {"explorer": "exec"}}]
        jev_route.validate(data)

    def test_metadata_floor_is_not_required_when_no_judge_is_considered(self):
        data = sample()
        data["roles"] = {"explorer": {"task": "inspect", "difficulty": "unknown", "absences": []}}
        data["required_roles"] = ["explorer"]
        data["plans"] = [{"id": "explore", "roles": {"explorer": "exec"}}]
        for field in ("family", "generation", "capability_rank"):
            data["roster"][0].pop(field)
        jev_route.validate(data)

    def test_judge_floor_is_provider_and_family_scoped(self):
        data = sample()
        data["roster"][1]["provider"] = "different-provider"
        data["roster"][1]["generation"] = 1
        jev_route.validate(data)
        data["roster"][1]["provider"] = "p"
        with self.assertRaises(jev_route.RouteError):
            jev_route.validate(data)

    def test_bad_judge_first_plan_references_fail_before_network(self):
        data = sample()
        data["plans"][0]["roles"] = {"judge": "judge", "executor": []}
        with mock.patch.object(jev_route, "_post") as post:
            with self.assertRaises(jev_route.RouteError):
                jev_route.route(data, "k", 3)
            post.assert_not_called()

    def test_network_redirect_and_errors_are_safe(self):
        class Redirect:
            def open(self, req, timeout):
                raise jev_route.RouteError("TypeSafe redirected the request; routing stopped.")

        with mock.patch.object(jev_route.urllib.request, "build_opener", return_value=Redirect()):
            with self.assertRaisesRegex(jev_route.RouteError, "redirected"):
                jev_route._post({}, "never-print-this-key", 1)
        with mock.patch.object(jev_route.urllib.request, "build_opener", side_effect=OSError("secret response body")):
            with self.assertRaises(jev_route.RouteError) as caught:
                jev_route._post({}, "never-print-this-key", 1)
        self.assertNotIn("secret", str(caught.exception))

    def test_deep_json_response_is_a_safe_failure(self):
        class DeepResponse:
            status = 200
            def __enter__(self):
                return self
            def __exit__(self, *args):
                return False
            def read(self, _limit):
                return b"[" * 20000 + b"0" + b"]" * 20000

        class Opener:
            def open(self, req, timeout):
                return DeepResponse()

        with mock.patch.object(jev_route.urllib.request, "build_opener", return_value=Opener()):
            with self.assertRaises(jev_route.RouteError):
                jev_route._post({}, "test-key", 1)

    def test_bad_choice_type_fails_closed(self):
        with self.assertRaises(jev_route.RouteError):
            jev_route._choice({"answers": {"q": {"type": "choice", "choice": {}, "confidence": 1,
                                                      "probabilities": {"x": 1}}}}, "q", {"x": "x"})

    def test_cli_error_does_not_echo_key_or_path(self):
        with tempfile.NamedTemporaryFile("w", encoding="utf-8", delete=False) as f:
            f.write("private-key")
            key_path = f.name
        try:
            with mock.patch.dict("os.environ", {"TYPESAFE_API_KEY": ""}):
                with mock.patch("sys.stderr", new_callable=__import__("io").StringIO) as err:
                    self.assertEqual(jev_route.main(["missing-input", "--key-file", key_path]), 2)
                self.assertNotIn(key_path, err.getvalue())
                self.assertNotIn("private-key", err.getvalue())
        finally:
            import os
            os.unlink(key_path)


if __name__ == "__main__":
    unittest.main()
