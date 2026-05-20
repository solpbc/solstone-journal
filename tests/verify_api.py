# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Utilities for API response baseline verification."""

from __future__ import annotations

import argparse
import json
import os
import re
from contextlib import contextmanager
from difflib import unified_diff
from pathlib import Path
from tempfile import TemporaryDirectory
from typing import Any

from freezegun import freeze_time

try:
    from tests._baseline_harness import (
        FROZEN_DATE,
        FROZEN_TZ_OFFSET,
        isolated_app_env,
        make_logged_in_test_client,
        prepare_isolated_journal,
    )
except ModuleNotFoundError:
    from _baseline_harness import (
        FROZEN_DATE,
        FROZEN_TZ_OFFSET,
        isolated_app_env,
        make_logged_in_test_client,
        prepare_isolated_journal,
    )

ENDPOINTS = [
    # convey/config.py
    {
        "app": "config",
        "name": "convey",
        "path": "/api/config/convey",
        "params": {},
        "status": 200,
    },
    # apps/sol/routes.py
    {
        "app": "sol",
        "name": "talents-day",
        "path": "/app/sol/api/talents/20260304",
        "params": {"facet": "work"},
        "status": 200,
    },
    {
        "app": "sol",
        "name": "run-detail",
        "path": "/app/sol/api/run/1700000000001",
        "params": {},
        "status": 200,
    },
    {
        "app": "sol",
        "name": "preview",
        "path": "/app/sol/api/preview/chat",
        "params": {},
        "status": 200,
    },
    {
        "app": "sol",
        "name": "stats-month",
        "path": "/app/sol/api/stats/202603",
        "params": {},
        "status": 200,
    },
    {
        "app": "sol",
        "name": "badge-count",
        "path": "/app/sol/api/badge-count",
        "params": {},
        "status": 200,
        "sandbox_only": True,  # reads date.today() live + sandbox produces boot-time talent runs
    },
    {
        "app": "sol",
        "name": "updated-days",
        "path": "/app/sol/api/updated-days",
        "params": {},
        "status": 200,
        "sandbox_only": True,  # live indexer computes differently than Flask test client
    },
    # convey/chat.py
    {
        "app": "chat",
        "name": "session",
        "path": "/api/chat/session",
        "params": {},
        "status": 200,
    },
    # apps/activities/routes.py
    {
        "app": "activities",
        "name": "stats-month",
        "path": "/app/activities/api/stats/202603",
        "params": {},
        "status": 200,
    },
    {
        "app": "activities",
        "name": "day-activities",
        "path": "/app/activities/api/day/20260304/activities",
        "params": {"facet": "work"},
        "status": 200,
    },
    {
        "app": "activities",
        "name": "screen-files",
        "path": "/app/activities/api/screen_files/20260304",
        "params": {},
        "status": 200,
    },
    # apps/entities/routes.py
    {
        "app": "entities",
        "name": "facet-entities",
        "path": "/app/entities/api/work",
        "params": {},
        "status": 200,
    },
    {
        "app": "entities",
        "name": "entity-detail",
        "path": "/app/entities/api/work/entity/romeo_montague",
        "params": {},
        "status": 200,
    },
    {
        "app": "entities",
        "name": "entity-types",
        "path": "/app/entities/api/types",
        "params": {},
        "status": 200,
    },
    {
        "app": "entities",
        "name": "journal-entities",
        "path": "/app/entities/api/journal",
        "params": {},
        "status": 200,
    },
    {
        "app": "entities",
        "name": "journal-entity-detail",
        "path": "/app/entities/api/journal/entity/first_test_entity",
        "params": {},
        "status": 200,
    },
    {
        "app": "entities",
        "name": "detected-preview",
        "path": "/app/entities/api/work/detected/preview",
        "params": {"name": "Romeo"},
        "status": 200,
    },
    # apps/import/routes.py
    {
        "app": "import",
        "name": "list",
        "path": "/app/import/api/list",
        "params": {},
        "status": 200,
    },
    {
        "app": "import",
        "name": "import-day",
        "path": "/app/import/api/20260304",
        "params": {},
        "status": 404,
    },
    # apps/observer/routes.py
    {
        "app": "observer",
        "name": "list",
        "path": "/app/observer/api/list",
        "params": {},
        "status": 200,
    },
    {
        "app": "observer",
        "name": "observer-key",
        "path": "/app/observer/api/example-key/key",
        "params": {},
        "status": 404,
    },
    {
        "app": "observer",
        "name": "ingest-day",
        "path": "/app/observer/ingest/example-key/segments/20260304",
        "params": {},
        "status": 401,
    },
    # apps/search/routes.py
    {
        "app": "search",
        "name": "search",
        "path": "/app/search/api/search",
        "params": {"q": "romeo", "limit": "5", "offset": "0"},
        "status": 200,
        "sandbox_only": True,
    },
    {
        "app": "search",
        "name": "day-results",
        "path": "/app/search/api/day_results",
        "params": {"q": "meeting", "day": "20260304", "offset": "0", "limit": "5"},
        "status": 200,
        "sandbox_only": True,
    },
    # apps/settings/routes.py
    {
        "app": "settings",
        "name": "config",
        "path": "/app/settings/api/config",
        "params": {},
        "status": 200,
    },
    {
        "app": "settings",
        "name": "transcribe",
        "path": "/app/settings/api/transcribe",
        "params": {},
        "status": 200,
    },
    {
        "app": "settings",
        "name": "providers",
        "path": "/app/settings/api/providers",
        "params": {},
        "status": 200,
    },
    {
        "app": "settings",
        "name": "generators",
        "path": "/app/settings/api/generators",
        "params": {},
        "status": 200,
    },
    {
        "app": "settings",
        "name": "vision",
        "path": "/app/settings/api/vision",
        "params": {},
        "status": 200,
    },
    {
        "app": "settings",
        "name": "observe",
        "path": "/app/settings/api/observe",
        "params": {},
        "status": 200,
    },
    {
        "app": "settings",
        "name": "facet",
        "path": "/app/settings/api/facet/montague",
        "params": {},
        "status": 200,
    },
    {
        "app": "settings",
        "name": "activities-defaults",
        "path": "/app/settings/api/activities/defaults",
        "params": {},
        "status": 200,
    },
    {
        "app": "settings",
        "name": "facet-activities",
        "path": "/app/settings/api/facet/montague/activities",
        "params": {},
        "status": 200,
    },
    {
        "app": "settings",
        "name": "sync",
        "path": "/app/settings/api/sync",
        "params": {},
        "status": 200,
    },
    # apps/speakers/routes.py
    {
        "app": "speakers",
        "name": "stats-month",
        "path": "/app/speakers/api/stats/202603",
        "params": {},
        "status": 200,
    },
    {
        "app": "speakers",
        "name": "segments",
        "path": "/app/speakers/api/segments/20260304",
        "params": {},
        "status": 200,
    },
    {
        "app": "speakers",
        "name": "speakers-segment",
        "path": "/app/speakers/api/speakers/20260304/default/090000_300",
        "params": {},
        "status": 200,
    },
    {
        "app": "speakers",
        "name": "review",
        "path": "/app/speakers/api/review/20260304/default/090000_300/audio",
        "params": {},
        "status": 200,
    },
    # apps/stats/routes.py
    {
        "app": "stats",
        "name": "stats",
        "path": "/app/stats/api/stats",
        "params": {},
        "status": 200,
    },
    # apps/todos/routes.py
    {
        "app": "todos",
        "name": "badge-count",
        "path": "/app/todos/api/badge-count",
        "params": {},
        "status": 200,
    },
    {
        "app": "todos",
        "name": "nudges",
        "path": "/app/todos/api/nudges",
        "params": {},
        "status": 200,
    },
    {
        "app": "todos",
        "name": "stats-month",
        "path": "/app/todos/api/stats/202603",
        "params": {},
        "status": 200,
    },
    # apps/tokens/routes.py
    {
        "app": "tokens",
        "name": "usage",
        "path": "/app/tokens/api/usage",
        "params": {"day": "20260304"},
        "status": 200,
    },
    {
        "app": "tokens",
        "name": "stats-month",
        "path": "/app/tokens/api/stats/202603",
        "params": {},
        "status": 200,
    },
    # apps/transcripts/routes.py
    {
        "app": "transcripts",
        "name": "ranges",
        "path": "/app/transcripts/api/ranges/20260304",
        "params": {},
        "status": 200,
    },
    {
        "app": "transcripts",
        "name": "segments",
        "path": "/app/transcripts/api/segments/20260304",
        "params": {},
        "status": 200,
    },
    {
        "app": "transcripts",
        "name": "segment-detail",
        "path": "/app/transcripts/api/segment/20260304/default/090000_300",
        "params": {},
        "status": 200,
    },
    {
        "app": "transcripts",
        "name": "stats-month",
        "path": "/app/transcripts/api/stats/202603",
        "params": {},
        "status": 200,
    },
    # apps/graph/routes.py
    {
        "app": "graph",
        "name": "graph",
        "path": "/app/graph/api/graph",
        "params": {},
        "status": 200,
        "sandbox_only": True,
    },
]


