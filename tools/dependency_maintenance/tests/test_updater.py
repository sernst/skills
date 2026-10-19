import io
import json
import os
from pathlib import Path
import re
import tempfile
import unittest
from unittest.mock import patch
import urllib.error

from tools.dependency_maintenance import updater as u


class PinFixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        for name in u.ALLOWLIST:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes((u.ROOT / name).read_bytes())
        self.current = dict(re.findall(r"^([a-z-]+)=([\d.]+)$", (self.root / "TOOL_VERSIONS").read_text(), re.MULTILINE))
        self.current["python"] = (self.root / ".python-version").read_text().strip()
        self.output = patch("builtins.print")
        self.output.start()
        self.addCleanup(self.output.stop)

    def snapshot(self):
        return {name: (self.root / name).read_bytes() for name in u.ALLOWLIST}

    def payload(self, name):
        value = self.current[name]
        if name == "rust":
            return f'[pkg.rust]\nversion = "{value} (hash date)"\n[pkg.rust.target.x86_64-unknown-linux-gnu]\navailable = true\n'
        if name == "zig":
            return json.dumps({"master": {"version": "99.0.0-dev.1"}, value: {"date": "2026-01-01"}})
        if name == "python":
            return json.dumps([{"name": f"Python {value}", "is_published": True, "pre_release": False}])
        if name == "psscriptanalyzer":
            return json.dumps({"tag_name": "v" + value, "draft": False, "prerelease": False})
        return json.dumps({"crate": {"max_stable_version": value}, "versions": [{"num": value, "yanked": False}]})

    def test_noop_checks_all_sources_without_writing(self):
        before = self.snapshot()
        seen = []
        def fetch(url):
            seen.append(url)
            name = next(name for name, source in u.SOURCES.items() if source == url)
            return self.payload(name)
        self.assertEqual(u.refresh(self.root, fetcher=fetch), {})
        self.assertEqual(set(seen), set(u.SOURCES.values()))
        self.assertEqual(before, self.snapshot())

    def test_synchronizes_rust_and_canonical_python_documentation(self):
        releases = dict(self.current, rust="100.0.0", python="100.0.0")
        changes = u.plan(self.root, releases)
        self.assertEqual(set(changes), u.ALLOWLIST)
        self.assertEqual(changes[".python-version"].strip(), b"100.0.0")
        self.assertIn(b"pin Python 100.0.0 and", changes["clis/skill-manager/docs/development.md"])
        self.assertIn(b'channel = "100.0.0"', changes["clis/skill-manager/rust-toolchain.toml"])
        self.assertEqual(self.snapshot()["TOOL_VERSIONS"], (self.root / "TOOL_VERSIONS").read_bytes())

    def test_duplicate_missing_drifted_and_new_inventory_fail_before_write(self):
        for mutation in ("duplicate", "missing", "drift", "new-tool"):
            with self.subTest(mutation=mutation):
                self.setUp()
                path = self.root / ".python-version"
                if mutation == "new-tool":
                    path = self.root / "TOOL_VERSIONS"
                    path.write_text(path.read_text() + "new-tool=1.2.3\n")
                else:
                    text = path.read_text()
                    line = self.current["python"]
                    if mutation == "drift":
                        path = self.root / "clis/skill-manager/docs/development.md"
                        text = path.read_text()
                    path.write_text(text.replace(line, line + "\n" + line if mutation == "duplicate" else "" if mutation == "missing" else "1.2.3"))
                before = self.snapshot()
                with self.assertRaises(u.MaintenanceError):
                    u.refresh(self.root, fetcher=lambda url: self.payload(next(name for name, source in u.SOURCES.items() if source == url)))
                self.assertEqual(before, self.snapshot())

    def test_malformed_late_source_never_partially_writes(self):
        before = self.snapshot()
        with self.assertRaises(u.MaintenanceError):
            u.refresh(self.root, fetcher=lambda url: "{}" if url == u.SOURCES["python"] else self.payload(next(name for name, source in u.SOURCES.items() if source == url)))
        self.assertEqual(before, self.snapshot())

    def test_downgrade_rejected_even_if_other_updates_available(self):
        with self.assertRaisesRegex(u.MaintenanceError, "refusing downgrade"):
            u.plan(self.root, dict(self.current, rust="0.0.1", python="100.0.0"))


