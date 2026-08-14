#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Freeze the health, stats, tokens, and sol GET reference surfaces.

Each journal phase runs in a fresh Python process.  That boundary is deliberate:
Sol's talent metadata is process-cached, while journal talent overrides do not
appear in the metadata response.  A fresh interpreter prevents a previous
phase's metadata from becoming the reference for the next one.
"""

from __future__ import annotations

import argparse
import getpass
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Any
from unittest.mock import patch

from corpus_scrub import (
    assert_egress_guard_can_see,
    assert_guard_can_see,
    assert_no_egress_attempted,
    assert_publishable,
    forbid_non_loopback_egress,
)

forbid_non_loopback_egress()
os.environ["TZ"] = "UTC"
if hasattr(time, "tzset"):
    time.tzset()


REPO_ROOT = Path(__file__).resolve().parent.parent
CAPTURE_ROOT = Path("/var/tmp/solstone-convey-system-corpus")
FIXTURES = {
    "health": REPO_ROOT / "core" / "fixtures" / "convey_health_corpus.json",
    "stats_tokens": REPO_ROOT / "core" / "fixtures" / "convey_stats_tokens_corpus.json",
    "sol": REPO_ROOT / "core" / "fixtures" / "convey_sol_corpus.json",
}
PHASES = (
    "unestablished",
    "corrupt",
    "established_empty",
    "established_populated",
    "populated_single_failure",
    "stats_absent",
    "stats_unparseable",
)
FIXED_PAST_DAYS = frozenset({"20260214", "20260315", "20260403"})
HEADER_ALLOWLIST = ("Content-Type", "Location", "Set-Cookie")
HOST_HEADER = "127.0.0.1"
POPULATED_DAY = "20260403"
# SVG path coordinates in the shipped Sol workspace happen to match the shared
# dotted-quad shape guard.  This literal is source-owned, not host-derived: it
# is the cubic-bezier wrench-icon fragment in solstone/apps/sol/workspace.html:1083
# and solstone/convey/static/convey_icons.js:41; 438 and 662 are not IPv4 octets.
AUTHORED_IPV4 = ("438.12.54.662",)


def _probe(name: str, path: str, contract: str) -> dict[str, str]:
    return {"contract": contract, "name": name, "path": path}


HEALTH_PROBES = (
    _probe("index", "/app/health/", "contract"),
    _probe("workspace", "/app/health/workspace", "contract"),
    _probe("background", "/app/health/background", "contract"),
    _probe("static_health_js", "/app/health/static/health.js", "contract"),
    _probe("api_state", "/app/health/api/state", "contract"),
    _probe("api_log_valid", f"/app/health/api/log?path={POPULATED_DAY}/health/health.log", "contract"),
    _probe("api_log_missing_path", "/app/health/api/log", "contract"),
    _probe("api_log_bad_pattern", "/app/health/api/log?path=not-a-valid-log-path", "contract"),
    _probe("api_log_not_found", f"/app/health/api/log?path={POPULATED_DAY}/health/missing.log", "contract"),
    _probe("api_info", "/app/health/api/info", "contract"),
)
STATS_PROBES = (
    _probe("index", "/app/stats/", "contract"),
    _probe("workspace", "/app/stats/workspace", "contract"),
    _probe("background", "/app/stats/background", "contract"),
    _probe("static_dashboard_js", "/app/stats/static/dashboard.js", "contract"),
    _probe("api_stats", "/app/stats/api/stats", "contract"),
)
TOKENS_PROBES = (
    _probe("index", "/app/tokens/", "reference-record"),
    _probe("day", f"/app/tokens/{POPULATED_DAY}", "reference-record"),
    _probe("workspace", "/app/tokens/workspace", "reference-record"),
    _probe("background", "/app/tokens/background", "reference-record"),
    _probe("api_usage", f"/app/tokens/api/usage?day={POPULATED_DAY}", "reference-record"),
    _probe("api_daily", f"/app/tokens/api/daily?days=3&day={POPULATED_DAY}", "reference-record"),
    _probe("api_index", "/app/tokens/api/index", "reference-record"),
    _probe("api_stats", "/app/tokens/api/stats/202604", "reference-record"),
)
SOL_PROBES = (
    _probe("index", "/app/sol/", "reference-record"),
    _probe("day", f"/app/sol/{POPULATED_DAY}", "reference-record"),
    _probe("bad_day", "/app/sol/notaday", "reference-record"),
    _probe("workspace", "/app/sol/workspace", "reference-record"),
    _probe("background", "/app/sol/background", "reference-record"),
    _probe("api_talents", f"/app/sol/api/talents/{POPULATED_DAY}", "reference-record"),
    _probe("api_talents_work", f"/app/sol/api/talents/{POPULATED_DAY}?facet=work", "reference-record"),
    _probe("api_run_completed", "/app/sol/api/run/1710000000000", "reference-record"),
    _probe("api_run_pending", "/app/sol/api/run/1710000060000", "reference-record"),
    _probe("api_run_malformed", "/app/sol/api/run/1710000120000", "reference-record"),
    _probe("api_output", f"/app/sol/api/output/{POPULATED_DAY}/talents/example-output.md", "reference-record"),
    _probe("api_output_traversal", f"/app/sol/api/output/{POPULATED_DAY}/../../etc/passwd", "reference-record"),
    _probe("api_preview_chat", "/app/sol/api/preview/chat", "reference-record"),
    _probe("api_index", "/app/sol/api/index", "reference-record"),
    _probe("api_stats", "/app/sol/api/stats/202604", "reference-record"),
    _probe("api_badge_count", "/app/sol/api/badge-count", "reference-record"),
    _probe("api_updated_days", "/app/sol/api/updated-days", "reference-record"),
    _probe("api_identity", "/app/sol/api/identity", "reference-record"),
)
PROBES = {"health": HEALTH_PROBES, "stats": STATS_PROBES, "tokens": TOKENS_PROBES, "sol": SOL_PROBES}

UNCAPTURED = {
    "health": [
        {"route": "/api/brain/check", "reason": "POST mutation outside GET-only corpus"},
        {"route": "/api/retry-import", "reason": "POST mutation outside GET-only corpus"},
        {"route": "/api/restart-observer", "reason": "POST mutation outside GET-only corpus"},
        {"route": "/api/reprocess", "reason": "POST mutation outside GET-only corpus"},
    ],
    "stats": [],
    "tokens": [
        {"route": "/<day> invalid day", "reason": "deliberately uncaptured page 404 branch"},
        {"route": "/api/usage invalid day", "reason": "deliberately uncaptured invalid-day branch"},
        {"route": "/api/daily invalid values", "reason": "deliberately uncaptured nonnumeric, out-of-range, and invalid-day branches"},
        {"route": "/api/stats/<month> invalid month", "reason": "deliberately uncaptured invalid-month branch"},
    ],
    "sol": [
        {"route": "/api/run/<use_id> not found", "reason": "deliberately uncaptured not-found branch"},
        {"route": "/api/stats/<month> invalid month", "reason": "deliberately uncaptured invalid-month branch"},
        {"route": "/api/output/<day>/<file> missing", "reason": "deliberately uncaptured missing-file branch"},
        {"route": "/api/preview/<name> unknown", "reason": "deliberately uncaptured unknown-talent branch"},
        {"route": "/api/set-name", "reason": "POST route pinned by shipped native authority"},
        {"route": "/api/reset", "reason": "POST route pinned by shipped native authority"},
        {"route": "/api/set-owner", "reason": "POST route pinned by shipped native authority"},
        {"route": "/api/sol-init", "reason": "POST route pinned by shipped native authority"},
    ],
}


def _matches(path: str, pattern: str) -> bool:
    parts, expected = path.split("."), pattern.split(".")
    return len(parts) == len(expected) and all(a == b or b == "*" for a, b in zip(parts, expected))


def _normalize(value: Any, *, path: str = "", root: str) -> tuple[Any, list[str]]:
    """Normalize only the enumerated volatile response field paths."""
    placeholders = {
        "response.body.generators.*.path": "<TALENT_PATH>",
        "response.body.generators.*.mtime": "<TALENT_MTIME>",
        "response.body.agent_errors.items.*.ts": "<CAPTURE_TODAY_TS>",
    }
    for pattern, placeholder in placeholders.items():
        if path and _matches(path, pattern):
            return placeholder, [path]
    if isinstance(value, str):
        if path == "response.headers.Location":
            normalized = re.sub(r"/\d{8}$", "/<CAPTURE_TODAY>", value)
            if normalized != value:
                return normalized, [path]
        if value.startswith(root):
            return value.replace(root, "<CAPTURE_ROOT>", 1), [f"{path}#capture_root"]
        return value, []
    if isinstance(value, dict):
        output: dict[str, Any] = {}
        hits: list[str] = []
        for key in sorted(value):
            item, item_hits = _normalize(value[key], path=f"{path}.{key}" if path else key, root=root)
            output[key] = item
            hits.extend(item_hits)
        return output, hits
    if isinstance(value, list):
        output = []
        hits = []
        for item in value:
            normalized, item_hits = _normalize(item, path=f"{path}.*" if path else "*", root=root)
            output.append(normalized)
            hits.extend(item_hits)
        return output, hits
    return value, []


def _record(client: Any, probe: dict[str, str], root: Path) -> dict[str, Any]:
    response = client.get(probe["path"], headers={"Host": HOST_HEADER}, follow_redirects=False)
    headers = {header: response.headers[header] for header in HEADER_ALLOWLIST if header in response.headers}
    content_type = response.headers.get("Content-Type", "")
    body: Any
    if content_type.startswith("application/json"):
        body = response.get_json()
    else:
        body = response.get_data(as_text=True)
    normalized, hits = _normalize(body, path="response.body", root=str(root))
    if probe["name"] == "index" and probe["path"].startswith(("/app/tokens/", "/app/sol/")):
        redirect_body = re.sub(r"/(?:app/(?:tokens|sol))/\d{8}", lambda match: match.group(0)[:-8] + "<CAPTURE_TODAY>", normalized)
        if redirect_body != normalized:
            normalized = redirect_body
            hits.append("response.body#redirect_target")
    normalized_headers, header_hits = _normalize(headers, path="response.headers", root=str(root))
    return {
        "contract": probe["contract"],
        "method": "GET",
        "name": probe["name"],
        "normalized": sorted(set(hits + header_hits)),
        "path": probe["path"],
        "response": {"body": normalized, "headers": normalized_headers, "status": response.status_code},
    }


def _run_phase_worker(phase: str, root: Path, capture_day: str) -> dict[str, list[dict[str, Any]]]:
    if "solstone.convey" in sys.modules:
        raise AssertionError("solstone.convey imported before the egress guard")
    from convey_system_seed import seed_phase

    from solstone.convey import create_app
    from solstone.think import cogitate_client
    from solstone.think.utils import get_journal

    if phase == "unestablished":
        root.mkdir(parents=True, exist_ok=True)
    else:
        seed_phase(root, phase, capture_day)
    os.environ["SOLSTONE_JOURNAL"] = str(root)
    if get_journal() != str(root):
        raise AssertionError("criterion 0: SOLSTONE_JOURNAL did not resolve to the phase root")
    os.environ["SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES"] = "1"
    talent_contract = {
        "tiers": [
            {"name": "normal", "talent_facing": True},
            {"name": "system-read", "talent_facing": True},
            {"name": "outbound", "talent_facing": True},
            {"name": "synthesis", "talent_facing": True},
            {"name": "diagnostic", "talent_facing": True},
        ]
    }
    # The real loader shells out through the native solstone-core handshake, outside this Python-only capture.
    with patch.object(cogitate_client, "load_talent_contract", return_value=talent_contract):
        app = create_app(str(root))
        app.config.update(TESTING=True)
        client = app.test_client()
        with patch("solstone.apps.health.routes.socket.gethostname", return_value="corpus-host"):
            captured = {app_name: [_record(client, probe, root) for probe in probes] for app_name, probes in PROBES.items()}
    assert_egress_guard_can_see(f"convey-system child {phase}")
    assert_no_egress_attempted(
        f"convey-system child {phase}", ignore=("example.invalid", "198.51.100.7")
    )
    return captured


def _reset_capture_root() -> None:
    home = Path.home().resolve()
    root = CAPTURE_ROOT.resolve()
    if root == home or home.is_relative_to(root) or root.is_relative_to(home):
        raise RuntimeError("capture root must be an absolute path outside $HOME")
    if CAPTURE_ROOT.exists():
        unexpected = {entry.name for entry in CAPTURE_ROOT.iterdir()} - set(PHASES)
        if unexpected:
            raise RuntimeError(f"capture root contains unexpected entries: {sorted(unexpected)}")
        for phase in PHASES:
            phase_root = CAPTURE_ROOT / phase
            if phase_root.exists():
                shutil.rmtree(phase_root)
    CAPTURE_ROOT.mkdir(parents=True, exist_ok=True)
    for phase in PHASES:
        (CAPTURE_ROOT / phase).mkdir()


def _child_payload(phase: str, root: Path, capture_day: str) -> dict[str, list[dict[str, Any]]]:
    try:
        return _run_phase_worker(phase, root, capture_day)
    except Exception as exc:
        print(f"convey-system phase {phase} failed: {exc}", file=sys.stderr)
        raise


def _collect_phases(capture_day: str) -> dict[str, dict[str, list[dict[str, Any]]]]:
    _reset_capture_root()
    captured: dict[str, dict[str, list[dict[str, Any]]]] = {}
    for phase in PHASES:
        phase_root = CAPTURE_ROOT / phase
        command = [
            sys.executable,
            os.path.abspath(__file__),
            "--_phase-worker",
            phase,
            "--_root",
            str(phase_root),
            "--_capture-day",
            capture_day,
        ]
        try:
            result = subprocess.run(command, capture_output=True, check=True, text=True)
        except subprocess.CalledProcessError as exc:
            detail = exc.stderr.strip() or exc.stdout.strip() or "no child output"
            raise RuntimeError(f"convey-system phase {phase} failed; no fixtures written: {detail}") from exc
        try:
            captured[phase] = json.loads(result.stdout)
        except json.JSONDecodeError as exc:
            raise RuntimeError(f"convey-system phase {phase} emitted non-JSON stdout") from exc
    return captured


def _fixture(apps: tuple[str, ...], captured: dict[str, dict[str, list[dict[str, Any]]]]) -> dict[str, Any]:
    return {
        "apps": list(apps),
        "header_allowlist": list(HEADER_ALLOWLIST),
        "phases": {phase: {app: captured[phase][app] for app in apps} for phase in PHASES},
        "rev": "1",
        "tz": "UTC",
        "uncaptured": {app: UNCAPTURED[app] for app in apps},
    }


def _render(fixtures: dict[Path, dict[str, Any]]) -> dict[Path, str]:
    return {path: json.dumps(fixture, indent=2, sort_keys=True) + "\n" for path, fixture in fixtures.items()}


def _absolute_paths(rendered: str) -> set[str]:
    candidates = set(re.findall(r"(?<![A-Za-z0-9_])/(?:[A-Za-z0-9_.-]+/?)+", rendered))
    return {
        candidate.rstrip("/")
        for candidate in candidates
        if candidate.startswith(("/home/", "/Users/", "/var/", "/tmp/", "/private/", "/opt/"))
    }


def assert_independent_scrub(rendered: str, *, root: Path, label: str) -> None:
    """Reject host-shaped values missed by the shared publication guard."""
    findings: list[str] = []
    hostname_label = socket.gethostname().split(".", 1)[0].lower()
    username = getpass.getuser()
    home = os.environ.get("HOME", "")
    lowered = rendered.lower()
    if hostname_label and hostname_label in lowered:
        findings.append("hostname first label")
    if username and username in rendered:
        findings.append("username")
    if home and home in rendered:
        findings.append("HOME")
    if re.search(r"\b\d{1,3}(?:-\d{1,3}){3}\b", rendered):
        findings.append("dash-joined dotted quad")
    root_text = str(root.resolve())
    outside = sorted(path for path in _absolute_paths(rendered) if not path.startswith(root_text))
    if outside:
        findings.append("absolute paths outside capture root: " + ", ".join(outside))
    if findings:
        raise RuntimeError(f"{label}: independent scrub found " + "; ".join(findings))


def _validate_counts(captured: dict[str, dict[str, list[dict[str, Any]]]]) -> None:
    expected = {app: len(probes) for app, probes in PROBES.items()}
    for phase in PHASES:
        actual = {app: len(captured[phase][app]) for app in PROBES}
        if actual != expected:
            raise AssertionError(f"probe count mismatch in {phase}: {actual} != {expected}")
    total = sum(len(captured[phase][app]) for phase in PHASES for app in PROBES)
    if total != 287:
        raise AssertionError(f"probe total mismatch: {total} != 287")


def build_fixtures() -> dict[Path, str]:
    capture_day = datetime.now().strftime("%Y%m%d")
    if capture_day in FIXED_PAST_DAYS:
        raise RuntimeError(f"capture day collides with a fixed seed day: {capture_day}")
    captured = _collect_phases(capture_day)
    _validate_counts(captured)
    fixtures = {
        FIXTURES["health"]: _fixture(("health",), captured),
        FIXTURES["stats_tokens"]: _fixture(("stats", "tokens"), captured),
        FIXTURES["sol"]: _fixture(("sol",), captured),
    }
    rendered = _render(fixtures)
    for path, text in rendered.items():
        assert_publishable(text, label=path.name, allowed_ipv4=AUTHORED_IPV4)
        assert_independent_scrub(text, root=CAPTURE_ROOT, label=path.name)
    assert_egress_guard_can_see("convey-system orchestrator")
    assert_guard_can_see("convey-system fixtures")
    assert_no_egress_attempted(
        "convey-system orchestrator", ignore=("example.invalid", "198.51.100.7")
    )
    return rendered


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail when committed fixtures differ")
    parser.add_argument("--_phase-worker")
    parser.add_argument("--_root")
    parser.add_argument("--_capture-day")
    args = parser.parse_args()
    if args._phase_worker:
        if not args._root or not args._capture_day:
            parser.error("--_phase-worker requires --_root and --_capture-day")
        payload = _child_payload(args._phase_worker, Path(args._root), args._capture_day)
        sys.stdout.write(json.dumps(payload, sort_keys=True) + "\n")
        return 0
    rendered = build_fixtures()
    stale = False
    for path, text in rendered.items():
        if args.check:
            if not path.exists() or path.read_text(encoding="utf-8") != text:
                print(f"convey-system corpus is stale: {path}", file=sys.stderr)
                stale = True
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
            print(f"wrote {path}")
    if stale:
        print("Run with: uv run python scripts/convey_system_corpus.py --check", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
