# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
import subprocess
from pathlib import Path

from scripts.build_native_sol_journal_host_commands import extract_partitions

EXCLUDED_DIRS = {
    ".git",
    ".venv",
    "build",
    "dist",
    "htmlcov",
    "journal",
    "logs",
    "node_modules",
    "scratch",
    "tmp",
    "vpe",
}
TEXT_SUFFIXES = {
    ".html",
    ".js",
    ".md",
    ".py",
    ".rules",
    ".sh",
    ".toml",
    ".txt",
    ".yml",
    ".yaml",
}
ROOT_TEXT_FILES = {
    "AGENTS.md",
    "CONTRIBUTING.md",
    "INSTALL.md",
    "Makefile",
    "README.md",
}
ACCESS_POSITIVE_EXPECTATIONS = {
    Path("AGENTS.md"): re.compile(r"\bsol call\b"),
    Path("INSTALL.md"): re.compile(r"\bsol skills\b"),
    Path("Makefile"): re.compile(r"\$\(VENV_BIN\)/sol skills\b"),
    Path("README.md"): re.compile(r"\bsol chat\b"),
}


def _is_text_surface(path: Path) -> bool:
    if any(part in EXCLUDED_DIRS for part in path.parts):
        return False
    if path.name == "CHANGELOG.md":
        return False
    if path.name in ROOT_TEXT_FILES:
        return True
    return path.suffix in TEXT_SUFFIXES


def _tracked_sol_lines() -> list[tuple[Path, int, str]]:
    result = subprocess.run(
        ["git", "grep", "-n", "-I", "-F", "sol ", "--"],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode not in {0, 1}:
        result.check_returncode()
    matches = []
    for raw_line in result.stdout.splitlines():
        path_text, line_number, line = raw_line.split(":", maxsplit=2)
        path = Path(path_text)
        if path.exists() and _is_text_surface(path):
            matches.append((path, int(line_number), line))
    return matches


def _skip_line(path: Path, line: str) -> bool:
    if path in {
        Path("tests/test_cli_prog_fidelity.py"),
        Path("tests/test_journal_cli_migration.py"),
    }:
        return True
    if (
        path == Path("tests/test_install_guard.py")
        and "managed by 'sol config'" in line
    ):
        return True
    # "how should sol think?" is owner-facing init wizard copy (a question to the
    # owner), not a `sol think` service-command reference. Exempt the prose phrase
    # wherever it appears (the init template and the tests that assert it).
    if "how should sol think" in line:
        return True
    # "let sol describe what's in it" is owner-facing import-affordance copy (sol
    # the keeper describing an image), not a `sol describe` service-command
    # reference. Exempt the prose phrase wherever it appears.
    if "let sol describe what's in it" in line:
        return True
    if path.parts and path.parts[0] == "tests":
        argv_markers = (
            "sys.argv",
            '["sol ',
            "usage: sol ",
            "run_main(mod,",
            "sol think.talents",
        )
        if any(marker in line for marker in argv_markers):
            return True
        if re.search(
            r'"sol (' + "|".join(map(re.escape, SERVICE_TERMS)) + r")\b", line
        ):
            return True
    return False


_PARTITIONS = extract_partitions()
SERVICE_TERMS = sorted(
    {*_PARTITIONS.service_commands, *_PARTITIONS.service_aliases},
    key=len,
    reverse=True,
)
SERVICE_SOL_RE = re.compile(
    r"\bsol (" + "|".join(map(re.escape, SERVICE_TERMS)) + r")\b"
)
SERVICE_SOL_LITERAL_RE = re.compile(
    r"""['"]sol['"]\s*,\s*['"](?:"""
    + "|".join(map(re.escape, SERVICE_TERMS))
    + r""")['"]"""
)


def test_service_tagged_commands_are_not_documented_as_sol() -> None:
    service_matches = [
        f"{path}:{line_number}: {line}"
        for path, line_number, line in _tracked_sol_lines()
        if not _skip_line(path, line) and SERVICE_SOL_RE.search(line)
    ]
    missing_access_expectations = [
        str(path)
        for path, expectation in ACCESS_POSITIVE_EXPECTATIONS.items()
        if not path.exists()
        or expectation.search(path.read_text(encoding="utf-8")) is None
    ]

    assert service_matches == []
    assert missing_access_expectations == []


def _production_python_files() -> list[Path]:
    return sorted(
        path
        for path in Path("solstone").rglob("*.py")
        if path.is_file() and "tests" not in path.parts
    )


def test_production_service_commands_do_not_dispatch_through_sol() -> None:
    matches = []
    for path in _production_python_files():
        text = path.read_text(encoding="utf-8")
        for match in SERVICE_SOL_LITERAL_RE.finditer(text):
            line_number = text.count("\n", 0, match.start()) + 1
            matches.append(f"{path}:{line_number}: {match.group(0)!r}")

    assert matches == []