def normalize(data: Any, journal_path: str) -> Any:
    """Return a normalized copy of endpoint JSON for deterministic baselines."""

    resolved_journal = str(Path(journal_path).resolve())
    project_root = str(Path(__file__).resolve().parent.parent)

    # Journal path contains project root, so replace journal first (longer match)
    path_replacements: list[tuple[str, str]] = []
    path_replacements.append((resolved_journal, "<JOURNAL>"))
    # If the fixture journal resolves differently (e.g., symlinks), add that too
    fixture_journal = str((Path.cwd() / "tests" / "fixtures" / "journal").resolve())
    if fixture_journal != resolved_journal:
        path_replacements.append((fixture_journal, "<JOURNAL>"))
    # Also match the raw (possibly relative) journal_path
    raw_journal = str(journal_path)
    if raw_journal != resolved_journal:
        path_replacements.append((raw_journal, "<JOURNAL>"))
    # Match the SOLSTONE_JOURNAL env var if set (may be relative)
    env_journal = os.environ.get("SOLSTONE_JOURNAL", "")
    if env_journal and env_journal not in (resolved_journal, raw_journal):
        path_replacements.append((env_journal, "<JOURNAL>"))
    path_replacements.append((project_root, "<PROJECT>"))
    # Sort by length descending so longer (more specific) paths match first
    path_replacements.sort(key=lambda x: len(x[0]), reverse=True)

    def _normalize_string(value: str) -> str:
        result = value
        for path, replacement in path_replacements:
            result = result.replace(path, replacement)
        # Normalize dynamic timestamp in prompt content
        result = re.sub(
            r"^Today is .*", "Today is <TIMESTAMP>", result, flags=re.MULTILINE
        )
        return result

    def walk(value: Any, key: str | None = None) -> Any:
        if isinstance(value, dict):
            result = {
                item_key: (
                    0
                    if item_key in {"mtime", "created_at", "file_mtime"}
                    and isinstance(item_value, (int, float))
                    else (
                        "<TIMESTAMP>"
                        if item_key == "generated_at" and isinstance(item_value, str)
                        else (
                            round(item_value, 1)
                            if item_key in {"score", "recency"}
                            and isinstance(item_value, float)
                            else walk(item_value, item_key)
                        )
                    )
                )
                for item_key, item_value in value.items()
            }
            if key == "provider_status":
                for _name, status in result.items():
                    if isinstance(status, dict) and "cogitate_cli" in status:
                        status["cogitate_cli_found"] = False
                        status["cogitate_ready"] = False
                        status["configured"] = False
                        status["generate_ready"] = False
                        cli = status.get("cogitate_cli", "")
                        issues = [
                            i
                            for i in status.get("issues", [])
                            if "CLI not found" not in i
                            and "not set" not in i
                            and "not reachable" not in i
                        ]
                        if cli and _name not in {"anthropic", "openai"}:
                            if _name == "ollama":
                                issues.append(
                                    f"{cli} CLI not found on PATH — run: "
                                    "curl -fsSL https://opencode.ai/install | bash"
                                )
                            else:
                                issues.append(f"{cli} CLI not found on PATH")
                        # Re-add generic key-not-set issues per provider
                        env_keys = {
                            "anthropic": "ANTHROPIC_API_KEY",
                            "google": "GOOGLE_API_KEY",
                            "openai": "OPENAI_API_KEY",
                        }
                        if _name in env_keys:
                            issues.append(f"{env_keys[_name]} not set")
                        if _name == "ollama":
                            issues.append(
                                "Ollama not reachable at http://localhost:11434"
                            )
                        status["issues"] = sorted(issues)
            if key == "bundled":
                for _name, status in result.items():
                    if isinstance(status, dict):
                        if "last_transition_at" in status:
                            status["last_transition_at"] = "<TIMESTAMP>"
                        if "binary_path" in status and status["binary_path"]:
                            status["binary_path"] = "<PATH>"
                        if "binary_exists" in status:
                            status["binary_exists"] = False
            # Normalize env-dependent API key presence
            if key in ("api_keys", "runtime_env"):
                for k in result:
                    if isinstance(result[k], bool):
                        result[k] = False
            return result

        if isinstance(value, list):
            walked = [walk(item, key) for item in value]
            # Sort lists of dicts for deterministic comparison
            if walked and all(isinstance(item, dict) for item in walked):
                try:
                    walked.sort(key=lambda x: json.dumps(x, sort_keys=True))
                except TypeError:
                    pass
            return walked

        if isinstance(value, str):
            return _normalize_string(str(value))

        return value

    return walk(data)


