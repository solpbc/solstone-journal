#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
from __future__ import annotations

import argparse
import hashlib
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


def source_digest(root: Path) -> str:
    """Bind the committed inventory to every native authority source."""
    paths = sorted((root / AUTHORITY_ROOT).rglob("authority.toml"))
    if not paths:
        raise ValueError(f"{root / AUTHORITY_ROOT}: no authority.toml files")
    digest = hashlib.sha256()
    for path in paths:
        digest.update(path.relative_to(root).as_posix().encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def render(entries: list[AuthorityEntry], output: Path, digest: str) -> str:
    generated_dir = output.parent
    lines = [
        "// SPDX-License-Identifier: AGPL-3.0-only",
        "// Copyright (c) 2026 sol pbc",
        f"// authority-source-sha256: {digest}",
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
    partition_errors = check_same_surface_executable_path_prefixes(entries)
    if partition_errors:
        for error in partition_errors:
            print(error)
        return 1
    rendered = rustfmt(render(entries, output, source_digest(root)), output.parent)
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
