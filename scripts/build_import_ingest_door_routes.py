#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Record and check the Convey journal-import route table."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sys
import tempfile
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator

from corpus_scrub import (
    _host_identifiers,
    assert_egress_guard_can_see,
    assert_no_egress_attempted,
    assert_publishable,
    egress_attempts,
    forbid_non_loopback_egress,
)

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = REPO_ROOT / "core/fixtures/import_ingest_door_routes.json"
SCHEMA = "solstone-import-ingest-door-routes-v1"
GENERATOR = "scripts/build_import_ingest_door_routes.py"
SAMPLE_KEY_PREFIX = "fixturekeyx7q"
SAMPLE_AREA = "segments"
RULE_PREFIX = "/app/import/journal/<key_prefix>/"
AUTOMATIC_METHODS = frozenset({"HEAD", "OPTIONS"})
EGRESS_CONTROL_DESTINATIONS = frozenset({"example.invalid", "198.51.100.7"})
FINDING_LABELS = (
    "IPv4 literals:",
    "home-shaped paths:",
    "host/account identifiers:",
)
_IPV4 = re.compile(r"^\d{1,3}(?:\.\d{1,3}){3}$")


forbid_non_loopback_egress()
assert_egress_guard_can_see("import ingest door routes")
print("egress guard positive control fired")


@dataclass(frozen=True)
class Observation:
    rendered: bytes
    segments_status: int
    day_suffixed_status: int


def _casefold_overlaps(left: str, right: str) -> bool:
    left = left.casefold()
    right = right.casefold()
    return left in right or right in left


def _assert_authored_prefix_is_safe(identifiers: set[str]) -> None:
    if any(_casefold_overlaps(SAMPLE_KEY_PREFIX, value) for value in identifiers):
        raise RuntimeError(
            "sample key prefix overlaps a host or account identifier; choose a new authored value"
        )


def _home_control_path(identifiers: set[str]) -> str:
    for index in range(10_000):
        candidate = f"/home/zqxmv{index}r"
        if not any(_casefold_overlaps(candidate, value) for value in identifiers):
            return candidate
    raise RuntimeError(
        "could not choose a home-path control independent of host identifiers"
    )


def _identifier_control_value(identifiers: set[str]) -> str:
    candidates = sorted(
        value
        for value in identifiers
        if "/" not in value and not _IPV4.fullmatch(value)
    )
    if not candidates:
        raise RuntimeError(
            "cannot establish an independent host/account identifier control: "
            "the host supplied no non-path, non-IPv4 identifier"
        )
    return candidates[0]


def _expect_scrub_control(
    *, label: str, rendered: str, expected: str, absent: tuple[str, ...]
) -> None:
    try:
        assert_publishable(rendered, label=label)
    except RuntimeError as error:
        message = str(error)
    else:
        raise RuntimeError(
            f"{label}: publication guard did not report the planted value"
        )
    if expected not in message:
        raise RuntimeError(
            f"{label}: publication guard did not name {expected!r}: {message}"
        )
    unexpected = [finding for finding in absent if finding in message]
    if unexpected:
        raise RuntimeError(
            f"{label}: publication guard reported unrelated findings: {', '.join(unexpected)}"
        )


def run_scrub_controls() -> None:
    identifiers = _host_identifiers()
    _assert_authored_prefix_is_safe(identifiers)

    _expect_scrub_control(
        label="IPv4 control",
        rendered='{"address":"203.0.113.9"}',
        expected="IPv4 literals:",
        absent=FINDING_LABELS[1:],
    )
    print("scrub control: IPv4")

    _expect_scrub_control(
        label="home path control",
        rendered=json.dumps({"home": _home_control_path(identifiers)}),
        expected="home-shaped paths:",
        absent=(FINDING_LABELS[0], FINDING_LABELS[2]),
    )
    print("scrub control: home path")

    _expect_scrub_control(
        label="host/account identifier control",
        rendered=json.dumps({"identifier": _identifier_control_value(identifiers)}),
        expected="host/account identifiers:",
        absent=FINDING_LABELS[:2],
    )
    print("scrub control: host/account identifier")


@contextmanager
def recording_environment(root: Path) -> Iterator[None]:
    overrides = {
        "SOLSTONE_JOURNAL": str(root),
        "SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES": "1",
        "SOL_SKIP_SUPERVISOR_CHECK": "1",
    }
    previous = {key: os.environ.get(key) for key in overrides}
    os.environ.update(overrides)
    try:
        yield
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


def _sample_path(rule: str) -> str:
    return rule.replace("<key_prefix>", SAMPLE_KEY_PREFIX).replace(
        "<area>", SAMPLE_AREA
    )


def observe_reference() -> Observation:
    root = Path(tempfile.mkdtemp(prefix="import-ingest-door-", dir="/var/tmp"))
    try:
        with recording_environment(root):
            from solstone.convey import create_app

            app = create_app(str(root))
            rules = [
                {
                    "rule": rule.rule,
                    "methods": sorted(rule.methods - AUTOMATIC_METHODS),
                    "path": _sample_path(rule.rule),
                }
                for rule in app.url_map.iter_rules()
                if rule.rule.startswith(RULE_PREFIX)
            ]
            rules.sort(key=lambda item: item["rule"])
            if not rules:
                raise RuntimeError("reference registered no journal-import door rules")

            client = app.test_client()
            segments_path = f"/app/import/journal/{SAMPLE_KEY_PREFIX}/ingest/segments"
            segments_status = client.post(segments_path).status_code
            day_suffixed_status = client.post(f"{segments_path}/20260801").status_code
    finally:
        shutil.rmtree(root)

    document = {
        "schema": SCHEMA,
        "generator": GENERATOR,
        "sample_key_prefix": SAMPLE_KEY_PREFIX,
        "sample_area": SAMPLE_AREA,
        "rules": rules,
    }
    return Observation(
        rendered=(json.dumps(document, indent=2, sort_keys=True) + "\n").encode(),
        segments_status=segments_status,
        day_suffixed_status=day_suffixed_status,
    )


