#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Capture the Body Convey reference surface against a synthetic journal.

This is a build-time oracle for the reference Flask app.  It creates only
temporary synthetic journals and freezes the responses in
``core/fixtures/convey_body_corpus.json``.
"""

from __future__ import annotations

import argparse
import hashlib
import inspect
import json
import os
import re
import shutil
import sys
import tempfile
from datetime import date
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

# Pin before importing time or any Solstone module: routes use process-local time.
os.environ["TZ"] = "UTC"
import time  # noqa: E402

if hasattr(time, "tzset"):
    time.tzset()

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "core" / "fixtures" / "convey_body_corpus.json"
sys.path.insert(0, str(REPO_ROOT))
ANCHOR = date(2026, 8, 1)
ANCHOR_DAY = ANCHOR.strftime("%Y%m%d")
PLACEHOLDER_ROOT = "<JOURNAL_ROOT>"
PLACEHOLDER_DAY = "<TODAY>"
RUN_TODAY = date.today().strftime("%Y%m%d")

Probe = tuple[str, str, str]

FULL_PROBES: tuple[Probe, ...] = (
    ("GET", "/app/body/", "body overview shell"),
    ("GET", "/app/body/trends", "body trends shell"),
    ("GET", f"/app/body/{ANCHOR_DAY}", "body date shell"),
    ("GET", "/app/body/api/status", "body archive status"),
    (
        "GET",
        "/app/body/api/recent?before=20260802&limit=100",
        "recent carousel cap over a populated archive",
    ),
    ("GET", f"/app/body/api/day/{ANCHOR_DAY}", "fully populated day card"),
    (
        "GET",
        "/app/body/api/window?from=2026-08-01T00%3A00%3A00%2B00%3A00"
        "&to=2026-08-02T00%3A00%3A00%2B00%3A00",
        "one-day transcript context window",
    ),
    ("GET", "/app/body/api/index", "date-nav index"),
    ("GET", "/app/body/api/stats/202608", "August date-nav month counts"),
    ("GET", "/app/body/api/trends", "trends cache state"),
)

NAMED_TORN_PROBES: tuple[Probe, ...] = (
    ("GET", "/app/body/api/status", "torn status"),
    ("GET", f"/app/body/api/day/{ANCHOR_DAY}", "torn anchor day"),
    (
        "GET",
        "/app/body/api/recent?before=20260802&limit=100",
        "torn recent carousel",
    ),
    ("GET", "/app/body/api/index", "torn date-nav index"),
    ("GET", "/app/body/api/stats/202608", "torn August stats"),
)

NAMED_TORN_ROUTES = {
    "status": "/app/body/api/status",
    "day": f"/app/body/api/day/{ANCHOR_DAY}",
    "recent": "/app/body/api/recent?before=20260802&limit=100",
    "index": "/app/body/api/index",
    "stats": "/app/body/api/stats/202608",
}

BODY_RULES = frozenset(
    {
        "/app/body/",
        "/app/body/trends",
        "/app/body/<day>",
        "/app/body/api/status",
        "/app/body/api/recent",
        "/app/body/api/trends",
        "/app/body/api/day/<day>",
        "/app/body/api/window",
        "/app/body/api/index",
        "/app/body/api/stats/<month>",
    }
)

# This is intentionally a path allowlist, not a date-shaped-value rule.
NORMALIZED_FIELDS: dict[str, set[str]] = {
    "/app/body/api/trends": {"generated_at_day"},
}

WALL_CLOCK_CALLS = (
    ("_build_trends_payload", "datetime.now()"),
    ("_expected_source_last_days", "datetime.now()"),
    ("_expected_source_last_days", "datetime.now()"),
    ("_build_source_freshness", "datetime.now()"),
)
WALL_CLOCK_RE = re.compile(r"\b(?:datetime\.now\(\)|date\.today\(\))")
FUNCTION_RE = re.compile(r"^def ([A-Za-z_][A-Za-z0-9_]*)\(")


def _normalize(value: Any, found: set[str], allowed: set[str], path: str = "") -> Any:
    if isinstance(value, dict):
        return {
            key: _normalize(item, found, allowed, f"{path}.{key}" if path else key)
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [_normalize(item, found, allowed, f"{path}[]") for item in value]
    if isinstance(value, str) and path in allowed:
        if len(value) == 8 and value.isdigit():
            found.add(path)
            return PLACEHOLDER_DAY
    return value


def _json_strings(value: Any) -> list[str]:
    if isinstance(value, dict):
        return [item for child in value.values() for item in _json_strings(child)]
    if isinstance(value, list):
        return [item for child in value for item in _json_strings(child)]
    return [value] if isinstance(value, str) else []


def _validate_wall_clock_contract(routes: Any) -> list[dict[str, Any]]:
    """Account for every reference wall-clock read without date-shape guessing."""

    current_function = ""
    calls: list[dict[str, Any]] = []
    for line_number, line in enumerate(Path(routes.__file__).read_text(encoding="utf-8").splitlines(), 1):
        function_match = FUNCTION_RE.match(line)
        if function_match:
            current_function = function_match.group(1)
        for match in WALL_CLOCK_RE.finditer(line):
            calls.append(
                {
                    "function": current_function,
                    "line": line_number,
                    "call": match.group(0),
                }
            )
    assert [(item["function"], item["call"]) for item in calls] == list(WALL_CLOCK_CALLS)
    assert "generated_at_day" in Path(routes.__file__).read_text(encoding="utf-8").splitlines()[
        calls[0]["line"] - 1
    ]
    return calls


def _matched_rule(app: Any, method: str, path: str) -> tuple[str, str]:
    """Match the concrete path through Flask's URL map, never by prefix."""

    clean_path = urlsplit(path).path
    adapter = app.url_map.bind("localhost")
    endpoint, _values = adapter.match(clean_path, method=method)
    rules = [
        rule
        for rule in app.url_map.iter_rules(endpoint)
        if method in rule.methods and rule.rule in BODY_RULES
    ]
    if len(rules) != 1:
        raise AssertionError(f"Expected one Body rule for {method} {clean_path}, got {rules}")
    return endpoint, rules[0].rule


