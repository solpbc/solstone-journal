#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
from __future__ import annotations

import argparse
import copy
import json
import os
import re
import subprocess
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
# The native-sol authority declarations. They used to live under `solstone/`,
# the Python package tree, and moved out with it; the shape below `AUTHORITY_ROOT`
# is unchanged, so every prefix rule here is one component shorter than it was.
AUTHORITY_ROOT = "core/native-sol"
DEFAULT_OUTPUT = (
    REPO_ROOT / "core/crates/solstone-core-sol-client/src/generated/inventory.rs"
)
SCHEMA = "native-sol-authority-v1"
PARAM_KEYS = {
    "name",
    "kind",
    "type",
    "required",
    "nargs",
    "multiple",
    "default",
    "options",
    "secondary",
    "hidden",
    "is_flag",
    "count",
    "flag_value",
}
PARAM_REQUIRED_KEYS = PARAM_KEYS - {"default", "flag_value"}
ORACLE_PATH = REPO_ROOT / "core/fixtures/native-sol/sol-call-grammar-v1.json"
ENTRY_TYPES = {
    "http",
    "moved-stub",
    "top-level-import",
    "top-level-link",
    "top-level-status",
    "local",
}
COMMAND_KINDS = {"command", "callback", "top-level"}
HTTP_METHODS = {"GET", "POST", "PUT", "PATCH", "DELETE"}
FINAL_ORACLE_TOTAL = 166
FINAL_HTTP_TOTAL = 161
FINAL_JOURNAL_PYTHON_COMPAT_TOTAL = 2
FINAL_TOP_LEVEL_IMPORT_TOTAL = 1
FINAL_TOP_LEVEL_LINK_TOTAL = 3
FINAL_TOP_LEVEL_STATUS_TOTAL = 1
FINAL_STUB_COUNTS = {"moved-stub": 2, "local": 1}
FINAL_HTTP_GROUP_COUNTS = {
    "activities": 6,
    "awareness": 4,
    "body": 3,
    "entities": 22,
    "facets": 3,
    "health": 4,
    "import": 5,
    "journal": 17,
    "link": 8,
    "profile": 4,
    "settings": 14,
    "sol": 2,
    "speakers": 31,
    "support": 15,
    "thinking": 18,
    "transcripts": 5,
}

# The frozen grammar records the pre-migration spellings.  These are the only
# two surviving commands whose native authority intentionally differs from it.
# Keep this table path-keyed and deliberately narrow: it documents migration
# policy rather than introducing a general per-authority override mechanism.
ORACLE_GRAMMAR_TRANSFORMS: dict[tuple[str, ...], dict[str, Any]] = {
    ("journal", "news"): {
        "path": ("journal", "news"),
        "help": "Read facet news.",
        "drop_params": {"write"},
    },
    ("journal", "retention", "purge"): {
        "path": ("journal", "retention", "list"),
        "help": "List original media ready for removal.",
        "drop_params": set(),
    },
    ("journal", "search"): {
        "path": ("journal", "search"),
        "help": "Search the journal index.\n\nUse 2-4 content terms instead of natural-language questions; question words\nlike what/how/did/when add noise in this keyword/BM25 index. Syntax: OR for\nany term, quoted phrases for exact text, and * for prefix matches. Zero\nresults means zero: broaden by dropping terms, using OR, then adding *.\nCounts help drill down with --facet, --agent, --day, and --time-bucket.\nResult ids are path:idx; read a hit with `solstone call journal read --path\n<path>` after stripping the :idx suffix.",
        "drop_params": set(),
    },
}

# These compatibility leaves are deliberately retired, rather than
# represented by a native authority.  They are excluded from the final native
# grammar partition while their removal from the Python compatibility host is
# completed in the later retirement round.
RETIRED_JOURNAL_ORACLE_PATHS = {
    ("journal", "export"),
    ("journal", "facet", "doctor"),
    ("journal", "facet", "merge"),
    ("journal", "merge"),
    ("chat", "start"),
    ("sol", "set-name"),
    ("sol", "reset"),
}


