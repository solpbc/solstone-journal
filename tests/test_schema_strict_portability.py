# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Offline CI gate for req_bfbdbux6 strict schema portability."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest

from solstone.apps.timeline.rollup import build_rollup_schema
from solstone.think.talent import hydrate_runtime_enums

REPO_ROOT = Path(__file__).resolve().parents[1]
BANNED_KEYS = frozenset(
    {
        "$schema",
        "$comment",
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
        "minimum",
        "maximum",
    }
)


def _discover_schemas() -> tuple[tuple[str, dict[str, Any]], ...]:
    discovered: list[tuple[str, dict[str, Any]]] = []
    for path in sorted((REPO_ROOT / "solstone").glob("**/*.schema.json")):
        schema_id = path.relative_to(REPO_ROOT).as_posix()
        discovered.append((schema_id, json.loads(path.read_text(encoding="utf-8"))))
    discovered.append(("build_rollup_schema(3)", build_rollup_schema(3)))
    return tuple(discovered)


SCHEMAS = _discover_schemas()


def violations(schema: dict[str, Any]) -> list[str]:
    found: list[str] = []

    root_is_object = schema.get("type") == "object" or (
        "properties" in schema and "type" not in schema
    )
    if not root_is_object:
        found.append("$: root schema must be an object")

    def walk(node: Any, path: str) -> None:
        if isinstance(node, dict):
            for key in node:
                if key in BANNED_KEYS:
                    found.append(f"{path}: banned key {key!r}")
                if key == "oneOf":
                    found.append(f"{path}: banned key 'oneOf'")

            if node.get("type") == "object" or "properties" in node:
                if node.get("additionalProperties") is not False:
                    found.append(f"{path}: object missing additionalProperties:false")
                properties = node.get("properties") or {}
                required = node.get("required") or []
                missing = sorted(set(properties) - set(required))
                if missing:
                    found.append(f"{path}: properties not required {missing!r}")

            for key, value in node.items():
                walk(value, f"{path}/{key}")
        elif isinstance(node, list):
            for index, value in enumerate(node):
                walk(value, f"{path}[{index}]")

    walk(schema, "$")
    return found


def banned_key_hits(schema: dict[str, Any]) -> list[str]:
    found: list[str] = []

    def walk(node: Any, path: str) -> None:
        if isinstance(node, dict):
            for key, value in node.items():
                if key in BANNED_KEYS:
                    found.append(f"{path}: banned key {key!r}")
                walk(value, f"{path}/{key}")
        elif isinstance(node, list):
            for index, value in enumerate(node):
                walk(value, f"{path}[{index}]")

    walk(schema, "$")
    return found


@pytest.mark.parametrize(
    ("schema_id", "schema"),
    [pytest.param(schema_id, schema, id=schema_id) for schema_id, schema in SCHEMAS],
)
def test_all_discovered_schemas_are_strict_portable(
    schema_id: str, schema: dict[str, Any]
) -> None:
    schema_violations = violations(schema)
    assert schema_violations == [], f"{schema_id}: {schema_violations}"


@pytest.mark.parametrize(
    "schema_path",
    [
        Path("solstone/talent/schedule.schema.json"),
        Path("solstone/talent/sense.schema.json"),
    ],
)
def test_zero_facet_runtime_hydration_of_shipped_schemas_has_no_banned_keys(
    schema_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr("solstone.think.talent._valid_runtime_facets", lambda: [])
    schema = json.loads((REPO_ROOT / schema_path).read_text(encoding="utf-8"))

    hydrated = hydrate_runtime_enums(schema)

    assert banned_key_hits(hydrated) == []


@pytest.mark.parametrize(
    "schema",
    [
        {
            "type": "object",
            "$comment": "bad",
            "properties": {
                "a": {"type": "array", "minItems": 1},
                "b": {"type": "string"},
            },
            "required": ["a"],
            "additionalProperties": False,
        }
    ],
)
def test_strict_portability_guard_rejects_bad_schema(schema: dict[str, Any]) -> None:
    assert violations(schema)
