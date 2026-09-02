#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
from __future__ import annotations

import argparse
import ast
import hashlib
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
ORACLE_COMMIT = "d8200fdf34e4af31f106c7f28fb73cd439d0081b"
ORACLE_PATH = "solstone/think/sol_cli.py"
ORACLE_BLOB = "ea62371d5c320329724d051032efbe20f165b25f"
OUTPUT = REPO_ROOT / "core/crates/solstone-core-cli-boundary/src/generated.rs"
EXPECTED_SERVICE_COMMANDS_COUNT = 42
EXPECTED_UNIVERSAL_COMMANDS = frozenset({"doctor", "check", "contract"})
EXPECTED_SERVICE_ALIASES = frozenset({"up", "down"})
EXPECTED_UNIVERSAL_ALIASES = frozenset()
SERVICE_SENTINELS = frozenset({"think", "setup"})
# Commands that remain in the pinned Python oracle but are now implemented as
# root-native journal primitives. They are not journal-host process commands.
NATIVE_ROOT_COMMANDS = frozenset({"indexer"})
# Commands introduced after the pinned Python oracle that are native journal
# host commands rather than root-native primitives.
NATIVE_ADDITIONAL_HOST_COMMANDS = frozenset({"mcp", "thinking"})
# Commands that remain in the pinned Python oracle but have been retired from
# the live journal host grammar.
RETIRED_HOST_COMMANDS = frozenset({"export", "warm", "maint", "observer", "restart-convey"})
# Declaration order is the duplicate-diagnostic section order.
REGISTRY_SURFACE_POSITIONS = {"COMMANDS": 1, "ALIASES": 2}
UNAVAILABLE_SURFACE = "<unavailable>"


@dataclass(frozen=True)
class JournalHostCommandPartitions:
    service_commands: tuple[str, ...]
    universal_commands: tuple[str, ...]
    service_aliases: tuple[str, ...]
    universal_aliases: tuple[str, ...]


@dataclass(frozen=True)
class RegistryLiteral:
    name: str
    node: ast.Dict
    surface_position: int


@dataclass(frozen=True)
class RegistryKeyOccurrence:
    registry: str
    key: str
    surface: str
    lineno: int
    col_offset: int


def call_surface(node: ast.AST, position: int) -> str | None:
    if not isinstance(node, ast.Call) or len(node.args) <= position:
        return None
    surface = node.args[position]
    if isinstance(surface, ast.Constant) and isinstance(surface.value, str):
        return surface.value
    return None


def literal_key(key: ast.expr | None) -> str | None:
    if isinstance(key, ast.Constant) and isinstance(key.value, str):
        return key.value
    return None


def extract_partitions(source_text: str | None = None) -> JournalHostCommandPartitions:
    tree = ast.parse(source_text if source_text is not None else oracle_text())
    registry_literals = scan_registry_literals(tree)
    validate_no_duplicate_registry_keys(registry_literals)
    commands: dict[str, list[str]] = {"service": [], "universal": []}
    aliases: dict[str, list[str]] = {"service": [], "universal": []}
    for registry_literal in registry_literals:
        if registry_literal.name == "COMMANDS":
            extend_names_by_surface(
                commands, registry_literal.node, registry_literal.surface_position
            )
        elif registry_literal.name == "ALIASES":
            extend_names_by_surface(
                aliases, registry_literal.node, registry_literal.surface_position
            )
    partitions = JournalHostCommandPartitions(
        service_commands=tuple(sorted(commands["service"])),
        universal_commands=tuple(sorted(commands["universal"])),
        service_aliases=tuple(sorted(aliases["service"])),
        universal_aliases=tuple(sorted(aliases["universal"])),
    )
    validate_partitions(partitions)
    return partitions


def oracle_text() -> str:
    blob = (
        subprocess.check_output(
            ["git", "rev-parse", f"{ORACLE_COMMIT}:{ORACLE_PATH}"], cwd=REPO_ROOT
        )
        .decode()
        .strip()
    )
    if blob != ORACLE_BLOB:
        raise RuntimeError(
            f"{ORACLE_COMMIT}:{ORACLE_PATH} is {blob}, expected {ORACLE_BLOB}"
        )
    data = subprocess.check_output(
        ["git", "show", f"{ORACLE_COMMIT}:{ORACLE_PATH}"], cwd=REPO_ROOT
    )
    header = f"blob {len(data)}\0".encode()
    digest = hashlib.sha1(header + data, usedforsecurity=False).hexdigest()
    if digest != ORACLE_BLOB:
        raise RuntimeError(f"extracted oracle blob is {digest}, expected {ORACLE_BLOB}")
    return data.decode()


def scan_registry_literals(tree: ast.Module) -> tuple[RegistryLiteral, ...]:
    registry_literals: list[RegistryLiteral] = []
    for node in tree.body:
        if isinstance(node, ast.Assign) and len(node.targets) == 1:
            target = node.targets[0]
            value = node.value
        elif isinstance(node, ast.AnnAssign):
            target = node.target
            value = node.value
        else:
            continue
        if not isinstance(target, ast.Name) or not isinstance(value, ast.Dict):
            continue
        surface_position = REGISTRY_SURFACE_POSITIONS.get(target.id)
        if surface_position is None:
            continue
        registry_literals.append(RegistryLiteral(target.id, value, surface_position))
    return tuple(registry_literals)


def scan_registry_key_occurrences(
    registry_literals: tuple[RegistryLiteral, ...],
) -> tuple[RegistryKeyOccurrence, ...]:
    occurrences: list[RegistryKeyOccurrence] = []
    for registry_literal in registry_literals:
        for key, value in zip(
            registry_literal.node.keys, registry_literal.node.values, strict=True
        ):
            key_value = literal_key(key)
            if key_value is None:
                continue
            occurrences.append(
                RegistryKeyOccurrence(
                    registry=registry_literal.name,
                    key=key_value,
                    surface=call_surface(value, registry_literal.surface_position)
                    or UNAVAILABLE_SURFACE,
                    lineno=key.lineno,
                    col_offset=key.col_offset,
                )
            )
    return tuple(occurrences)