@dataclass(frozen=True)
class AuthorityEntry:
    authority: Path
    authority_path: str
    source: Path
    module: str
    surface: str
    path: tuple[str, ...]
    kind: str
    help: str
    params: list[dict[str, Any]]
    operation_id: str
    entry_type: str
    method: str | None
    route: str | None
    contract_operation_id: str | None
    handler: str
    resident: bool


def rust_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def rust_option(value: str | None) -> str:
    if value is None:
        return "None"
    return f"Some({rust_string(value)})"


def module_name(path: Path) -> str:
    name = re.sub(r"[^A-Za-z0-9_]", "_", path.as_posix())
    name = re.sub(r"_+", "_", name).strip("_")
    if name[0].isdigit():
        name = f"native_{name}"
    return name


def logical_command_path(authority: Path, root: Path) -> Path:
    return authority.relative_to(root / AUTHORITY_ROOT).parent / "command.rs"


def command_source(authority: Path, root: Path) -> Path:
    rel = authority.relative_to(root / AUTHORITY_ROOT).as_posix()
    parts = rel.split("/")
    if (
        len(parts) == 4
        and parts[0] == "apps"
        and parts[2] == "native"
        and parts[3] == "authority.toml"
    ):
        return (
            root
            / "core/crates/solstone-core-sol-client/native/apps"
            / parts[1]
            / "command.rs"
        )
    if (
        len(parts) == 4
        and parts[0] == "think"
        and parts[1] == "native"
        and parts[3] == "authority.toml"
    ):
        return (
            root
            / "core/crates/solstone-core-sol-client/native/think"
            / parts[2]
            / "command.rs"
        )
    if (
        len(parts) == 5
        and parts[0] == "think"
        and parts[1] == "tools"
        and parts[2] == "native"
        and parts[4] == "authority.toml"
    ):
        return (
            root
            / "core/crates/solstone-core-sol-client/native/tools"
            / parts[3]
            / "command.rs"
        )
    raise ValueError(f"{authority}: native command source prefix is not mapped")


def load_authority(path: Path, root: Path) -> list[AuthorityEntry]:
    try:
        data = tomllib.loads(path.read_text())
    except tomllib.TOMLDecodeError as error:
        raise ValueError(f"{path}: malformed TOML: {error}") from error

    if data.get("schema") != SCHEMA:
        raise ValueError(f"{path}: schema must be {SCHEMA!r}")
    source_name = require_string(data, "source", path)
    if source_name != "command.rs":
        raise ValueError(f"{path}: source must be 'command.rs'")
    source = command_source(path, root)
    if not source.is_file():
        raise ValueError(f"{path}: source {source_name!r} does not exist at {source}")
    entries = data.get("entries")
    if not isinstance(entries, list) or not entries:
        raise ValueError(f"{path}: entries must be a non-empty list")

    source_text = source.read_text()
    output: list[AuthorityEntry] = []
    for index, raw_entry in enumerate(entries):
        if not isinstance(raw_entry, dict):
            raise ValueError(f"{path}: entry {index} must be a table")
        output.append(parse_entry(path, source, source_text, raw_entry, index, root))
    return output