def _record(app: Any, client: Any, probe: Probe, root: Path) -> dict[str, Any]:
    method, path, why = probe
    endpoint, rule = _matched_rule(app, method, path)
    # Deliberately uncaught: a reference exception must fail corpus generation.
    response = client.open(path, method=method)
    raw = response.get_data()
    redacted = raw.replace(str(root).encode(), PLACEHOLDER_ROOT.encode())
    content_type = response.headers.get("Content-Type", "")
    case: dict[str, Any] = {
        "method": method,
        "path": path,
        "why": why,
        "endpoint": endpoint,
        "rule": rule,
        "status": response.status_code,
        "content_type": content_type,
    }

    if "json" not in content_type:
        case["body_bytes"] = len(redacted)
        case["body_sha256"] = hashlib.sha256(redacted).hexdigest()
        case["body_sha256_basis"] = "raw-body"
        if response.status_code >= 400:
            case["body_text"] = redacted.decode("utf-8", errors="replace")
        return case

    payload = json.loads(redacted)
    omitted: list[str] = []
    if urlsplit(path).path == "/app/body/api/status" and isinstance(payload, dict):
        if "freshness" in payload:
            del payload["freshness"]
            omitted.append("freshness")
    found: set[str] = set()
    normalized = _normalize(
        payload,
        found,
        NORMALIZED_FIELDS.get(urlsplit(path).path, set()),
    )
    hash_input = json.dumps(normalized, sort_keys=True, separators=(",", ":")).encode(
        "utf-8"
    )
    case.update(
        {
            "body_bytes": len(hash_input),
            "body_sha256": hashlib.sha256(hash_input).hexdigest(),
            "body_sha256_basis": "normalized-json",
            "json": normalized,
            "normalized_fields": sorted(found),
        }
    )
    if omitted:
        case["body_omitted_fields"] = omitted
    if isinstance(normalized, dict):
        case["reason_code"] = normalized.get("reason_code")
    return case


def _find_case(cases: list[dict[str, Any]], path: str) -> dict[str, Any]:
    for case in cases:
        if case["path"] == path:
            return case
    raise AssertionError(f"Missing captured case for {path}")


def _json_case(cases: list[dict[str, Any]], path: str) -> dict[str, Any]:
    value = _find_case(cases, path).get("json")
    if not isinstance(value, dict):
        raise AssertionError(f"Expected JSON object for {path}, got {value!r}")
    return value


def _drain_trends_flight(routes: Any) -> None:
    if not routes._trends_warm_flight.acquire(timeout=10):
        raise AssertionError("Timed out waiting for Body trends warm")
    routes._trends_warm_flight.release()


