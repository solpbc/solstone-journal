#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Journal config single-owner transaction lint.

Invariant: ``config/journal.json`` is a single-owner transactional domain.
Production code outside ``solstone.think.journal_config`` must not import or
call the private raw serializer, compose its own read/lock/write cycle, replace
the config file directly, re-export or wrap the private serializer, or create a
second lock for the same file.

Design decisions:

  D1 - Import-binding resolution. Bare-name matching is forbidden. The scanner
  records direct imports, module aliases, and dotted imports before deciding
  whether a call or assignment targets ``_write_journal_config``,
  ``get_journal_config_path``, ``atomic_replace``, ``hold_lock``, or
  ``fcntl.flock``.

  D2 - Config-path-specific replacement. ``atomic_replace`` is legitimate for
  many other domains. This gate flags replacement only when the replacement
  target resolves to ``get_journal_config_path(...)`` or a path expression
  ending in ``config/journal.json``.

  D3 - Owner boundary. The only production owner file is
  ``solstone/think/journal_config.py``. Tests are excluded via the same
  ``_is_test_file`` convention as sibling gates.

  D4 - Empty allowlist. The migrated tree must be green without production
  exceptions. A new violation fails immediately.

Exit codes:
  0 - no production violations
  1 - any production violation is found
"""

from __future__ import annotations

import argparse
import ast
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

OWNER_FILES: frozenset[str] = frozenset({"solstone/think/journal_config.py"})
ALLOWLIST: dict[tuple[str, str], int] = {}
CONFIG_FILENAME = "journal.json"
SIDECAR_LOCK_FILENAME = "." + CONFIG_FILENAME + ".lock"


def _is_owner(rel: Path) -> bool:
    return rel.as_posix() in OWNER_FILES


def _is_test_file(rel: Path) -> bool:
    return (
        "tests" in rel.parts
        or rel.name == "conftest.py"
        or (rel.name.startswith("test_") and rel.suffix == ".py")
    )


def discover_modules(root: Path) -> list[Path]:
    """Return posix-relative non-owner, non-test modules under ``solstone/``."""
    scope = root / "solstone"
    if not scope.is_dir():
        return []

    found: list[Path] = []
    for path in sorted(scope.rglob("*.py")):
        rel = path.relative_to(root)
        if "__pycache__" in rel.parts:
            continue
        if _is_test_file(rel):
            continue
        if _is_owner(rel):
            continue
        found.append(rel)
    return found


def _dotted_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        base = _dotted_name(node.value)
        if base:
            return f"{base}.{node.attr}"
    return None


class _Bindings:
    def __init__(self) -> None:
        self.private_serializer_names: set[str] = set()
        self.journal_config_modules: set[str] = set()
        self.get_path_names: set[str] = set()
        self.atomic_replace_names: set[str] = set()
        self.journal_io_modules: set[str] = set()
        self.hold_lock_names: set[str] = set()
        self.fcntl_modules: set[str] = set()
        self.flock_names: set[str] = set()
        self.os_modules: set[str] = set()
        self.os_replace_names: set[str] = set()


def _collect_bindings(tree: ast.AST) -> _Bindings:
    bindings = _Bindings()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                bound = alias.asname or alias.name
                if alias.name == "solstone.think.journal_config":
                    bindings.journal_config_modules.add(bound)
                elif alias.name == "solstone.think.journal_io":
                    bindings.journal_io_modules.add(bound)
                elif alias.name == "solstone.think.journal_io.atomic":
                    bindings.journal_io_modules.add(bound)
                elif alias.name == "solstone.think.journal_io.locking":
                    bindings.journal_io_modules.add(bound)
                elif alias.name == "fcntl":
                    bindings.fcntl_modules.add(bound)
                elif alias.name == "os":
                    bindings.os_modules.add(bound)
        elif isinstance(node, ast.ImportFrom):
            module = node.module or ""
            for alias in node.names:
                bound = alias.asname or alias.name
                if module == "solstone.think" and alias.name == "journal_config":
                    bindings.journal_config_modules.add(bound)
                elif module == "solstone.think.journal_config":
                    if alias.name == "_write_journal_config":
                        bindings.private_serializer_names.add(bound)
                    elif alias.name == "get_journal_config_path":
                        bindings.get_path_names.add(bound)
                elif module in {
                    "solstone.think.journal_io",
                    "solstone.think.journal_io.atomic",
                }:
                    if alias.name == "atomic_replace":
                        bindings.atomic_replace_names.add(bound)
                    elif alias.name == "hold_lock":
                        bindings.hold_lock_names.add(bound)
                elif module == "solstone.think.journal_io.locking":
                    if alias.name == "hold_lock":
                        bindings.hold_lock_names.add(bound)
                elif module == "fcntl" and alias.name == "flock":
                    bindings.flock_names.add(bound)
                elif module == "os" and alias.name == "replace":
                    bindings.os_replace_names.add(bound)
    return bindings


def _called_attr(func: ast.expr, modules: set[str], attr: str, full_name: str) -> bool:
    if not isinstance(func, ast.Attribute) or func.attr != attr:
        return False
    if isinstance(func.value, ast.Name) and func.value.id in modules:
        return True
    return _dotted_name(func) == full_name


def _is_private_serializer_expr(node: ast.AST, bindings: _Bindings) -> bool:
    if isinstance(node, ast.Name):
        return node.id in bindings.private_serializer_names
    if isinstance(node, ast.Attribute) and node.attr == "_write_journal_config":
        if isinstance(node.value, ast.Name) and node.value.id in (
            bindings.journal_config_modules
        ):
            return True
        return (
            _dotted_name(node) == "solstone.think.journal_config._write_journal_config"
        )
    return False


def _is_private_serializer_call(node: ast.Call, bindings: _Bindings) -> bool:
    return _is_private_serializer_expr(node.func, bindings)


def _is_get_config_path_call(node: ast.AST, bindings: _Bindings) -> bool:
    if not isinstance(node, ast.Call):
        return False
    func = node.func
    if isinstance(func, ast.Name) and func.id in bindings.get_path_names:
        return True
    return _called_attr(
        func,
        bindings.journal_config_modules,
        "get_journal_config_path",
        "solstone.think.journal_config.get_journal_config_path",
    )


def _constant_path_parts(node: ast.AST) -> list[str]:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return [node.value]
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
        return _constant_path_parts(node.left) + _constant_path_parts(node.right)
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
        left = "".join(_constant_path_parts(node.left))
        right = "".join(_constant_path_parts(node.right))
        if left or right:
            return [left + right]
    if isinstance(node, ast.Call):
        parts: list[str] = []
        for arg in node.args:
            parts.extend(_constant_path_parts(arg))
        return parts
    return []


def _parts_end_with_config_path(parts: list[str]) -> bool:
    normalized: list[str] = []
    for part in parts:
        normalized.extend(
            piece for piece in part.replace("\\", "/").split("/") if piece
        )
    return len(normalized) >= 2 and normalized[-2:] == ["config", CONFIG_FILENAME]


def _parts_contain_sidecar_lock(parts: list[str]) -> bool:
    for part in parts:
        if SIDECAR_LOCK_FILENAME in part.replace("\\", "/").split("/"):
            return True
    return False


def _assigned_path_names(
    tree: ast.AST, bindings: _Bindings
) -> tuple[set[str], set[str]]:
    config_path_names: set[str] = set()
    sidecar_lock_names: set[str] = set()
    changed = True
    while changed:
        changed = False
        for node in ast.walk(tree):
            value: ast.AST | None = None
            targets: list[ast.expr] = []
            if isinstance(node, ast.Assign):
                value = node.value
                targets = list(node.targets)
            elif isinstance(node, ast.AnnAssign):
                value = node.value
                targets = [node.target]
            if value is None:
                continue
            is_config = _is_config_path_expr(value, bindings, config_path_names)
            is_sidecar = _is_sidecar_lock_expr(value, sidecar_lock_names)
            for target in targets:
                if not isinstance(target, ast.Name):
                    continue
                if is_config and target.id not in config_path_names:
                    config_path_names.add(target.id)
                    changed = True
                if is_sidecar and target.id not in sidecar_lock_names:
                    sidecar_lock_names.add(target.id)
                    changed = True
    return config_path_names, sidecar_lock_names


def _is_config_path_expr(
    node: ast.AST,
    bindings: _Bindings,
    config_path_names: set[str],
) -> bool:
    if isinstance(node, ast.Name):
        return node.id in config_path_names
    if _is_get_config_path_call(node, bindings):
        return True
    return _parts_end_with_config_path(_constant_path_parts(node))


def _is_sidecar_lock_expr(node: ast.AST, sidecar_lock_names: set[str]) -> bool:
    if isinstance(node, ast.Name):
        return node.id in sidecar_lock_names
    return _parts_contain_sidecar_lock(_constant_path_parts(node))


def _called_atomic_replace(func: ast.expr, bindings: _Bindings) -> bool:
    if isinstance(func, ast.Name):
        return func.id in bindings.atomic_replace_names
    return _called_attr(
        func,
        bindings.journal_io_modules,
        "atomic_replace",
        "solstone.think.journal_io.atomic_replace",
    )


def _called_hold_lock(func: ast.expr, bindings: _Bindings) -> bool:
    if isinstance(func, ast.Name):
        return func.id in bindings.hold_lock_names
    return _called_attr(
        func,
        bindings.journal_io_modules,
        "hold_lock",
        "solstone.think.journal_io.hold_lock",
    )


def _called_flock(func: ast.expr, bindings: _Bindings) -> bool:
    if isinstance(func, ast.Name):
        return func.id in bindings.flock_names
    return _called_attr(func, bindings.fcntl_modules, "flock", "fcntl.flock")


def _called_os_replace(func: ast.expr, bindings: _Bindings) -> bool:
    if isinstance(func, ast.Name):
        return func.id in bindings.os_replace_names
    return _called_attr(func, bindings.os_modules, "replace", "os.replace")


def _is_path_replace_call(node: ast.Call) -> bool:
    return isinstance(node.func, ast.Attribute) and node.func.attr == "replace"


_PLAIN_WRITE_METHODS = frozenset({"write_text", "write_bytes"})
_WRITE_MODE_CHARS = frozenset("wax+")


def _plain_write_method(node: ast.Call) -> str | None:
    """Return the method name for ``<expr>.write_text(...)`` / ``.write_bytes(...)``.

    D5 - Plain-write detection. ``atomic_replace``/``os.replace``/``Path.replace``
    are not the only ways to publish bytes at a path. A bare ``write_text`` is a
    non-atomic, unlocked, default-mode write of the same file, and it was outside
    this gate's detection set while a live instance sat in the tree.
    """
    func = node.func
    if isinstance(func, ast.Attribute) and func.attr in _PLAIN_WRITE_METHODS:
        return func.attr
    return None


def _open_mode_is_write(node: ast.Call, mode_index: int) -> bool:
    """True when an ``open``-shaped call names a writing mode."""
    mode: str | None = None
    if len(node.args) > mode_index and isinstance(node.args[mode_index], ast.Constant):
        value = node.args[mode_index].value
        if isinstance(value, str):
            mode = value
    for keyword in node.keywords:
        if keyword.arg == "mode" and isinstance(keyword.value, ast.Constant):
            value = keyword.value.value
            if isinstance(value, str):
                mode = value
    return mode is not None and bool(_WRITE_MODE_CHARS & frozenset(mode))


def _is_builtin_open_call(func: ast.expr) -> bool:
    """True for a bare ``open(...)`` call."""
    return isinstance(func, ast.Name) and func.id == "open"


def scan_source(source: str, filename: str = "<source>") -> list[tuple[int, str, str]]:
    """Return sorted ``(lineno, kind, detail)`` production violations."""
    tree = ast.parse(source, filename=filename)
    bindings = _collect_bindings(tree)
    config_path_names, sidecar_lock_names = _assigned_path_names(tree, bindings)
    findings: list[tuple[int, str, str]] = []

    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom):
            if node.module == "solstone.think.journal_config":
                for alias in node.names:
                    if alias.name == "_write_journal_config":
                        findings.append(
                            (
                                node.lineno,
                                "private_serializer",
                                "_write_journal_config import",
                            )
                        )
        elif isinstance(node, (ast.Assign, ast.AnnAssign)):
            value = node.value
            if value is not None and _is_private_serializer_expr(value, bindings):
                findings.append(
                    (
                        node.lineno,
                        "private_serializer_wrapper",
                        "_write_journal_config alias or re-export",
                    )
                )
        elif isinstance(node, ast.FunctionDef):
            for child in ast.walk(node):
                if isinstance(child, ast.Call) and _is_private_serializer_call(
                    child, bindings
                ):
                    findings.append(
                        (
                            child.lineno,
                            "private_serializer_wrapper",
                            f"{node.name} wraps _write_journal_config",
                        )
                    )
        elif isinstance(node, ast.Call):
            if _is_private_serializer_call(node, bindings):
                findings.append(
                    (
                        node.lineno,
                        "private_serializer",
                        "_write_journal_config call",
                    )
                )
            elif _called_atomic_replace(node.func, bindings):
                if node.args and _is_config_path_expr(
                    node.args[0], bindings, config_path_names
                ):
                    findings.append(
                        (
                            node.lineno,
                            "journal_config_replace",
                            "atomic_replace targets config/journal.json",
                        )
                    )
            elif _called_os_replace(node.func, bindings):
                if len(node.args) >= 2 and _is_config_path_expr(
                    node.args[1], bindings, config_path_names
                ):
                    findings.append(
                        (
                            node.lineno,
                            "journal_config_replace",
                            "os.replace targets config/journal.json",
                        )
                    )
            elif _is_path_replace_call(node):
                if node.args and _is_config_path_expr(
                    node.args[0], bindings, config_path_names
                ):
                    findings.append(
                        (
                            node.lineno,
                            "journal_config_replace",
                            "Path.replace targets config/journal.json",
                        )
                    )
            elif (plain_write := _plain_write_method(node)) is not None:
                if isinstance(node.func, ast.Attribute) and _is_config_path_expr(
                    node.func.value, bindings, config_path_names
                ):
                    findings.append(
                        (
                            node.lineno,
                            "journal_config_replace",
                            f"{plain_write} targets config/journal.json",
                        )
                    )
            elif _is_builtin_open_call(node.func):
                if (
                    node.args
                    and _is_config_path_expr(node.args[0], bindings, config_path_names)
                    and _open_mode_is_write(node, 1)
                ):
                    findings.append(
                        (
                            node.lineno,
                            "journal_config_replace",
                            "open() for writing targets config/journal.json",
                        )
                    )
            elif isinstance(node.func, ast.Attribute) and node.func.attr == "open":
                if _is_config_path_expr(
                    node.func.value, bindings, config_path_names
                ) and _open_mode_is_write(node, 0):
                    findings.append(
                        (
                            node.lineno,
                            "journal_config_replace",
                            "Path.open() for writing targets config/journal.json",
                        )
                    )
            elif _called_hold_lock(node.func, bindings):
                if node.args and _is_config_path_expr(
                    node.args[0], bindings, config_path_names
                ):
                    findings.append(
                        (
                            node.lineno,
                            "second_config_lock",
                            "hold_lock targets config/journal.json",
                        )
                    )
            elif _called_flock(node.func, bindings):
                if (
                    sidecar_lock_names
                    or SIDECAR_LOCK_FILENAME in source
                    or any(
                        _is_sidecar_lock_expr(arg, sidecar_lock_names)
                        for arg in node.args
                    )
                ):
                    findings.append(
                        (
                            node.lineno,
                            "second_config_lock",
                            "fcntl.flock targets journal config sidecar lock",
                        )
                    )

    findings.sort()
    return findings


def scan_file(path: Path) -> list[tuple[int, str, str]]:
    return scan_source(path.read_text(encoding="utf-8"), filename=str(path))


def count_violations(root: Path) -> dict[tuple[str, str], int]:
    """Map ``(posix-relpath, kind)`` -> occurrence count across production."""
    counts: dict[tuple[str, str], int] = {}
    for rel in discover_modules(root):
        for _lineno, kind, _detail in scan_file(root / rel):
            key = (rel.as_posix(), kind)
            counts[key] = counts.get(key, 0) + 1
    return counts


def evaluate(
    root: Path,
    allowlist: dict[tuple[str, str], int],
) -> tuple[list[str], list[str]]:
    """Return ``(over, tracked)`` human-readable lines."""
    over: list[str] = []
    tracked: list[str] = []

    for rel in discover_modules(root):
        rel_str = rel.as_posix()
        by_kind: dict[str, list[tuple[int, str]]] = {}
        for lineno, kind, detail in scan_file(root / rel):
            by_kind.setdefault(kind, []).append((lineno, detail))
        for kind, entries in sorted(by_kind.items()):
            count = len(entries)
            allowed = allowlist.get((rel_str, kind), 0)
            if count > allowed:
                details = ", ".join(
                    f"line {lineno} ({detail})" for lineno, detail in entries
                )
                over.append(
                    f"{rel_str}: {kind} count {count} exceeds allowed {allowed}: "
                    f"{details}"
                )
            elif allowed:
                tracked.append(f"{rel_str}: {count}/{allowed} {kind} (allowlisted)")
    return over, tracked


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Journal config owner lint")
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="Repository root to scan (defaults to the checkout root).",
    )
    args = parser.parse_args(argv)

    over, tracked = evaluate(args.root, ALLOWLIST)

    if tracked:
        print("journal-config-owner: known violations (allowlisted):")
        for line in tracked:
            print(f"  {line}")
        print()

    if over:
        print("journal-config-owner: NEW violations:", file=sys.stderr)
        for line in over:
            print(f"  {line}", file=sys.stderr)
        print(file=sys.stderr)
        print(
            "Route config/journal.json writes through "
            "solstone.think.journal_config.mutate_journal_config.",
            file=sys.stderr,
        )
        return 1

    print("journal-config-owner: pass")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
