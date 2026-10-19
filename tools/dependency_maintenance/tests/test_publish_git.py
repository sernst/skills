"""Proposal lifecycle regressions with actual Git history and a local bare origin."""

import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools.dependency_maintenance import updater as u


class GitPublicationTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name) / "checkout"
        self.origin = Path(temporary.name) / "origin.git"
        self.root.mkdir()
        # Ignore machine Git configuration, signing, and inherited identities.
        env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                   GIT_TERMINAL_PROMPT="0", GH_TOKEN="local-fixture-token")
        self.environment = patch.dict(os.environ, env, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.git("init", "--bare", str(self.origin))
        self.git("init", "--initial-branch=main")
        self.git("config", "user.name", "Maintainer")
        self.git("config", "user.email", "maintainer@example.com")
        self.git("config", "core.autocrlf", "false")
        self.git("config", "commit.gpgsign", "false")
        self.write_pins("1.0.0")
        (self.root / "README.md").write_text("fixture\n", encoding="utf-8")
        self.git("add", ".")
        self.git("commit", "-m", "initial main")
        self.git("remote", "add", "origin", str(self.origin))
        self.git("push", "--set-upstream", "origin", "main")
        self.calls = []
        self.open_pr = False

    def git(self, *args):
        return u.run("git", *args, root=self.root)

    def write_pins(self, value):
        (self.root / "TOOL_VERSIONS").write_text(f"just={value}\n", encoding="utf-8")

    def runner(self, *args, **kwargs):
        self.calls.append(args)
        if args[:3] == ("gh", "pr", "list"):
            return json.dumps([{"number": 1}] if self.open_pr else [])
        if args[:3] == ("gh", "pr", "create"):
            self.open_pr = True
            return "https://github.com/fixture/repo/pull/1"
        if args[:3] == ("gh", "pr", "edit"):
            return ""
        return u.run(*args, **kwargs)

    def publish(self, value="2.0.0", date="2001-01-01T00:00:00+00:00"):
        self.write_pins(value)
        with patch.dict(os.environ, {"GIT_AUTHOR_DATE": date, "GIT_COMMITTER_DATE": date}):
            return u.publish(self.root, "fixture/repo", runner=self.runner)

    def remote_head(self):
        return self.git("ls-remote", "--heads", "origin", u.BRANCH).split()[0]

    def pushes(self):
        return sum(args[:2] == ("git", "push") for args in self.calls)

    def assert_amend_preserved(self, *, human_author):
        self.publish()
        with (self.root / "TOOL_VERSIONS").open("a", encoding="utf-8") as output:
            output.write("# independent maintainer edit\n")
        self.git("add", "TOOL_VERSIONS")
        if human_author:
            self.git("-c", f"user.email={u.BOT_EMAIL}", "commit", "--amend", "--no-edit",
                     "--author=Maintainer <maintainer@example.com>")
        else:
            # Ordinary amend preserves bot author and uses human committer.
            self.git("commit", "--amend", "--no-edit")
        identities = self.git("log", "-1", "--format=%ae%n%ce").splitlines()
        self.assertEqual(identities, ["maintainer@example.com", u.BOT_EMAIL] if human_author
                         else [u.BOT_EMAIL, "maintainer@example.com"])
        self.git("push", "--force", "origin", u.BRANCH)
        amended = self.remote_head()
        self.git("switch", "main")
        with self.assertRaisesRegex(u.MaintenanceError, "independent edits"):
            self.publish("3.0.0")
        self.assertEqual(self.remote_head(), amended)
        self.assertIn("independent maintainer edit", self.git("show", f"{amended}:TOOL_VERSIONS"))
        self.assertEqual(self.pushes(), 1)

    def test_human_committer_amend_is_preserved(self):
        self.assert_amend_preserved(human_author=False)

    def test_human_author_amend_is_preserved(self):
        self.assert_amend_preserved(human_author=True)

    def assert_merged_branch_reused(self, *, no_ff):
        self.publish()
        merged = self.remote_head()
        self.git("switch", "main")
        self.git("merge", "--no-ff" if no_ff else "--ff-only", u.BRANCH, "-m", "merge proposal")
        self.git("push", "origin", "main")
        main = self.git("rev-parse", "HEAD")
        self.assertEqual(self.remote_head(), merged)  # Hosted branch was retained.
        self.open_pr = False  # Previous proposal was merged.
        self.publish("3.0.0")
        proposed = self.remote_head()
        self.assertNotEqual(proposed, merged)
        self.assertEqual(self.git("rev-parse", f"{proposed}^"), main)
        self.assertEqual(self.git("show", f"{proposed}:TOOL_VERSIONS"), "just=3.0.0")
        self.assertEqual(self.pushes(), 2)

    def test_retained_fast_forward_merged_branch_reused(self):
        self.assert_merged_branch_reused(no_ff=False)

    def test_retained_merge_commit_branch_reused(self):
        self.assert_merged_branch_reused(no_ff=True)

    def assert_identical_proposal_reused(self, *, missing_pr):
        self.publish()
        proposed = self.remote_head()
        self.git("switch", "main")
        main = self.git("rev-parse", "HEAD")
        self.open_pr = not missing_pr
        self.calls.clear()
        result = self.publish(date="2002-01-01T00:00:00+00:00")
        self.assertEqual(self.remote_head(), proposed)
        self.assertEqual(self.git("rev-parse", "HEAD"), main)
        self.assertEqual(self.pushes(), 0)
        self.assertFalse(any("commit" in args for args in self.calls))
        operation = "create" if missing_pr else "edit"
        self.assertEqual(sum(args[:3] == ("gh", "pr", operation) for args in self.calls), 1)
        self.assertEqual(result, "https://github.com/fixture/repo/pull/1" if missing_pr else "1")

    def test_identical_proposal_keeps_head_and_updates_existing_pr(self):
        self.assert_identical_proposal_reused(missing_pr=False)

    def test_identical_proposal_repairs_missing_pr_without_push(self):
        self.assert_identical_proposal_reused(missing_pr=True)

    def test_new_main_refreshes_proposal_even_when_tree_is_identical(self):
        self.publish()
        proposed = self.remote_head()
        self.git("switch", "main")
        # Main advanced and returned to the same tree. A new proposal must still
        # start from this current base rather than keep an obsolete parent.
        self.git("commit", "--allow-empty", "-m", "main advanced")
        self.git("push", "origin", "main")
        main = self.git("rev-parse", "HEAD")
        self.publish(date="2002-01-01T00:00:00+00:00")
        refreshed = self.remote_head()
        self.assertNotEqual(refreshed, proposed)
        self.assertEqual(self.git("rev-parse", f"{refreshed}^"), main)
        self.assertEqual(self.git("rev-parse", f"{refreshed}^{{tree}}"),
                         self.git("rev-parse", f"{proposed}^{{tree}}"))
        self.assertEqual(self.pushes(), 2)


if __name__ == "__main__":
    unittest.main()
