# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Offline egress guard for Oura importer modules."""

from __future__ import annotations

import ast
from pathlib import Path

IMPORTER_ROOT = Path(__file__).resolve().parents[1] / "solstone" / "think" / "importers"

ALLOWED_EGRESS: frozenset[tuple[str, str, str]] = frozenset()

SAFE_IMPORTS: frozenset[str] = frozenset({"urllib.parse"})

IMPORT_CAPABILITIES: dict[str, str] = {
    "aiohttp": "http_client",
    "http": "http_client",
    "http.client": "http_client",
    "http.server": "loopback_http_server",
    "httpx": "http_client",
    "imaplib": "mail_client",
    "importlib": "dynamic_import",
    "poplib": "mail_client",
    "pycurl": "http_client",
    "requests": "http_client",
    "smtplib": "mail_client",
    "socket": "socket",
    "ssl": "tls",
    "subprocess": "process_escape",
    "urllib": "http_client",
    "urllib.error": "http_client",
    "urllib.request": "http_client",
    "urllib3": "http_client",
    "webbrowser": "browser_open",
}

CALL_CAPABILITIES: dict[str, str] = {
    "__import__": "dynamic_import",
    "_CallbackHTTPServer": "loopback_http_server",
    "aiohttp": "http_client",
    "http.client": "http_client",
    "http.server.BaseHTTPRequestHandler": "loopback_http_server",
    "http.server.HTTPServer": "loopback_http_server",
    "httpx": "http_client",
    "imaplib": "mail_client",
    "importlib": "dynamic_import",
    "os.popen": "process_escape",
    "os.system": "process_escape",
    "poplib": "mail_client",
    "pycurl": "http_client",
    "requests": "http_client",
    "smtplib": "mail_client",
    "socket": "socket",
    "ssl": "tls",
    "subprocess": "process_escape",
    "urllib.request": "http_client",
    "urllib3": "http_client",
    "webbrowser.open": "browser_open",
}


def _oura_module_paths() -> list[Path]:
    return sorted(IMPORTER_ROOT.glob("oura*.py"))


def _parents(tree: ast.AST) -> dict[ast.AST, ast.AST]:
    parents: dict[ast.AST, ast.AST] = {}
    for parent in ast.walk(tree):
        for child in ast.iter_child_nodes(parent):
            parents[child] = parent
    return parents


def _owner(node: ast.AST, parents: dict[ast.AST, ast.AST]) -> str:
    function_owner: str | None = None
    cursor: ast.AST | None = node
    while cursor is not None:
        if isinstance(cursor, ast.ClassDef):
            if cursor.name in {"_CallbackHTTPServer", "_CallbackHandler"}:
                return cursor.name
            return function_owner or cursor.name
        if isinstance(cursor, (ast.FunctionDef, ast.AsyncFunctionDef)):
            function_owner = cursor.name
        cursor = parents.get(cursor)
    return function_owner or "<module>"


def _rooted_capability(name: str, capabilities: dict[str, str]) -> str | None:
    if any(name == safe or name.startswith(f"{safe}.") for safe in SAFE_IMPORTS):
        return None
    for prefix, capability in sorted(
        capabilities.items(), key=lambda item: len(item[0]), reverse=True
    ):
        if name == prefix or name.startswith(f"{prefix}."):
            return capability
    return None


def _import_names(node: ast.Import | ast.ImportFrom) -> list[tuple[str, str]]:
    if isinstance(node, ast.Import):
        return [
            (alias.name, alias.asname or alias.name.split(".")[0])
            for alias in node.names
        ]
    module = node.module or ""
    names: list[tuple[str, str]] = []
    for alias in node.names:
        if alias.name == "*":
            names.append((module, alias.asname or alias.name))
        else:
            names.append(
                (
                    f"{module}.{alias.name}" if module else alias.name,
                    alias.asname or alias.name,
                )
            )
    return names


def _bindings(tree: ast.AST) -> dict[str, str]:
    bindings: dict[str, str] = {}
    for node in ast.walk(tree):
        if not isinstance(node, (ast.Import, ast.ImportFrom)):
            continue
        for full_name, bound_name in _import_names(node):
            bindings[bound_name] = full_name
            root = full_name.split(".", 1)[0]
            if bound_name == root:
                bindings[root] = root
    return bindings


