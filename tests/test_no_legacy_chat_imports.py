# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import ast
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def _git_ls(*patterns: str) -> list[str]:
    result = subprocess.run(
        ["git", "ls-files", *patterns],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return [line for line in result.stdout.splitlines() if line]


ALLOWED_UNIFIED_PATHS = {
    ROOT / "apps/sol/maint/006_rename_unified_triage_providers.py",
    ROOT / "tests/test_maint_006_rename_unified_triage_providers.py",
}
FORBIDDEN_CHAT_LITERALS = {
    "conversationBackdrop",
    "conversationMessages",
    "chatBarResponsePanel",
    "chatBarThinking",
    "chatBarResponse",
    "chatBarDismiss",
    "conversation-backdrop",
    "conversation-messages",
    "conversation-separator",
    "solstone:conversationState",
    "solstone:chatBarState",
    "panelFocusTrapHandler",
    "openPanel",
    "closePanel",
    "_closeConversationPanel",
}


def _parts(*pieces: str) -> str:
    return "".join(pieces)


BANNED_NAMES = {
    _parts("_", "display_", "mode"),
    _parts("record_", "exchange"),
    _parts("build_", "memory_", "context"),
    _parts("INJECTION_", "MARKER"),
    _parts("inject_", "memory"),
    _parts("get_", "recent_", "exchanges"),
    _parts("get_", "today_", "exchanges"),
    _parts("TRIAGE_", "AGENT_", "NAMES"),
    _parts("record_", "triage_", "exchange"),
    _parts("compute_", "display_", "mode"),
}
LEGACY_CHAT_MODULE = _parts("think", ".", "conversation")
LEGACY_MEMORY_MODULE = _parts("talent", ".", "conversation_", "memory")
LEGACY_NAME = _parts("uni", "fied")


def _python_files() -> list[Path]:
    # `git ls-files` excludes anything gitignored (`/journal/*` on dev boxes can
    # be 100+ GB of capture data; `ROOT.rglob` walks all of it on every call).
    return [path for line in _git_ls("*.py") if (path := ROOT / line).exists()]


def _text_scan_files() -> list[Path]:
    blocked_parts = ("tests/fixtures",)
    return [
        path
        for line in _git_ls("*.html", "*.js")
        if not any(part in line for part in blocked_parts)
        if (path := ROOT / line).exists()
    ]


def _parse(path: Path) -> ast.Module | None:
    try:
        return ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    except SyntaxError:
        return None


def test_no_legacy_chat_imports_or_usages():
    violations: list[str] = []

    for path in _python_files():
        tree = _parse(path)
        if tree is None:
            continue
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                for alias in node.names:
                    if alias.name in {LEGACY_CHAT_MODULE, LEGACY_MEMORY_MODULE}:
                        violations.append(f"{path}: import {alias.name}")
            elif isinstance(node, ast.ImportFrom):
                if node.module in {LEGACY_CHAT_MODULE, LEGACY_MEMORY_MODULE}:
                    violations.append(f"{path}: from {node.module} import ...")
            elif isinstance(node, ast.Name) and node.id in BANNED_NAMES:
                violations.append(f"{path}: name {node.id}")
            elif isinstance(node, ast.Attribute) and node.attr in BANNED_NAMES:
                violations.append(f"{path}: attribute {node.attr}")

    assert violations == []


def test_no_live_unified_literals_outside_migration_paths():
    violations: list[str] = []

    for path in _python_files():
        if path in ALLOWED_UNIFIED_PATHS:
            continue
        if path == Path(__file__).resolve():
            continue
        tree = _parse(path)
        if tree is None:
            continue
        for node in ast.walk(tree):
            if isinstance(node, ast.Constant) and node.value == LEGACY_NAME:
                violations.append(str(path))

    assert violations == []


def test_no_legacy_chat_dom_literals_in_templates_or_js():
    violations: list[str] = []
    this_file = Path(__file__).resolve()

    for path in _text_scan_files():
        if path.resolve() == this_file:
            continue
        content = path.read_text(encoding="utf-8")
        for literal in FORBIDDEN_CHAT_LITERALS:
            if literal in content:
                violations.append(f"{path}: {literal}")

    assert violations == []
