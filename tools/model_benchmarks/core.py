"""Validation, normalization, fetching, and rendering for benchmark snapshots.

This module intentionally uses only the Python standard library. External
benchmark content is untrusted and must pass a source-specific positive
contract before it is rendered into agent context.
"""

from __future__ import annotations

import hashlib
import html
import json
import os
import re
import tempfile
import time
import urllib.error
import urllib.request
from collections.abc import Callable, Iterable, Mapping, MutableMapping, Sequence
from dataclasses import dataclass
from datetime import date, datetime, timezone
from decimal import Decimal, InvalidOperation
from html.parser import HTMLParser
from pathlib import Path
from typing import Any

PARSER_VERSION = 5
_CONTROL_RE = re.compile(r"[\x00-\x1f\x7f]")
_URI_RE = re.compile(r"(?i)(?:[a-z][a-z0-9+.-]*:)?//")
_HASH_RE = re.compile(r"Normalized SHA-256: `([a-f0-9]{64})`")
_IDENTIFIER_RE = {
    "effort": re.compile(r"^[A-Za-z][A-Za-z0-9]*(?:[ -][A-Za-z0-9]+)?$"),
    "harness": re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$"),
    "config": re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+:/-]*$"),
    "version": re.compile(r"^[0-9]+(?:\.[0-9]+){1,3}$"),
}
_IDENTIFIER_LIMIT = {"effort": 24, "harness": 64, "config": 150, "version": 30}


class BenchmarkError(RuntimeError):
    """A fail-closed benchmark validation or update error."""


@dataclass
class Row:
    model: str
    effort: str
    harness: str
    config: str
    score: Decimal
    cost: Decimal
    ci_low: Decimal | None = None
    ci_high: Decimal | None = None
    sample_count: int | None = None
    run_count: int | None = None
    pareto: bool = False


@dataclass
class Benchmark:
    id: str
    display_name: str
    version: str
    published_at: str
    task_count: int | None
    score_label: str
    rows: list[Row]


@dataclass(frozen=True)
class Snapshot:
    content: str
    semantic_hash: str


@dataclass(frozen=True)
class UpdateResult:
    changed: bool
    rows: int
    bytes: int
    hash: str


def trusted_scalar(value: Any, field: str, maximum_length: int = 160, *, allow_empty: bool = False) -> str:
    if not isinstance(value, str):
        raise BenchmarkError(f"{field} must be a string.")
    if not allow_empty and not value.strip():
        raise BenchmarkError(f"{field} must not be empty.")
    if len(value) > maximum_length:
        raise BenchmarkError(f"{field} exceeds {maximum_length} characters.")
    if _CONTROL_RE.search(value):
        raise BenchmarkError(f"{field} contains a control character.")
    return value


def identifier(value: Any, field: str, kind: str, allowed_values: Sequence[str] | None = None) -> str:
    if kind not in _IDENTIFIER_RE:
        raise ValueError(f"unsupported identifier kind: {kind}")
    text = trusted_scalar(value, field, _IDENTIFIER_LIMIT[kind])
    if text != text.strip():
        raise BenchmarkError(f"{field} has leading or trailing whitespace.")
    if _URI_RE.search(text):
        raise BenchmarkError(f"{field} contains a URI-like value.")
    if allowed_values is not None and text not in allowed_values:
        raise BenchmarkError(f"{field} is not an allowlisted {kind} value.")
    if not _IDENTIFIER_RE[kind].fullmatch(text):
        raise BenchmarkError(f"{field} does not match the {kind} identifier grammar.")
    return text


def source_model(value: Any, field: str, source: Mapping[str, Any]) -> str:
    text = trusted_scalar(value, field, 100)
    if text != text.strip():
        raise BenchmarkError(f"{field} has leading or trailing whitespace.")
    if _URI_RE.search(text):
        raise BenchmarkError(f"{field} contains a URI-like value.")
    for pattern in source["modelPatterns"]:
        if re.fullmatch(pattern, text):
            return text
    raise BenchmarkError(f"{field} is not an allowlisted model family for source {source['id']}.")


