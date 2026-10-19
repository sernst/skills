"""Bounded official metadata reads, exact pin edits, and proposal publication.

Only metadata is downloaded. Package installation and compatibility checks belong
to the normal PR quality gate, with its advisory policy unchanged.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.request


class MaintenanceError(Exception):
    """An actionable fail-closed maintenance error."""


ROOT = Path(__file__).resolve().parents[2]
BRANCH = "codex/dependency-maintenance-updates"
TITLE = "chore: update pinned build tools and runtimes"
ISSUE_TITLE = "[automation] Dependency maintenance failed"
BOT_EMAIL = "dependency-updater[bot]@users.noreply.github.com"
MAX_BYTES = 4 * 1024 * 1024
VERSION = r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)"
SOURCES = {
    "rust": "https://static.rust-lang.org/dist/channel-rust-stable.toml",
    "just": "https://crates.io/api/v1/crates/just",
    "psscriptanalyzer": "https://api.github.com/repos/PowerShell/PSScriptAnalyzer/releases/latest",
    "cargo-llvm-cov": "https://crates.io/api/v1/crates/cargo-llvm-cov",
    "cargo-deny": "https://crates.io/api/v1/crates/cargo-deny",
    "cargo-zigbuild": "https://crates.io/api/v1/crates/cargo-zigbuild",
    "zig": "https://ziglang.org/download/index.json",
    "workflow-audit": "https://crates.io/api/v1/crates/zizmor",
    "python": "https://www.python.org/api/v2/downloads/release/?is_published=true&pre_release=false",
}
ALLOWLIST = frozenset((
    "TOOL_VERSIONS", "clis/skill-manager/rust-toolchain.toml",
    "clis/skill-manager/docs/development.md", ".python-version",
))


def version(value: object) -> tuple[int, int, int]:
    if not isinstance(value, str) or re.fullmatch(VERSION, value) is None:
        raise MaintenanceError("Expected an exact stable three-part version.")
    return tuple(map(int, value.split(".")))


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise MaintenanceError("Metadata redirect refused; review the official source URL.")


def fetch(url: str, *, opener=None, sleeper=time.sleep) -> str:
    if url not in SOURCES.values():
        raise MaintenanceError("Metadata URL is outside the reviewed source allowlist.")
    opener = opener or urllib.request.build_opener(NoRedirect()).open
    request = urllib.request.Request(url, headers={"User-Agent": "sernst-skills-dependency-updater/1"})
    for attempt in range(3):
        try:
            with opener(request, timeout=30) as response:
                if response.status != 200:
                    raise MaintenanceError(f"Metadata returned HTTP {response.status}.")
                data = response.read(MAX_BYTES + 1)
                if len(data) > MAX_BYTES:
                    raise MaintenanceError("Metadata exceeded the 4 MiB limit.")
                return data.decode("utf-8")
        except urllib.error.HTTPError as exc:
            if exc.code not in (408, 429, 500, 502, 503, 504) or attempt == 2:
                raise MaintenanceError(f"Metadata fetch failed with HTTP {exc.code}.") from None
        except (OSError, UnicodeError):
            if attempt == 2:
                raise MaintenanceError("Metadata fetch failed after three bounded attempts.") from None
        if attempt < 2:
            sleeper(2 ** attempt)
    raise AssertionError("unreachable")


def latest(name: str, content: str) -> str:
    """Use registry stable flags and numeric ordering, never dates or lexicography."""
    try:
        if name == "rust":
            package = tomllib.loads(content)["pkg"]["rust"]
            if package["target"]["x86_64-unknown-linux-gnu"]["available"] is not True:
                raise ValueError("stable Rust package unavailable")
            selected = package["version"].split(" ")[0]
        else:
            data = json.loads(content)
            if name == "python":
                if not isinstance(data, list) or not data:
                    raise ValueError("expected complete release list")
                candidates = []
                for item in data:
                    if not isinstance(item, dict) or not isinstance(item.get("name"), str):
                        raise ValueError("malformed Python release")
                    if not isinstance(item.get("is_published"), bool) or not isinstance(item.get("pre_release"), bool):
                        raise ValueError("missing Python release flags")
                    if item["is_published"] and not item["pre_release"] and not item["name"].startswith("Python install manager "):
                        match = re.fullmatch(r"Python (" + VERSION + r")", item["name"])
                        if match is None:
                            raise ValueError("stable Python release name malformed")
                        candidates.append(match[1])
                selected = max(candidates, key=version)
            elif name == "zig":
                if not isinstance(data, dict) or not isinstance(data.get("master"), dict):
                    raise ValueError("missing Zig master sentinel")
                candidates = []
                for tag, item in data.items():
                    if tag == "master":
                        continue
                    if not isinstance(item, dict) or "date" not in item:
                        raise ValueError("malformed Zig release")
                    if re.fullmatch(VERSION, tag):
                        candidates.append(tag)
                    elif re.fullmatch(VERSION + r"-(?:dev|alpha|beta|rc)[.\d-]*", tag) is None:
                        raise ValueError("unknown Zig release key")
                selected = max(candidates, key=version)
            elif name == "psscriptanalyzer":
                if data["draft"] is not False or data["prerelease"] is not False:
                    raise ValueError("GitHub latest release is not stable")
                selected = data["tag_name"].removeprefix("v")
            else:
                # Inspect the complete versions list as well as max_stable_version:
                # a yanked latest release must never be proposed.
                selected = data["crate"]["max_stable_version"]
                releases = data["versions"]
                if not isinstance(releases, list) or not releases:
                    raise ValueError("missing crate versions")
                candidates = []
                for item in releases:
                    if not isinstance(item.get("num"), str) or not isinstance(item.get("yanked"), bool):
                        raise ValueError("malformed crate version")
                    if re.fullmatch(VERSION + r"(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", item["num"]) is None:
                        raise ValueError("unknown crate version syntax")
                    if re.fullmatch(VERSION, item["num"]) and not item["yanked"]:
                        candidates.append(item["num"])
                if selected != max(candidates, key=version):
                    raise ValueError("registry stable maximum disagrees with non-yanked versions")
        version(selected)
        return selected
    except (KeyError, TypeError, ValueError, AttributeError, MaintenanceError) as exc:
        raise MaintenanceError(f"{name}: malformed or empty official release metadata; review {SOURCES[name]}.") from exc


def replace_pin(text: str, pattern: str, old: str, new: str, path: str) -> str:
    matches = list(re.finditer(pattern, text, re.MULTILINE))
    if len(matches) != 1 or matches[0][1] != old:
        raise MaintenanceError(f"{path}: expected one synchronized pin; review missing, duplicate, or drifted pins.")
    match = matches[0]
    return text[:match.start(1)] + new + text[match.end(1):]


def plan(root: Path, releases: dict[str, str]) -> dict[str, bytes]:
    if set(releases) != set(SOURCES):
        raise MaintenanceError("Release inventory must cover every reviewed tool and runtime.")
    original = {path: (root / path).read_bytes() for path in ALLOWLIST}
    texts = {path: data.decode("utf-8") for path, data in original.items()}
    pins = dict(re.findall(r"^([a-z-]+)=(" + VERSION + r")\r?$", texts["TOOL_VERSIONS"], re.MULTILINE))
    lines = [line for line in texts["TOOL_VERSIONS"].splitlines() if line and not line.startswith("#")]
    if set(pins) != set(SOURCES) - {"python"} or len(lines) != len(pins):
        raise MaintenanceError("TOOL_VERSIONS inventory changed; update the maintenance source map first.")
    python_match = re.fullmatch(r"(" + VERSION + r")\r?\n?", texts[".python-version"])
    if python_match is None:
        raise MaintenanceError(".python-version must contain exactly one stable version line.")
    pins["python"] = python_match[1]
    for name, old in pins.items():
        new = releases[name]
        if version(new) < version(old):
            raise MaintenanceError(f"{name}: upstream stable {new} is older than pin {old}; refusing downgrade.")
        if name != "python":
            texts["TOOL_VERSIONS"] = replace_pin(texts["TOOL_VERSIONS"], rf"^{name}=({VERSION})\r?$", old, new, "TOOL_VERSIONS")
    path = "clis/skill-manager/rust-toolchain.toml"
    texts[path] = replace_pin(texts[path], r'^channel = "(' + VERSION + r')"\r?$', pins["rust"], releases["rust"], path)
    texts[".python-version"] = replace_pin(texts[".python-version"], r"^(" + VERSION + r")\r?$", pins["python"], releases["python"], ".python-version")
    path = "clis/skill-manager/docs/development.md"
    texts[path] = replace_pin(texts[path], r"pin Python (" + VERSION + r") and", pins["python"], releases["python"], path)
    return {path: text.encode("utf-8") for path, text in texts.items() if text.encode("utf-8") != original[path]}


def refresh(root: Path, *, fetcher=fetch, write: bool = True) -> dict[str, bytes]:
    releases = {}
    for name, url in SOURCES.items():
        print(f"Checking {name}: {url}")
        releases[name] = latest(name, fetcher(url))
    updates = plan(root, releases)  # Validate every source and every pin before writing.
    if write:
        for path, content in updates.items():
            (root / path).write_bytes(content)
    print(f"{len(updates)} allowlisted files {'updated' if write else 'would change'}.")
    return updates


def run(*args: str, root: Path = ROOT, env=None) -> str:
    result = subprocess.run(args, cwd=root, env=env, check=False, capture_output=True, text=True)
    if result.returncode:
        # Do not echo subprocess output: authentication diagnostics may contain secrets.
        raise MaintenanceError(f"{args[0]} {args[1]} failed (exit {result.returncode}); inspect repository permissions or concurrent branch changes.")
    return result.stdout.rstrip("\r\n")


def changed_files(root: Path, runner=run) -> list[str]:
    raw = runner("git", "status", "--porcelain=v1", "-z", "--untracked-files=all", root=root)
    files = []
    for entry in raw.split("\0"):
        if not entry:
            continue
        if entry[:2] not in (" M", "M ", "??"):
            raise MaintenanceError("Unexpected staged, renamed, conflicted, or deleted files; refusing publication.")
        files.append(entry[3:])
    if not set(files) <= ALLOWLIST:
        raise MaintenanceError("Changed files outside the dependency pin allowlist; refusing publication.")
    return sorted(files)


def publish(root: Path, repository: str, *, runner=run) -> str | None:
    files = changed_files(root, runner)
    if not files:
        print("Pins are current; no branch or pull request created.")
        return None
    if re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository) is None:
        raise MaintenanceError("Repository must be OWNER/REPOSITORY.")
    token = os.environ.get("GH_TOKEN", "")
    if not token:
        raise MaintenanceError("GH_TOKEN App token is required to publish proposals.")
    runner("git", "fetch", "origin", "main", root=root)
    if runner("git", "rev-parse", "HEAD", root=root) != runner("git", "rev-parse", "origin/main", root=root):
        raise MaintenanceError("Main advanced or checkout is not main; rerun from the current main revision.")
    remote = runner("git", "ls-remote", "--heads", "origin", f"refs/heads/{BRANCH}", root=root)
    expected = remote.split()[0] if remote else ""
    if expected:
        runner("git", "fetch", "origin", f"refs/heads/{BRANCH}", root=root)
        authors = runner("git", "log", "--format=%ae", "origin/main..FETCH_HEAD", root=root).splitlines()
        paths = runner("git", "diff", "--name-only", "origin/main...FETCH_HEAD", root=root).splitlines()
        if not authors or set(authors) != {BOT_EMAIL} or not set(paths) <= ALLOWLIST:
            raise MaintenanceError("Owned proposal branch contains independent edits; resolve it manually before refreshing.")
    runner("git", "switch", "--force-create", BRANCH, root=root)
    runner("git", "add", "--", *files, root=root)
    runner("git", "-c", "user.name=dependency-updater[bot]", "-c", f"user.email={BOT_EMAIL}", "commit", "-m", TITLE, root=root)
    # Ephemeral environment-only HTTP credential; never put a token in a remote URL,
    # command argument, persisted git configuration, PR body, or log.
    env = dict(os.environ)
    credential = base64.b64encode(f"x-access-token:{token}".encode()).decode()
    env.update({"GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "http.https://github.com/.extraheader", "GIT_CONFIG_VALUE_0": f"AUTHORIZATION: basic {credential}"})
    runner("git", "push", f"--force-with-lease=refs/heads/{BRANCH}:{expected}", "origin", f"HEAD:refs/heads/{BRANCH}", root=root, env=env)
    prs = json.loads(runner("gh", "pr", "list", "--repo", repository, "--state", "open", "--head", BRANCH, "--base", "main", "--json", "number", root=root))
    body = "Automated proposal for the latest stable repository-owned build tools and runtimes.\n\nChanged pins: " + ", ".join(f"`{path}`" for path in files) + ".\n\nNormal PR checks and human review are required. This workflow never approves or merges. Cargo and Actions updates are proposed separately by Dependabot."
    if len(prs) > 1:
        raise MaintenanceError("Multiple open maintenance PRs found; resolve duplicates manually.")
    if prs:
        number = str(prs[0]["number"])
        runner("gh", "pr", "edit", number, "--repo", repository, "--title", TITLE, "--body", body, root=root)
        return number
    return runner("gh", "pr", "create", "--repo", repository, "--base", "main", "--head", BRANCH, "--title", TITLE, "--body", body, root=root)


def issue(state: str, repository: str, run_url: str, *, runner=run) -> None:
    if re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository) is None:
        raise MaintenanceError("Repository must be OWNER/REPOSITORY.")
    matches = json.loads(runner("gh", "issue", "list", "--repo", repository, "--state", "all", "--search", f'"{ISSUE_TITLE}" in:title', "--limit", "100", "--json", "number,title,state"))
    if not isinstance(matches, list) or len(matches) >= 100:
        raise MaintenanceError("Failure-issue search was malformed or reached its limit; resolve issue deduplication manually.")
    exact = [item for item in matches if item["title"] == ISSUE_TITLE]
    existing = min(exact, key=lambda item: item["number"]) if exact else None
    body = f"Dependency maintenance {state}.\n\nRun: {run_url}\n\nInspect the first failed step for the source, pin drift, permission, or concurrent branch diagnostic. No automatic approval or merge occurs; advisory failures remain blocking."
    if state == "failure":
        if existing:
            if existing["state"] == "CLOSED":
                runner("gh", "issue", "reopen", str(existing["number"]), "--repo", repository)
            runner("gh", "issue", "edit", str(existing["number"]), "--repo", repository, "--body", body)
        else:
            runner("gh", "issue", "create", "--repo", repository, "--title", ISSUE_TITLE, "--body", body)
    elif existing and existing["state"] == "OPEN":
        runner("gh", "issue", "close", str(existing["number"]), "--repo", repository, "--comment", body)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("refresh", "check", "diff", "publish", "failure", "recovery"))
    parser.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY", ""))
    parser.add_argument("--run-url", default="")
    args = parser.parse_args(argv)
    try:
        if args.command in ("refresh", "check"):
            refresh(ROOT, write=args.command == "refresh")
        elif args.command == "diff":
            changed = bool(changed_files(ROOT))
            print(f"changed={str(changed).lower()}")
            if os.environ.get("GITHUB_OUTPUT"):
                with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
                    output.write(f"changed={str(changed).lower()}\n")
        elif args.command == "publish":
            print(publish(ROOT, args.repository) or "No proposal needed.")
        else:
            issue(args.command, args.repository, args.run_url)
        return 0
    except (MaintenanceError, OSError, ValueError) as exc:
        print(f"Error: dependency maintenance failed: {exc}", file=sys.stderr)
        return 1