def _create_phase_app(root: Path) -> Any:
    from solstone.convey import create_app

    os.environ["SOLSTONE_JOURNAL"] = str(root)
    # Must happen immediately before every create_app call.
    os.environ["SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES"] = "1"
    return create_app(str(root))


def _capture_phase(root: Path, probes: tuple[Probe, ...]) -> tuple[list[dict[str, Any]], Any]:
    app = _create_phase_app(root)
    client = app.test_client()
    return [_record(app, client, probe, root) for probe in probes], app


def _damage(root: Path, phase: str) -> None:
    db_path = root / "imports" / "health-dedupe.sqlite"
    if phase == "torn_no_db":
        if not db_path.is_file():
            raise AssertionError(f"Missing expected dedupe db: {db_path}")
        db_path.unlink()
        return
    if phase == "torn_db_unreadable":
        import sqlite3

        with sqlite3.connect(db_path) as connection:
            connection.execute("DROP TABLE health_dedupe")
        return
    if phase == "torn_shard":
        shard = root / "imports" / "20260810_080000" / "normalized" / "2026-08.jsonl"
        if not shard.is_file():
            raise AssertionError(f"Missing expected normalized shard: {shard}")
        shard.write_text(shard.read_text(encoding="utf-8") + "{ invalid json\n", encoding="utf-8")
        return
    if phase == "corrupt_config":
        config = root / "config" / "journal.json"
        config.write_text('{"setup": ', encoding="utf-8")
        return
    raise ValueError(f"Unknown damage phase: {phase}")


def _diff_paths(left: Any, right: Any, path: str = "") -> list[str]:
    if isinstance(left, dict) and isinstance(right, dict):
        paths: list[str] = []
        for key in sorted(set(left) | set(right)):
            child = f"{path}.{key}" if path else key
            if key not in left or key not in right:
                paths.append(child)
            else:
                paths.extend(_diff_paths(left[key], right[key], child))
        return paths
    return [path] if left != right else []


def _named_route_matrix(cases_by_phase: dict[str, list[dict[str, Any]]]) -> dict[str, dict[str, dict[str, Any]]]:
    matrix: dict[str, dict[str, dict[str, Any]]] = {}
    for phase, cases in cases_by_phase.items():
        matrix[phase] = {}
        for name, path in NAMED_TORN_ROUTES.items():
            case = _find_case(cases, path)
            matrix[phase][name] = {
                "status": case["status"],
                "reason_code": case.get("reason_code"),
            }
    return matrix


