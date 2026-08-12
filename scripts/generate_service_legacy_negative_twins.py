#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Generate and verify bundled negative service-evidence twins.

This is a hand-run evidence-capture tool, not a CI dependency. It derives
field mutations from each default raw fixture and rejects every mutation using
the same normalization path as the committed corpus.
"""

from __future__ import annotations

import base64
import copy
import hashlib
import json
import plistlib
import re
import shutil
import xml.etree.ElementTree as element_tree
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from normalize_service_legacy_evidence import (
    FOLLOW_CENSUS,
    NORMALIZED_ROOT,
    PLATFORMS,
    RAW_ROOT,
    NormalizationError,
    normalized_variant,
    plist_from_raw,
    read_json,
)

ROOT = Path(__file__).resolve().parents[1]
NEGATIVE_ROOT = ROOT / "core/fixtures/service_legacy_evidence/negative"
SCHEMA = "service-legacy-negative-twins"
SCHEMA_VERSION = 1
MUTATION_KINDS = ("missing", "duplicate", "wrong-type", "changed-value", "unrelated-extra")
SECTION_RE = re.compile(r"^\[([^]\r\n]+)\]$")


class TwinError(RuntimeError):
    """A negative twin is malformed or unexpectedly accepted."""


@dataclass(frozen=True)
class Field:
    artifact: str
    identity: str
    location: tuple[str | int, ...]
    kind: str


def clone_raw(raw: dict[str, Any]) -> dict[str, Any]:
    return copy.deepcopy(raw)


def write_plist(raw: dict[str, Any], plist: dict[str, Any]) -> None:
    raw["raw"]["plist_base64"] = base64.b64encode(plistlib.dumps(plist, sort_keys=False)).decode("ascii")
    refresh_digest(raw)


def write_unit(raw: dict[str, Any], unit: str) -> None:
    raw["raw"]["systemd_unit"] = unit
    refresh_digest(raw)


def refresh_digest(raw: dict[str, Any]) -> None:
    combined = base64.b64decode(raw["raw"]["plist_base64"]) + bytes([0]) + raw["raw"]["systemd_unit"].encode("utf-8")
    raw["raw"]["sha256"] = hashlib.sha256(combined).hexdigest()


def at_path(root: Any, path: tuple[str | int, ...]) -> Any:
    value = root
    for part in path:
        value = value[part]
    return value


def parent_and_key(root: Any, path: tuple[str | int, ...]) -> tuple[Any, str | int]:
    if not path:
        raise TwinError("field path cannot be empty")
    return at_path(root, path[:-1]), path[-1]


def plist_fields(value: Any, path: tuple[str | int, ...] = ()) -> list[Field]:
    fields: list[Field] = []
    if isinstance(value, dict):
        for key in sorted(value):
            child_path = (*path, key)
            fields.append(Field("plist", "plist." + ".".join(map(str, child_path)), child_path, "dict"))
            fields.extend(plist_fields(value[key], child_path))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            child_path = (*path, index)
            fields.append(Field("plist", "plist." + ".".join(map(str, child_path)), child_path, "list"))
            fields.extend(plist_fields(child, child_path))
    return fields


def changed_value(value: Any) -> Any:
    if isinstance(value, bool):
        return not value
    if isinstance(value, int):
        return value + 1
    if isinstance(value, str):
        return "__SERVICE_LEGACY_TWIN_CHANGED__"
    if isinstance(value, list):
        changed = copy.deepcopy(value)
        if changed:
            changed[0] = changed_value(changed[0])
        else:
            changed.append("__SERVICE_LEGACY_TWIN_CHANGED__")
        return changed
    if isinstance(value, dict):
        changed = copy.deepcopy(value)
        key = sorted(changed)[0]
        changed[key] = changed_value(changed[key])
        return changed
    raise TwinError(f"unsupported plist value type: {type(value).__name__}")


def wrong_type(value: Any) -> Any:
    if isinstance(value, dict):
        return "__SERVICE_LEGACY_TWIN_WRONG_TYPE__"
    if isinstance(value, list):
        return {"wrong": "type"}
    return []


def plist_mutation(raw: dict[str, Any], field: Field, kind: str) -> dict[str, Any]:
    candidate = clone_raw(raw)
    plist = plist_from_raw(candidate, Path("<negative-twin>"))
    parent, key = parent_and_key(plist, field.location)
    value = parent[key]
    if kind == "missing":
        del parent[key]
    elif kind == "duplicate":
        if isinstance(parent, list):
            parent.insert(int(key) + 1, copy.deepcopy(value))
            write_plist(candidate, plist)
            return candidate
        return duplicate_plist_key(candidate, field.location)
    elif kind == "wrong-type":
        parent[key] = wrong_type(value)
    elif kind == "changed-value":
        parent[key] = changed_value(value)
    elif kind == "unrelated-extra":
        if isinstance(parent, dict):
            parent[f"__UNRELATED_EXTRA__{key}"] = "__SERVICE_LEGACY_TWIN_EXTRA__"
        else:
            parent.insert(int(key) + 1, "__SERVICE_LEGACY_TWIN_EXTRA__")
    else:
        raise TwinError(f"unknown mutation kind: {kind}")
    write_plist(candidate, plist)
    return candidate


def paired_value(dictionary: element_tree.Element, key: str) -> element_tree.Element:
    children = list(dictionary)
    for index in range(0, len(children), 2):
        if children[index].tag == "key" and children[index].text == key:
            return children[index + 1]
    raise TwinError(f"plist XML key is missing: {key}")


def xml_value_at(dictionary: element_tree.Element, path: tuple[str | int, ...]) -> element_tree.Element:
    value: element_tree.Element = dictionary
    for part in path:
        if isinstance(part, str):
            if value.tag != "dict":
                raise TwinError(f"plist XML path expects dict before {part}")
            value = paired_value(value, part)
        else:
            if value.tag != "array":
                raise TwinError(f"plist XML path expects array before {part}")
            value = list(value)[part]
    return value


def duplicate_plist_key(raw: dict[str, Any], path: tuple[str | int, ...]) -> dict[str, Any]:
    if not isinstance(path[-1], str):
        raise TwinError("XML duplicate requires a dictionary key")
    candidate = clone_raw(raw)
    data = base64.b64decode(candidate["raw"]["plist_base64"])
    root = element_tree.fromstring(data)
    dictionary = xml_value_at(next(child for child in root if child.tag == "dict"), path[:-1])
    children = list(dictionary)
    for index in range(0, len(children), 2):
        if children[index].tag == "key" and children[index].text == path[-1]:
            dictionary.insert(index + 2, copy.deepcopy(children[index]))
            dictionary.insert(index + 3, copy.deepcopy(children[index + 1]))
            candidate["raw"]["plist_base64"] = base64.b64encode(element_tree.tostring(root, encoding="utf-8")).decode("ascii")
            refresh_digest(candidate)
            return candidate
    raise TwinError(f"cannot duplicate plist key: {path}")


def plist_shape(value: Any) -> Any:
    if isinstance(value, dict):
        return ("dict", tuple((key, plist_shape(child)) for key, child in sorted(value.items())))
    if isinstance(value, list):
        return ("list", tuple(plist_shape(child) for child in value))
    return type(value).__name__


def xml_has_duplicate_keys(encoded: str) -> bool:
    root = element_tree.fromstring(base64.b64decode(encoded))
    for dictionary in root.iter("dict"):
        keys = [child.text for child in list(dictionary)[::2] if child.tag == "key"]
        if len(keys) != len(set(keys)):
            return True
    return False


def unit_lines(unit: str) -> list[str]:
    return unit.splitlines()


def unit_fields(unit: str) -> list[Field]:
    fields: list[Field] = []
    section = ""
    for index, line in enumerate(unit_lines(unit)):
        header = SECTION_RE.match(line)
        if header:
            section = header.group(1)
            fields.append(Field("systemd", f"systemd.section.{section}", (index,), "section"))
        elif line and not line.startswith(("#", ";")):
            key, separator, value = line.partition("=")
            if not separator:
                raise TwinError(f"invalid canonical systemd line: {line!r}")
            identity = f"systemd.{section}.{key}"
            if key == "Environment":
                env_key, env_separator, _env_value = value.partition("=")
                if not env_separator:
                    raise TwinError(f"invalid canonical environment line: {line!r}")
                identity += f".{env_key}"
            fields.append(Field("systemd", identity, (index,), "line"))
    return fields


def unit_mutation(raw: dict[str, Any], field: Field, kind: str) -> dict[str, Any] | None:
    candidate = clone_raw(raw)
    lines = unit_lines(candidate["raw"]["systemd_unit"])
    index = int(field.location[0])
    line = lines[index]
    key = line.partition("=")[0]
    if kind == "duplicate" and field.kind == "line" and key == "Environment":
        return None
    if kind == "missing":
        del lines[index]
    elif kind == "duplicate":
        lines.insert(index + 1, line)
    elif kind == "wrong-type":
        lines[index] = line.strip("[]") if field.kind == "section" else key
    elif kind == "changed-value":
        if field.kind == "section":
            lines[index] = f"[{line.strip('[]')}_TWIN]"
        else:
            lines[index] = f"{key}=__SERVICE_LEGACY_TWIN_CHANGED__"
    elif kind == "unrelated-extra":
        lines.insert(index + 1, "[UNRELATED_EXTRA]" if field.kind == "section" else "UNRELATED_EXTRA=1")
    else:
        raise TwinError(f"unknown mutation kind: {kind}")
    write_unit(candidate, "\n".join(lines) + "\n")
    return candidate


def unit_shape(unit: str) -> tuple[tuple[str, tuple[str, ...]], ...]:
    sections: list[tuple[str, list[str]]] = []
    names: set[str] = set()
    current: list[str] | None = None
    for line in unit_lines(unit):
        if not line or line.startswith(("#", ";")):
            continue
        header = SECTION_RE.match(line)
        if header:
            name = header.group(1)
            if name in names:
                raise TwinError(f"duplicate systemd section: {name}")
            names.add(name)
            current = []
            sections.append((name, current))
            continue
        if current is None or "=" not in line:
            raise TwinError(f"invalid systemd field line: {line!r}")
        key, _value = line.split("=", 1)
        if not key:
            raise TwinError("systemd field has empty key")
        current.append(key)
    return tuple((name, tuple(keys)) for name, keys in sections)


def strict_shape_matches(candidate: dict[str, Any], base_plist_shape: Any, base_unit_shape: Any) -> bool:
    try:
        if xml_has_duplicate_keys(candidate["raw"]["plist_base64"]):
            return False
        plist = plist_from_raw(candidate, Path("<negative-twin>"))
        return plist_shape(plist) == base_plist_shape and unit_shape(candidate["raw"]["systemd_unit"]) == base_unit_shape
    except (TwinError, NormalizationError, element_tree.ParseError, ValueError, plistlib.InvalidFileException):
        return False


def canonical_members() -> set[str]:
    members: set[str] = set()
    for path in NORMALIZED_ROOT.rglob("*.json"):
        fixture = read_json(path)
        for variant in fixture["variants"].values():
            members.add(json.dumps({"plist": variant["plist"], "systemd_unit": variant["systemd_unit"]}, sort_keys=True))
    return members


def verified_twin(
    *,
    raw: dict[str, Any],
    field: Field,
    kind: str,
    candidate: dict[str, Any],
    base_plist_shape: Any,
    base_unit_shape: Any,
    members: set[str],
) -> dict[str, Any]:
    shape_matches = strict_shape_matches(candidate, base_plist_shape, base_unit_shape)
    expected_rejection = "normalization_not_member" if kind == "changed-value" and shape_matches else "shape_rejected"
    if shape_matches:
        try:
            normalized = normalized_variant(candidate, Path("<negative-twin>"))
        except NormalizationError:
            normalized = None
        if normalized is not None and json.dumps(normalized, sort_keys=True) in members:
            raise TwinError(f"negative twin normalized to an accepted corpus member: {field.identity}/{kind}")
    elif expected_rejection != "shape_rejected":
        raise TwinError(f"negative twin had unexpected shape rejection: {field.identity}/{kind}")
    digest = hashlib.sha256(
        f"{raw['blob']}:{raw['platform']}:{field.identity}:{kind}".encode("utf-8")
    ).hexdigest()
    return {
        "expected_rejection": expected_rejection,
        "field": field.identity,
        "id": digest,
        "mutation_kind": kind,
        "mutated": {
            "plist_base64": candidate["raw"]["plist_base64"],
            "systemd_unit": candidate["raw"]["systemd_unit"],
        },
    }


def twins_for(raw: dict[str, Any], members: set[str]) -> list[dict[str, Any]]:
    base_plist = plist_from_raw(raw, Path("<default-raw>"))
    base_plist_shape = plist_shape(base_plist)
    base_unit_shape = unit_shape(raw["raw"]["systemd_unit"])
    twins: list[dict[str, Any]] = []
    for field in [*plist_fields(base_plist), *unit_fields(raw["raw"]["systemd_unit"])]:
        for kind in MUTATION_KINDS:
            candidate = plist_mutation(raw, field, kind) if field.artifact == "plist" else unit_mutation(raw, field, kind)
            if candidate is None:
                continue
            twins.append(
                verified_twin(
                    raw=raw,
                    field=field,
                    kind=kind,
                    candidate=candidate,
                    base_plist_shape=base_plist_shape,
                    base_unit_shape=base_unit_shape,
                    members=members,
                )
            )
    return twins


def write_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8")


def main() -> int:
    entries = read_json(FOLLOW_CENSUS)["entries"]
    members = canonical_members()
    shutil.rmtree(NEGATIVE_ROOT, ignore_errors=True)
    count = 0
    for entry in entries:
        for platform in PLATFORMS:
            raw_path = RAW_ROOT / entry["blob"] / platform / "default.json"
            raw = read_json(raw_path)
            twins = twins_for(raw, members)
            write_json(
                NEGATIVE_ROOT / entry["blob"] / f"{platform}.json",
                {
                    "base_profile": "default",
                    "blob": entry["blob"],
                    "platform": platform,
                    "schema": SCHEMA,
                    "schema_version": SCHEMA_VERSION,
                    "twins": twins,
                },
            )
            count += len(twins)
    print(f"wrote {count} negative twins")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