def normalize_for_compare(
    endpoint: dict[str, Any], data: Any, journal_path: str
) -> Any:
    """Normalize payloads for deterministic baseline comparison."""

    normalized = normalize(data, journal_path)
    if endpoint["app"] == "settings" and endpoint["name"] == "transcribe":
        if isinstance(normalized, dict):
            # runtime_label is environment-dependent; tested separately.
            normalized.pop("runtime_label", None)
    return normalized


def baseline_path(endpoint: dict[str, str]) -> Path:
    """Compute baseline file path for an endpoint entry."""

    return Path("tests/baselines/api") / endpoint["app"] / f"{endpoint['name']}.json"


def _extract_json(response: Any) -> Any:
    """Load JSON from either Flask response or requests response."""

    if hasattr(response, "get_json"):
        payload = response.get_json(silent=True)
        if payload is None:
            raise ValueError("response is not JSON")
        return payload

    try:
        return response.json()
    except Exception as exc:
        raise ValueError("response is not JSON") from exc


def fetch_endpoint(client: Any, endpoint: dict[str, Any]) -> tuple[int, Any]:
    """Call endpoint and return (status_code, parsed_json)."""

    response = client.get(endpoint["path"], query_string=endpoint.get("params", {}))
    return response.status_code, _extract_json(response)