def parse_entry(
    authority: Path,
    source: Path,
    source_text: str,
    raw_entry: dict[str, Any],
    index: int,
    root: Path,
) -> AuthorityEntry:
    label = f"{authority}: entry {index}"
    raw_path = raw_entry.get("path")
    if (
        not isinstance(raw_path, list)
        or not raw_path
        or any(not isinstance(item, str) or not item for item in raw_path)
    ):
        raise ValueError(f"{label}: path must be a non-empty string list")
    command_path = tuple(raw_path)
    surface = raw_entry.get("surface", "sol-call")
    if surface not in {
        "sol-call",
        "sol-import",
        "sol-link",
        "sol-status",
    }:
        raise ValueError(f"{label}: unsupported surface {surface!r}")
    kind = require_string(raw_entry, "kind", Path(label))
    if kind not in COMMAND_KINDS:
        raise ValueError(f"{label}: unsupported kind {kind!r}")
    entry_type = require_string(raw_entry, "entry_type", Path(label))
    if entry_type not in ENTRY_TYPES:
        raise ValueError(f"{label}: unsupported entry_type {entry_type!r}")
    params = raw_entry.get("params", [])
    if not isinstance(params, list):
        raise ValueError(f"{label}: params must be a list")
    canonical_params: list[dict[str, Any]] = []
    for param_index, param in enumerate(params):
        if not isinstance(param, dict):
            raise ValueError(f"{label}: params[{param_index}] must be a table")
        keys = set(param)
        if not PARAM_REQUIRED_KEYS.issubset(keys) or not keys.issubset(PARAM_KEYS):
            raise ValueError(
                f"{label}: params[{param_index}] keys {sorted(keys)} must include "
                f"{sorted(PARAM_REQUIRED_KEYS)} and may include default/flag_value"
            )
        canonical_params.append({key: param.get(key) for key in PARAM_KEYS})

    handler = require_string(raw_entry, "handler", Path(label))
    if not re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", handler):
        raise ValueError(f"{label}: handler {handler!r} is not a Rust identifier")
    if re.search(rf"\bpub\s+fn\s+{re.escape(handler)}\s*\(", source_text) is None:
        raise ValueError(f"{label}: handler {handler!r} is missing from {source}")
    resident = raw_entry.get("resident", False)
    if not isinstance(resident, bool):
        raise ValueError(f"{label}: resident must be a boolean")
    if resident and (surface == "sol-call" or kind != "top-level"):
        raise ValueError(
            f"{label}: resident entries must be non-sol-call top-level commands"
        )

    method = raw_entry.get("method")
    route = raw_entry.get("route")
    contract_operation_id = raw_entry.get("contract_operation_id")
    if entry_type == "http":
        method = require_optional_string(method, "method", label)
        route = require_optional_string(route, "route", label)
        contract_operation_id = optional_string(
            contract_operation_id, "contract_operation_id", label
        )
        if method not in HTTP_METHODS:
            raise ValueError(f"{label}: unsupported HTTP method {method!r}")
        if (
            not route.startswith("/")
            or "//" in route
            or any(ch.isspace() for ch in route)
        ):
            raise ValueError(f"{label}: noncanonical route {route!r}")
    else:
        require_absent(method, "method", label)
        require_absent(route, "route", label)
        require_absent(contract_operation_id, "contract_operation_id", label)
        method = None
        route = None
        contract_operation_id = None

    return AuthorityEntry(
        authority=authority,
        authority_path=authority.relative_to(root).as_posix(),
        source=source,
        module=module_name(logical_command_path(authority, root)),
        surface=surface,
        path=command_path,
        kind=kind,
        help=require_text(raw_entry, "help", Path(label)),
        params=canonical_params,
        operation_id=require_string(raw_entry, "operation_id", Path(label)),
        entry_type=entry_type,
        method=method,
        route=route,
        contract_operation_id=contract_operation_id,
        handler=handler,
        resident=resident,
    )


def require_string(data: dict[str, Any], key: str, path: Path) -> str:
    value = data.get(key)
    if not isinstance(value, str) or not value:
        raise ValueError(f"{path}: {key} must be a non-empty string")
    return value


def require_text(data: dict[str, Any], key: str, path: Path) -> str:
    value = data.get(key)
    if not isinstance(value, str):
        raise ValueError(f"{path}: {key} must be a string")
    return value