class MetadataTests(unittest.TestCase):
    def test_all_setup_python_workflows_consume_the_canonical_pin(self):
        workflows = list((u.ROOT / ".github/workflows").glob("*.yml"))
        consumers = 0
        for workflow in workflows:
            text = workflow.read_text(encoding="utf-8")
            setups = len(re.findall(r"uses: actions/setup-python@", text))
            consumers += setups
            self.assertEqual(len(re.findall(r"python-version-file: \.python-version\s*$", text, re.MULTILINE)), setups, workflow.name)
            self.assertNotRegex(text, r"(?m)^\s+python-version:")
        self.assertGreater(consumers, 0)

    def test_python_semantic_max_ignores_calendar_installer_dates_and_prerelease(self):
        data = [
            {"name": name, "is_published": published, "pre_release": pre}
            for name, published, pre in (
                ("Python 3.9.99", True, False), ("Python 3.10.2", True, False),
                ("Python 3.10.10", True, False), ("Python 4.0.0rc1", True, True),
                ("Python 4.0.0", False, False), ("Python install manager 26.4", True, False),
            )
        ]
        self.assertEqual(u.latest("python", json.dumps(data)), "3.10.10")

    def test_crates_exclude_yanked_and_prerelease_validate_max(self):
        data = {"crate": {"max_stable_version": "1.10.0"}, "versions": [
            {"num": num, "yanked": yanked} for num, yanked in (
                ("1.9.0", False), ("1.10.0", False), ("2.0.0", True), ("3.0.0-rc.1", False),
            )
        ]}
        self.assertEqual(u.latest("just", json.dumps(data)), "1.10.0")
        data["crate"]["max_stable_version"] = "2.0.0"
        with self.assertRaises(u.MaintenanceError):
            u.latest("just", json.dumps(data))

    def test_python_future_stable_major_is_not_excluded(self):
        data = [{"name": f"Python {value}", "is_published": True, "pre_release": False} for value in ("3.99.9", "4.0.0")]
        self.assertEqual(u.latest("python", json.dumps(data)), "4.0.0")

    def test_zig_numeric_order_and_master_exclusion(self):
        data = {"master": {"version": "2.0.0-dev.1"}, "0.9.0": {"date": "2026-12-01"}, "0.10.0": {"date": "2026-01-01"}, "0.11.0-rc.1": {"date": "2026-02-01"}}
        self.assertEqual(u.latest("zig", json.dumps(data)), "0.10.0")

    def test_invalid_shapes_and_stable_flags_fail(self):
        for name in u.SOURCES:
            for payload in ("{}", "[]", "garbage"):
                with self.subTest(name=name, payload=payload), self.assertRaises(u.MaintenanceError):
                    u.latest(name, payload)
        for flag in ("draft", "prerelease"):
            data = {"tag_name": "v1.2.3", "draft": False, "prerelease": False}
            data[flag] = True
            with self.assertRaises(u.MaintenanceError):
                u.latest("psscriptanalyzer", json.dumps(data))

    def test_unknown_crate_version_does_not_silently_bless_stale_maximum(self):
        data = {"crate": {"max_stable_version": "1.0.0"}, "versions": [
            {"num": "1.0.0", "yanked": False}, {"num": "unknown-release", "yanked": False},
        ]}
        with self.assertRaises(u.MaintenanceError):
            u.latest("just", json.dumps(data))

    def test_fetch_limit_timeout_retry_and_no_retry_permanent_http(self):
        class Response(io.BytesIO):
            status = 200
        attempts = []
        def flaky(request, timeout):
            attempts.append(timeout)
            if len(attempts) < 3:
                raise urllib.error.HTTPError(request.full_url, 503, "unavailable", {}, None)
            return Response(b"ok")
        waits = []
        self.assertEqual(u.fetch(u.SOURCES["python"], opener=flaky, sleeper=waits.append), "ok")
        self.assertEqual(attempts, [30, 30, 30])
        self.assertEqual(waits, [1, 2])
        def forbidden(request, timeout):
            raise urllib.error.HTTPError(request.full_url, 403, "forbidden", {}, None)
        with self.assertRaisesRegex(u.MaintenanceError, "403"):
            u.fetch(u.SOURCES["rust"], opener=forbidden, sleeper=lambda _: self.fail("retried 403"))
        with self.assertRaisesRegex(u.MaintenanceError, "4 MiB"):
            u.fetch(u.SOURCES["rust"], opener=lambda *a, **kw: Response(b"x" * (u.MAX_BYTES + 1)))
        with self.assertRaisesRegex(u.MaintenanceError, "allowlist"):
            u.fetch("https://attacker.invalid/metadata")


