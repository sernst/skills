"""Shell-neutral command-line interface for model benchmark maintenance."""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Mapping, Sequence
from datetime import datetime
from pathlib import Path
from typing import Any

from .core import BenchmarkError, update_benchmarks

PACKAGE_ROOT = Path(__file__).resolve().parent
REPOSITORY_ROOT = PACKAGE_ROOT.parents[1]
DEFAULT_REGISTRY = PACKAGE_ROOT / "sources.json"
DEFAULT_OUTPUT = REPOSITORY_ROOT / "skills/running-as-maestro/references/benchmark-snapshot.md"
ISSUE_TITLE = "[automation] Model benchmark refresh failed"


def issue_token(environ: Mapping[str, str] | None = None) -> str:
    """Return the standard Actions token, with ``gh`` CLI compatibility.

    GitHub Actions conventionally exposes ``GITHUB_TOKEN``.  The GitHub CLI
    also recognizes ``GH_TOKEN``, so accepting it as a fallback keeps the
    issue lifecycle shell-neutral without allowing an alternate token to
    override an explicitly supplied standard token.
    """
    values = os.environ if environ is None else environ
    return values.get("GITHUB_TOKEN") or values.get("GH_TOKEN") or ""


def _timestamp(value: str) -> datetime:
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise argparse.ArgumentTypeError("must be an ISO-8601 timestamp") from exc
    if parsed.tzinfo is None:
        raise argparse.ArgumentTypeError("must include a timezone")
    return parsed


def _add_update_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--registry", type=Path, default=DEFAULT_REGISTRY, help="source registry JSON")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT, help="generated snapshot path")
    parser.add_argument("--fixture-root", type=Path, help="read source fixtures instead of the network")
    parser.add_argument("--retrieved-at", type=_timestamp, help="fixed provenance timestamp (tests only)")


