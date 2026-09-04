from __future__ import annotations

import copy
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
import urllib.error
import urllib.parse
from contextlib import redirect_stderr, redirect_stdout
from datetime import datetime, timezone
from decimal import Decimal
from pathlib import Path
from unittest import mock

from tools.model_benchmarks import cli
from tools.model_benchmarks.core import (
    PARSER_VERSION,
    BenchmarkError,
    Row,
    assert_source_rows,
    fetch_source,
    identifier,
    markdown_scalar,
    parse_cursorbench,
    parse_deepswe,
    read_registry,
    render_snapshot,
    set_pareto,
    source_model,
    update_benchmarks,
)

PACKAGE = Path(__file__).resolve().parents[1]
REGISTRY_PATH = PACKAGE / "sources.json"
FIXTURES = PACKAGE / "fixtures"


class BenchmarkTestCase(unittest.TestCase):
    def setUp(self) -> None:
        self.registry = read_registry(REGISTRY_PATH)
        self.deep_source = next(item for item in self.registry["sources"] if item["id"] == "deepswe")
        self.cursor_source = next(item for item in self.registry["sources"] if item["id"] == "cursorbench")
        self.deep_content = (FIXTURES / "deepswe.json").read_text(encoding="utf-8")
        self.cursor_content = (FIXTURES / "cursorbench.html").read_text(encoding="utf-8")

    def fixture_registry(self, directory: Path) -> Path:
        registry = copy.deepcopy(self.registry)
        for source in registry["sources"]:
            source["minimumRows"] = 2
        path = directory / "sources.json"
        path.write_text(json.dumps(registry), encoding="utf-8")
        return path