def verify_all(client: Any, journal_path: str) -> list[str]:
    """Compare all endpoint responses against stored baselines."""

    failures: list[str] = []
    for endpoint in ENDPOINTS:
        identifier = f"{endpoint['app']}/{endpoint['name']}"
        path = baseline_path(endpoint)

        try:
            status, payload = fetch_endpoint(client, endpoint)
        except Exception as exc:
            failures.append(f"{identifier}: failed to fetch endpoint: {exc}")
            continue

        if status != endpoint["status"]:
            failures.append(
                f"{identifier}: expected status {endpoint['status']} got {status}"
            )
            continue

        if not path.exists():
            failures.append(f"{identifier}: baseline file not found: {path}")
            continue

        actual = normalize_for_compare(endpoint, payload, journal_path)
        expected = normalize_for_compare(
            endpoint, json.loads(path.read_text()), journal_path
        )
        if actual != expected:
            actual_dump = json.dumps(
                actual, indent=2, sort_keys=True, ensure_ascii=False
            ).splitlines(keepends=True)
            expected_dump = json.dumps(
                expected, indent=2, sort_keys=True, ensure_ascii=False
            ).splitlines(keepends=True)
            diff = "".join(
                unified_diff(
                    expected_dump,
                    actual_dump,
                    fromfile=f"{identifier} expected",
                    tofile=f"{identifier} actual",
                    lineterm="",
                )
            )
            failures.append(f"{identifier}:\n{diff}")

    return failures