class GitHubIssues:
    def __init__(self, repository: str, token: str, api_url: str = "https://api.github.com") -> None:
        if not repository or "/" not in repository:
            raise BenchmarkError("repository must be OWNER/REPOSITORY.")
        if not token:
            raise BenchmarkError("GITHUB_TOKEN is required for issue lifecycle commands.")
        self.repository = repository
        self.token = token
        self.api_url = api_url.rstrip("/")

    def request(self, method: str, path: str, body: Mapping[str, Any] | None = None) -> Any:
        data = json.dumps(body).encode("utf-8") if body is not None else None
        request = urllib.request.Request(
            self.api_url + path,
            data=data,
            method=method,
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self.token}",
                "User-Agent": "sernst-skills-model-benchmark-updater/1",
                "X-GitHub-Api-Version": "2022-11-28",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                content = response.read()
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")[:500]
            raise BenchmarkError(f"GitHub API {method} {path} failed with HTTP {exc.code}: {detail}") from None
        except OSError as exc:
            raise BenchmarkError(f"GitHub API {method} {path} failed: {exc}") from None
        return json.loads(content) if content else None

    def find_issue(self) -> Mapping[str, Any] | None:
        # Search is deliberately title- and repository-scoped.  Listing issues only
        # inspects one page, so an older deduplication issue can disappear behind
        # unrelated repository activity.  GitHub search has distinct open and
        # closed state qualifiers, so query both explicitly; is:issue keeps pull
        # requests out server-side.
        matches: list[Mapping[str, Any]] = []
        for state in ("open", "closed"):
            query = urllib.parse.urlencode(
                {
                    "q": f'repo:{self.repository} is:issue state:{state} in:title "{ISSUE_TITLE}"',
                    "per_page": "100",
                    "sort": "created",
                    "order": "asc",
                }
            )
            response = self.request("GET", f"/search/issues?{query}")
            if not isinstance(response, Mapping):
                raise BenchmarkError("GitHub issue search response was not an object.")
            total_count = response.get("total_count")
            if (
                "total_count" not in response
                or not isinstance(total_count, int)
                or isinstance(total_count, bool)
                or total_count < 0
            ):
                raise BenchmarkError("GitHub issue search response did not contain a valid total_count.")
            if not isinstance(response.get("items"), list):
                raise BenchmarkError("GitHub issue search response did not contain an items array.")
            if response.get("incomplete_results") is not False:
                raise BenchmarkError("GitHub issue search returned incomplete results.")
            if total_count != len(response["items"]):
                raise BenchmarkError("GitHub issue search total_count did not match returned items.")
            matches.extend(
                issue
                for issue in response["items"]
                if isinstance(issue, Mapping)
                and issue.get("title") == ISSUE_TITLE
                and "pull_request" not in issue
                and isinstance(issue.get("number"), int)
                and not isinstance(issue.get("number"), bool)
            )
        # Exact-title duplicates should not exist, but choosing the lowest issue
        # number makes recovery deterministic and prevents a new duplicate.
        return min(matches, key=lambda issue: issue["number"]) if matches else None

    def record_failure(self, run_url: str) -> None:
        issue = self.find_issue()
        body = f"@sernst the model benchmark refresh failed closed. The last-known-good snapshot was retained. Inspect: {run_url}"
        base = f"/repos/{self.repository}"
        if issue is None:
            created = self.request("POST", f"{base}/issues", {"title": ISSUE_TITLE, "body": body})
            print(f"Opened refresh failure issue #{created['number']}.")
            return
        number = issue["number"]
        if issue.get("state") == "closed":
            self.request("PATCH", f"{base}/issues/{number}", {"state": "open"})
            print(f"Reopened refresh failure issue #{number}.")
        self.request("POST", f"{base}/issues/{number}/comments", {"body": body})
        print(f"Recorded refresh failure on issue #{number}.")

    def record_recovery(self, run_url: str, snapshot_path: Path | None, pull_request: str | None) -> None:
        issue = self.find_issue()
        if issue is None or issue.get("state") == "closed":
            print("No open refresh failure issue; no recovery action needed.")
            return
        provenance = "Snapshot provenance unavailable in this run."
        if snapshot_path is not None and snapshot_path.is_file():
            lines = [line for line in snapshot_path.read_text(encoding="utf-8").splitlines() if line.startswith("Source:")]
            if lines:
                provenance = "\n".join(lines)
        pr_text = f"Update PR: #{pull_request}" if pull_request else "No update PR was needed."
        body = f"Refresh recovered. {pr_text}\n\n{provenance}\n\nRun: {run_url}"
        number = issue["number"]
        base = f"/repos/{self.repository}/issues/{number}"
        self.request("POST", f"{base}/comments", {"body": body})
        self.request("PATCH", base, {"state": "closed"})
        print(f"Recorded recovery and closed issue #{number}.")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m tools.model_benchmarks",
        description="Validate and maintain the generated maestro model-benchmark snapshot.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)
    refresh = subparsers.add_parser("refresh", help="fetch, validate, and atomically update the snapshot")
    _add_update_arguments(refresh)
    check = subparsers.add_parser("check", help="check whether a validated update is available without writing")
    _add_update_arguments(check)
    issue = subparsers.add_parser("issue", help="maintain the deduplicated refresh failure issue")
    issue_subparsers = issue.add_subparsers(dest="issue_state", required=True)
    for state in ("failure", "recovery"):
        state_parser = issue_subparsers.add_parser(state, help=f"record refresh {state}")
        state_parser.add_argument("--repository", required=True, help="GitHub OWNER/REPOSITORY")
        state_parser.add_argument("--run-url", required=True, help="workflow run URL")
        state_parser.add_argument("--api-url", default=os.environ.get("GITHUB_API_URL", "https://api.github.com"), help=argparse.SUPPRESS)
        if state == "recovery":
            state_parser.add_argument("--pull-request", help="update pull-request number")
            state_parser.add_argument("--snapshot", type=Path, default=DEFAULT_OUTPUT, help="snapshot used for recovery provenance")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        if args.command in ("refresh", "check"):
            result = update_benchmarks(
                args.registry, args.output, fixture_root=args.fixture_root,
                retrieved_at=args.retrieved_at, check=args.command == "check",
            )
            return 2 if args.command == "check" and result.changed else 0
        issues = GitHubIssues(args.repository, issue_token(), args.api_url)
        if args.issue_state == "failure":
            issues.record_failure(args.run_url)
        else:
            issues.record_recovery(args.run_url, args.snapshot, args.pull_request)
        return 0
    except (BenchmarkError, OSError) as exc:
        print(f"Error: benchmark operation failed without modifying the snapshot: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
