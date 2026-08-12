#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Derive parsed semantic deltas between adjacent service-generator blobs.

This is a hand-run evidence-capture tool, not a CI dependency. It compares
normalized plist values and parsed systemd section/key/value fields, never raw
serialization or formatting whitespace.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from normalize_service_legacy_evidence import (
    FOLLOW_CENSUS,
    NORMALIZED_ROOT,
    PLATFORMS,
    read_json,
)
from service_legacy_paths import evidence_root

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = evidence_root() / "semantic-deltas.json"
SCHEMA = "service-legacy-semantic-deltas"
SCHEMA_VERSION = 1


class DeltaError(RuntimeError):
    """A normalized fixture cannot be represented as parsed semantics."""


def flatten_plist(value: Any, field: str = "plist") -> dict[str, Any]:
    if isinstance(value, dict):
        if not value:
            return {field: {}}
        flattened: dict[str, Any] = {}
        for key in sorted(value):
            flattened.update(flatten_plist(value[key], f"{field}.{key}"))
        return flattened
    if isinstance(value, list):
        if not value:
            return {field: []}
        flattened = {}
        for index, child in enumerate(value):
            flattened.update(flatten_plist(child, f"{field}[{index}]"))
        return flattened
    return {field: value}


def parse_systemd(unit: str) -> dict[str, Any]:
    fields: dict[str, Any] = {}
    section: str | None = None
    for line in unit.splitlines():
        if not line or line.startswith(("#", ";")):
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1]
            fields[f"systemd.section.{section}"] = True
            continue
        if section is None or "=" not in line:
            raise DeltaError(f"invalid normalized systemd line: {line!r}")
        key, value = line.split("=", 1)
        field = f"systemd.{section}.{key}"
        if key == "Environment":
            environment_key, separator, environment_value = value.partition("=")
            if not separator:
                raise DeltaError(f"invalid normalized environment line: {line!r}")
            field += f".{environment_key}"
            value = environment_value
        if field in fields:
            raise DeltaError(f"duplicate normalized systemd semantic field: {field}")
        fields[field] = value
    return fields


def semantic_fields(blob: str, platform: str) -> dict[str, Any]:
    fixture = read_json(NORMALIZED_ROOT / blob / f"{platform}.json")
    variants = fixture.get("variants")
    if not isinstance(variants, dict):
        raise DeltaError(f"normalized fixture lacks variants: {blob}/{platform}")
    variant = variants.get("optional_keys_absent", variants.get("canonical"))
    if not isinstance(variant, dict):
        raise DeltaError(
            f"normalized fixture lacks semantic default variant: {blob}/{platform}"
        )
    plist = variant.get("plist")
    unit = variant.get("systemd_unit")
    if not isinstance(plist, dict) or not isinstance(unit, str):
        raise DeltaError(
            f"normalized fixture has invalid semantic content: {blob}/{platform}"
        )
    return {**flatten_plist(plist), **parse_systemd(unit)}


def changes_for(from_blob: str, to_blob: str, platform: str) -> list[dict[str, Any]]:
    old = semantic_fields(from_blob, platform)
    new = semantic_fields(to_blob, platform)
    changes: list[dict[str, Any]] = []
    for field in sorted(set(old) | set(new)):
        if field not in old:
            changes.append(
                {
                    "field": field,
                    "new_value": new[field],
                    "old_value": None,
                    "operation": "add",
                    "platform": platform,
                }
            )
        elif field not in new:
            changes.append(
                {
                    "field": field,
                    "new_value": None,
                    "old_value": old[field],
                    "operation": "remove",
                    "platform": platform,
                }
            )
        elif old[field] != new[field]:
            changes.append(
                {
                    "field": field,
                    "new_value": new[field],
                    "old_value": old[field],
                    "operation": "replace",
                    "platform": platform,
                }
            )
    return changes


def main() -> int:
    entries = read_json(FOLLOW_CENSUS).get("entries")
    if not isinstance(entries, list) or len(entries) != 44:
        raise DeltaError("follow census must contain exactly 44 entries")
    records: list[dict[str, Any]] = []
    for previous, current in zip(entries, entries[1:]):
        changes: list[dict[str, Any]] = []
        for platform in PLATFORMS:
            changes.extend(changes_for(previous["blob"], current["blob"], platform))
        records.append(
            {
                "changes": changes,
                "from_blob": previous["blob"],
                "to_blob": current["blob"],
            }
        )
    OUTPUT.write_text(
        json.dumps(
            {"deltas": records, "schema": SCHEMA, "schema_version": SCHEMA_VERSION},
            indent=2,
            sort_keys=True,
            ensure_ascii=False,
        )
        + "\n",
        encoding="utf-8",
    )
    print(
        f"wrote {len(records)} semantic-delta records with {sum(len(record['changes']) for record in records)} changes"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