def _assert_and_analyze(
    seed_manifest: dict[str, Any],
    cases_by_phase: dict[str, list[dict[str, Any]]],
    *,
    fixed_app: Any,
    routes: Any,
    native_deviations: list[Any],
    clock_reads: list[dict[str, Any]],
) -> tuple[dict[str, Any], dict[str, bool], dict[str, Any]]:
    fixed_cases = cases_by_phase["fixed"]
    first_run_cases = cases_by_phase["first_run"]
    fixed_status = _json_case(fixed_cases, "/app/body/api/status")
    first_status = _json_case(first_run_cases, "/app/body/api/status")
    no_db_status = _json_case(cases_by_phase["torn_no_db"], "/app/body/api/status")
    fixed_day = _json_case(fixed_cases, f"/app/body/api/day/{ANCHOR_DAY}")
    first_day = _json_case(first_run_cases, f"/app/body/api/day/{ANCHOR_DAY}")
    fixed_recent = _json_case(
        fixed_cases, "/app/body/api/recent?before=20260802&limit=100"
    )
    fixed_trends = _json_case(fixed_cases, "/app/body/api/trends")
    first_trends = _json_case(first_run_cases, "/app/body/api/trends")

    fixed_rules = {case["rule"] for case in fixed_cases}
    first_rules = {case["rule"] for case in first_run_cases}
    assert fixed_rules == BODY_RULES
    assert first_rules == BODY_RULES
    assert fixed_status == first_status

    assert fixed_status["normalized"]["total"] == seed_manifest["sqlite_normalized_total"]
    assert fixed_status["day_counts"] == seed_manifest["dedupe_day_counts_by_start_date"]
    assert len(fixed_status["imports"]) == len(seed_manifest["valid_import_ids"]) == 3
    assert fixed_status["coverage_window"]["start"]
    assert fixed_status["coverage_window"]["end"]
    assert fixed_status["coverage_window"]["start"] != fixed_status["coverage_window"]["end"]
    assert len(fixed_status["normalized"]["by_month"]) >= 2
    assert fixed_status["normalized"]["total"] > 500
    assert {item["import_id"] for item in fixed_status["imports"]} == set(
        seed_manifest["valid_import_ids"]
    )
    assert len(fixed_recent["days"]) == 31
    assert fixed_recent["has_more"] is True

    assert fixed_day["glucose_series"]
    assert fixed_day["heart"] and fixed_day["heart"]["series"]
    assert fixed_day["sleep"] is not None
    assert fixed_day["activity"] and fixed_day["activity"]["workouts"]
    assert fixed_day["body_measurements"] and fixed_day["body_measurements"]["facts"]
    assert fixed_day["mind_sound"] and fixed_day["mind_sound"]["facts"]
    assert fixed_day["other_signals"] and fixed_day["other_signals"]["facts"]
    assert fixed_day["audit"]["import_ids"] == seed_manifest["valid_import_ids"]
    assert fixed_day["recovery"]["facts"][0]["detail"].startswith("82")
    assert fixed_day["heart"]["series"]["count"] >= 12
    assert fixed_day["activity"]["steps"]["source"] == "Oura (API)"

    cross_midnight = seed_manifest["special_rows"]["cross_midnight_sleep"]
    assert cross_midnight["start_date"][:10].replace("-", "") != cross_midnight["day"]
    assert "20260731" in fixed_status["day_counts"]
    assert (
        seed_manifest["raw_normalized_total"]
        > seed_manifest["sqlite_normalized_total"]
    )

    assert fixed_trends["warming"] is False
    assert first_trends == {"warming": True}
    assert "typical" in fixed_day["recovery"]["facts"][0]
    assert fixed_day["sleep"].get("score_typical")
    assert fixed_day["sleep"].get("asleep_typical")
    heart_facts = {item["label"]: item for item in fixed_day["heart"]["facts"]}
    assert heart_facts["Resting heart rate"].get("typical")
    assert "typical" not in first_day["recovery"]["facts"][0]
    assert "score_typical" not in first_day["sleep"]

    diff_paths = _diff_paths(first_status, no_db_status)
    assert "archive.day_grid" in diff_paths
    assert "archive.recent_days" in diff_paths
    assert first_status["normalized"]["by_source"] == no_db_status["normalized"]["by_source"]
    assert first_status["imports"] == no_db_status["imports"]
    assert first_status["sources_month"] == no_db_status["sources_month"]

    matrix = _named_route_matrix(
        {phase: cases_by_phase[phase] for phase in ("torn_no_db", "torn_db_unreadable", "torn_shard", "corrupt_config")}
    )
    assert all(value["status"] == 200 for value in matrix["torn_no_db"].values())
    for phase in ("torn_db_unreadable", "torn_shard", "corrupt_config"):
        assert any(value["status"] >= 400 for value in matrix[phase].values())
    assert all(
        value == {"status": 500, "reason_code": "internal_error"}
        for value in matrix["torn_db_unreadable"].values()
    )
    assert matrix["torn_shard"] == {
        "status": {"status": 500, "reason_code": "internal_error"},
        "day": {"status": 400, "reason_code": "invalid_day"},
        "recent": {"status": 500, "reason_code": "internal_error"},
        "index": {"status": 200, "reason_code": None},
        "stats": {"status": 200, "reason_code": None},
    }
    assert all(
        value == {"status": 500, "reason_code": "corrupt_config"}
        for value in matrix["corrupt_config"].values()
    )

    failing_route_sets = {
        phase: {name for name, value in entries.items() if value["status"] >= 400}
        for phase, entries in matrix.items()
    }
    severity_ladder_is_strict = (
        failing_route_sets["torn_no_db"]
        < failing_route_sets["torn_shard"]
        < failing_route_sets["torn_db_unreadable"]
    )
    assert severity_ladder_is_strict
    assert failing_route_sets["corrupt_config"]
    corrupt_config_includes_day_route = "day" in failing_route_sets["corrupt_config"]

    phases = list(matrix)
    indistinguishable: list[list[str]] = []
    status_only_pairs: list[dict[str, Any]] = []
    for index, left in enumerate(phases):
        for right in phases[index + 1 :]:
            left_vector = list(matrix[left].values())
            right_vector = list(matrix[right].values())
            if left_vector == right_vector:
                indistinguishable.append([left, right])
            if [item["status"] for item in left_vector] == [
                item["status"] for item in right_vector
            ] and left_vector != right_vector:
                status_only_pairs.append(
                    {
                        "phases": [left, right],
                        "status_codes": [item["status"] for item in left_vector],
                        "reason_codes": {
                            left: [item["reason_code"] for item in left_vector],
                            right: [item["reason_code"] for item in right_vector],
                        },
                    }
                )
    assert indistinguishable == []
    assert [item["phases"] for item in status_only_pairs] == [
        ["torn_db_unreadable", "corrupt_config"]
    ]

    normalized_occurrences = [
        {"phase": phase, "path": case["path"], "fields": case["normalized_fields"]}
        for phase, cases in cases_by_phase.items()
        for case in cases
        if case.get("normalized_fields")
    ]
    assert normalized_occurrences == [
        {
            "phase": "fixed",
            "path": "/app/body/api/trends",
            "fields": ["generated_at_day"],
        }
    ]
    for cases in cases_by_phase.values():
        for case in cases:
            if (
                urlsplit(case["path"]).path == "/app/body/api/status"
                and case["status"] == 200
            ):
                assert case.get("body_omitted_fields") == ["freshness"]

    run_today_leaks = [
        {"phase": phase, "path": case["path"]}
        for phase, cases in cases_by_phase.items()
        for case in cases
        if "json" in case
        and urlsplit(case["path"]).path
        not in {"/app/body/api/status", "/app/body/api/trends"}
        and RUN_TODAY in _json_strings(case["json"])
    ]
    assert not run_today_leaks

    all_cases = [case for phase_cases in cases_by_phase.values() for case in phase_cases]
    hash_basis_is_binary = all(
        (
            "json" in case
            and case["body_sha256_basis"] == "normalized-json"
            and case["body_bytes"]
            == len(
                json.dumps(case["json"], sort_keys=True, separators=(",", ":")).encode(
                    "utf-8"
                )
            )
            and case["body_sha256"]
            == hashlib.sha256(
                json.dumps(case["json"], sort_keys=True, separators=(",", ":")).encode(
                    "utf-8"
                )
            ).hexdigest()
        )
        or ("json" not in case and case["body_sha256_basis"] == "raw-body")
        for case in all_cases
    )
    assert hash_basis_is_binary
    assert native_deviations == []

    generator_source = Path(__file__).read_text(encoding="utf-8")
    assert not re.search(
        r'app\.config\[\s*["\']TESTING["\']\s*\]\s*=\s*True', generator_source
    )
    assert "try:" not in inspect.getsource(_record)
    testing_default = fixed_app.config.get("TESTING") is False
    assert testing_default

    no_db_self_contradiction = {
        "normalized_by_source_populated": bool(no_db_status["normalized"]["by_source"]),
        "normalized_total_zero": no_db_status["normalized"]["total"] == 0,
    }
    no_db_self_contradiction["contradiction_present"] = all(
        no_db_self_contradiction.values()
    )
    assert no_db_self_contradiction["contradiction_present"]

    db_unreadable_structural = (
        all(value["status"] >= 500 for value in matrix["torn_db_unreadable"].values())
        and Path(routes.__file__).read_text(encoding="utf-8").count(
            "_read_health_dedupe_stats("
        )
        >= 2
    )
    assert db_unreadable_structural

    analysis = {
        "named_routes": NAMED_TORN_ROUTES,
        "matrix": matrix,
        "torn_no_db_all_named_routes_2xx": {
            "observed": all(
                value["status"] < 300 for value in matrix["torn_no_db"].values()
            ),
            "explanation": "With health-dedupe.sqlite absent, every named route "
            "returns 2xx because _read_health_dedupe_stats treats an absent file "
            "as zero aggregate stats rather than an error.",
        },
        "torn_phase_severity_ladder": {
            "failing_routes": {
                phase: sorted(routes) for phase, routes in failing_route_sets.items()
            },
            "strictly_nested": severity_ladder_is_strict,
            "nested_order": ["torn_no_db", "torn_shard", "torn_db_unreadable"],
            "corrupt_config_includes_day_route": corrupt_config_includes_day_route,
            "corrupt_config_day_route_explanation": "True because Convey's global "
            "root.require_access gate calls journal_is_active() before the Body "
            "view, so corrupt config blocks the day route too.",
            "torn_db_unreadable_failure_is_structural": db_unreadable_structural,
            "torn_db_unreadable_failure_explanation": "All five named consumers "
            "reach the shared _read_health_dedupe_stats reader (routes.py:885), "
            "so five 500s are one shared SQL failure path observed five times, "
            "not five independent behaviors.",
        },
        "torn_no_db_self_contradiction": no_db_self_contradiction,
        "http_boundary_indistinguishable_pairs": indistinguishable,
        "same_status_code_only_pairs": status_only_pairs,
        "status_sql_vs_shard_diff": {
            "comparison": ["first_run", "torn_no_db"],
            "differing_paths": diff_paths,
            "unchanged_fields": [
                "imports",
                "normalized.by_source",
                "latest_by_source",
                "sources_month",
                "sources_month_label",
                "archive.import_count",
                "archive.sources",
            ],
            "correction": "archive.day_grid and archive.recent_days are populated "
            "in first_run and empty only when the dedupe database is absent.",
        },
        "normalized_occurrences": normalized_occurrences,
        "run_today_exact_leaks": run_today_leaks,
        "wall_clock_calls": clock_reads,
    }
    assertions = {
        "criterion_4_fixed_route_coverage": fixed_rules == BODY_RULES,
        "criterion_5_major_day_card_families": bool(
            fixed_day["glucose_series"]
            and fixed_day["heart"]
            and fixed_day["sleep"]
            and fixed_day["activity"]
            and fixed_day["body_measurements"]
            and fixed_day["mind_sound"]
            and fixed_day["other_signals"]
        ),
        "criterion_6_populated_seed_archive": (
            fixed_status["normalized"]["total"] > 500
            and len(fixed_status["normalized"]["by_month"]) >= 2
            and len(fixed_status["imports"]) == 3
        ),
        "criterion_7_start_date_aggregate_and_seed_traps": (
            fixed_status["day_counts"] == seed_manifest["dedupe_day_counts_by_start_date"]
            and cross_midnight["start_date"][:10].replace("-", "") != cross_midnight["day"]
            and seed_manifest["raw_normalized_total"]
            > seed_manifest["sqlite_normalized_total"]
        ),
        "criterion_8_status_sql_vs_shard_split": bool(
            diff_paths and no_db_self_contradiction["contradiction_present"]
        ),
        "criterion_9_torn_phase_severity_ladder": severity_ladder_is_strict,
        "criterion_10_wall_clock_normalization_completeness": (
            normalized_occurrences
            == [
                {
                    "phase": "fixed",
                    "path": "/app/body/api/trends",
                    "fields": ["generated_at_day"],
                }
            ]
            and not run_today_leaks
            and len(clock_reads) == len(WALL_CLOCK_CALLS)
        ),
        "criterion_11_explicit_cache_warms_and_baselines": (
            fixed_trends["warming"] is False
            and first_trends == {"warming": True}
            and "typical" in fixed_day["recovery"]["facts"][0]
        ),
        "criterion_12_hash_basis_and_native_deviations": (
            hash_basis_is_binary and native_deviations == []
        ),
        "criterion_13_default_exception_propagation": testing_default,
    }
    assert all(assertions.values())
    route_coverage = {
        "expected_rules": sorted(BODY_RULES),
        "fixed": [
            {key: case[key] for key in ("method", "path", "endpoint", "rule")}
            for case in fixed_cases
        ],
        "first_run": [
            {key: case[key] for key in ("method", "path", "endpoint", "rule")}
            for case in first_run_cases
        ],
    }
    return analysis, assertions, route_coverage


