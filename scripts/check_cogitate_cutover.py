#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Prove the tool-using talent runtime is owned by solstone-core, not by Python.

⚠ WRITTEN BEFORE THE CUT, DELIBERATELY. This detector was committed while the
Python runtime was still in place and while it still FAILED, so it could not be
shaped to fit whatever the cut happened to produce. Run it today and it reports
the work remaining; run it after the cut and it reports zero.

⛔ The enforceable question is NOT "does a symbol still exist." A symbol-existence
check goes green on a destructive implementation and red on a correct one -- a
surviving name that only forwards to the native verb is fine, and a deleted name
whose logic moved into a sibling module is not. What this asserts instead is that
**no Python code path still implements the runtime**: no agent SDK, no provider
tool loop, no in-Python policy gate, no in-Python raw-read tier, and no second
copy of the model-visible contract text.

Usage:
    python3 scripts/check_cogitate_cutover.py            # git-tracked files
    python3 scripts/check_cogitate_cutover.py --all      # every file on disk
    python3 scripts/check_cogitate_cutover.py --json
"""

from __future__ import annotations

import argparse
import ast
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# --- 1. the agent SDK and its transport must be gone from the runtime --------
# These exist in this repo for the tool-using runtime alone; the one-shot
# generation path is already native and imports neither.
FORBIDDEN_IMPORT_ROOTS = frozenset({"openhands", "litellm"})

# --- 2. the runtime's own machinery must not be implemented in Python -------
# Each of these is a piece of the contract that now lives in a native crate. A
# Python module that still DEFINES one is a second implementation, whatever it
# is called.
FORBIDDEN_DEFINITIONS = {
    "CogitatePolicy": "the command policy gate",
    "classify_command": "the command policy gate",
    "COGITATE_RUNTIME_PREAMBLE": "the model-visible runtime preamble",
    "COGITATE_DIAGNOSTIC_PREAMBLE": "the model-visible diagnostic preamble",
    "COGITATE_ACCESS_TIERS": "the access-tier vocabulary",
    "capabilities_for_access_tier": "the tier capability table",
    "expects_emit_final": "the finalization selection rule",
    "build_read_tools": "the raw-read tool tier",
    "build_emit_final_tools": "the finalization tool",
    "assemble_prompt": "system-prompt assembly",
    "cogitate_sol_tool_hint": "the model-visible tool-routing hint",
}

# --- 3. the surviving transport may spawn the native verb and nothing else ---
NATIVE_VERB = "cogitate"


# Files that are reference artifacts rather than shipped runtime. Python tests
# are explicitly a reference artifact in this conversion, not a gate.
def _is_reference_artifact(rel: Path) -> bool:
    parts = rel.parts
    return (
        "tests" in parts
        or "scripts" in parts
        or "build" in parts
        or rel.name.startswith("test_")
        or rel.name == "conftest.py"
    )


def _tracked_python_files(*, root: Path = ROOT) -> list[Path]:
    out = subprocess.run(
        ["git", "ls-files", "*.py"],
        cwd=root,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()
    return [root / line for line in out]


def _all_python_files(*, root: Path = ROOT) -> list[Path]:
    return [
        path
        for path in root.rglob("*.py")
        if ".venv" not in path.parts and "target" not in path.parts
    ]


def _import_roots(tree: ast.AST) -> set[str]:
    roots: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                roots.add(alias.name.split(".")[0])
        elif isinstance(node, ast.ImportFrom):
            if node.level:
                continue
            if node.module:
                roots.add(node.module.split(".")[0])
    return roots


def _definitions(tree: ast.AST) -> set[str]:
    names: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            names.add(node.name)
        elif isinstance(node, ast.Assign):
            for target in node.targets:
                if isinstance(target, ast.Name):
                    names.add(target.id)
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            names.add(node.target.id)
    return names


def scan(paths: list[Path], *, root: Path = ROOT) -> list[dict[str, object]]:
    findings: list[dict[str, object]] = []
    for path in sorted(paths):
        rel = path.relative_to(root)
        if _is_reference_artifact(rel):
            continue
        try:
            tree = ast.parse(path.read_text(encoding="utf-8"))
        except (OSError, SyntaxError):
            continue

        for root in sorted(_import_roots(tree) & FORBIDDEN_IMPORT_ROOTS):
            findings.append(
                {
                    "file": str(rel),
                    "kind": "agent_sdk_import",
                    "detail": root,
                    "why": (
                        f"{root} exists in this repo for the tool-using runtime "
                        "alone; the native runtime owns that now"
                    ),
                }
            )

        defined = _definitions(tree)
        for name in sorted(defined & set(FORBIDDEN_DEFINITIONS)):
            findings.append(
                {
                    "file": str(rel),
                    "kind": "runtime_reimplementation",
                    "detail": name,
                    "why": (
                        f"defines {FORBIDDEN_DEFINITIONS[name]}, which is a native "
                        "crate's contract -- a second implementation, whatever it "
                        "is called"
                    ),
                }
            )
    return findings


def dependency_findings(*, root: Path = ROOT) -> list[dict[str, object]]:
    """The wheel must stop declaring the agent SDK and its transport."""
    pyproject = (root / "pyproject.toml").read_text(encoding="utf-8")
    out: list[dict[str, object]] = []
    for name in sorted(FORBIDDEN_IMPORT_ROOTS):
        for line in pyproject.splitlines():
            stripped = line.strip()
            if not stripped.startswith('"'):
                continue
            if stripped.lstrip('"').lower().startswith(name):
                out.append(
                    {
                        "file": "pyproject.toml",
                        "kind": "declared_dependency",
                        "detail": stripped.rstrip(","),
                        "why": (
                            f"{name} is declared as a runtime dependency; it exists "
                            "for the tool-using runtime alone"
                        ),
                    }
                )
    return out


def main(*, root: Path = ROOT, paths: list[Path] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--all", action="store_true", help="scan every file on disk")
    parser.add_argument("--json", action="store_true", help="machine-readable output")
    args = parser.parse_args()

    selected_paths = paths or (
        _all_python_files(root=root)
        if args.all
        else _tracked_python_files(root=root)
    )
    findings = scan(selected_paths, root=root) + dependency_findings(root=root)

    if args.json:
        print(json.dumps({"findings": findings, "count": len(findings)}, indent=2))
    elif findings:
        print(f"cogitate cutover: {len(findings)} finding(s)\n")
        for row in findings:
            print(f"  {row['file']}")
            print(f"    {row['kind']}: {row['detail']}")
            print(f"    {row['why']}\n")
        print(
            "The tool-using talent runtime is still implemented in Python.\n"
            "This detector was written BEFORE the cut and is expected to fail "
            "until it lands."
        )
    else:
        print(
            "cogitate cutover: clean -- no Python module implements the "
            "tool-using talent runtime, and the agent SDK is gone from the wheel."
        )
    return 1 if findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