def _dotted_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        base = _dotted_name(node.value)
        if base:
            return f"{base}.{node.attr}"
    return None


def _resolve_name(name: str, bindings: dict[str, str]) -> str:
    head, dot, tail = name.partition(".")
    bound = bindings.get(head)
    if not bound:
        return name
    return f"{bound}{dot}{tail}" if dot else bound


def _allowed(module: str, owner: str, capability: str) -> bool:
    return (module, owner, capability) in ALLOWED_EGRESS


def _violation(
    module: str,
    owner: str,
    capability: str,
    detail: str,
) -> str | None:
    if _allowed(module, owner, capability):
        return None
    return f"{module}.{owner}: {capability}: {detail}"


def _egress_violations(module: str, source: str) -> list[str]:
    tree = ast.parse(source)
    parents = _parents(tree)
    bindings = _bindings(tree)
    violations: list[str] = []

    for node in ast.walk(tree):
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            owner = _owner(node, parents)
            for full_name, _bound_name in _import_names(node):
                capability = _rooted_capability(full_name, IMPORT_CAPABILITIES)
                if capability is None:
                    continue
                violation = _violation(module, owner, capability, f"import {full_name}")
                if violation:
                    violations.append(violation)
            continue

        if isinstance(node, ast.ClassDef):
            for base in node.bases:
                name = _dotted_name(base)
                if name is None:
                    continue
                resolved = _resolve_name(name, bindings)
                capability = _rooted_capability(resolved, CALL_CAPABILITIES)
                if capability is None:
                    continue
                violation = _violation(
                    module, node.name, capability, f"class base {resolved}"
                )
                if violation:
                    violations.append(violation)
            continue

        if isinstance(node, ast.Call):
            name = _dotted_name(node.func)
            if name is None:
                continue
            resolved = _resolve_name(name, bindings)
            capability = _rooted_capability(resolved, CALL_CAPABILITIES)
            if capability is None:
                continue
            owner = _owner(node, parents)
            violation = _violation(module, owner, capability, f"call {resolved}")
            if violation:
                violations.append(violation)
            continue

        if isinstance(node, ast.Attribute):
            name = _dotted_name(node)
            if name is None:
                continue
            resolved = _resolve_name(name, bindings)
            capability = CALL_CAPABILITIES.get(resolved)
            if capability is None:
                continue
            owner = _owner(node, parents)
            violation = _violation(module, owner, capability, f"reference {resolved}")
            if violation:
                violations.append(violation)

    return violations


def test_oura_egress_guard_covers_all_oura_modules() -> None:
    paths = _oura_module_paths()

    assert paths
    assert {path.name for path in paths} == {"oura.py"}

    violations: list[str] = []
    for path in paths:
        violations.extend(
            _egress_violations(path.stem, path.read_text(encoding="utf-8"))
        )

    assert violations == []


def test_oura_egress_guard_reports_synthetic_violation() -> None:
    source = """
import os
import smtplib
import socket
import subprocess
import urllib3

socket.socket()
smtplib.SMTP("localhost")
subprocess.run(["true"])
os.system("true")
urllib3.PoolManager()
__import__("ssl")
"""

    violations = _egress_violations("oura_future", source)

    assert any(
        "oura_future.<module>: process_escape: import subprocess" in violation
        for violation in violations
    )
    assert any(
        "oura_future.<module>: process_escape: call subprocess.run" in violation
        for violation in violations
    )
    assert any(
        "oura_future.<module>: process_escape: call os.system" in violation
        for violation in violations
    )
    assert any(
        "oura_future.<module>: socket: import socket" in violation
        for violation in violations
    )
    assert any(
        "oura_future.<module>: mail_client: import smtplib" in violation
        for violation in violations
    )
    assert any(
        "oura_future.<module>: http_client: import urllib3" in violation
        for violation in violations
    )
    assert any("dynamic_import" in violation for violation in violations)
