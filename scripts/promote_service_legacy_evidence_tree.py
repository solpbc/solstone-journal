#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Atomically promote a complete staged service-legacy fixture tree.

This is a hand-run evidence-capture helper, not a CI dependency. It keeps the
previous tree intact until the replacement is completely staged and restores it
if promotion cannot complete.
"""

from __future__ import annotations

import argparse
import shutil
from pathlib import Path


class PromotionError(RuntimeError):
    """A staged fixture tree cannot safely replace the current tree."""


def promote_tree(staged: Path, destination: Path, expected_files: int) -> None:
    """Replace *destination* with a complete staged tree, rolling back on failure."""
    if not staged.is_dir():
        raise PromotionError(f"staged fixture directory is missing: {staged}")
    files = [path for path in staged.rglob("*") if path.is_file()]
    if len(files) != expected_files or any(path.suffix != ".json" for path in files):
        raise PromotionError(
            f"staged fixture tree is incomplete: expected {expected_files} JSON files, found {len(files)}"
        )
    if not destination.is_dir():
        raise PromotionError(f"current fixture directory is missing: {destination}")
    backup = destination.with_name(destination.name + ".previous")
    if backup.exists():
        raise PromotionError(
            f"recovery backup exists: {backup}; inspect or restore it before retrying promotion"
        )

    destination.replace(backup)
    try:
        staged.replace(destination)
    except OSError as error:
        try:
            backup.replace(destination)
        except OSError as rollback_error:
            raise PromotionError(
                f"promotion failed and rollback also failed; current data is at {backup}: {rollback_error}"
            ) from error
        raise PromotionError(
            f"promotion failed; restored previous fixture tree: {error}"
        ) from error
    shutil.rmtree(backup)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--staged", type=Path, required=True, help="complete staged fixture directory"
    )
    parser.add_argument(
        "--destination",
        type=Path,
        required=True,
        help="current fixture directory to replace",
    )
    parser.add_argument(
        "--expected-files", type=int, required=True, help="required JSON fixture count"
    )
    args = parser.parse_args()
    promote_tree(args.staged.resolve(), args.destination.resolve(), args.expected_files)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