def _probe_failures(observation: Observation) -> list[str]:
    failures: list[str] = []
    if observation.segments_status == 404:
        failures.append("segments probe returned 404; expected a routed status")
    if observation.day_suffixed_status != 404:
        failures.append(
            "day-suffixed segments probe returned "
            f"{observation.day_suffixed_status}; expected 404"
        )
    return failures


def _fixture_failures(expected: object, actual: object) -> list[str]:
    if not isinstance(expected, dict) or not isinstance(actual, dict):
        return ["fixture document is not an object"]
    failures: list[str] = []
    for key in ("schema", "generator", "sample_key_prefix", "sample_area"):
        if expected.get(key) != actual.get(key):
            failures.append(
                f"fixture {key} differs: expected {expected.get(key)!r}, "
                f"reference produced {actual.get(key)!r}"
            )
    expected_rules = expected.get("rules")
    actual_rules = actual.get("rules")
    if not isinstance(expected_rules, list) or not isinstance(actual_rules, list):
        return [*failures, "fixture rules are not a list"]
    expected_by_rule = {
        item.get("rule"): item for item in expected_rules if isinstance(item, dict)
    }
    actual_by_rule = {
        item.get("rule"): item for item in actual_rules if isinstance(item, dict)
    }
    for rule in sorted(set(expected_by_rule) - set(actual_by_rule)):
        failures.append(f"reference no longer registers rule {rule!r}")
    for rule in sorted(set(actual_by_rule) - set(expected_by_rule)):
        failures.append(f"reference registers unexpected rule {rule!r}")
    for rule in sorted(set(expected_by_rule) & set(actual_by_rule)):
        for key in ("methods", "path"):
            if expected_by_rule[rule].get(key) != actual_by_rule[rule].get(key):
                failures.append(
                    f"rule {rule!r} {key} differs: "
                    f"expected {expected_by_rule[rule].get(key)!r}, "
                    f"reference produced {actual_by_rule[rule].get(key)!r}"
                )
    return failures


def _load_document(path: Path) -> object:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError(f"could not read fixture {path}: {error}") from error


def _finish_egress_check() -> None:
    assert_no_egress_attempted(
        "import ingest door routes", ignore=EGRESS_CONTROL_DESTINATIONS
    )
    attempts = [
        attempt
        for attempt in egress_attempts()
        if attempt not in EGRESS_CONTROL_DESTINATIONS
    ]
    print(f"egress attempts excluding controls: {attempts}")


def build(output: Path) -> int:
    run_scrub_controls()
    first = observe_reference()
    second = observe_reference()
    failures = _probe_failures(first) + _probe_failures(second)
    if first.rendered != second.rendered:
        failures.append("two isolated reference walks produced different fixture bytes")
    try:
        assert_publishable(first.rendered.decode(), label="import ingest door routes")
    except RuntimeError as error:
        failures.append(str(error))
    try:
        _finish_egress_check()
    except RuntimeError as error:
        failures.append(str(error))
    if failures:
        for failure in failures:
            print(f"error: {failure}", file=sys.stderr)
        return 1
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(first.rendered)
    print("door rules: non-empty")
    print(f"segments probe status: {first.segments_status}")
    print(f"day-suffixed probe status: {first.day_suffixed_status}")
    print("reference writes confined to throwaway roots")
    print("scrub: clean")
    print(f"wrote: {output}")
    return 0


def check() -> int:
    run_scrub_controls()
    observation = observe_reference()
    failures = _probe_failures(observation)
    try:
        expected = _load_document(DEFAULT_OUTPUT)
    except RuntimeError as error:
        failures.append(str(error))
    else:
        actual = json.loads(observation.rendered)
        failures.extend(_fixture_failures(expected, actual))
    try:
        assert_publishable(
            observation.rendered.decode(), label="import ingest door routes"
        )
    except RuntimeError as error:
        failures.append(str(error))
    try:
        _finish_egress_check()
    except RuntimeError as error:
        failures.append(str(error))
    if failures:
        for failure in failures:
            print(f"error: {failure}", file=sys.stderr)
        return 1
    print(
        "ok: journal-import door routes match the reference; "
        f"segments probe status {observation.segments_status}; "
        f"day-suffixed probe status {observation.day_suffixed_status}"
    )
    return 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="compare against the fixture"
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=DEFAULT_OUTPUT,
        help="fixture destination in build mode",
    )
    args = parser.parse_args()
    if args.check and args.output != DEFAULT_OUTPUT:
        parser.error("--output cannot be used with --check")
    return args


def main() -> int:
    args = parse_args()
    return check() if args.check else build(args.output)


if __name__ == "__main__":
    raise SystemExit(main())