class PublicationTests(unittest.TestCase):
    def runner(self, *, dirty=" M TOOL_VERSIONS\0", existing=False, prs=None, independent=False):
        calls = []
        def invoke(*args, **kwargs):
            calls.append((args, kwargs))
            if args[1:3] == ("status", "--porcelain=v1"):
                return dirty
            if args[1] == "rev-parse":
                return "a" * 40
            if args[1] == "ls-remote":
                return "b" * 40 + "\trefs/heads/" + u.BRANCH if existing else ""
            if args[1] == "log":
                return "human@example.com\n" + u.BOT_EMAIL if independent else "\n".join([u.BOT_EMAIL] * 2)
            if args[1] == "diff":
                return "TOOL_VERSIONS"
            if args[:3] == ("gh", "pr", "list"):
                return json.dumps(prs or [])
            if args[:3] == ("gh", "pr", "create"):
                return "https://github.com/owner/repo/pull/1"
            return ""
        return invoke, calls

    def test_noop_never_creates_branch_or_requires_credentials(self):
        runner, calls = self.runner(dirty="")
        with patch.dict(os.environ, {}, clear=True):
            self.assertIsNone(u.publish(u.ROOT, "owner/repo", runner=runner))
        self.assertEqual(len(calls), 1)

    def test_existing_pr_refreshed_with_lease_scoped_auth_and_no_duplicate(self):
        runner, calls = self.runner(existing=True, prs=[{"number": 42}])
        with patch.dict(os.environ, {"GH_TOKEN": "secret-fixture-token"}):
            self.assertEqual(u.publish(u.ROOT, "owner/repo", runner=runner), "42")
        commands = [args for args, _ in calls]
        self.assertFalse(any(args[:3] == ("gh", "pr", "create") for args in commands))
        self.assertTrue(any(args[:3] == ("gh", "pr", "edit") for args in commands))
        push = next((args, kw) for args, kw in calls if args[:2] == ("git", "push"))
        self.assertIn(f"--force-with-lease=refs/heads/{u.BRANCH}:" + "b" * 40, push[0])
        self.assertIn("AUTHORIZATION: basic ", push[1]["env"]["GIT_CONFIG_VALUE_0"])
        self.assertNotIn("secret-fixture-token", repr(commands))
        self.assertFalse(any("merge" in args or "--approve" in args for args in commands))

    def test_outside_allowlist_and_independent_branch_edits_never_push(self):
        for kwargs in ({"dirty": " M unexpected.txt\0"}, {"dirty": " M .github/workflows/pr.yml\0"}, {"existing": True, "independent": True}, {"dirty": " D TOOL_VERSIONS\0"}):
            with self.subTest(kwargs=kwargs):
                runner, calls = self.runner(**kwargs)
                with patch.dict(os.environ, {"GH_TOKEN": "secret"}), self.assertRaises(u.MaintenanceError):
                    u.publish(u.ROOT, "owner/repo", runner=runner)
                self.assertFalse(any(args[:2] == ("git", "push") for args, _ in calls))

    def test_new_proposal_creates_exactly_one_pr(self):
        runner, calls = self.runner()
        with patch.dict(os.environ, {"GH_TOKEN": "fixture-token"}):
            self.assertEqual(u.publish(u.ROOT, "owner/repo", runner=runner), "https://github.com/owner/repo/pull/1")
        self.assertEqual(sum(args[:3] == ("gh", "pr", "create") for args, _ in calls), 1)

    def test_stale_main_never_creates_branch_or_pushes(self):
        runner, calls = self.runner()
        def stale(*args, **kwargs):
            if args[:3] == ("git", "rev-parse", "origin/main"):
                return "new-main-sha"
            return runner(*args, **kwargs)
        with patch.dict(os.environ, {"GH_TOKEN": "fixture-token"}), self.assertRaisesRegex(u.MaintenanceError, "Main advanced"):
            u.publish(u.ROOT, "owner/repo", runner=stale)
        self.assertFalse(any(args[:2] in (("git", "switch"), ("git", "push")) for args, _ in calls))

    def test_failure_updates_dependency_issue_without_comment_spam(self):
        calls = []
        def runner(*args, **kwargs):
            calls.append(args)
            if args[:3] == ("gh", "issue", "list"):
                return json.dumps([{"number": 10, "title": u.ISSUE_TITLE, "state": "OPEN"}])
            return ""
        u.issue("failure", "owner/repo", "https://github.com/owner/repo/actions/runs/1", runner=runner)
        self.assertTrue(any(args[:3] == ("gh", "issue", "edit") for args in calls))
        self.assertFalse(any(args[:3] in (("gh", "issue", "create"), ("gh", "issue", "comment")) for args in calls))


if __name__ == "__main__":
    unittest.main()