def require_optional_string(value: Any, key: str, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError(f"{label}: {key} must be a non-empty string")
    return value


def optional_string(value: Any, key: str, label: str) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str) or not value:
        raise ValueError(f"{label}: {key} must be a non-empty string when present")
    return value


def require_absent(value: Any, key: str, label: str) -> None:
    if value is not None:
        raise ValueError(f"{label}: {key} is only valid for http entries")


def discover(root: Path) -> list[AuthorityEntry]:
    base = root / AUTHORITY_ROOT
    authority_paths = sorted(
        set(base.glob("**/native/authority.toml"))
        | set(base.glob("**/native/**/authority.toml"))
    )
    if not authority_paths:
        raise ValueError(
            f"no authority declarations under {base}; a re-rooted glob that "
            "matches nothing would leave every gate below vacuously green"
        )
    entries: list[AuthorityEntry] = []
    seen_paths: dict[tuple[str, tuple[str, ...]], Path] = {}
    seen_operations: dict[str, Path] = {}
    for authority in authority_paths:
        if is_private_app_authority(authority, root):
            continue
        for entry in load_authority(authority, root):
            path_key = (entry.surface, entry.path)
            if path_key in seen_paths:
                raise ValueError(
                    f"{entry.authority}: duplicate path {list(entry.path)!r} on surface {entry.surface!r}; "
                    f"first declared in {seen_paths[path_key]}"
                )
            if entry.operation_id in seen_operations:
                raise ValueError(
                    f"{entry.authority}: duplicate operation_id {entry.operation_id!r}; "
                    f"first declared in {seen_operations[entry.operation_id]}"
                )
            seen_paths[path_key] = entry.authority
            seen_operations[entry.operation_id] = entry.authority
            entries.append(entry)
    return entries


def is_private_app_authority(authority: Path, root: Path) -> bool:
    try:
        parts = authority.relative_to(root / AUTHORITY_ROOT).parts
    except ValueError:
        return False
    return len(parts) >= 3 and parts[0] == "apps" and parts[1].startswith("_")


def render(entries: list[AuthorityEntry], output: Path) -> str:
    generated_dir = output.parent
    lines = [
        "// SPDX-License-Identifier: AGPL-3.0-only",
        "// Copyright (c) 2026 sol pbc",
        "",
        "use crate::aggregate::{Handler, InventoryEntry};",
        "use crate::resident::ResidentHandler;",
        "",
    ]
    seen_modules: set[str] = set()
    for entry in entries:
        if entry.module in seen_modules:
            continue
        seen_modules.add(entry.module)
        rel = os.path.relpath(entry.source, generated_dir)
        lines.append(f"#[path = {rust_string(Path(rel).as_posix())}]")
        lines.append(f"mod {entry.module};")
    if entries:
        lines.append("")
    lines.append("pub const ENTRIES: &[InventoryEntry] = &[")
    for entry in entries:
        path_items = ", ".join(rust_string(item) for item in entry.path)
        params_json = json.dumps(
            entry.params, ensure_ascii=False, sort_keys=True, separators=(",", ":")
        )
        lines.extend(
            [
                "    InventoryEntry {",
                f"        surface: {rust_string(entry.surface)},",
                f"        path: &[{path_items}],",
                f"        kind: {rust_string(entry.kind)},",
                f"        help: {rust_string(entry.help)},",
                f"        authority_path: {rust_string(entry.authority_path)},",
                f"        params_json: {rust_string(params_json)},",
                f"        entry_type: {rust_string(entry.entry_type)},",
                f"        operation_id: {rust_string(entry.operation_id)},",
                f"        method: {rust_option(entry.method)},",
                f"        route: {rust_option(entry.route)},",
                f"        contract_operation_id: {rust_option(entry.contract_operation_id)},",
                f"        handler: {rust_string(entry.handler)},",
                f"        resident: {str(entry.resident).lower()},",
                "    },",
            ]
        )
    lines.append("];")
    lines.append("")
    lines.append("pub const HANDLERS: &[Handler] = &[")
    for entry in entries:
        if entry.resident:
            continue
        lines.append(f"    {entry.module}::{entry.handler},")
    lines.append("];")
    lines.append("")
    lines.append("pub const RESIDENT_HANDLERS: &[ResidentHandler] = &[")
    for entry in entries:
        if not entry.resident:
            continue
        lines.append(f"    {entry.module}::{entry.handler},")
    lines.append("];")
    lines.append("")
    return "\n".join(lines)


