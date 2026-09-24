#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Guard that the removed natural-language time parser has no live callers."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
NEEDLES = ("parse_time_range", "timefhuman")
SKIP_DIRS = {
    ".git",
    ".hg",
    ".hypothesis",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".venv",
    "__pycache__",
    "build",
    "dist",
    "htmlcov",
    "node_modules",
    "scratch",
    "target",
}
ALLOWED_PATHS = {
    "scripts/check_removed_time_parser_ready.py",
}


@dataclass(frozen=True)
class Blocker:
    path: Path
    line_number: int
    line: str


def find_blockers(root: Path = REPO_ROOT) -> list[Blocker]:
    blockers: list[Blocker] = []
    for path in iter_files(root):
        rel = path.relative_to(root).as_posix()
        if rel in ALLOWED_PATHS:
            continue
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except UnicodeDecodeError:
            continue
        for line_number, line in enumerate(lines, 1):
            if any(needle in line for needle in NEEDLES):
                blockers.append(Blocker(path, line_number, line.strip()))
    return blockers


def iter_files(root: Path) -> list[Path]:
    files: list[Path] = []
    for path in root.rglob("*"):
        parts = path.relative_to(root).parts
        if any(part in SKIP_DIRS or part.endswith(".egg-info") for part in parts):
            continue
        if path.is_file():
            files.append(path)
    return sorted(files)


def main() -> int:
    blockers = find_blockers(REPO_ROOT)
    if blockers:
        print("removed time parser readiness: RETAIN")
        for blocker in blockers:
            rel = blocker.path.relative_to(REPO_ROOT).as_posix()
            print(f"- {rel}:{blocker.line_number}: {blocker.line}")
        return 1
    print("removed time parser readiness: delete-safe")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