def update_all(
    client: Any,
    journal_path: str,
    *,
    include_sandbox_only: bool,
) -> int:
    """Refresh all endpoint baselines from current responses."""

    updated = 0
    for endpoint in ENDPOINTS:
        if endpoint.get("sandbox_only") and not include_sandbox_only:
            continue
        identifier = f"{endpoint['app']}/{endpoint['name']}"
        path = baseline_path(endpoint)
        path.parent.mkdir(parents=True, exist_ok=True)

        status, payload = fetch_endpoint(client, endpoint)
        if status != endpoint["status"]:
            print(
                f"warn: {identifier} returned {status}, expected {endpoint['status']}, "
                "still writing normalized payload"
            )

        normalized = normalize(payload, journal_path)
        path.write_text(
            json.dumps(normalized, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
        )
        updated += 1

    return updated


class _HttpClient:
    """Minimal requests-like object for endpoint fetching."""

    def __init__(self, base_url: str, password: str | None = None) -> None:
        self.base_url = base_url.rstrip("/")
        self._auth = ("", password) if password else None

    def get(self, path: str, query_string: dict[str, Any] | None = None):
        import requests

        return requests.get(
            f"{self.base_url}{path}", params=query_string, auth=self._auth
        )


def _resolve_journal_path() -> str:
    """Resolve journal path from env or sandbox metadata."""

    env_path = Path.cwd() / "tests" / "fixtures" / "journal"
    journal = Path(os.environ.get("SOLSTONE_JOURNAL", str(env_path)))
    if journal.is_absolute():
        return str(journal)
    return str(Path(journal).resolve())


def _resolve_sandbox_journal() -> str | None:
    marker = Path(".sandbox.journal")
    if not marker.exists():
        return None
    value = marker.read_text().strip()
    if not value:
        return None
    return str(Path(value).resolve())


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="API baseline verification tool")
    parser.add_argument(
        "command",
        choices=["verify", "update"],
        help="Whether to verify or regenerate baselines",
    )
    parser.add_argument(
        "--base-url",
        help="Use HTTP mode against this base URL instead of Flask test client",
    )
    parser.add_argument(
        "--password",
        help="Password for Basic Auth in HTTP mode",
    )
    return parser.parse_args(argv)


def _resolve_http_journal() -> str:
    env_path = os.environ.get("SOLSTONE_JOURNAL")
    if env_path:
        return str(Path(env_path).resolve())
    sandbox_path = _resolve_sandbox_journal()
    if sandbox_path:
        return sandbox_path
    return _resolve_journal_path()


@contextmanager
def client_context(
    base_url: str | None, password: str | None = None
) -> tuple[Any, str]:
    if base_url:
        yield _HttpClient(base_url, password=password), _resolve_http_journal()
        return

    with TemporaryDirectory(prefix="api-baseline-journal-") as tmpdir:
        journal_path = prepare_isolated_journal(Path(tmpdir) / "journal")
        with freeze_time(FROZEN_DATE, tz_offset=FROZEN_TZ_OFFSET):
            with isolated_app_env(journal_path):
                yield make_logged_in_test_client(journal_path), str(journal_path)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    with client_context(args.base_url, password=args.password) as (
        client,
        journal_path,
    ):
        if args.command == "verify":
            failures = verify_all(client, journal_path)
            if failures:
                print(f"API baseline verification failed ({len(failures)} endpoints):")
                for item in failures:
                    print(item)
                    print()
                print("Run 'make update-api-baselines' to update baselines")
                return 1
            print(f"API baseline verification passed for {len(ENDPOINTS)} endpoints.")
            return 0

        updated = update_all(
            client,
            journal_path,
            include_sandbox_only=bool(args.base_url),
        )
        print(f"Updated {updated} baseline files.")
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