def bounded_decimal(value: Any, field: str, minimum: Decimal | int, maximum: Decimal | int) -> Decimal:
    if isinstance(value, bool) or not isinstance(value, (int, float, str, Decimal)):
        raise BenchmarkError(f"{field} must be numeric.")
    try:
        number = Decimal(str(value))
    except (InvalidOperation, ValueError):
        raise BenchmarkError(f"{field} must be numeric.") from None
    if not number.is_finite():
        raise BenchmarkError(f"{field} must be finite.")
    if number < Decimal(minimum) or number > Decimal(maximum):
        raise BenchmarkError(f"{field} must be between {minimum} and {maximum}.")
    return number


def bounded_integer(value: Any, field: str, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise BenchmarkError(f"{field} must be an integer.")
    if value < minimum or value > maximum:
        raise BenchmarkError(f"{field} must be between {minimum} and {maximum}.")
    return value


def markdown_scalar(value: str) -> str:
    escaped = (
        value.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
        .replace("'", "&#39;")
        .replace(":", "&#58;")
        .replace("/", "&#47;")
    )
    for character in ("\\", "|", "`", "*", "_", "[", "]", "(", ")", "!"):
        escaped = escaped.replace(character, "\\" + character)
    return escaped


def sha256(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def _require_mapping(value: Any, field: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise BenchmarkError(f"{field} must be an object.")
    return value


def _require_fields(value: Mapping[str, Any], fields: Iterable[str], context: str) -> None:
    for field in fields:
        if field not in value:
            raise BenchmarkError(f"{context} is missing {field}.")


def _iso_timestamp(value: Any, field: str) -> str:
    text = trusted_scalar(value, field, 40)
    try:
        parsed = datetime.fromisoformat(text.replace("Z", "+00:00"))
    except ValueError:
        raise BenchmarkError(f"{field} is not an ISO timestamp.") from None
    if parsed.tzinfo is None:
        raise BenchmarkError(f"{field} must include a timezone.")
    return parsed.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_deepswe(content: str, source: Mapping[str, Any]) -> Benchmark:
    try:
        document = json.loads(content, parse_float=Decimal)
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise BenchmarkError(f"deepswe JSON is invalid: {exc}") from None
    document = _require_mapping(document, "deepswe document")
    _require_fields(document, ("generated_at", "n_tasks_in_set", "rows"), "deepswe schema")
    published_at = _iso_timestamp(document["generated_at"], "deepswe.generated_at")
    task_count = bounded_integer(document["n_tasks_in_set"], "deepswe.n_tasks_in_set", 1, 100_000)
    inputs = document["rows"]
    if not isinstance(inputs, list):
        raise BenchmarkError("deepswe.rows must be an array.")
    required = (
        "model", "harness", "reasoning_effort", "config", source["scoreField"],
        source["costField"], "ci_lo", "ci_hi", "n_attempted", "n_runs",
    )
    rows: list[Row] = []
    for index, raw in enumerate(inputs):
        item = _require_mapping(raw, f"deepswe row {index}")
        _require_fields(item, required, "deepswe row")
        model = source_model(item["model"], f"deepswe.row[{index}].model", source)
        harness = identifier(item["harness"], f"deepswe.row[{index}].harness", "harness")
        if harness != source["harness"]:
            raise BenchmarkError("deepswe.harness does not match the registered harness.")
        effort_value = item["reasoning_effort"]
        if effort_value is None or effort_value == "":
            effort_value = "default"
        effort = identifier(effort_value, f"deepswe.row[{index}].reasoning_effort", "effort", source["effortLabels"])
        config = identifier(item["config"], f"deepswe.row[{index}].config", "config")
        expected_config = source["configTemplate"].replace("{model}", model.replace("-", "_")).replace("{effort}", effort)
        if config != expected_config:
            raise BenchmarkError("deepswe.config does not correspond to the validated model and effort.")
        score_ratio = bounded_decimal(item[source["scoreField"]], f"deepswe.{source['scoreField']}", 0, 1)
        cost = bounded_decimal(item[source["costField"]], f"deepswe.{source['costField']}", 0, 10_000)
        ci_low = bounded_decimal(item["ci_lo"], "deepswe.ci_lo", 0, 1)
        ci_high = bounded_decimal(item["ci_hi"], "deepswe.ci_hi", 0, 1)
        if ci_low > score_ratio or ci_high < score_ratio or ci_low > ci_high:
            raise BenchmarkError("deepswe confidence interval does not contain the score.")
        rows.append(Row(
            model=model, effort=effort, harness=harness, config=config,
            score=score_ratio * 100, cost=cost, ci_low=ci_low * 100,
            ci_high=ci_high * 100,
            sample_count=bounded_integer(item["n_attempted"], "deepswe.n_attempted", 1, 1_000_000),
            run_count=bounded_integer(item["n_runs"], "deepswe.n_runs", 1, 10_000),
        ))
    return Benchmark(
        id="deepswe", display_name="DeepSWE",
        version=identifier(source["version"], "deepswe.version", "version"),
        published_at=published_at, task_count=task_count,
        score_label=trusted_scalar(source["scoreLabel"], "deepswe.scoreLabel", 30), rows=rows,
    )


class _TableParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.tables: list[list[list[tuple[str, str]]]] = []
        self._table: list[list[tuple[str, str]]] | None = None
        self._row: list[tuple[str, str]] | None = None
        self._cell_kind: str | None = None
        self._cell_parts: list[str] = []
        self._ignored = 0

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        attributes = dict(attrs)
        if self._ignored:
            self._ignored += 1
            return
        if tag in ("script", "style") or (tag == "span" and "md:hidden" in (attributes.get("class") or "").split()):
            self._ignored = 1
        elif tag == "table":
            if self._table is not None:
                raise BenchmarkError("cursorbench contains nested tables.")
            self._table = []
        elif tag == "tr" and self._table is not None:
            self._row = []
        elif tag in ("th", "td") and self._row is not None:
            self._cell_kind = tag
            self._cell_parts = []

    def handle_endtag(self, tag: str) -> None:
        if self._ignored:
            self._ignored -= 1
            return
        if tag in ("th", "td") and self._cell_kind == tag and self._row is not None:
            text = re.sub(r"\s+", " ", html.unescape("".join(self._cell_parts))).strip()
            self._row.append((tag, text))
            self._cell_kind = None
        elif tag == "tr" and self._row is not None and self._table is not None:
            self._table.append(self._row)
            self._row = None
        elif tag == "table" and self._table is not None:
            self.tables.append(self._table)
            self._table = None

    def handle_data(self, data: str) -> None:
        if not self._ignored and self._cell_kind is not None:
            self._cell_parts.append(data)


def parse_cursorbench(content: str, source: Mapping[str, Any]) -> Benchmark:
    try:
        version_pattern = re.compile(source["versionPattern"])
    except re.error as exc:
        raise BenchmarkError(f"cursorbench versionPattern is invalid: {exc}") from None
    version_match = version_pattern.search(content)
    if not version_match:
        raise BenchmarkError("cursorbench version marker is missing or changed.")
    version = identifier(version_match.group(1), "cursorbench.version", "version")
    date_match = re.search(r"cursorbench-changelog-([0-9]{4}-[0-9]{2}-[0-9]{2})", content)
    if not date_match:
        raise BenchmarkError("cursorbench source-update timestamp is missing or changed.")
    try:
        date.fromisoformat(date_match.group(1))
    except ValueError:
        raise BenchmarkError("cursorbench source-update timestamp is invalid.") from None
    parser = _TableParser()
    try:
        parser.feed(content)
        parser.close()
    except BenchmarkError:
        raise
    except Exception as exc:
        raise BenchmarkError(f"cursorbench HTML is invalid: {exc}") from None
    unique: dict[tuple[tuple[tuple[str, str], ...], ...], list[list[tuple[str, str]]]] = {}
    for table in parser.tables:
        key = tuple(tuple(row) for row in table)
        unique[key] = table
    if len(unique) != 1:
        raise BenchmarkError(f"cursorbench expected one unique rendered table; found {len(unique)}.")
    table = next(iter(unique.values()))
    if len(table) < 2:
        raise BenchmarkError("cursorbench rendered table has no data rows.")
    expected_headers = source["expectedHeaders"]
    header = table[0]
    headers = [text for kind, text in header if kind == "th"]
    if len(header) != len(headers) or headers != expected_headers:
        raise BenchmarkError("cursorbench rendered table headers changed.")
    effort_labels = sorted(source["effortLabels"], key=len, reverse=True)
    rows: list[Row] = []
    for index, raw_row in enumerate(table[1:], 1):
        if any(kind != "td" for kind, _ in raw_row) or len(raw_row) != 6:
            raise BenchmarkError(f"cursorbench row {index} has {len(raw_row)} cells; expected 6.")
        cells = [text for _, text in raw_row]
        combined = trusted_scalar(cells[1], f"cursorbench.row[{index}].model", 120)
        effort = "default"
        model = combined
        for label in effort_labels:
            suffix = " " + label
            if combined.endswith(suffix):
                model = combined[: -len(suffix)]
                effort = label
                break
        model = source_model(model, f"cursorbench.row[{index}].model", source)
        effort = identifier(effort, f"cursorbench.row[{index}].effort", "effort", [*source["effortLabels"], "default"])
        score_match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)\s*%", cells[2])
        if not score_match:
            raise BenchmarkError(f"cursorbench row {index} score changed format.")
        cost_match = re.fullmatch(r"\$\s*([0-9]+(?:\.[0-9]+)?)", cells[3])
        if not cost_match:
            raise BenchmarkError(f"cursorbench row {index} cost changed format.")
        rows.append(Row(
            model=model, effort=effort,
            harness=trusted_scalar(source["harness"], "cursorbench.harness", 100),
            config=trusted_scalar(source["config"], "cursorbench.config", 150),
            score=bounded_decimal(score_match.group(1), f"cursorbench.row[{index}].score", 0, 100),
            cost=bounded_decimal(cost_match.group(1), f"cursorbench.row[{index}].cost", 0, 10_000),
        ))
    return Benchmark(
        id="cursorbench", display_name="CursorBench", version=version,
        published_at=date_match.group(1), task_count=None,
        score_label=trusted_scalar(source["scoreLabel"], "cursorbench.scoreLabel", 30), rows=rows,
    )


def read_registry(path: Path) -> MutableMapping[str, Any]:
    try:
        registry = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise BenchmarkError(f"Benchmark registry is invalid JSON: {exc}") from None
    registry = dict(_require_mapping(registry, "benchmark registry"))
    if registry.get("schemaVersion") != 1:
        raise BenchmarkError("Benchmark registry schemaVersion must be 1.")
    snapshot = _require_mapping(registry.get("snapshot"), "snapshot")
    bounded_integer(snapshot.get("maximumBytes"), "snapshot.maximumBytes", 1024, 1_048_576)
    bounded_integer(snapshot.get("maximumRows"), "snapshot.maximumRows", 1, 10_000)
    sources = registry.get("sources")
    if not isinstance(sources, list) or not sources:
        raise BenchmarkError("Benchmark registry has no sources.")
    allowed_adapters = {"deepswe-json", "cursorbench-html"}
    seen: set[str] = set()
    for raw in sources:
        source = _require_mapping(raw, "source")
        source_id = trusted_scalar(source.get("id"), "source.id", 40)
        if not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", source_id):
            raise BenchmarkError(f"Invalid source id: {source_id}")
        if source_id in seen:
            raise BenchmarkError(f"Duplicate source id: {source_id}")
        seen.add(source_id)
        if not isinstance(source.get("enabled"), bool):
            raise BenchmarkError(f"Source {source_id} enabled must be boolean.")
        if source.get("adapter") not in allowed_adapters:
            raise BenchmarkError(f"Source {source_id} uses a non-allowlisted adapter.")
        for field in ("url", "canonicalUrl"):
            url = trusted_scalar(source.get(field), f"{source_id}.{field}", 300)
            if not re.fullmatch(r"https://[^\s]+", url):
                raise BenchmarkError(f"Source {source_id} {field} must be HTTPS.")
        trusted_scalar(source.get("scope"), f"{source_id}.scope", 180)
        trusted_scalar(source.get("caveat"), f"{source_id}.caveat", 220)
        bounded_integer(source.get("minimumRows"), f"{source_id}.minimumRows", 1, 10_000)
        bounded_integer(source.get("maximumRows"), f"{source_id}.maximumRows", source["minimumRows"], 10_000)
        patterns = source.get("modelPatterns")
        if not isinstance(patterns, list) or not 1 <= len(patterns) <= 20:
            raise BenchmarkError(f"Source {source_id} must define 1..20 modelPatterns.")
        for pattern_value in patterns:
            pattern = trusted_scalar(pattern_value, f"{source_id}.modelPatterns", 200)
            if not pattern.startswith("^") or not pattern.endswith("$"):
                raise BenchmarkError(f"Source {source_id} modelPatterns must be anchored.")
            try:
                re.compile(pattern)
            except re.error as exc:
                raise BenchmarkError(f"Source {source_id} has an invalid model pattern: {exc}") from None
        efforts = source.get("effortLabels")
        if not isinstance(efforts, list) or not efforts:
            raise BenchmarkError(f"Source {source_id} must define effortLabels.")
        for effort in efforts:
            identifier(effort, f"{source_id}.effortLabels", "effort")
        if source["adapter"] == "deepswe-json":
            identifier(source.get("harness"), f"{source_id}.harness", "harness")
            template = trusted_scalar(source.get("configTemplate"), f"{source_id}.configTemplate", 100)
            if template != "mini_swe_agent_{model}_{effort}":
                raise BenchmarkError(f"Source {source_id} configTemplate is not the reviewed DeepSWE structure.")
            for field in ("version", "scoreLabel", "scoreField", "costField"):
                trusted_scalar(source.get(field), f"{source_id}.{field}", 60)
        else:
            trusted_scalar(source.get("harness"), f"{source_id}.harness", 100)
            trusted_scalar(source.get("config"), f"{source_id}.config", 150)
            trusted_scalar(source.get("versionPattern"), f"{source_id}.versionPattern", 100)
            headers = source.get("expectedHeaders")
            if not isinstance(headers, list) or headers != ["", "Model", "Score", "Cost / task", "Tokens / task", "Steps / task"]:
                raise BenchmarkError(f"Source {source_id} expectedHeaders is not the reviewed CursorBench schema.")
    return registry


def fetch_source(
    source: Mapping[str, Any], *, opener: Callable[..., Any] = urllib.request.urlopen,
    sleeper: Callable[[float], None] = time.sleep,
) -> str:
    last_error: Exception | None = None
    request = urllib.request.Request(source["url"], headers={"User-Agent": "sernst-skills-model-benchmark-updater/1"})
    for attempt in range(1, 4):
        try:
            with opener(request, timeout=30) as response:
                status = getattr(response, "status", response.getcode())
                if status != 200:
                    raise urllib.error.HTTPError(source["url"], status, f"HTTP {status}", {}, None)
                return response.read().decode("utf-8")
        except urllib.error.HTTPError as exc:
            last_error = exc
            if 400 <= exc.code < 500 and exc.code not in (408, 429):
                raise BenchmarkError(f"{source['id']} fetch failed with non-retryable HTTP {exc.code}.") from None
        except (OSError, UnicodeError) as exc:
            last_error = exc
        if attempt < 3:
            print(f"Warning: {source['id']} fetch attempt {attempt} failed; retrying.")
            sleeper(2 ** (attempt - 1))
    raise BenchmarkError(f"{source['id']} fetch failed after 3 attempts: {last_error}")


def parse_source(source: Mapping[str, Any], content: str) -> Benchmark:
    adapter = source["adapter"]
    if adapter == "deepswe-json":
        return parse_deepswe(content, source)
    if adapter == "cursorbench-html":
        return parse_cursorbench(content, source)
    raise BenchmarkError(f"Unsupported adapter: {adapter}")


def assert_source_rows(source: Mapping[str, Any], rows: Sequence[Row]) -> None:
    minimum = bounded_integer(source["minimumRows"], f"{source['id']}.minimumRows", 1, 10_000)
    maximum = bounded_integer(source["maximumRows"], f"{source['id']}.maximumRows", minimum, 10_000)
    if not minimum <= len(rows) <= maximum:
        raise BenchmarkError(f"{source['id']} returned {len(rows)} rows; expected {minimum}..{maximum}.")
    seen: set[tuple[str, str, str, str]] = set()
    for row in rows:
        key = (row.model, row.effort, row.harness, row.config)
        if key in seen:
            raise BenchmarkError(f"{source['id']} returned a duplicate model/effort/config row.")
        seen.add(key)


def set_pareto(rows: Sequence[Row]) -> None:
    for row in rows:
        row.pareto = not any(
            candidate is not row
            and candidate.cost <= row.cost
            and candidate.score >= row.score
            and (candidate.cost < row.cost or candidate.score > row.score)
            for candidate in rows
        )


def _decimal_semantic(value: Decimal | None) -> str:
    if value is None:
        return ""
    return format(value.normalize(), "f")


def semantic_text(benchmark: Benchmark) -> str:
    lines = [f"{benchmark.id}|{benchmark.version}|{benchmark.published_at}|{benchmark.score_label}|{benchmark.task_count or ''}"]
    for row in benchmark.rows:
        lines.append("|".join((
            row.model, row.effort, row.harness, row.config,
            _decimal_semantic(row.score), _decimal_semantic(row.cost),
            _decimal_semantic(row.ci_low), _decimal_semantic(row.ci_high),
            str(row.sample_count or ""), str(row.run_count or ""), str(row.pareto),
        )))
    return "\n".join(lines)


def _uncertainty(row: Row) -> str:
    parts: list[str] = []
    if row.ci_low is not None and row.ci_high is not None:
        parts.append(f"95% CI {row.ci_low:.2f}–{row.ci_high:.2f}%")
    if row.sample_count is not None:
        parts.append(f"n={row.sample_count:,}")
    if row.run_count is not None:
        parts.append(f"runs={row.run_count:,}")
    return "; ".join(parts) or "—"


def render_snapshot(registry: Mapping[str, Any], benchmarks: Sequence[Benchmark], retrieved_at: datetime) -> Snapshot:
    source_texts = [semantic_text(benchmark) for benchmark in benchmarks]
    source_hashes = {benchmark.id: sha256(text) for benchmark, text in zip(benchmarks, source_texts)}
    semantic_hash = sha256(f"parser={PARSER_VERSION}\n" + "\n".join(source_texts))
    if retrieved_at.tzinfo is None:
        raise BenchmarkError("retrieved_at must include a timezone.")
    retrieved = retrieved_at.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    lines = [
        "# Model benchmark snapshot", "",
        "Generated supporting evidence for maestro model/effort selection. Compare only",
        "within a source and version; task-specific judgment and the current roster remain",
        "authoritative. `★` marks the point-estimate cost/performance Pareto frontier.", "",
        f"- Retrieved after semantic change: `{retrieved}`",
        f"- Parser version: `{PARSER_VERSION}`",
        f"- Normalized SHA-256: `{semantic_hash}`",
        "- Scores and costs are source-reported; no composite or cross-source ranking is calculated.",
    ]
    by_id = {source["id"]: source for source in registry["sources"]}
    for benchmark in benchmarks:
        source = by_id[benchmark.id]
        published = f" · source updated `{benchmark.published_at}`" if benchmark.published_at else ""
        tasks = f" · tasks `{benchmark.task_count}`" if benchmark.task_count is not None else ""
        lines.extend((
            "", f"## {benchmark.display_name}", "",
            f"Source: [{benchmark.display_name}]({source['canonicalUrl']}) · version `{benchmark.version}`{published}{tasks} · normalized SHA-256 `{source_hashes[benchmark.id]}`",
            "", f"Metric: `{benchmark.score_label}` · {source['scope']} {source['caveat']}", "",
        ))
        harness = markdown_scalar(source["harness"])
        if source["adapter"] == "deepswe-json":
            lines.append(f"Shared harness: `{harness}` · configuration is derived from model + effort.")
        else:
            lines.append(f"Shared harness/config: `{harness}` · `{markdown_scalar(source['config'])}`.")
        lines.extend((
            "", "| model | effort | score | avg cost/task | uncertainty / sample | Pareto |",
            "| --- | --- | ---: | ---: | --- | :---: |",
        ))
        for row in benchmark.rows:
            lines.append(
                f"| {markdown_scalar(row.model)} | {markdown_scalar(row.effort)} | "
                f"{row.score:.2f}% | ${row.cost:.3f} | {_uncertainty(row)} | {'★' if row.pareto else ''} |"
            )
    return Snapshot(content="\n".join(lines) + "\n", semantic_hash=semantic_hash)


def update_benchmarks(
    registry_path: Path, output_path: Path, *, fixture_root: Path | None = None,
    retrieved_at: datetime | None = None, check: bool = False,
    fetcher: Callable[[Mapping[str, Any]], str] = fetch_source,
    emit: Callable[[str], None] = print,
) -> UpdateResult:
    registry = read_registry(registry_path)
    enabled = [source for source in registry["sources"] if source["enabled"]]
    if not enabled:
        raise BenchmarkError("Benchmark registry has no enabled sources.")
    mode = "check" if check else "refresh"
    emit(f"Benchmark {mode} plan: {len(enabled)} allowlisted sources -> {output_path}")
    benchmarks: list[Benchmark] = []
    for source in enabled:
        if fixture_root is not None:
            extension = "json" if source["adapter"] == "deepswe-json" else "html"
            fixture_path = fixture_root / f"{source['id']}.{extension}"
            if not fixture_path.is_file():
                raise BenchmarkError(f"Missing fixture: {fixture_path}")
            content = fixture_path.read_text(encoding="utf-8")
            emit(f"Reading {source['id']} fixture: {fixture_path}")
        else:
            emit(f"Fetching {source['id']} from {source['url']}")
            content = fetcher(source)
        benchmark = parse_source(source, content)
        assert_source_rows(source, benchmark.rows)
        set_pareto(benchmark.rows)
        benchmark.rows.sort(key=lambda row: (-row.score, row.cost, row.model, row.effort))
        benchmarks.append(benchmark)
        emit(f"Validated {len(benchmark.rows)} {source['id']} rows.")
    total_rows = sum(len(benchmark.rows) for benchmark in benchmarks)
    maximum_rows = bounded_integer(registry["snapshot"]["maximumRows"], "snapshot.maximumRows", 1, 10_000)
    if total_rows > maximum_rows:
        raise BenchmarkError(f"Snapshot has {total_rows} rows; maximum is {maximum_rows}.")
    snapshot = render_snapshot(registry, benchmarks, retrieved_at or datetime.now(timezone.utc))
    byte_count = len(snapshot.content.encode("utf-8"))
    maximum_bytes = bounded_integer(registry["snapshot"]["maximumBytes"], "snapshot.maximumBytes", 1024, 1_048_576)
    if byte_count > maximum_bytes:
        raise BenchmarkError(f"Snapshot is {byte_count} bytes; maximum is {maximum_bytes}.")
    existing_hash: str | None = None
    if output_path.is_file():
        match = _HASH_RE.search(output_path.read_text(encoding="utf-8"))
        if match:
            existing_hash = match.group(1)
    result = UpdateResult(existing_hash != snapshot.semantic_hash, total_rows, byte_count, snapshot.semantic_hash)
    if not result.changed:
        emit(f"Unchanged: {total_rows} rows; no file written.")
        return result
    if check:
        emit(f"Update available: {total_rows} rows, {byte_count} bytes; no file written.")
        return result
    output_path.parent.mkdir(parents=True, exist_ok=True)
    temporary: Path | None = None
    try:
        descriptor, name = tempfile.mkstemp(prefix=".benchmark-snapshot.", suffix=".tmp", dir=output_path.parent)
        temporary = Path(name)
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as stream:
            stream.write(snapshot.content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, output_path)
        temporary = None
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    emit(f"Updated: {total_rows} rows, {byte_count} bytes -> {output_path}")
    return result