def transformed_oracle_entries(
    oracle_path: Path,
) -> tuple[list[str], dict[tuple[str, ...], dict[str, Any]]]:
    """Return the final native grammar projection of the frozen oracle."""
    if not oracle_path.is_file():
        return [f"{oracle_path} is missing"], {}
    oracle = json.loads(oracle_path.read_text())
    output: dict[tuple[str, ...], dict[str, Any]] = {}
    errors: list[str] = []
    for raw_entry in oracle.get("entries", []):
        if not isinstance(raw_entry, dict) or not isinstance(
            raw_entry.get("path"), list
        ):
            continue
        path = tuple(raw_entry["path"])
        if path in RETIRED_JOURNAL_ORACLE_PATHS:
            continue
        entry = copy.deepcopy(raw_entry)
        transform = ORACLE_GRAMMAR_TRANSFORMS.get(path)
        if transform is not None:
            entry["path"] = list(transform["path"])
            entry["help"] = transform["help"]
            entry["params"] = [
                param
                for param in entry.get("params", [])
                if param.get("name") not in transform["drop_params"]
            ]
            path = tuple(transform["path"])
        if path in output:
            errors.append(f"transformed oracle has duplicate path {list(path)!r}")
        output[path] = entry
    return errors, output


def check_oracle_subset(entries: list[AuthorityEntry], oracle_path: Path) -> list[str]:
    if not oracle_path.is_file():
        return [f"{oracle_path} is missing"]
    errors, oracle_entries = transformed_oracle_entries(oracle_path)
    for entry in entries:
        if entry.surface != "sol-call":
            continue
        expected = oracle_entries.get(entry.path)
        if expected is None:
            errors.append(
                f"{entry.authority}: path {list(entry.path)!r} is not in frozen oracle"
            )
            continue
        for field in ("kind", "help"):
            actual = getattr(entry, field)
            if actual != expected[field]:
                errors.append(
                    f"{entry.authority}: {list(entry.path)!r} {field} {actual!r} "
                    f"!= oracle {expected[field]!r}"
                )
        if entry.params != expected["params"]:
            errors.append(
                f"{entry.authority}: {list(entry.path)!r} params differ from frozen oracle"
            )
    return errors