class AdapterTests(BenchmarkTestCase):
    def test_fixtures_parse_all_rows_and_split_model_effort(self) -> None:
        deep = parse_deepswe(self.deep_content, self.deep_source)
        cursor = parse_cursorbench(self.cursor_content, self.cursor_source)
        self.assertEqual(3, len(deep.rows))
        self.assertEqual(3, len(cursor.rows))
        self.assertEqual(("GPT-5.6 Sol", "Max"), (cursor.rows[0].model, cursor.rows[0].effort))
        self.assertEqual((self.cursor_source["harness"], self.cursor_source["config"]), (cursor.rows[0].harness, cursor.rows[0].config))

    def test_current_live_model_labels_pass_positive_contracts(self) -> None:
        deep_models = (
            "claude-fable-5", "claude-opus-4-8", "claude-opus-5", "claude-sonnet-4-6", "claude-sonnet-5",
            "deepseek-v4-flash", "deepseek-v4-pro", "gemini-3-1-pro-preview", "gemini-3-5-flash",
            "gemini-3-6-flash", "gemini-3-7-flash", "glm-5-2", "glm-5-3", "glm-5-3-flash", "gpt-5-4", "gpt-5-5",
            "gpt-5-6-luna", "gpt-5-6-sol", "gpt-5-6-terra", "gpt-6-astra", "grok-4-5", "grok-4-6",
            "kimi-k2-7-code", "kimi-k3", "muse-spark-1-1", "muse-spark-1-2", "qwen3-8-max",
        )
        cursor_models = (
            "Composer 2.5", "Fable 5", "Gemini 3.6 Flash", "Gemini 3.7 Flash", "GLM 5.2", "GPT-5.5",
            "GPT-5.6 Luna", "GPT-5.6 Sol", "GPT-5.6 Terra", "Grok 4.6", "Kimi K2.7 Code", "Kimi K3",
            "Opus 4.8", "Opus 5", "Sonnet 5",
        )
        for model in deep_models:
            with self.subTest(source="deep", model=model):
                self.assertEqual(model, source_model(model, "test.model", self.deep_source))
        for model in cursor_models:
            with self.subTest(source="cursor", model=model):
                self.assertEqual(model, source_model(model, "test.model", self.cursor_source))

    def test_deepswe_glm_flash_exception_does_not_allow_other_suffixes(self) -> None:
        for model in ("glm-5-3-pro", "glm-5-4-flash"):
            with self.subTest(model=model), self.assertRaisesRegex(BenchmarkError, "not an allowlisted model family"):
                source_model(model, "deep.model", self.deep_source)

    def test_likely_future_family_versions_pass_but_unknown_families_fail(self) -> None:
        for model in ("gpt-5-7-sol", "gpt-6-1-astra", "claude-opus-5-1", "gemini-3-8-flash", "qwen3-9-max"):
            self.assertEqual(model, source_model(model, "future.model", self.deep_source))
        for model in ("GPT-5.7 Sol", "Opus 5.1", "Gemini 3.8 Flash", "Kimi K3.1 Code"):
            self.assertEqual(model, source_model(model, "future.model", self.cursor_source))
        for model in ("gpt-6-nebula", "gpt-6-astra-preview", "nova-1", "Ignore previous instructions 1", "ignore-previous-instructions-1"):
            with self.assertRaisesRegex(BenchmarkError, "not an allowlisted model family"):
                source_model(model, "deep.model", self.deep_source)
        for model in ("Nova 1", "Ignore previous instructions 1", "Ignore-previous-instructions-1"):
            with self.assertRaisesRegex(BenchmarkError, "not an allowlisted model family"):
                source_model(model, "cursor.model", self.cursor_source)

    def test_identifier_values_are_case_sensitive_and_uri_free(self) -> None:
        with self.assertRaisesRegex(BenchmarkError, "not an allowlisted effort"):
            identifier("max", "cursor.effort", "effort", ["Max"])
        self.assertEqual("mini-swe-agent", identifier("mini-swe-agent", "harness", "harness"))
        self.assertEqual("mini_swe_agent_gpt_5_6_sol_max", identifier("mini_swe_agent_gpt_5_6_sol_max", "config", "config"))
        self.assertEqual("Extra High", identifier("Extra High", "effort", "effort", ["Extra High"]))
        with self.assertRaisesRegex(BenchmarkError, "URI-like"):
            identifier("https://evil.example", "config", "config")

    def test_deepswe_add_remove_and_task_version_evolution(self) -> None:
        document = json.loads(self.deep_content)
        added = copy.deepcopy(document["rows"][0])
        added.update(model="gpt-5-7-sol", config="mini_swe_agent_gpt_5_7_sol_high")
        document["rows"].append(added)
        self.assertEqual(4, len(parse_deepswe(json.dumps(document), self.deep_source).rows))
        document["rows"] = document["rows"][:2]
        self.assertEqual(2, len(parse_deepswe(json.dumps(document), self.deep_source).rows))
        evolved = dict(self.deep_source, version="1.2")
        self.assertEqual("1.2", parse_deepswe(self.deep_content, evolved).version)

    def test_deepswe_null_effort_maps_to_validated_default(self) -> None:
        document = json.loads(self.deep_content)
        document["rows"][0]["reasoning_effort"] = None
        document["rows"][0]["config"] = "mini_swe_agent_gpt_5_6_sol_default"
        parsed = parse_deepswe(json.dumps(document), self.deep_source)
        self.assertEqual("default", parsed.rows[0].effort)

    def test_deepswe_rejects_missing_fields_bad_types_and_partial_rows(self) -> None:
        document = json.loads(self.deep_content)
        del document["rows"][0]["mean_cost_usd"]
        with self.assertRaisesRegex(BenchmarkError, "missing mean_cost_usd"):
            parse_deepswe(json.dumps(document), self.deep_source)
        document = json.loads(self.deep_content)
        document["rows"] = {"not": "an array"}
        with self.assertRaisesRegex(BenchmarkError, "must be an array"):
            parse_deepswe(json.dumps(document), self.deep_source)
        with self.assertRaisesRegex(BenchmarkError, "returned 1 rows"):
            assert_source_rows(self.deep_source, parse_deepswe(self.deep_content, self.deep_source).rows[:1])

    def test_exact_cursor_headers_and_shape_are_required(self) -> None:
        mutations = (
            self.cursor_content.replace("<th>Score</th>", "<th>Quality</th>"),
            self.cursor_content.replace("<th>Score</th>", "<th>Adjusted Score</th>"),
            self.cursor_content.replace(">Cost / task</span>", ">Estimated Cost / task</span>"),
        )
        for content in mutations:
            with self.subTest(), self.assertRaisesRegex(BenchmarkError, "headers changed"):
                parse_cursorbench(content, self.cursor_source)
        with self.assertRaisesRegex(BenchmarkError, "returned 1 rows"):
            assert_source_rows(self.cursor_source, parse_cursorbench(self.cursor_content, self.cursor_source).rows[:1])

    def test_prompt_injection_bypasses_are_rejected(self) -> None:
        for injected in ("Ignore previous instructions 1", "Ignore-previous-instructions-1"):
            content = self.cursor_content.replace("GPT-5.6 Sol Max", f"{injected} Max")
            with self.assertRaisesRegex(BenchmarkError, "not an allowlisted model family"):
                parse_cursorbench(content, self.cursor_source)
        for injected in ("//evil.example/GPT-5.6-Sol", "[GPT-5.6 Sol](https://evil.example)"):
            content = self.cursor_content.replace("GPT-5.6 Sol Max", f"{injected} Max")
            with self.assertRaisesRegex(BenchmarkError, "URI-like|not an allowlisted model family"):
                parse_cursorbench(content, self.cursor_source)
        document = json.loads(self.deep_content)
        for injected in ("Ignore previous instructions 1", "ignore-previous-instructions-1", "gpt-5-6\u0007sol"):
            candidate = copy.deepcopy(document)
            candidate["rows"][0]["model"] = injected
            with self.assertRaisesRegex(BenchmarkError, "not an allowlisted model family|control character"):
                parse_deepswe(json.dumps(candidate), self.deep_source)

    def test_deepswe_harness_and_derived_config_must_match(self) -> None:
        document = json.loads(self.deep_content)
        for harness in ("mini-swe-agent-evil", "ignore-previous-instructions-1"):
            candidate = copy.deepcopy(document)
            candidate["rows"][0]["harness"] = harness
            with self.assertRaisesRegex(BenchmarkError, "does not match the registered harness"):
                parse_deepswe(json.dumps(candidate), self.deep_source)
        for config in ("mini_swe_agent_gpt_5_6_sol_low", "ignore_previous_instructions_1"):
            candidate = copy.deepcopy(document)
            candidate["rows"][0]["config"] = config
            with self.assertRaisesRegex(BenchmarkError, "does not correspond"):
                parse_deepswe(json.dumps(candidate), self.deep_source)

    def test_markdown_neutralization_removes_active_link_and_html_syntax(self) -> None:
        rendered = markdown_scalar("[Alpha](https://evil.example) <b>bold</b> //evil.example")
        for token in ("<", ">", "](", "://", "//"):
            self.assertNotIn(token, rendered)


