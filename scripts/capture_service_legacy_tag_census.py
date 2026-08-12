#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Capture the historical ``service.py`` v0 tag census.

This is a hand-run, one-time-per-regeneration capture tool, not a CI
dependency. It independently derives the tag and history inventories from
live git before writing the committed fixture used by later evidence work.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "core/fixtures/service_legacy_evidence/tag-census.json"
CURRENT_PATH = "solstone/think/service.py"
LEGACY_PATH = "think/service.py"
EXPECTED_TAG_COUNT = 66


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout


def tree_entry(tag: str) -> tuple[str | None, str | None]:
    output = git("ls-tree", tag, "--", CURRENT_PATH, LEGACY_PATH)
    entries = [line for line in output.splitlines() if line]
    if not entries:
        return None, None
    if len(entries) != 1:
        raise RuntimeError(
            f"expected at most one service.py path at {tag}, found {entries!r}"
        )
    metadata, path = entries[0].split("\t", 1)
    _mode, object_type, blob = metadata.split()
    if object_type != "blob":
        raise RuntimeError(f"expected blob service.py entry at {tag}, found {object_type}")
    if path not in {CURRENT_PATH, LEGACY_PATH}:
        raise RuntimeError(f"unexpected service.py path at {tag}: {path}")
    return path, blob


def history_blobs() -> set[str]:
    commits = [
        line
        for line in git("log", "--follow", "--format=%H", "--", CURRENT_PATH).splitlines()
        if line
    ]
    blobs: set[str] = set()
    for commit in commits:
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
        blobs.add(git("rev-parse", f"{commit}:{paths[0]}").strip())
    return blobs


def main() -> int:
    tags = [
        line
        for line in git("tag", "--list", "--sort=version:refname", "v0.*").splitlines()
        if line
    ]
    if len(tags) != EXPECTED_TAG_COUNT:
        raise RuntimeError(f"expected {EXPECTED_TAG_COUNT} v0 tags, found {len(tags)}")

    follow_blobs = history_blobs()
    records: list[dict[str, str | None]] = []
    for tag in tags:
        path, blob = tree_entry(tag)
        if blob is not None and blob not in follow_blobs:
            raise RuntimeError(f"tag {tag} has blob absent from --follow history: {blob}")
        records.append({"tag": tag, "path": path, "blob": blob})

    payload = {
        "schema": "service-legacy-tag-census",
        "schema_version": 1,
        "tags": records,
    }
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {len(records)} tags", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
