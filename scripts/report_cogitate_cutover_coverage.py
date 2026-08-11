#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Report the file coverage of the cogitate-cutover detector.

This deliberately reuses the detector's file selection and reference-artifact
rules. The detector itself remains limited to enforcing the cutover invariant;
this companion reports the AC4 coverage accounting without changing it.
"""

from __future__ import annotations

import argparse
import ast
from collections import Counter
from pathlib import Path

if __package__:
    from . import check_cogitate_cutover as checker
else:
    import check_cogitate_cutover as checker


def _reference_artifact_reason(rel: Path) -> str | None:
    """Return the first detector skip rule that applies to ``rel``."""
    parts = rel.parts
    if "tests" in parts:
        return "reference artifact: tests path component"
    if "scripts" in parts:
        return "reference artifact: scripts path component"
    if "build" in parts:
        return "reference artifact: build path component"
    if rel.name.startswith("test_"):
        return "reference artifact: test_* filename"
    if rel.name == "conftest.py":
        return "reference artifact: conftest.py filename"
    return None


def coverage(paths: list[Path], *, root: Path) -> Counter[str]:
    """Classify a detector candidate set without reimplementing its policy."""
    counts: Counter[str] = Counter(selected=len(paths))
    for path in sorted(paths):
        rel = path.relative_to(root)
        if checker._is_reference_artifact(rel):
            reason = _reference_artifact_reason(rel)
            assert reason is not None
            counts[reason] += 1
            continue
        try:
            ast.parse(path.read_text(encoding="utf-8"))
        except OSError:
            counts["skipped: read error"] += 1
        except SyntaxError:
            counts["skipped: parse error"] += 1
        else:
            counts["parsed"] += 1
    return counts


def main(*, root: Path = checker.ROOT, paths: list[Path] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--all", action="store_true", help="inspect every file on disk"
    )
    args = parser.parse_args()
    selected_paths = paths if paths is not None else (
        checker._all_python_files(root=root)
        if args.all
        else checker._tracked_python_files(root=root)
    )
    counts = coverage(selected_paths, root=root)
    findings = checker.scan(selected_paths, root=root)
    skipped = sum(
        count
        for reason, count in counts.items()
        if reason not in {"selected", "parsed"}
    )

    print("cogitate cutover coverage:")
    print(f"  selected: {counts['selected']}")
    print(f"  parsed: {counts['parsed']}")
    print(f"  skipped: {skipped}")
    reasons = sorted(
        reason for reason in counts if reason not in {"selected", "parsed"}
    )
    for reason in reasons:
        print(f"    {reason}: {counts[reason]}")
    print(f"  detector findings: {len(findings)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