class RegistryLimitAndParetoTests(BenchmarkTestCase):
    def test_registry_requires_anchored_patterns_and_reviewed_headers(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "bad.json"
            registry = copy.deepcopy(self.registry)
            registry["sources"][0]["modelPatterns"][0] = "claude-[0-9]+"
            path.write_text(json.dumps(registry), encoding="utf-8")
            with self.assertRaisesRegex(BenchmarkError, "modelPatterns must be anchored"):
                read_registry(path)
            registry = copy.deepcopy(self.registry)
            registry["sources"][1]["expectedHeaders"][2] = "Quality"
            path.write_text(json.dumps(registry), encoding="utf-8")
            with self.assertRaisesRegex(BenchmarkError, "reviewed CursorBench schema"):
                read_registry(path)

    def test_source_row_floors_ceilings_and_duplicates(self) -> None:
        self.assertGreaterEqual(self.deep_source["minimumRows"], 40)
        self.assertGreaterEqual(self.cursor_source["minimumRows"], 35)
        deep = parse_deepswe(self.deep_content, self.deep_source)
        limited = dict(self.deep_source, minimumRows=1, maximumRows=2)
        with self.assertRaisesRegex(BenchmarkError, "returned 3 rows"):
            assert_source_rows(limited, deep.rows)
        permissive = dict(self.deep_source, minimumRows=1, maximumRows=10)
        with self.assertRaisesRegex(BenchmarkError, "duplicate"):
            assert_source_rows(permissive, [deep.rows[0], copy.deepcopy(deep.rows[0])])

    def test_pareto_maximizes_score_and_minimizes_cost(self) -> None:
        rows = [
            Row("a", "high", "h", "a", Decimal(70), Decimal(2)),
            Row("b", "high", "h", "b", Decimal(65), Decimal(1)),
            Row("c", "low", "h", "c", Decimal(60), Decimal(3)),
        ]
        set_pareto(rows)
        self.assertEqual([True, True, False], [row.pareto for row in rows])


class UpdateTests(BenchmarkTestCase):
    def test_compact_rendering_provenance_idempotence_and_no_churn(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            registry = self.fixture_registry(root)
            output = root / "snapshot.md"
            first = update_benchmarks(registry, output, fixture_root=FIXTURES, retrieved_at=datetime(2026, 8, 20, 12, tzinfo=timezone.utc), emit=lambda _: None)
            self.assertTrue(first.changed)
            before = output.read_bytes()
            text = before.decode()
            self.assertIn("| gpt-5-6-sol | high | 70.00% | $2.000", text)
            self.assertIn("| gemini-3-7-flash | low | 60.00% | $3.000 |", text)
            self.assertIn(f"- Parser version: `{PARSER_VERSION}`", text)
            self.assertEqual(2, len(__import__("re").findall(r"normalized SHA-256 `[a-f0-9]{64}`", text)))
            self.assertNotIn("| model | effort | harness / config |", text)
            self.assertIn("Shared harness: `mini-swe-agent` · configuration is derived from model + effort.", text)
            self.assertIn("Shared harness/config: `Cursor benchmark agent` · `published CursorBench configuration`.", text)
            self.assertNotIn("mini_swe_agent", text)
            second = update_benchmarks(registry, output, fixture_root=FIXTURES, retrieved_at=datetime(2026, 8, 21, 12, tzinfo=timezone.utc), emit=lambda _: None)
            self.assertFalse(second.changed)
            self.assertEqual(before, output.read_bytes())

    def test_source_failure_retains_last_known_good_snapshot(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            registry = self.fixture_registry(root)
            output = root / "snapshot.md"
            update_benchmarks(registry, output, fixture_root=FIXTURES, emit=lambda _: None)
            before = output.read_bytes()
            bad = root / "bad-fixtures"
            bad.mkdir()
            (bad / "deepswe.json").write_text(self.deep_content, encoding="utf-8")
            (bad / "cursorbench.html").write_text(self.cursor_content.replace("<th>Score</th>", "<th>Quality</th>"), encoding="utf-8")
            with self.assertRaisesRegex(BenchmarkError, "headers changed"):
                update_benchmarks(registry, output, fixture_root=bad, emit=lambda _: None)
            self.assertEqual(before, output.read_bytes())

    def test_size_and_total_row_limits_fail_before_write(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            registry = json.loads(self.fixture_registry(root).read_text())
            registry["snapshot"]["maximumBytes"] = 1024
            path = root / "small.json"
            path.write_text(json.dumps(registry), encoding="utf-8")
            output = root / "snapshot.md"
            with self.assertRaisesRegex(BenchmarkError, "maximum is 1024"):
                update_benchmarks(path, output, fixture_root=FIXTURES, emit=lambda _: None)
            self.assertFalse(output.exists())
            registry["snapshot"]["maximumBytes"] = 24576
            registry["snapshot"]["maximumRows"] = 5
            path.write_text(json.dumps(registry), encoding="utf-8")
            with self.assertRaisesRegex(BenchmarkError, "maximum is 5"):
                update_benchmarks(path, output, fixture_root=FIXTURES, emit=lambda _: None)

    def test_atomic_replace_failure_leaves_existing_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            registry = self.fixture_registry(root)
            output = root / "snapshot.md"
            output.write_bytes(b"last-known-good\n")
            with (
                mock.patch("tools.model_benchmarks.core.os.replace", side_effect=OSError("synthetic replace failure")),
                self.assertRaisesRegex(OSError, "synthetic replace failure"),
            ):
                update_benchmarks(registry, output, fixture_root=FIXTURES, emit=lambda _: None)
            self.assertEqual(b"last-known-good\n", output.read_bytes())
            self.assertEqual([], list(root.glob(".benchmark-snapshot.*.tmp")))

    def test_rendered_snapshot_is_deterministic(self) -> None:
        deep = parse_deepswe(self.deep_content, self.deep_source)
        cursor = parse_cursorbench(self.cursor_content, self.cursor_source)
        for benchmark in (deep, cursor):
            set_pareto(benchmark.rows)
        at = datetime(2026, 8, 20, tzinfo=timezone.utc)
        self.assertEqual(render_snapshot(self.registry, [deep, cursor], at), render_snapshot(self.registry, [deep, cursor], at))


class FetchTests(BenchmarkTestCase):
    class Response:
        status = 200
        def __enter__(self): return self
        def __exit__(self, *_): return False
        def getcode(self): return self.status
        def read(self): return b"eventual success"

    def test_transient_fetches_retry_exactly_three_times(self) -> None:
        attempts = []
        sleeps = []
        def opener(*_args, **_kwargs):
            attempts.append(1)
            if len(attempts) < 3:
                raise urllib.error.URLError("synthetic transient failure")
            return self.Response()
        source = {"id": "retry-fixture", "url": "https://example.test/data"}
        with redirect_stdout(io.StringIO()):
            result = fetch_source(source, opener=opener, sleeper=sleeps.append)
        self.assertEqual("eventual success", result)
        self.assertEqual(3, len(attempts))
        self.assertEqual([1, 2], sleeps)

    def test_nonretryable_http_failure_stops_immediately(self) -> None:
        attempts = []
        def opener(*_args, **_kwargs):
            attempts.append(1)
            raise urllib.error.HTTPError("https://example.test", 404, "not found", {}, None)
        with self.assertRaisesRegex(BenchmarkError, "non-retryable HTTP 404"):
            fetch_source({"id": "fixture", "url": "https://example.test"}, opener=opener, sleeper=lambda _: None)
        self.assertEqual(1, len(attempts))


class CliTests(BenchmarkTestCase):
    def test_help_and_check_refresh_exit_contract(self) -> None:
        help_run = subprocess.run([sys.executable, "-m", "tools.model_benchmarks", "--help"], cwd=cli.REPOSITORY_ROOT, text=True, capture_output=True, check=False)
        self.assertEqual(0, help_run.returncode)
        self.assertIn("{refresh,check,issue}", help_run.stdout)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            registry = self.fixture_registry(root)
            output = root / "snapshot.md"
            common = ["--registry", str(registry), "--output", str(output), "--fixture-root", str(FIXTURES), "--retrieved-at", "2026-08-20T12:00:00Z"]
            check = subprocess.run([sys.executable, "-m", "tools.model_benchmarks", "check", *common], cwd=cli.REPOSITORY_ROOT, capture_output=True, text=True, check=False)
            self.assertEqual(2, check.returncode, check.stderr)
            self.assertFalse(output.exists())
            refresh = subprocess.run([sys.executable, "-m", "tools.model_benchmarks", "refresh", *common], cwd=cli.REPOSITORY_ROOT, capture_output=True, text=True, check=False)
            self.assertEqual(0, refresh.returncode, refresh.stderr)
            current = subprocess.run([sys.executable, "-m", "tools.model_benchmarks", "check", *common], cwd=cli.REPOSITORY_ROOT, capture_output=True, text=True, check=False)
            self.assertEqual(0, current.returncode, current.stderr)

    def test_cli_validation_failure_is_exit_one_and_does_not_write(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "snapshot.md"
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                exit_code = cli.main(["check", "--registry", str(Path(temporary) / "missing.json"), "--output", str(output)])
            self.assertEqual(1, exit_code)
            self.assertFalse(output.exists())


class IssueLifecycleTests(unittest.TestCase):
    def test_issue_command_prefers_github_token_then_falls_back_to_gh_token(self) -> None:
        arguments = ["issue", "failure", "--repository", "sernst/skills", "--run-url", "https://example.test/run"]
        with mock.patch.object(cli, "GitHubIssues") as issues:
            with mock.patch.dict(os.environ, {"GITHUB_TOKEN": "standard", "GH_TOKEN": "fallback"}, clear=True):
                self.assertEqual(0, cli.main(arguments))
            issues.assert_called_once_with("sernst/skills", "standard", "https://api.github.com")

        with mock.patch.object(cli, "GitHubIssues") as issues:
            with mock.patch.dict(os.environ, {"GH_TOKEN": "fallback"}, clear=True):
                self.assertEqual(0, cli.main(arguments))
            issues.assert_called_once_with("sernst/skills", "fallback", "https://api.github.com")

    def test_issue_command_fails_closed_when_no_token_is_available(self) -> None:
        arguments = ["issue", "failure", "--repository", "sernst/skills", "--run-url", "https://example.test/run"]
        with (
            mock.patch.dict(os.environ, {}, clear=True),
            redirect_stdout(io.StringIO()),
            redirect_stderr(io.StringIO()),
        ):
            self.assertEqual(1, cli.main(arguments))

    def test_find_issue_uses_encoded_repository_scoped_search_and_exact_match(self) -> None:
        client = cli.GitHubIssues("sernst/skills", "token")
        requests = []
        # Search is already scoped, so this covers a match that a noisy first
        # page from the general repository issues endpoint would not contain.
        responses = iter([
            {
                "total_count": 3,
                "incomplete_results": False,
                "items": [
                    {"number": 99, "title": "unrelated issue", "state": "open"},
                    {"number": 12, "title": f"{cli.ISSUE_TITLE} (old)", "state": "open"},
                    {"number": 7, "title": cli.ISSUE_TITLE, "state": "open"},
                ],
            },
            {
                "total_count": 2,
                "incomplete_results": False,
                "items": [
                    {"number": 3, "title": cli.ISSUE_TITLE, "state": "closed", "pull_request": {}},
                    {"number": 8, "title": cli.ISSUE_TITLE, "state": "closed"},
                ],
            },
        ])
        client.request = lambda method, path, body=None: (requests.append((method, path, body)), next(responses))[1]  # type: ignore[method-assign]

        issue = client.find_issue()

        self.assertEqual(7, issue["number"])
        self.assertEqual(2, len(requests))
        for request, state in zip(requests, ("open", "closed"), strict=True):
            method, path, body = request
            self.assertEqual("GET", method)
            self.assertIsNone(body)
            self.assertTrue(path.startswith("/search/issues?"))
            query = urllib.parse.parse_qs(urllib.parse.urlsplit(path).query)
            self.assertEqual([f'repo:sernst/skills is:issue state:{state} in:title "{cli.ISSUE_TITLE}"'], query["q"])
            self.assertEqual(["100"], query["per_page"])
            self.assertEqual(["created"], query["sort"])
            self.assertEqual(["asc"], query["order"])

    def test_failure_creates_once_when_search_returns_no_exact_issue(self) -> None:
        client = cli.GitHubIssues("sernst/skills", "token")
        requests = []
        responses = iter([
            {"total_count": 0, "incomplete_results": False, "items": []},
            {"total_count": 0, "incomplete_results": False, "items": []},
            {"number": 11},
        ])
        client.request = lambda method, path, body=None: (requests.append((method, path, body)), next(responses))[1]  # type: ignore[method-assign]

        with redirect_stdout(io.StringIO()):
            client.record_failure("https://example.test/run/0")

        self.assertEqual(["GET", "GET", "POST"], [request[0] for request in requests])
        self.assertEqual("/repos/sernst/skills/issues", requests[2][1])
        self.assertEqual(cli.ISSUE_TITLE, requests[2][2]["title"])

    def test_failure_mentions_sernst_and_reopens_deduplicated_issue(self) -> None:
        client = cli.GitHubIssues("sernst/skills", "token")
        requests = []
        responses = iter([
            {"total_count": 0, "incomplete_results": False, "items": []},
            {"total_count": 1, "incomplete_results": False, "items": [{"number": 7, "title": cli.ISSUE_TITLE, "state": "closed"}]},
            {}, {},
        ])
        client.request = lambda method, path, body=None: (requests.append((method, path, body)), next(responses))[1]  # type: ignore[method-assign]
        with redirect_stdout(io.StringIO()):
            client.record_failure("https://example.test/run/1")
        self.assertEqual("open", requests[2][2]["state"])
        self.assertIn("@sernst", requests[3][2]["body"])
        self.assertEqual(1, sum("/comments" in request[1] for request in requests))

    def test_recovery_comments_with_provenance_and_closes(self) -> None:
        client = cli.GitHubIssues("sernst/skills", "token")
        requests = []
        responses = iter([
            {"total_count": 1, "incomplete_results": False, "items": [{"number": 7, "title": cli.ISSUE_TITLE, "state": "open"}]},
            {"total_count": 0, "incomplete_results": False, "items": []},
            {}, {},
        ])
        client.request = lambda method, path, body=None: (requests.append((method, path, body)), next(responses))[1]  # type: ignore[method-assign]
        with tempfile.TemporaryDirectory() as temporary:
            snapshot = Path(temporary) / "snapshot.md"
            snapshot.write_text("Source: DeepSWE provenance\n", encoding="utf-8")
            with redirect_stdout(io.StringIO()):
                client.record_recovery("https://example.test/run/2", snapshot, "42")
        self.assertIn("Update PR: #42", requests[2][2]["body"])
        self.assertIn("Source: DeepSWE provenance", requests[2][2]["body"])
        self.assertEqual({"state": "closed"}, requests[3][2])

    def test_closed_exact_issue_is_found_for_recovery_but_not_closed_again(self) -> None:
        client = cli.GitHubIssues("sernst/skills", "token")
        requests = []
        responses = iter([
            {"total_count": 0, "incomplete_results": False, "items": []},
            {"total_count": 1, "incomplete_results": False, "items": [{"number": 7, "title": cli.ISSUE_TITLE, "state": "closed"}]},
        ])
        client.request = lambda method, path, body=None: (requests.append((method, path, body)), next(responses))[1]  # type: ignore[method-assign]

        with redirect_stdout(io.StringIO()):
            client.record_recovery("https://example.test/run/3", None, None)

        self.assertEqual(2, len(requests))
        self.assertTrue(requests[0][1].startswith("/search/issues?"))

    def test_search_failure_exits_fail_closed_without_issue_mutation(self) -> None:
        requests = []
        def fail_request(method: str, path: str, body: object = None) -> object:
            requests.append((method, path, body))
            raise BenchmarkError("synthetic search failure")

        with (
            mock.patch.dict(os.environ, {"GITHUB_TOKEN": "token"}, clear=False),
            mock.patch.object(cli.GitHubIssues, "request", side_effect=fail_request),
            redirect_stdout(io.StringIO()),
            redirect_stderr(io.StringIO()),
        ):
            exit_code = cli.main([
                "issue", "failure", "--repository", "sernst/skills", "--run-url", "https://example.test/run/4",
            ])
        self.assertEqual(1, exit_code)
        self.assertEqual(["GET"], [request[0] for request in requests])

    def test_incomplete_search_results_fail_closed_before_issue_mutation(self) -> None:
        client = cli.GitHubIssues("sernst/skills", "token")
        requests = []
        client.request = lambda method, path, body=None: (requests.append((method, path, body)), {"total_count": 0, "items": [], "incomplete_results": True})[1]  # type: ignore[method-assign]

        with self.assertRaisesRegex(BenchmarkError, "incomplete results"):
            client.record_failure("https://example.test/run/5")

        self.assertEqual(["GET"], [request[0] for request in requests])

    def test_invalid_search_counts_fail_closed_before_issue_mutation(self) -> None:
        invalid_responses = {
            "missing": {"incomplete_results": False, "items": []},
            "boolean": {"total_count": False, "incomplete_results": False, "items": []},
            "negative": {"total_count": -1, "incomplete_results": False, "items": []},
            "non_integer": {"total_count": "0", "incomplete_results": False, "items": []},
            "inconsistent": {"total_count": 1, "incomplete_results": False, "items": []},
        }
        for name, response in invalid_responses.items():
            with self.subTest(name=name):
                client = cli.GitHubIssues("sernst/skills", "token")
                requests = []
                client.request = lambda method, path, body=None, response=response, requests=requests: (requests.append((method, path, body)), response)[1]  # type: ignore[method-assign]

                with self.assertRaisesRegex(BenchmarkError, "valid total_count|total_count did not match"):
                    client.record_failure("https://example.test/run/6")

                self.assertEqual(["GET"], [request[0] for request in requests])


if __name__ == "__main__":
    unittest.main()
