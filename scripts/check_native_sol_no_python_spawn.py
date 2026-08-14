#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Static no-Python-spawn lint for the migrated native sol surface."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

try:
    from scripts.build_native_sol_inventory import REPO_ROOT, discover
except ModuleNotFoundError:  # pragma: no cover - direct script execution path.
    from build_native_sol_inventory import REPO_ROOT, discover  # type: ignore[no-redef]

ALLOWLIST: dict[tuple[str, str], str] = {}

CLIENT_CRATES = (
    REPO_ROOT / "core/crates/solstone-core-cogitate",
    REPO_ROOT / "core/crates/solstone-core-convey-body",
    REPO_ROOT / "core/crates/solstone-core-sol-client",
    REPO_ROOT / "core/crates/solstone-core-sol-client-cli",
)
SEAM_FILE = REPO_ROOT / "core/crates/solstone-core-sol-client/src/seam.rs"
CLI_LIB_FILE = REPO_ROOT / "core/crates/solstone-core-sol-client-cli/src/lib.rs"
PARITY_TEST_FILE = (
    REPO_ROOT / "core/crates/solstone-core-sol-client-cli/tests/parity.rs"
)

FORBIDDEN_PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("direct-std-process-command", re.compile(r"\bstd::process::Command\b")),
    ("direct-tokio-process", re.compile(r"\btokio::process\b")),
    ("direct-process-command", re.compile(r"\bprocess::Command\b")),
    ("direct-command-new", re.compile(r"\bCommand::new\s*\(")),
    ("direct-spawn-call", re.compile(r"\.spawn\s*\(")),
    ("direct-output-call", re.compile(r"\.output\s*\(")),
    ("direct-exec-call", re.compile(r"\bexec(?:[lv][pe]?|ve)?\s*\(")),
    ("pyo3-reference", re.compile(r"\b(?:pyo3|PyO3)\b")),
    ("cpython-reference", re.compile(r"\b(?:cpython|CPython)\b")),
    ("python-fallback-symbol", re.compile(r"\bpython_(?:fallback|dispatch)\b")),
    ("compat-dispatch-symbol", re.compile(r"\bcompat(?:ibility)?_dispatch\b")),
    ("fallback-to-python-symbol", re.compile(r"\bfallback_to_python\b")),
    (
        "python-fallback-string",
        re.compile(r"\b(?:fallback|dispatch)[^\n\"]*python3?\b"),
    ),
)


@dataclass(frozen=True)
class Violation:
    file: str
    kind: str
    detail: str

    @property
    def key(self) -> tuple[str, str]:
        return (self.file, self.kind)


def rel(path: Path) -> str:
    return path.relative_to(REPO_ROOT).as_posix()


def rust_files_to_scan() -> list[Path]:
    files: set[Path] = set()
    files.update(authority_sources())
    for crate in CLIENT_CRATES:
        files.update(crate.rglob("*.rs"))
    return sorted(path for path in files if path.is_file())


def authority_sources() -> set[Path]:
    return {entry.source for entry in discover(REPO_ROOT)}


def collect_violations() -> list[Violation]:
    violations: list[Violation] = []
    for path in rust_files_to_scan():
        text = path.read_text(encoding="utf-8")
        violations.extend(scan_forbidden_patterns(path, text))
        violations.extend(scan_process_spawner_import(path, text))
    violations.extend(check_required_spawn_guard_tests())
    return violations


def scan_forbidden_patterns(path: Path, text: str) -> list[Violation]:
    violations: list[Violation] = []
    for kind, pattern in FORBIDDEN_PATTERNS:
        if pattern.search(text):
            violations.append(
                Violation(
                    rel(path),
                    kind,
                    "migrated native sol code must not spawn or dispatch to Python",
                )
            )
    return violations


def scan_process_spawner_import(path: Path, text: str) -> list[Violation]:
    if path == SEAM_FILE:
        return []
    if re.search(r"\bProcessSpawner\b", text):
        return [
            Violation(
                rel(path),
                "process-spawner-outside-seam",
                "ProcessSpawner is only allowed in the shared seam definition; "
                "host implementations belong in the process shell",
            )
        ]
    return []


def check_required_spawn_guard_tests() -> list[Violation]:
    checks = {
        SEAM_FILE: (
            "missing-failing-spawner-test",
            "failing_spawner_always_errors",
        ),
        CLI_LIB_FILE: (
            "missing-unsupported-without-spawn-test",
            "classifies_unported_call_as_unsupported_without_spawn_path",
        ),
        PARITY_TEST_FILE: (
            "missing-native-parity-test",
            "native_matches_sol_call_parity_vectors",
        ),
    }
    violations: list[Violation] = []
    for path, (kind, needle) in checks.items():
        text = path.read_text(encoding="utf-8")
        if needle in text:
            continue
        violations.append(
            Violation(
                rel(path),
                kind,
                f"required Rust proof {needle} is missing",
            )
        )
    return violations


def main() -> int:
    violations = collect_violations()
    actual = {violation.key: violation for violation in violations}
    allowed = set(ALLOWLIST)
    unexpected = sorted(set(actual) - allowed)
    stale = sorted(allowed - set(actual))

    if unexpected or stale:
        if unexpected:
            print("native sol no-python-spawn violations:")
            for key in unexpected:
                violation = actual[key]
                print(f"- {violation.file}: {violation.kind}: {violation.detail}")
        if stale:
            print("stale native sol no-python-spawn allowlist entries:")
            for file, kind in stale:
                print(f"- {file}: {kind}: {ALLOWLIST[(file, kind)]}")
        return 1

    print("native sol no-python-spawn ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