def check_complete_partition(
    entries: list[AuthorityEntry],
    oracle_path: Path,
    *,
    expected_oracle_total: int = FINAL_ORACLE_TOTAL,
    expected_http_total: int = FINAL_HTTP_TOTAL,
    expected_journal_total: int = FINAL_JOURNAL_PYTHON_COMPAT_TOTAL,
    expected_stub_counts: dict[str, int] | None = None,
    expected_http_group_counts: dict[str, int] | None = None,
) -> list[str]:
    """Validate the final native/journal/stub partition."""

    expected_stub_counts = expected_stub_counts or FINAL_STUB_COUNTS
    expected_http_group_counts = expected_http_group_counts or FINAL_HTTP_GROUP_COUNTS
    oracle_errors, oracle_entries = transformed_oracle_entries(oracle_path)
    oracle_paths = set(oracle_entries)
    errors = list(oracle_errors)
    if len(oracle_paths) != expected_oracle_total:
        errors.append(
            f"oracle path count {len(oracle_paths)} != {expected_oracle_total}"
        )
    entries = [entry for entry in entries if entry.surface == "sol-call"]
    authority_paths: dict[tuple[str, ...], AuthorityEntry] = {}
    operation_ids: dict[str, AuthorityEntry] = {}
    for entry in entries:
        existing_path = authority_paths.get(entry.path)
        if existing_path is not None:
            errors.append(
                f"duplicate authority path {list(entry.path)!r}: "
                f"{existing_path.authority} and {entry.authority}"
            )
        authority_paths[entry.path] = entry
        existing_operation = operation_ids.get(entry.operation_id)
        if existing_operation is not None:
            errors.append(
                f"duplicate operation_id {entry.operation_id!r}: "
                f"{existing_operation.authority} and {entry.authority}"
            )
        operation_ids[entry.operation_id] = entry

    errors.extend(check_oracle_subset(entries, oracle_path))
    extra = sorted(set(authority_paths) - oracle_paths)
    if extra:
        errors.append(f"authority paths outside frozen oracle: {format_paths(extra)}")

    uncovered = sorted(oracle_paths - set(authority_paths))
    non_journal_uncovered = [
        path for path in uncovered if first_group(path) != "journal"
    ]
    if len(uncovered) != expected_journal_total:
        errors.append(
            f"uncovered oracle path count {len(uncovered)} != "
            f"journal Python-compat count {expected_journal_total}"
        )
    if non_journal_uncovered:
        errors.append(
            f"uncovered non-journal oracle paths: {format_paths(non_journal_uncovered)}"
        )
    expected_entry_type_counts = {"http": expected_http_total, **expected_stub_counts}
    actual_entry_type_counts: dict[str, int] = {}
    for entry in entries:
        actual_entry_type_counts[entry.entry_type] = (
            actual_entry_type_counts.get(entry.entry_type, 0) + 1
        )
    for entry_type, expected_count in sorted(expected_entry_type_counts.items()):
        actual = actual_entry_type_counts.get(entry_type, 0)
        if actual != expected_count:
            errors.append(f"{entry_type} authority count {actual} != {expected_count}")
    unexpected_entry_types = sorted(
        set(actual_entry_type_counts) - set(expected_entry_type_counts)
    )
    if unexpected_entry_types:
        errors.append(f"unexpected sol-call entry types: {unexpected_entry_types!r}")

    http_entries = [entry for entry in entries if entry.entry_type == "http"]
    expected_group_total = sum(expected_http_group_counts.values())
    if expected_group_total != expected_http_total:
        errors.append(
            f"expected HTTP group total {expected_group_total} != {expected_http_total}"
        )
    group_counts: dict[str, int] = {}
    for entry in http_entries:
        group = first_group(entry.path)
        group_counts[group] = group_counts.get(group, 0) + 1
    for group, expected_count in sorted(expected_http_group_counts.items()):
        actual = group_counts.get(group, 0)
        if actual != expected_count:
            errors.append(f"{group} HTTP authority count {actual} != {expected_count}")
    unexpected_groups = sorted(set(group_counts) - set(expected_http_group_counts))
    if unexpected_groups:
        errors.append(f"unexpected HTTP authority groups: {unexpected_groups!r}")
    return errors


def check_top_level_partition(entries: list[AuthorityEntry]) -> list[str]:
    errors: list[str] = []
    expected = {
        ("sol-import", "top-level-import"): FINAL_TOP_LEVEL_IMPORT_TOTAL,
        ("sol-link", "top-level-link"): FINAL_TOP_LEVEL_LINK_TOTAL,
        ("sol-status", "top-level-status"): FINAL_TOP_LEVEL_STATUS_TOTAL,
    }
    actual: dict[tuple[str, str], int] = {}
    for entry in entries:
        if entry.surface == "sol-call":
            continue
        key = (entry.surface, entry.entry_type)
        actual[key] = actual.get(key, 0) + 1
    for key, expected_count in sorted(expected.items()):
        actual_count = actual.get(key, 0)
        if actual_count != expected_count:
            errors.append(f"{key} authority count {actual_count} != {expected_count}")
    unexpected = sorted(set(actual) - set(expected))
    if unexpected:
        errors.append(f"unexpected top-level native authorities: {unexpected!r}")
    return errors


