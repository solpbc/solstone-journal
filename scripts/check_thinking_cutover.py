#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Report shipped paths that still reach the Python thinking tree.

The predicate is deliberately limited to shipped request code and shipped native
compile inputs. It scans production Python under ``solstone/`` and Rust under
``core/``; it excludes documentation, repository tests, app test subtrees, and
``scripts/`` because those are not shipped request paths or native build inputs.
The retained Python thinking tree is also excluded as the reference, not a
consumer of itself.
"""

from __future__ import annotations

import argparse
import ast
import json
import re
from dataclasses import asdict, dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
PYTHON_FORMS = ("solstone.apps.thinking", "apps.thinking")
PATH_FORMS = ("solstone/apps/thinking", "apps/thinking")
PATH_ATTRIBUTE_RE = re.compile(r'authority_path:\s*"([^"\n]+)"')
RUST_PATH_RE = re.compile(r'#\[path\s*=\s*"([^"\n]+)"\]')


@dataclass(frozen=True)
class Finding:
    file: str
    line: int
    kind: str
    detail: str
    why: str


def _matches_python_module(value: str) -> bool:
    return any(value == form or value.startswith(f"{form}.") for form in PYTHON_FORMS)


def _matches_path(value: str) -> bool:
    return any(form in value for form in PATH_FORMS)


def _source_files(root: Path) -> list[Path]:
    return sorted(
        path
        for path in (root / "solstone").rglob("*.py")
        if "tests" not in path.parts
        and not path.is_relative_to(root / "solstone" / "apps" / "thinking")
    )


def _rust_files(root: Path) -> list[Path]:
    return sorted(
        path
        for path in (root / "core").rglob("*.rs")
        if "tests" not in path.parts
    )


def _call_name(node: ast.Call) -> str | None:
    if isinstance(node.func, ast.Name):
        return node.func.id
    if isinstance(node.func, ast.Attribute) and isinstance(node.func.value, ast.Name):
        return f"{node.func.value.id}.{node.func.attr}"
    return None


def _literal_string(node: ast.AST) -> str | None:
    return node.value if isinstance(node, ast.Constant) and isinstance(node.value, str) else None


def _python_findings(root: Path, path: Path) -> list[Finding]:
    rel = path.relative_to(root).as_posix()
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    findings: list[Finding] = []
    seen: set[tuple[int, str]] = set()

    def add(line: int, kind: str, detail: str, why: str) -> None:
        key = (line, kind)
        if key not in seen:
            seen.add(key)
            findings.append(Finding(rel, line, kind, detail, why))

    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                if _matches_python_module(alias.name):
                    add(node.lineno, "served-python-coupling", alias.name, "production import reaches the Python thinking module")
        elif isinstance(node, ast.ImportFrom) and node.module:
            imports_parent_thinking = (
                node.module in {"solstone.apps", "apps"}
                and any(alias.name == "thinking" for alias in node.names)
            )
            imports_thinking = _matches_python_module(node.module) or imports_parent_thinking
            if imports_thinking:
                detail = f"{node.module}.thinking" if imports_parent_thinking else node.module
                add(node.lineno, "served-python-coupling", detail, "production import reaches the Python thinking module")
        elif isinstance(node, ast.Call):
            name = _call_name(node)
            first = _literal_string(node.args[0]) if node.args else None
            if name in {"import_module", "importlib.import_module"} and first and _matches_python_module(first):
                add(node.lineno, "served-python-coupling", first, "runtime module-name import reaches the Python thinking module")
            elif name == "url_for" and first and first.startswith("app:thinking."):
                add(node.lineno, "served-python-coupling", first, "request-time endpoint name requires the Python thinking blueprint")
        elif isinstance(node, ast.Constant) and isinstance(node.value, str):
            if _matches_python_module(node.value):
                add(
                    node.lineno,
                    "served-python-coupling",
                    node.value,
                    "production module-name string reaches the Python thinking module",
                )
            elif _matches_path(node.value):
                add(
                    node.lineno,
                    "served-python-coupling",
                    node.value,
                    "production path string reaches the Python thinking tree",
                )
    return findings


def _masked_rust_line(line: str) -> str | None:
    """Mask double-quoted strings and line comments before counting braces."""
    masked: list[str] = []
    in_string = False
    escaped = False
    index = 0
    while index < len(line):
        character = line[index]
        if in_string:
            masked.append(" ")
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == '"':
                in_string = False
        elif character == "/" and index + 1 < len(line) and line[index + 1] == "/":
            masked.extend(" " * (len(line) - index))
            break
        elif character == '"':
            masked.append(" ")
            in_string = True
        elif re.match(r"(?:br|r)#*\"", line[index:]):
            return None
        else:
            masked.append(character)
        index += 1
    return None if in_string else "".join(masked)


def _test_scope_by_line(lines: list[str]) -> set[int]:
    """Mark only confidently delimited Rust cfg(test) modules.

    Ambiguous scopes are deliberately left unmarked so path findings fail closed
    as blocking instead of being downgraded to recorded test fidelity.
    """
    marked: set[int] = set()
    pending_test_module = False
    depth: int | None = None
    module_re = re.compile(r"^(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{")
    for number, line in enumerate(lines, start=1):
        masked = _masked_rust_line(line)
        stripped = masked.strip() if masked is not None else ""

        if depth is not None:
            if masked is None:
                depth = None
                continue
            marked.add(number)
            depth += masked.count("{") - masked.count("}")
            if depth <= 0:
                depth = None
            continue

        if pending_test_module:
            if masked is None:
                pending_test_module = False
            elif not stripped or stripped.startswith("#["):
                continue
            elif module_re.match(stripped):
                depth = masked.count("{") - masked.count("}")
                pending_test_module = False
                if depth > 0:
                    marked.add(number)
                else:
                    depth = None
                continue
            else:
                pending_test_module = False

        if masked is not None and stripped == "#[cfg(test)]":
            pending_test_module = True
    return marked


def _rust_findings(root: Path, path: Path) -> list[Finding]:
    rel = path.relative_to(root).as_posix()
    lines = path.read_text(encoding="utf-8").splitlines()
    test_lines = _test_scope_by_line(lines)
    findings: list[Finding] = []
    for number, line in enumerate(lines, start=1):
        path_match = RUST_PATH_RE.search(line)
        if path_match:
            target = (path.parent / path_match.group(1)).resolve()
            if target.is_relative_to(root / "solstone" / "apps" / "thinking"):
                findings.append(
                    Finding(
                        rel,
                        number,
                        "served-python-coupling",
                        path_match.group(1),
                        f"Rust #[path] compile input resolves to {target.relative_to(root)}",
                    )
                )
            continue

        authority_match = PATH_ATTRIBUTE_RE.search(line)
        if authority_match and _matches_path(authority_match.group(1)):
            findings.append(
                Finding(
                    rel,
                    number,
                    "recorded-provenance",
                    authority_match.group(1),
                    "inventory metadata names the retained native authority but is not opened by the binary",
                )
            )
            continue

        if _matches_path(line):
            if number in test_lines:
                findings.append(
                    Finding(
                        rel,
                        number,
                        "recorded-reference-fidelity",
                        line.strip(),
                        "cfg(test) parity reads the retained Python reference asset",
                    )
                )
            else:
                findings.append(
                    Finding(
                        rel,
                        number,
                        "served-python-coupling",
                        line.strip(),
                        "shipped Rust path string reaches the Python thinking tree",
                    )
                )
    return findings


def scan(root: Path) -> tuple[list[Finding], list[Finding]]:
    findings = [
        finding
        for path in _source_files(root)
        for finding in _python_findings(root, path)
    ] + [
        finding
        for path in _rust_files(root)
        for finding in _rust_findings(root, path)
    ]
    findings.sort(key=lambda item: (item.file, item.line, item.kind, item.detail))
    return (
        [item for item in findings if item.kind == "served-python-coupling"],
        [item for item in findings if item.kind != "served-python-coupling"],
    )


def _print_finding(finding: Finding) -> None:
    print(f"  {finding.file}:{finding.line}")
    print(f"    {finding.kind}: {finding.detail}")
    print(f"    {finding.why}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="emit machine-readable findings")
    args = parser.parse_args(argv)
    blocking, recorded = scan(ROOT)
    if args.json:
        print(json.dumps({
            "blocking": [asdict(item) for item in blocking],
            "recorded": [asdict(item) for item in recorded],
            "blocking_count": len(blocking),
            "recorded_count": len(recorded),
        }, indent=2))
    else:
        print(f"thinking cutover: {len(blocking)} blocking finding(s); {len(recorded)} recorded finding(s)")
        for finding in [*blocking, *recorded]:
            _print_finding(finding)
        if blocking:
            print("\nShipped request code or native build inputs still reach the Python thinking tree.")
        else:
            print("\nthinking cutover: clean -- no shipped request or native build input reaches the Python thinking tree.")
    return 1 if blocking else 0


if __name__ == "__main__":
    raise SystemExit(main())