def build_corpus() -> dict[str, Any]:
    import body_corpus_seed

    from solstone.apps.body import routes

    cases_by_phase: dict[str, list[dict[str, Any]]] = {}
    native_deviations: list[Any] = []
    clock_reads = _validate_wall_clock_contract(routes)
    with tempfile.TemporaryDirectory(prefix="convey-body-corpus-") as temp:
        temp_root = Path(temp)
        fixed_root = temp_root / "fixed"
        seed_manifest = body_corpus_seed.seed_populated_body_journal(
            fixed_root, anchor=ANCHOR
        )

        if "solstone.apps.body.events" in sys.modules:
            raise AssertionError("Body events must not be imported by corpus capture")
        fixed_app = _create_phase_app(fixed_root)
        if "solstone.apps.body.events" in sys.modules:
            raise AssertionError("create_app imported Body cache-warm events")
        assert routes._load_trends_cache(fixed_root) is None
        stats_thread = routes.warm_dedupe_stats_cache()
        assert stats_thread is not None and stats_thread.ident is not None
        stats_thread.join(timeout=10)
        assert not stats_thread.is_alive()
        trends_thread = routes.warm_trends_cache(after=stats_thread)
        assert trends_thread is not None and trends_thread.ident is not None
        trends_thread.join(timeout=10)
        assert not trends_thread.is_alive()
        fixed_client = fixed_app.test_client()
        cases_by_phase["fixed"] = [
            _record(fixed_app, fixed_client, probe, fixed_root) for probe in FULL_PROBES
        ]

        first_root = temp_root / "first_run"
        shutil.copytree(fixed_root, first_root)
        first_cases, _first_app = _capture_phase(first_root, FULL_PROBES)
        cases_by_phase["first_run"] = first_cases
        _drain_trends_flight(routes)

        for phase in ("torn_no_db", "torn_db_unreadable", "torn_shard", "corrupt_config"):
            phase_root = temp_root / phase
            shutil.copytree(fixed_root, phase_root)
            _damage(phase_root, phase)
            phase_cases, _phase_app = _capture_phase(phase_root, NAMED_TORN_PROBES)
            cases_by_phase[phase] = phase_cases
            _drain_trends_flight(routes)

        analysis, assertions, route_coverage = _assert_and_analyze(
            seed_manifest,
            cases_by_phase,
            fixed_app=fixed_app,
            routes=routes,
            native_deviations=native_deviations,
            clock_reads=clock_reads,
        )

    return {
        "schema": "solstone-convey-body-corpus-v1",
        "generator": "scripts/convey_body_corpus.py",
        "seeder": "body_corpus_seed.seed_populated_body_journal",
        "tz": "UTC",
        "placeholders": {"day": PLACEHOLDER_DAY, "journal_root": PLACEHOLDER_ROOT},
        "native_deviations": native_deviations,
        "journal": seed_manifest,
        "route_coverage": route_coverage,
        "freshness_contract": {
            "captured": False,
            "hashed": False,
            "status_path": "/app/body/api/status",
            "excluded_field": "freshness",
            "configured_quiet_days": {"Synthetic Watch": 14, "Synthetic CGM": 14},
            "clock_reads": [
                {**clock_read, "purpose": purpose}
                for clock_read, purpose in zip(
                    clock_reads[1:],
                    (
                        "current month cache signature",
                        "recent month scan window",
                        "days since source delivery",
                    ),
                    strict=True,
                )
            ],
        },
        "normalization_contract": {
            "allowlist": {path: sorted(fields) for path, fields in NORMALIZED_FIELDS.items()},
            "expected_occurrences": 1,
            "reference_wall_clock_calls": clock_reads,
            "exact_today_scan": {
                "excluded_paths": ["/app/body/api/status", "/app/body/api/trends"],
                "purpose": "Detect an unnormalized bare current-day leak without "
                "treating seeded YYYY-MM values as wall-clock data.",
            },
        },
        "cache_warm_contract": {
            "create_app_imports_body_events": False,
            "fixed_explicit_warms": ["warm_dedupe_stats_cache", "warm_trends_cache"],
            "fixed_trends_warming": False,
            "first_run_trends_warming": True,
        },
        "torn_phase_analysis": analysis,
        "generator_assertions": assertions,
        "cases": cases_by_phase,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="exit non-zero if the corpus would change")
    args = parser.parse_args()
    corpus = build_corpus()
    rendered = json.dumps(corpus, indent=2, sort_keys=True) + "\n"
    if args.check:
        if not CORPUS_PATH.exists():
            print(f"missing corpus: {CORPUS_PATH}", file=sys.stderr)
            return 1
        if CORPUS_PATH.read_text(encoding="utf-8") != rendered:
            print(
                f"body corpus is stale: {CORPUS_PATH}\n"
                "regenerate with: uv run python scripts/convey_body_corpus.py",
                file=sys.stderr,
            )
            return 1
        print(f"body corpus is current: {CORPUS_PATH}")
        return 0
    CORPUS_PATH.parent.mkdir(parents=True, exist_ok=True)
    CORPUS_PATH.write_text(rendered, encoding="utf-8")
    total = sum(len(cases) for cases in corpus["cases"].values())
    print(f"wrote {CORPUS_PATH} ({total} cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
