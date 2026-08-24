#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Journal raw-mechanic lint.

This check flags raw durable-write and locked read-modify-write mechanics used
against owner journal data anywhere outside ``solstone.think.journal_io``.
Unlike ``check_journal_io_access.py``, owner modules are intentionally scanned:
owners may import journal_io primitives, but after migration they must not carry
their own raw replacement or bare exclusive-flock mechanics.

Flagged mechanics:

  D1 - ``os.replace``. Calls are resolved through ``import os`` / aliases and
  ``from os import replace`` bindings. Bare attribute-name matching is forbidden.

  D2 - ``Path.replace`` heuristic. The detector flags one-argument
  ``.replace(...)`` calls only when the receiver looks like a temp path:
  direct ``.with_suffix/.with_name/.with_stem(...)`` receiver, a receiver name
  containing ``tmp``/``temp``, or a receiver name bound from known temp/path
  creation patterns. This deliberately avoids string/date ``replace`` calls.
  Limit: a non-temp-named Path variable with one non-string argument is a false
  negative; in this repository the ``os.replace`` rule and temp-name tracking
  cover every real durable-write site. The temp-substring receiver-name match
  can also over-match names like ``template`` or ``attempt``.

  D3 - ``flock(LOCK_EX)``. ``fcntl.flock`` calls with a lock-mode subtree
  containing ``LOCK_EX`` and not ``LOCK_NB`` are flagged. ``LOCK_UN`` and
  ``LOCK_EX | LOCK_NB`` are not violations.

  D4 - raw write calls. ``open(write)``, ``Path.open(write)``,
  ``Path.write_text``, and ``Path.write_bytes`` are flagged only when the target
  is statically traceable to a ``get_journal()``-derived journal-DATA path:
  root plus L2 anchor, with one-hop local alias tracking. Producers such as
  ``day_path()``, ``segment_path()``, ``resolve_journal_path()``, and
  ``contained_path()`` are deliberately not roots, and non-DATA anchors are
  deliberately ignored.

