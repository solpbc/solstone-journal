#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
from __future__ import annotations

import argparse
import ast
import hashlib
import json
import subprocess
import sys
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
OUTPUT = REPO_ROOT / "core/fixtures/native-sol/root-contract-v1.json"
ORACLE_COMMIT = "dd04f55c8"
ORACLE_PATH = "solstone/think/sol_cli.py"
ORACLE_BLOB = "a20570fc0994f6215a013e8c89ce7776ddec7d17"
CALL_PATH = "solstone/think/call.py"
JOURNAL_PLACEHOLDER = "${JOURNAL}"
VERSION_PLACEHOLDER = "${VERSION}"
RETIRED_ACCESS_COMMANDS = frozenset({"contract", "notify", "doctor", "check"})


def git_bytes(*args: str) -> bytes:
    return subprocess.check_output(["git", *args], cwd=REPO_ROOT)


def verify_oracle_blob() -> str:
    blob = git_bytes("rev-parse", f"{ORACLE_COMMIT}:{ORACLE_PATH}").decode().strip()
    if blob != ORACLE_BLOB:
        raise RuntimeError(
            f"{ORACLE_COMMIT}:{ORACLE_PATH} is {blob}, expected {ORACLE_BLOB}"
        )
    text = git_bytes("show", f"{ORACLE_COMMIT}:{ORACLE_PATH}")
    digest = git_hash_object(text)
    if digest != ORACLE_BLOB:
        raise RuntimeError(f"extracted oracle blob is {digest}, expected {ORACLE_BLOB}")
    return text.decode()


def git_hash_object(data: bytes) -> str:
    header = f"blob {len(data)}\0".encode()
    return hashlib.sha1(header + data, usedforsecurity=False).hexdigest()


def constants(tree: ast.Module) -> dict[str, str]:
    values: dict[str, str] = {}
    for node in tree.body:
        if isinstance(node, ast.Assign) and len(node.targets) == 1:
            target = node.targets[0]
            value = node.value
        elif isinstance(node, ast.AnnAssign):
            target = node.target
            value = node.value
        else:
            continue
        if isinstance(target, ast.Name) and isinstance(value, ast.Constant):
            if isinstance(value.value, str):
                values[target.id] = value.value
    return values


def string_value(node: ast.AST, names: dict[str, str]) -> str:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    if isinstance(node, ast.Name) and node.id in names:
        return names[node.id]
    raise RuntimeError(f"unsupported string expression: {ast.dump(node)}")


def tuple_strings(node: ast.AST, names: dict[str, str]) -> list[str]:
    if not isinstance(node, ast.Tuple):
        raise RuntimeError(f"expected tuple of strings, got {ast.dump(node)}")
    return [string_value(item, names) for item in node.elts]


def assignment(tree: ast.Module, name: str) -> ast.AST:
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == name
            for target in node.targets
        ):
            return node.value
        if isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            if node.target.id == name and node.value is not None:
                return node.value
    raise RuntimeError(f"missing assignment {name}")


def function(tree: ast.Module, name: str) -> ast.FunctionDef:
    for node in tree.body:
        if isinstance(node, ast.FunctionDef) and node.name == name:
            return node
    raise RuntimeError(f"missing function {name}")


def print_literals(fn: ast.FunctionDef) -> list[str]:
    values: list[str] = []
    for node in ast.walk(fn):
        if (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Name)
            and node.func.id == "print"
            and node.args
            and isinstance(node.args[0], ast.Constant)
            and isinstance(node.args[0].value, str)
        ):
            values.append(node.args[0].value)
    return values


def access_groups(tree: ast.Module, names: dict[str, str]) -> list[dict[str, Any]]:
    raw = assignment(tree, "ACCESS_HELP_GROUPS")
    if not isinstance(raw, ast.Tuple):
        raise RuntimeError("ACCESS_HELP_GROUPS must be a tuple")
    groups: list[dict[str, Any]] = []
    for item in raw.elts:
        if not isinstance(item, ast.Call) or len(item.args) < 2:
            raise RuntimeError(f"invalid ACCESS_HELP_GROUPS item: {ast.dump(item)}")
        groups.append(
            {
                "heading": string_value(item.args[0], names),
                "commands": tuple_strings(item.args[1], names),
            }
        )
    if not groups:
        raise RuntimeError("ACCESS_HELP_GROUPS extraction is empty")
    return groups


