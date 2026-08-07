#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Ensure bundled local generate remains owned by solstone-core."""

from __future__ import annotations

import argparse
import ast
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RETIRED = frozenset(
    {
        "_prepare_bundled_request",
        "_raise_bundled_status",
        "_bundled_error_type",
        "ContextWindowResolution",
        "resolve_context_window",
        "context_window_tokens",
        "count_tokens",
    }
)
FORBIDDEN_CALLS = frozenset(
    {
        "resolve_context_window",
        "count_tokens",
        "connect",
        "read_server_capacity",
        "acquire_local_slot",
        "acquire_local_slot_async",
    }
)


def _test(rel: Path) -> bool:
    return (
        "tests" in rel.parts
        or rel.name.startswith("test_")
        or rel.name == "conftest.py"
    )


def _call_name(node: ast.Call) -> str:
    if isinstance(node.func, ast.Name):
        return node.func.id
    if isinstance(node.func, ast.Attribute):
        return node.func.attr
    return ""


def _bundled_body(function: ast.FunctionDef | ast.AsyncFunctionDef) -> list[ast.stmt]:
    for node in ast.walk(function):
        if (
            isinstance(node, ast.If)
            and isinstance(node.test, ast.Attribute)
            and node.test.attr == "is_bundled"
        ):
            return node.body
    return []


def _forbidden_call(node: ast.Call) -> bool:
    if _call_name(node) in FORBIDDEN_CALLS:
        return True
    return _call_name(node) in {"post", "AsyncClient"} and (
        "/v1/chat/completions" in ast.unparse(node)
    )


class _BundledBodyWalker(ast.NodeVisitor):
    """Collect direct bundled-branch calls without entering nested scopes."""

    def __init__(self) -> None:
        self.calls: list[ast.Call] = []

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        return None

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        return None

    def visit_Call(self, node: ast.Call) -> None:
        self.calls.append(node)
        self.generic_visit(node)


def _bundled_calls(function: ast.FunctionDef | ast.AsyncFunctionDef) -> list[ast.Call]:
    walker = _BundledBodyWalker()
    for statement in _bundled_body(function):
        walker.visit(statement)
    return walker.calls


def scan_source(source: str, filename: str, relative: str) -> list[tuple[str, int, str]]:
    tree = ast.parse(source, filename=filename)
    findings: list[tuple[str, int, str]] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.FunctionDef | ast.AsyncFunctionDef) and node.name in RETIRED:
            findings.append((relative, node.lineno, f"retired symbol {node.name}"))
    for function in (
        node
        for node in tree.body
        if isinstance(node, ast.FunctionDef | ast.AsyncFunctionDef)
        and node.name in {"run_generate", "run_agenerate"}
    ):
        for call in _bundled_calls(function):
            if _forbidden_call(call):
                findings.append(
                    (relative, call.lineno, "bundled branch owns local transport/admission")
                )
    return findings


def scan_file(path: Path, relative: str | None = None) -> list[tuple[str, int, str]]:
    return scan_source(
        path.read_text(encoding="utf-8"),
        str(path),
        relative or path.as_posix(),
    )


def scan_directory(directory: Path) -> list[tuple[str, int, str]]:
    return [
        finding
        for path in sorted(directory.rglob("*.py"))
        for finding in scan_file(path, path.relative_to(directory).as_posix())
    ]


def scan_production(root: Path) -> list[tuple[str, int, str]]:
    return [
        finding
        for path in sorted((root / "solstone").rglob("*.py"))
        if not _test(path.relative_to(root))
        for finding in scan_file(path, path.relative_to(root).as_posix())
    ]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Check local generate cutover")
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    findings = scan_production(args.root)
    if not findings:
        return 0
    for path, line, message in findings:
        print(f"{path}:{line}: {message}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
