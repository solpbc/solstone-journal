#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Capture the historical ``service.py`` commit census.

This is a hand-run, one-time-per-regeneration capture tool, not a CI
dependency. It derives its complete inventory from the live git history and
writes the committed fixture consumed by later evidence work.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

from service_legacy_paths import capture_input, evidence_root

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = evidence_root() / "follow-census.json"
CURRENT_PATH = "solstone/think/service.py"
LEGACY_PATH = "think/service.py"
EXPECTED_COUNT = 44


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout


def path_at_commit(commit: str) -> str:
    paths = [
        path
        for path in (CURRENT_PATH, LEGACY_PATH)
        if subprocess.run(
            ["git", "cat-file", "-e", f"{commit}:{path}"],
            cwd=ROOT,
            capture_output=True,
            check=False,
        ).returncode
        == 0
    ]
    if len(paths) != 1:
        raise RuntimeError(
            f"expected exactly one service.py path at {commit}, found {paths!r}"
        )
    return paths[0]


def main() -> int:
    head = capture_input()
    commits = [
        line
        for line in git(
            "log", "--follow", "--format=%H", head, "--", CURRENT_PATH
        ).splitlines()
        if line
    ]
    commits.reverse()
    if len(commits) != EXPECTED_COUNT:
        raise RuntimeError(
            f"expected {EXPECTED_COUNT} service.py commits, found {len(commits)}"
        )

    entries: list[dict[str, object]] = []
    blobs: set[str] = set()
    for index, commit in enumerate(commits):
        path = path_at_commit(commit)
        blob = git("rev-parse", f"{commit}:{path}").strip()
        if blob in blobs:
            raise RuntimeError(f"duplicate service.py blob at index {index}: {blob}")
        blobs.add(blob)
        entries.append({"index": index, "commit": commit, "path": path, "blob": blob})

    payload = {
        "schema": "service-legacy-follow-census",
        "schema_version": 1,
        "root_commit": entries[0]["commit"],
        "head_commit": entries[-1]["commit"],
        "entries": entries,
    }
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(
        f"wrote {len(entries)} entries: {entries[0]['commit']}..{entries[-1]['commit']}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
