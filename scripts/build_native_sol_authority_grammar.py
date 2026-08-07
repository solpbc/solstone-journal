#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

try:
    from scripts.build_native_sol_inventory import (
        FINAL_JOURNAL_PYTHON_COMPAT_TOTAL,
        ORACLE_PATH,
        REPO_ROOT,
        AuthorityEntry,
        check_oracle_subset,
        discover,
        format_paths,
        transformed_oracle_entries,
    )
except ModuleNotFoundError:  # pragma: no cover - direct script execution path.
    from build_native_sol_inventory import (  # type: ignore[no-redef]
        FINAL_JOURNAL_PYTHON_COMPAT_TOTAL,
        ORACLE_PATH,
        REPO_ROOT,
        AuthorityEntry,
        check_oracle_subset,
        discover,
        format_paths,
        transformed_oracle_entries,
    )

SCHEMA = "native-sol-authority-grammar-v1"


def authority_projection(entries: list[AuthorityEntry]) -> list[dict[str, Any]]:
    return [
        {
            "path": list(entry.path),
            "kind": entry.kind,
            "help": entry.help,
            "params": entry.params,
        }
        for entry in sorted(entries, key=lambda item: item.path)
        if entry.surface == "sol-call"
    ]


def build_projection(root: Path) -> tuple[bytes, list[str]]:
    entries = [entry for entry in discover(root) if entry.surface == "sol-call"]
    projection = authority_projection(entries)
    errors: list[str] = []
    if not entries:
        errors.append("native sol authority scan is empty")
    if not projection:
        errors.append("native sol authority grammar projection is empty")
    errors.extend(reconcile_against_oracle(entries))
    payload = {"schema": SCHEMA, "entries": projection}
    data = (
        json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
        + "\n"
    ).encode("utf-8")
    return data, errors


def reconcile_against_oracle(entries: list[AuthorityEntry]) -> list[str]:
    errors = check_oracle_subset(entries, ORACLE_PATH)
    oracle_errors, oracle_entries = transformed_oracle_entries(ORACLE_PATH)
    oracle_paths = set(oracle_entries)
    errors.extend(oracle_errors)
    authority_paths = {entry.path for entry in entries if entry.surface == "sol-call"}
    if not authority_paths:
        return errors
    extra = sorted(authority_paths - oracle_paths)
    if extra:
        errors.append(f"authority paths outside frozen oracle: {format_paths(extra)}")
    uncovered = sorted(oracle_paths - authority_paths)
    non_journal_uncovered = [path for path in uncovered if path[0] != "journal"]
    if non_journal_uncovered:
        errors.append(
            "non-journal oracle paths lack native authority: "
            f"{format_paths(non_journal_uncovered)}"
        )
    if len(uncovered) != FINAL_JOURNAL_PYTHON_COMPAT_TOTAL:
        errors.append(
            "frozen oracle remainder count "
            f"{len(uncovered)} != {FINAL_JOURNAL_PYTHON_COMPAT_TOTAL}"
        )
    if not uncovered:
        errors.append("frozen oracle remainder is empty")
    return errors


def describe(data: bytes) -> str:
    payload = json.loads(data)
    return (
        f"entries={len(payload['entries'])} bytes={len(data)} "
        f"sha256={hashlib.sha256(data).hexdigest()}"
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build and check the authority-derived native sol grammar."
    )
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    parser.add_argument(
        "--output",
        type=Path,
        help="Optional path for the derived authority projection.",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Check authority projection against the frozen grammar oracle.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    data, errors = build_projection(args.root.resolve())
    if errors:
        print("native sol authority grammar check failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(data)
        print(f"wrote {args.output}: {describe(data)}")
    elif args.check:
        print(f"native sol authority grammar ok: {describe(data)}")
    else:
        sys.stdout.buffer.write(data)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
