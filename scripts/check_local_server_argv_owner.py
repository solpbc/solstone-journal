#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Keep bundled local-server argv construction in solstone-core."""

from __future__ import annotations

import argparse
import ast
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OWNER_FUNCTIONS: frozenset[tuple[str, str]] = frozenset()
_HOST = "--host"
_PORT = "--port"
_LLAMA_FLAGS: frozenset[str] = frozenset(
    {"--n-gpu-layers", "--kv-unified", "--cache-ram", "--no-context-shift"}
)
_LOCAL_PROCESS_NAMES: frozenset[str] = frozenset(
    {"LOCAL_SERVER_PROCESS_NAME", "MLX_SERVER_PROCESS_NAME"}
)


def _is_test_file(rel: Path) -> bool:
    return "tests" in rel.parts or rel.name == "conftest.py" or (
        rel.name.startswith("test_") and rel.suffix == ".py"
    )


def discover_modules(root: Path) -> list[Path]:
    """Return non-test Python modules under the production solstone tree."""
    scope = root / "solstone"
    if not scope.is_dir():
        return []
    found: list[Path] = []
    for path in sorted(scope.rglob("*.py")):
        rel = path.relative_to(root)
        if "__pycache__" in rel.parts or _is_test_file(rel):
            continue
        found.append(rel)
    return found


def _string_constants(node: ast.AST) -> set[str]:
    return {
        value.value
        for value in ast.walk(node)
        if isinstance(value, ast.Constant) and isinstance(value.value, str)
    }


def _assigned_names(node: ast.AST) -> set[str]:
    if isinstance(node, ast.Name):
        return {node.id}
    if isinstance(node, (ast.Tuple, ast.List)):
        return {name for item in node.elts for name in _assigned_names(item)}
    return set()


def _is_launch_call(node: ast.Call) -> bool:
    return isinstance(node.func, ast.Name) and node.func.id == "_launch_process" and any(
        isinstance(argument, ast.Name) and argument.id in _LOCAL_PROCESS_NAMES
        for argument in node.args
    )


class _FunctionBodyWalker(ast.NodeVisitor):
    def __init__(self) -> None:
        self.list_values: dict[str, set[str]] = {}
        self.launch_list_names: set[str] = set()
        self.launch_list_values: list[set[str]] = []

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        return None

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        return None

    def visit_Assign(self, node: ast.Assign) -> None:
        if isinstance(node.value, ast.List):
            values = _string_constants(node.value)
            for target in node.targets:
                for name in _assigned_names(target):
                    self.list_values[name] = set(values)
        self.generic_visit(node)

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        if isinstance(node.value, ast.List):
            for name in _assigned_names(node.target):
                self.list_values[name] = _string_constants(node.value)
        self.generic_visit(node)

    def visit_Call(self, node: ast.Call) -> None:
        if (
            isinstance(node.func, ast.Attribute)
            and node.func.attr in {"extend", "append"}
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id in self.list_values
        ):
            for argument in node.args:
                self.list_values[node.func.value.id].update(_string_constants(argument))
        if _is_launch_call(node):
            for argument in node.args:
                if isinstance(argument, ast.Name):
                    self.launch_list_names.add(argument.id)
                elif isinstance(argument, ast.List):
                    self.launch_list_values.append(_string_constants(argument))
        self.generic_visit(node)


def _is_local_server_argv(
    values: set[str], *, launched_as_local_process: bool
) -> bool:
    return _HOST in values and _PORT in values and (
        bool(values & _LLAMA_FLAGS) or launched_as_local_process
    )


def _function_violation(node: ast.FunctionDef | ast.AsyncFunctionDef) -> bool:
    walker = _FunctionBodyWalker()
    for statement in node.body:
        walker.visit(statement)
    if any(
        _is_local_server_argv(
            values,
            launched_as_local_process=name in walker.launch_list_names,
        )
        for name, values in walker.list_values.items()
    ):
        return True
    return any(
        _is_local_server_argv(values, launched_as_local_process=True)
        for values in walker.launch_list_values
    )


class _FunctionCollector(ast.NodeVisitor):
    def __init__(self) -> None:
        self.functions: list[ast.FunctionDef | ast.AsyncFunctionDef] = []

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self.functions.append(node)
        self.generic_visit(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self.functions.append(node)
        self.generic_visit(node)


def scan_source(source: str, filename: str, relative_path: str) -> list[tuple[str, int, str]]:
    """Return local-server argv ownership violations from one Python source file."""
    tree = ast.parse(source, filename=filename)
    collector = _FunctionCollector()
    collector.visit(tree)
    return [
        (relative_path, node.lineno, node.name)
        for node in collector.functions
        if (relative_path, node.name) not in OWNER_FUNCTIONS
        and _function_violation(node)
    ]


def scan_file(path: Path, relative_path: str | None = None) -> list[tuple[str, int, str]]:
    return scan_source(
        path.read_text(encoding="utf-8"),
        str(path),
        relative_path or path.as_posix(),
    )


def scan_directory(directory: Path) -> list[tuple[str, int, str]]:
    """Scan every Python file in an arbitrary directory, for gate fixtures/tests."""
    return [
        finding
        for path in sorted(directory.rglob("*.py"))
        for finding in scan_file(path, path.relative_to(directory).as_posix())
    ]


def scan_production(root: Path) -> list[tuple[str, int, str]]:
    """Scan production modules under ``root/solstone`` only."""
    return [
        finding
        for rel in discover_modules(root)
        for finding in scan_file(root / rel, rel.as_posix())
    ]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Check local-server argv ownership")
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    violations = scan_production(args.root)
    if not violations:
        return 0
    print("local-server-argv-owner: violations:", file=sys.stderr)
    for path, line, function_name in violations:
        print(f"  {path}:{line}: {function_name}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