def is_strict_path_prefix(left: tuple[str, ...], right: tuple[str, ...]) -> bool:
    return len(left) < len(right) and right[: len(left)] == left


def check_same_surface_executable_path_prefixes(
    entries: list[AuthorityEntry],
) -> list[str]:
    errors: list[str] = []
    by_surface: dict[str, list[AuthorityEntry]] = {}
    for entry in entries:
        by_surface.setdefault(entry.surface, []).append(entry)
    for surface, surface_entries in sorted(by_surface.items()):
        ordered = sorted(
            surface_entries, key=lambda entry: (entry.path, entry.authority_path)
        )
        for index, left in enumerate(ordered):
            for right in ordered[index + 1 :]:
                prefix: AuthorityEntry | None = None
                child: AuthorityEntry | None = None
                if is_strict_path_prefix(left.path, right.path):
                    prefix = left
                    child = right
                elif is_strict_path_prefix(right.path, left.path):
                    prefix = right
                    child = left
                if prefix is None or child is None:
                    continue
                # help.rs:is_sol_call_group treats any leaf as not-a-group, so
                # moved-stub/callback leaves also make child paths unreachable.
                errors.append(
                    "native sol executable path prefix conflict on surface "
                    f"{surface!r}: {list(prefix.path)!r} declared in "
                    f"{prefix.authority} is a strict prefix of "
                    f"{list(child.path)!r} declared in {child.authority}"
                )
    return errors


def collect_oracle_paths(oracle_path: Path) -> tuple[list[str], set[tuple[str, ...]]]:
    if not oracle_path.is_file():
        return [f"{oracle_path} is missing"], set()
    oracle = json.loads(oracle_path.read_text())
    errors: list[str] = []
    paths: set[tuple[str, ...]] = set()
    for index, entry in enumerate(oracle.get("entries", [])):
        path = entry.get("path") if isinstance(entry, dict) else None
        if (
            not isinstance(path, list)
            or not path
            or any(not isinstance(item, str) or not item for item in path)
        ):
            errors.append(f"{oracle_path}: entries[{index}] has invalid path")
            continue
        path_tuple = tuple(path)
        if path_tuple in paths:
            errors.append(f"{oracle_path}: duplicate oracle path {path!r}")
        paths.add(path_tuple)
    return errors, paths


def first_group(path: tuple[str, ...]) -> str:
    return path[0] if path else ""


def format_paths(paths: list[tuple[str, ...]]) -> list[list[str]]:
    return [list(path) for path in paths]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build native sol generated inventory."
    )
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--check", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    output = args.output.resolve()
    entries = discover(root)
    partition_errors = check_complete_partition(entries, ORACLE_PATH)
    partition_errors.extend(check_top_level_partition(entries))
    partition_errors.extend(check_same_surface_executable_path_prefixes(entries))
    if partition_errors:
        for error in partition_errors:
            print(error)
        return 1
    rendered = rustfmt(render(entries, output), output.parent)
    if args.check:
        existing = output.read_text()
        if existing != rendered:
            print(f"{output} is stale; run make build-native-sol-inventory")
            return 1
        print(f"{output} is current")
        return 0
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(rendered)
    print(f"wrote {output}")
    return 0


def rustfmt(text: str, directory: Path) -> str:
    directory.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w", suffix=".rs", dir=directory, delete=False
    ) as handle:
        handle.write(text)
        temp_path = Path(handle.name)
    try:
        subprocess.run(["rustfmt", "--edition", "2024", str(temp_path)], check=True)
        return temp_path.read_text()
    finally:
        temp_path.unlink(missing_ok=True)


if __name__ == "__main__":
    raise SystemExit(main())