def filter_retired_access_commands(
    groups: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    present = {
        command
        for group in groups
        for command in group["commands"]
        if command in RETIRED_ACCESS_COMMANDS
    }
    missing = sorted(RETIRED_ACCESS_COMMANDS - present)
    if missing:
        raise RuntimeError(
            f"retired access command(s) absent from oracle: {', '.join(missing)}"
        )
    filtered: list[dict[str, Any]] = []
    for group in groups:
        commands = [
            command
            for command in group["commands"]
            if command not in RETIRED_ACCESS_COMMANDS
        ]
        if commands:
            filtered.append({"heading": group["heading"], "commands": commands})
    return filtered


def call_overrides(call_tree: ast.Module) -> dict[str, str]:
    raw = assignment(call_tree, "CALL_NAME_OVERRIDES")
    if not isinstance(raw, ast.Dict):
        raise RuntimeError("CALL_NAME_OVERRIDES must be a dict")
    result: dict[str, str] = {}
    for key, value in zip(raw.keys, raw.values, strict=True):
        if not isinstance(key, ast.Constant) or not isinstance(key.value, str):
            raise RuntimeError("CALL_NAME_OVERRIDES keys must be strings")
        if not isinstance(value, ast.Constant) or not isinstance(value.value, str):
            raise RuntimeError("CALL_NAME_OVERRIDES values must be strings")
        result[key.value] = value.value
    return result


def builtin_call_groups(call_tree: ast.Module) -> list[str]:
    groups: list[str] = []
    for node in call_tree.body:
        if not isinstance(node, ast.Expr) or not isinstance(node.value, ast.Call):
            continue
        call = node.value
        if (
            isinstance(call.func, ast.Attribute)
            and isinstance(call.func.value, ast.Name)
            and call.func.value.id == "call_app"
            and call.func.attr == "add_typer"
        ):
            for keyword in call.keywords:
                if keyword.arg == "name":
                    groups.append(string_value(keyword.value, {}))
    if not groups:
        raise RuntimeError("no builtin call groups extracted")
    return groups


def app_call_groups(call_tree: ast.Module) -> list[str]:
    overrides = call_overrides(call_tree)
    tree = git_bytes(
        "ls-tree", "-r", "--name-only", ORACLE_COMMIT, "solstone/apps"
    ).decode()
    app_names = sorted(
        {
            Path(line).parts[2]
            for line in tree.splitlines()
            if line.endswith("/call.py")
            and len(Path(line).parts) >= 4
            and not Path(line).parts[2].startswith("_")
        }
    )
    if not app_names:
        raise RuntimeError("old-tree app call discovery is empty")
    return [overrides.get(name, name) for name in app_names]


def call_groups() -> list[str]:
    call_tree = ast.parse(git_bytes("show", f"{ORACLE_COMMIT}:{CALL_PATH}").decode())
    groups = app_call_groups(call_tree) + builtin_call_groups(call_tree)
    if len(groups) != 20:
        raise RuntimeError(f"root call group count {len(groups)} != 20: {groups!r}")
    if len(groups) != len(set(groups)):
        raise RuntimeError(f"duplicate root call groups: {groups!r}")
    return groups


def render_stdout(
    *,
    header: str,
    usage: str,
    groups: list[dict[str, Any]],
    apps: list[str],
) -> str:
    lines: list[str] = [
        header,
        "",
        usage,
        "",
    ]
    for group in groups:
        lines.append(group["heading"])
        lines.extend(f"  {command}" for command in group["commands"])
        lines.append("")
    lines.append("Apps (sol call <app>):")
    lines.extend(f"  call {name}" for name in apps)
    lines.append("")
    return "\n".join(lines) + "\n"


def build() -> dict[str, Any]:
    oracle_text = verify_oracle_blob()
    tree = ast.parse(oracle_text)
    names = constants(tree)
    help_literals = print_literals(function(tree, "print_help"))
    header = next(
        value.strip() for value in help_literals if value.startswith("sol - ")
    )
    usage = next(
        value.strip() for value in help_literals if value.startswith("Usage: sol ")
    )
    groups = filter_retired_access_commands(access_groups(tree, names))
    apps = call_groups()
    return {
        "schema": "native-sol-root-contract-v1",
        "oracle": {
            "commit": ORACLE_COMMIT,
            "path": ORACLE_PATH,
            "blob": ORACLE_BLOB,
        },
        "placeholders": {
            "journal": JOURNAL_PLACEHOLDER,
            "version": VERSION_PLACEHOLDER,
        },
        "header": header,
        "usage": usage,
        "access_groups": groups,
        "call_groups": apps,
        "expected_bare_sol_stdout": render_stdout(
            header=header, usage=usage, groups=groups, apps=apps
        ),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build native sol root contract fixture."
    )
    parser.add_argument("--output", type=Path, default=OUTPUT)
    parser.add_argument("--check", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    output = args.output.resolve()
    rendered = json.dumps(build(), indent=2, sort_keys=True) + "\n"
    if args.check:
        if not output.is_file():
            print(f"{output} is missing")
            return 1
        if output.read_text() != rendered:
            print(f"{output} is stale; run make build-native-sol-root-contract")
            return 1
        print(f"{output} is current")
        return 0
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(rendered)
    print(f"wrote {output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
