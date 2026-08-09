# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Offline CI gate for strict structured-output schema portability."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest

from solstone.apps.timeline.rollup import build_rollup_schema

REPO_ROOT = Path(__file__).resolve().parents[1]


def _discover_schemas() -> tuple[tuple[str, dict[str, Any]], ...]:
    discovered: list[tuple[str, dict[str, Any]]] = []
    for path in sorted((REPO_ROOT / "solstone").glob("**/*.schema.json")):
        schema_id = path.relative_to(REPO_ROOT).as_posix()
        schema = json.loads(path.read_text(encoding="utf-8"))
        if isinstance(schema.get("x-journal-contract"), dict):
            continue
        discovered.append((schema_id, schema))
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
    "schema",
    [
        {
            "type": "object",
            "properties": {
                "a": {"type": "array"},
                "b": {"type": "string"},
                "c": {"oneOf": [{"type": "string"}, {"type": "integer"}]},
            },
            "required": ["a"],
        }
    ],
)
def test_strict_portability_guard_rejects_bad_schema(schema: dict[str, Any]) -> None:
    schema_violations = violations(schema)

    assert any(
        "object missing additionalProperties:false" in v for v in schema_violations
    )
    assert any("properties not required" in v for v in schema_violations)
    assert any("banned key 'oneOf'" in v for v in schema_violations)