def validate_no_duplicate_registry_keys(
    registry_literals: tuple[RegistryLiteral, ...],
) -> None:
    occurrences_by_registry: dict[str, dict[str, list[RegistryKeyOccurrence]]] = {
        registry: {} for registry in REGISTRY_SURFACE_POSITIONS
    }
    for occurrence in scan_registry_key_occurrences(registry_literals):
        occurrences_by_registry[occurrence.registry].setdefault(
            occurrence.key, []
        ).append(occurrence)

    duplicate_sections: list[str] = []
    for registry in REGISTRY_SURFACE_POSITIONS:
        occurrences_by_key = occurrences_by_registry[registry]
        for key in sorted(
            key
            for key, occurrences in occurrences_by_key.items()
            if len(occurrences) > 1
        ):
            occurrences = sorted(
                occurrences_by_key[key],
                key=lambda occurrence: (occurrence.lineno, occurrence.col_offset),
            )
            occurrence_text = ", ".join(
                f"line {occurrence.lineno} surface={occurrence.surface}"
                for occurrence in occurrences
            )
            duplicate_sections.append(f"{registry} {key!r} [{occurrence_text}]")

    if duplicate_sections:
        raise RuntimeError(
            f"journal-host duplicate registry keys: {'; '.join(duplicate_sections)}"
        )


def extract(source_text: str | None = None) -> list[str]:
    partitions = extract_partitions(source_text)
    return sorted(
        (
            set(partitions.service_commands + partitions.service_aliases)
            - NATIVE_ROOT_COMMANDS
            - RETIRED_HOST_COMMANDS
        )
        | NATIVE_ADDITIONAL_HOST_COMMANDS
    )


def validate_partitions(partitions: JournalHostCommandPartitions) -> None:
    result = sorted(
        set(
            partitions.service_commands
            + partitions.universal_commands
            + partitions.service_aliases
            + partitions.universal_aliases
        )
    )
    if not result:
        raise RuntimeError("journal-host command extraction is empty")

    command_alias_overlap = sorted(
        set(partitions.service_commands + partitions.universal_commands)
        & set(partitions.service_aliases + partitions.universal_aliases)
    )
    if command_alias_overlap:
        raise RuntimeError(
            f"journal-host COMMANDS and ALIASES overlap: {command_alias_overlap!r}"
        )

    missing = sorted(SERVICE_SENTINELS - set(partitions.service_commands))
    if missing:
        raise RuntimeError(
            f"journal-host service COMMANDS missing sentinels: {missing!r}"
        )

    if len(partitions.service_commands) != EXPECTED_SERVICE_COMMANDS_COUNT:
        raise RuntimeError(
            "journal-host service COMMANDS count "
            f"{len(partitions.service_commands)} != {EXPECTED_SERVICE_COMMANDS_COUNT}: "
            f"{list(partitions.service_commands)!r}"
        )

    assert_exact_set(
        "journal-host universal COMMANDS",
        set(partitions.universal_commands),
        EXPECTED_UNIVERSAL_COMMANDS,
        changed_surface=set(partitions.service_commands),
    )
    assert_exact_set(
        "journal-host service ALIASES",
        set(partitions.service_aliases),
        EXPECTED_SERVICE_ALIASES,
        changed_surface=set(partitions.universal_aliases),
    )
    if set(partitions.universal_aliases) != EXPECTED_UNIVERSAL_ALIASES:
        raise RuntimeError(
            "journal-host universal ALIASES must be empty; "
            f"found={list(partitions.universal_aliases)!r}"
        )


def assert_exact_set(
    label: str,
    actual: set[str],
    expected: frozenset[str],
    *,
    changed_surface: set[str],
) -> None:
    if actual == expected:
        return
    missing = sorted(expected - actual)
    extra = sorted(actual - expected)
    changed = sorted(set(missing) & changed_surface)
    raise RuntimeError(
        f"{label} drifted; missing={missing!r}; extra={extra!r}; "
        f"changed_surface={changed!r}"
    )


def extend_names_by_surface(
    names_by_surface: dict[str, list[str]], node: ast.Dict, position: int
) -> None:
    for key, value in zip(node.keys, node.values, strict=True):
        key_value = literal_key(key)
        if key_value is None:
            continue
        surface = call_surface(value, position)
        if surface in names_by_surface:
            names_by_surface[surface].append(key_value)


def rust_string(value: str) -> str:
    return repr(value).replace("'", '"')


def render(commands: list[str]) -> str:
    lines = [
        "// SPDX-License-Identifier: AGPL-3.0-only",
        "// Copyright (c) 2026 sol pbc",
        "",
        f"pub const JOURNAL_HOST_COMMAND_COUNT: usize = {len(commands)};",
        "pub const JOURNAL_HOST_COMMANDS: &[&str] = &[",
    ]
    lines.extend(f"    {rust_string(command)}," for command in commands)
    lines.extend(["];", ""])
    return "\n".join(lines)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build native sol journal-host command list."
    )
    parser.add_argument("--output", type=Path, default=OUTPUT)
    parser.add_argument("--check", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    output = args.output.resolve()
    rendered = render(extract())
    if args.check:
        if not output.is_file():
            print(f"{output} is missing")
            return 1
        if output.read_text() != rendered:
            print(f"{output} is stale; run make build-native-sol-journal-host-commands")
            return 1
        print(f"{output} is current")
        return 0
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(rendered)
    print(f"wrote {output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