The check ships green with an empty allowlist. The allowlist is keyed by
``(file, kind)`` with an allowed count so it can ratchet down, matching the
existing journal_io access check.
"""

from __future__ import annotations

import argparse
import ast
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

PATH_TEMP_METHODS: frozenset[str] = frozenset({"with_suffix", "with_name", "with_stem"})
VIOLATION_KINDS: frozenset[str] = frozenset(
    {
        "os.replace",
        "Path.replace",
        "flock(LOCK_EX)",
        "open(write)",
        "Path.open(write)",
        "Path.write_text",
        "Path.write_bytes",
    }
)

# Derived from the §7 L2 table. Runtime/cache/ops dirs (`logs`, `health`,
# `indexer`, `tokens`, `push`, `maint`, `stats.json`) are deliberately omitted.
JOURNAL_DATA_ANCHORS: frozenset[str] = frozenset(
    {
        ".config",
        "awareness",
        "chronicle",
        "config",
        "entities",
        "facets",
        "identity",
        "imports",
        "link",
        "observations.jsonl",
        "skills",
        "streams",
        "talents",
        "timeline.json",
    }
)

EXCLUDED_FILES: frozenset[str] = frozenset(
    {
        # Ops/runtime state and single-process guards.
        "solstone/think/catchup_state.py",
        "solstone/think/scheduler.py",
        "solstone/think/readiness.py",
        "solstone/think/providers/state.py",
        "solstone/think/providers/local_install.py",
        "solstone/think/providers/oci_image.py",
        "solstone/think/providers/mlx_install.py",
        "solstone/think/providers/ced_install.py",
        "solstone/think/providers/rfdetr_install.py",
        "solstone/think/providers/nvattest_install.py",
        "solstone/think/steward.py",
        "solstone/talent/steward.py",
        "solstone/think/install_guard.py",
        "solstone/think/install_models.py",
        "solstone/think/setup.py",
        "solstone/think/user_config.py",
        "solstone/think/voice/brain.py",
        "solstone/think/sync_check.py",
        "solstone/think/runner.py",
        # App-storage and temporary upload/transcription files.
        "solstone/apps/support/routes.py",
        "solstone/observe/transcribe/_parakeet_coreml.py",
        "solstone/think/journal_export.py",
        # UI/pipeline runtime state and generated source artifacts, not owner journal content.
        "solstone/apps/home/routes.py",
        "solstone/think/data_state.py",
    }
)

EXCLUDED_PREFIXES: tuple[str, ...] = ()

# The merge engine's _atomic_copytree() uses a same-parent temp directory and
# os.replace() for atomic tree visibility; journal_io currently only exposes
# file-level install primitives.
ALLOWLIST: dict[tuple[str, str], int] = {("solstone/think/merge.py", "os.replace"): 1}


def _is_test_file(rel: Path) -> bool:
    return (
        "tests" in rel.parts
        or rel.name == "conftest.py"
        or (rel.name.startswith("test_") and rel.suffix == ".py")
    )


def _is_excluded(rel: Path) -> bool:
    rel_str = rel.as_posix()
    return rel_str in EXCLUDED_FILES or any(
        rel_str.startswith(prefix) for prefix in EXCLUDED_PREFIXES
    )


def discover_modules(root: Path) -> list[Path]:
    """Return posix-relative scanned modules under ``solstone/``."""
    scope = root / "solstone"
    if not scope.is_dir():
        return []

    found: list[Path] = []
    for path in sorted(scope.rglob("*.py")):
        rel = path.relative_to(root)
        rel_str = rel.as_posix()
        if "__pycache__" in rel.parts:
            continue
        if rel_str.startswith("solstone/think/journal_io/"):
            continue
        if _is_test_file(rel):
            continue
        if _is_excluded(rel):
            continue
        found.append(rel)
    return found


def _call_detail(func: ast.expr) -> str:
    try:
        return ast.unparse(func)
    except Exception:
        return "<call>"


def _is_attr_call(expr: ast.AST, attrs: frozenset[str]) -> bool:
    return (
        isinstance(expr, ast.Call)
        and isinstance(expr.func, ast.Attribute)
        and expr.func.attr in attrs
    )


def _is_tempfile_call(
    expr: ast.AST,
    name: str,
    tempfile_aliases: set[str],
    direct_names: set[str],
) -> bool:
    if not isinstance(expr, ast.Call):
        return False
    func = expr.func
    if isinstance(func, ast.Name):
        return func.id in direct_names
    return (
        isinstance(func, ast.Attribute)
        and func.attr == name
        and isinstance(func.value, ast.Name)
        and func.value.id in tempfile_aliases
    )


def _iter_target_names(target: ast.AST) -> list[str]:
    if isinstance(target, ast.Name):
        return [target.id]
    if isinstance(target, (ast.Tuple, ast.List)):
        names: list[str] = []
        for elt in target.elts:
            names.extend(_iter_target_names(elt))
        return names
    return []


# Narrow by design: only get_journal()/Path(get_journal()) and one-hop local
# aliases are roots. day_path(), segment_path(), resolve_journal_path(), and
# contained_path() are intentionally not treated as roots.
def _is_get_journal_call(expr: ast.AST) -> bool:
    if not isinstance(expr, ast.Call):
        return False
    func = expr.func
    return (isinstance(func, ast.Name) and func.id == "get_journal") or (
        isinstance(func, ast.Attribute) and func.attr == "get_journal"
    )


def _is_path_of_get_journal(expr: ast.AST) -> bool:
    return (
        isinstance(expr, ast.Call)
        and isinstance(expr.func, ast.Name)
        and expr.func.id == "Path"
        and len(expr.args) == 1
        and _is_get_journal_call(expr.args[0])
    )


def _is_journal_root(expr: ast.AST, root_names: set[str]) -> bool:
    return (
        _is_get_journal_call(expr)
        or _is_path_of_get_journal(expr)
        or (isinstance(expr, ast.Name) and expr.id in root_names)
    )


def _journal_data_path(expr: ast.AST, root_names: set[str]) -> bool:
    if isinstance(expr, ast.BinOp) and isinstance(expr.op, ast.Div):
        bottom = expr
        while isinstance(bottom.left, ast.BinOp) and isinstance(
            bottom.left.op, ast.Div
        ):
            bottom = bottom.left
        return (
            _is_journal_root(bottom.left, root_names)
            and isinstance(bottom.right, ast.Constant)
            and isinstance(bottom.right.value, str)
            and bottom.right.value in JOURNAL_DATA_ANCHORS
        )

    return (
        isinstance(expr, ast.Call)
        and isinstance(expr.func, ast.Name)
        and expr.func.id == "Path"
        and len(expr.args) >= 2
        and _is_journal_root(expr.args[0], root_names)
        and isinstance(expr.args[1], ast.Constant)
        and isinstance(expr.args[1].value, str)
        and expr.args[1].value in JOURNAL_DATA_ANCHORS
    )


def _is_journal_data_target(
    expr: ast.AST,
    root_names: set[str],
    target_names: set[str],
) -> bool:
    return _journal_data_path(expr, root_names) or (
        isinstance(expr, ast.Name) and expr.id in target_names
    )


def _collect_journal_path_vars(tree: ast.AST) -> tuple[set[str], set[str]]:
    root_names: set[str] = set()
    target_names: set[str] = set()

    for node in _iter_scope_assignments(tree):
        if not isinstance(node, (ast.Assign, ast.AnnAssign)) or node.value is None:
            continue
        targets = node.targets if isinstance(node, ast.Assign) else [node.target]
        if _is_get_journal_call(node.value) or _is_path_of_get_journal(node.value):
            for target in targets:
                root_names.update(_iter_target_names(target))

    for node in _iter_scope_assignments(tree):
        if not isinstance(node, (ast.Assign, ast.AnnAssign)) or node.value is None:
            continue
        targets = node.targets if isinstance(node, ast.Assign) else [node.target]
        if _journal_data_path(node.value, root_names):
            for target in targets:
                target_names.update(_iter_target_names(target))

    return root_names, target_names


_SCOPE_NODES = (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda)


def _iter_scope_assignments(scope: ast.AST) -> list[ast.Assign | ast.AnnAssign]:
    assignments: list[ast.Assign | ast.AnnAssign] = []

    def visit(node: ast.AST) -> None:
        if node is not scope and isinstance(node, _SCOPE_NODES):
            return
        if isinstance(node, (ast.Assign, ast.AnnAssign)):
            assignments.append(node)
        for child in ast.iter_child_nodes(node):
            visit(child)

    visit(scope)
    return assignments


def _collect_parent_map(tree: ast.AST) -> dict[ast.AST, ast.AST]:
    parents: dict[ast.AST, ast.AST] = {}
    for parent in ast.walk(tree):
        for child in ast.iter_child_nodes(parent):
            parents[child] = parent
    return parents


def _nearest_scope(
    node: ast.AST,
    tree: ast.AST,
    parents: dict[ast.AST, ast.AST],
) -> ast.AST:
    current = parents.get(node)
    while current is not None:
        if current is tree or isinstance(current, _SCOPE_NODES):
            return current
        current = parents.get(current)
    return tree


def _iter_scopes(tree: ast.AST) -> list[ast.AST]:
    scopes = [tree]
    scopes.extend(node for node in ast.walk(tree) if isinstance(node, _SCOPE_NODES))
    return scopes


def _collect_bindings(
    tree: ast.AST,
) -> tuple[set[str], set[str], set[str], set[str], set[str]]:
    os_aliases: set[str] = set()
    os_replace_names: set[str] = set()
    fcntl_aliases: set[str] = set()
    flock_names: set[str] = set()
    tempfile_aliases: set[str] = set()
    mkstemp_names: set[str] = set()
    named_temp_names: set[str] = set()
    temp_names: set[str] = set()

    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                bound = alias.asname or alias.name
                if alias.name == "os":
                    os_aliases.add(bound)
                elif alias.name == "fcntl":
                    fcntl_aliases.add(bound)
                elif alias.name == "tempfile":
                    tempfile_aliases.add(bound)
        elif isinstance(node, ast.ImportFrom):
            module = node.module or ""
            for alias in node.names:
                bound = alias.asname or alias.name
                if module == "os" and alias.name == "replace":
                    os_replace_names.add(bound)
                elif module == "fcntl" and alias.name == "flock":
                    flock_names.add(bound)
                elif module == "tempfile" and alias.name == "mkstemp":
                    mkstemp_names.add(bound)
                elif module == "tempfile" and alias.name == "NamedTemporaryFile":
                    named_temp_names.add(bound)

    for node in ast.walk(tree):
        if isinstance(node, (ast.Assign, ast.AnnAssign)):
            value = node.value
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            if value is None:
                continue
            for target in targets:
                if _is_attr_call(value, PATH_TEMP_METHODS) or _is_tempfile_call(
                    value,
                    "NamedTemporaryFile",
                    tempfile_aliases,
                    named_temp_names,
                ):
                    temp_names.update(_iter_target_names(target))
                if _is_tempfile_call(value, "mkstemp", tempfile_aliases, mkstemp_names):
                    if (
                        isinstance(target, (ast.Tuple, ast.List))
                        and len(target.elts) >= 2
                        and isinstance(target.elts[1], ast.Name)
                    ):
                        temp_names.add(target.elts[1].id)
        elif isinstance(node, ast.With):
            for item in node.items:
                if item.optional_vars is None:
                    continue
                if _is_tempfile_call(
                    item.context_expr,
                    "NamedTemporaryFile",
                    tempfile_aliases,
                    named_temp_names,
                ):
                    temp_names.update(_iter_target_names(item.optional_vars))

    return os_aliases, os_replace_names, fcntl_aliases, flock_names, temp_names


def _is_os_replace_call(
    func: ast.expr,
    os_aliases: set[str],
    os_replace_names: set[str],
) -> bool:
    if isinstance(func, ast.Name):
        return func.id in os_replace_names
    return (
        isinstance(func, ast.Attribute)
        and func.attr == "replace"
        and isinstance(func.value, ast.Name)
        and func.value.id in os_aliases
    )


def _is_temp_name(name: str, temp_names: set[str]) -> bool:
    lowered = name.lower()
    return "tmp" in lowered or "temp" in lowered or name in temp_names


def _is_path_like_receiver(receiver: ast.expr, temp_names: set[str]) -> bool:
    if _is_attr_call(receiver, PATH_TEMP_METHODS):
        return True
    return isinstance(receiver, ast.Name) and _is_temp_name(receiver.id, temp_names)


def _is_path_replace_call(node: ast.Call, temp_names: set[str]) -> bool:
    if not isinstance(node.func, ast.Attribute) or node.func.attr != "replace":
        return False
    if len(node.args) != 1 or node.keywords:
        return False
    if any(isinstance(arg, ast.Starred) for arg in node.args):
        return False
    arg = node.args[0]
    if isinstance(arg, ast.Constant) and isinstance(arg.value, str):
        return False
    return _is_path_like_receiver(node.func.value, temp_names)


def _is_flock_call(
    func: ast.expr,
    fcntl_aliases: set[str],
    flock_names: set[str],
) -> bool:
    if isinstance(func, ast.Name):
        return func.id in flock_names
    return (
        isinstance(func, ast.Attribute)
        and func.attr == "flock"
        and isinstance(func.value, ast.Name)
        and func.value.id in fcntl_aliases
    )


def _lock_mode_names(node: ast.AST) -> set[str]:
    names: set[str] = set()
    for child in ast.walk(node):
        if isinstance(child, ast.Name):
            names.add(child.id)
        elif isinstance(child, ast.Attribute):
            names.add(child.attr)
    return names


def _is_bare_lock_ex(node: ast.Call) -> bool:
    if len(node.args) < 2:
        return False
    lock_names = _lock_mode_names(node.args[1])
    return "LOCK_EX" in lock_names and "LOCK_NB" not in lock_names


def _open_mode_arg(node: ast.Call, pos_index: int) -> ast.AST | None:
    if len(node.args) > pos_index:
        return node.args[pos_index]
    for keyword in node.keywords:
        if keyword.arg == "mode":
            return keyword.value
    return None


def _mode_is_write(mode_arg: ast.AST | None) -> bool:
    return (
        isinstance(mode_arg, ast.Constant)
        and isinstance(mode_arg.value, str)
        and mode_arg.value.startswith(("w", "a"))
    )


def scan_source(source: str, filename: str = "<source>") -> list[tuple[int, str, str]]:
    """Return ``(lineno, kind, detail)`` mechanic violations for source."""
    tree = ast.parse(source, filename=filename)
    os_aliases, os_replace_names, fcntl_aliases, flock_names, temp_names = (
        _collect_bindings(tree)
    )
    parents = _collect_parent_map(tree)
    scope_vars = {
        id(scope): _collect_journal_path_vars(scope) for scope in _iter_scopes(tree)
    }

    findings: list[tuple[int, str, str]] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        root_names, target_names = scope_vars[id(_nearest_scope(node, tree, parents))]

        if _is_os_replace_call(node.func, os_aliases, os_replace_names):
            findings.append((node.lineno, "os.replace", _call_detail(node.func)))
            continue

        if _is_path_replace_call(node, temp_names):
            findings.append((node.lineno, "Path.replace", _call_detail(node.func)))
            continue

        if _is_flock_call(node.func, fcntl_aliases, flock_names) and _is_bare_lock_ex(
            node
        ):
            findings.append((node.lineno, "flock(LOCK_EX)", _call_detail(node.func)))
            continue

        if isinstance(node.func, ast.Name) and node.func.id == "open":
            mode = _open_mode_arg(node, 1)
            if (
                node.args
                and _mode_is_write(mode)
                and _is_journal_data_target(node.args[0], root_names, target_names)
            ):
                findings.append((node.lineno, "open(write)", _call_detail(node.func)))
            continue

        if isinstance(node.func, ast.Attribute) and node.func.attr == "open":
            mode = _open_mode_arg(node, 0)
            if _mode_is_write(mode) and _is_journal_data_target(
                node.func.value,
                root_names,
                target_names,
            ):
                findings.append(
                    (node.lineno, "Path.open(write)", _call_detail(node.func))
                )
            continue

        if isinstance(node.func, ast.Attribute) and node.func.attr == "write_text":
            if _is_journal_data_target(node.func.value, root_names, target_names):
                findings.append(
                    (node.lineno, "Path.write_text", _call_detail(node.func))
                )
            continue

        if isinstance(node.func, ast.Attribute) and node.func.attr == "write_bytes":
            if _is_journal_data_target(node.func.value, root_names, target_names):
                findings.append(
                    (node.lineno, "Path.write_bytes", _call_detail(node.func))
                )

    findings.sort()
    return findings


def scan_file(path: Path) -> list[tuple[int, str, str]]:
    return scan_source(path.read_text(encoding="utf-8"), filename=str(path))


def count_violations(root: Path) -> dict[tuple[str, str], int]:
    """Map ``(posix-relpath, kind)`` -> occurrence count across the tree."""
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
    """Return ``(new_violations, tracked)`` human-readable lines."""
    new: list[str] = []
    tracked: list[str] = []
    for rel in discover_modules(root):
        rel_str = rel.as_posix()
        findings = scan_file(root / rel)
        by_kind: dict[str, list[int]] = {}
        for lineno, kind, _detail in findings:
            by_kind.setdefault(kind, []).append(lineno)
        for kind, linenos in sorted(by_kind.items()):
            count = len(linenos)
            allowed = allowlist.get((rel_str, kind), 0)
            if count > allowed:
                lines = ", ".join(str(n) for n in sorted(linenos))
                new.append(
                    f"{rel_str}: raw journal I/O mechanic {kind} "
                    f"({count} occurrence(s), allowed {allowed}) at line(s) {lines}"
                )
            elif allowed:
                tracked.append(f"{rel_str}: {count}/{allowed} {kind} (allowlisted)")
    return new, tracked


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Journal raw-mechanic lint")
    parser.add_argument(
        "--root",
        type=Path,
        default=ROOT,
        help="Repository root to scan (defaults to the checkout root).",
    )
    args = parser.parse_args(argv)

    new, tracked = evaluate(args.root, ALLOWLIST)

    if tracked:
        print("journal-io-mechanic: known violations (allowlisted, ratcheting down):")
        for line in tracked:
            print(f"  {line}")
        print()

    if new:
        print("journal-io-mechanic: NEW violations:", file=sys.stderr)
        for line in new:
            print(f"  {line}", file=sys.stderr)
        print(file=sys.stderr)
        print(
            "Route durable journal writes through solstone.think.journal_io.",
            file=sys.stderr,
        )
        return 1

    print("journal-io-mechanic: pass")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
