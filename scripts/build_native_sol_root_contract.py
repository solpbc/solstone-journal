#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
OUTPUT = REPO_ROOT / "core/fixtures/native-sol/root-contract-v1.json"
JOURNAL_PLACEHOLDER = "${JOURNAL}"
VERSION_PLACEHOLDER = "${VERSION}"

HEADER = "solstone - journal access CLI"
USAGE = "Usage: solstone <command> [args...]"
APPS_HEADING = "Apps (solstone call <app>):"
ACCESS_GROUPS: list[dict[str, Any]] = [
    {"heading": "Your journal", "commands": ["call", "import"]},
    {"heading": "Tools", "commands": ["skills", "link"]},
]
CALL_GROUPS: list[str] = [
    "activities",
    "awareness",
    "body",
    "entities",
    "facets",
    "import",
    "link",
    "settings",
    "sol",
    "speakers",
    "support",
    "thinking",
    "transcripts",
    "health",
    "journal",
    "navigate",
    "profile",
    "identity",
]


def render_stdout(
    *,
    header: str,
    usage: str,
    groups: list[dict[str, Any]],
    apps: list[str],
) -> str:
    lines: list[str] = [
        header,
        "",
        usage,
        "",
    ]
    for group in groups:
        lines.append(group["heading"])
        lines.extend(f"  {command}" for command in group["commands"])
        lines.append("")
    lines.append(APPS_HEADING)
    lines.extend(f"  call {name}" for name in apps)
    lines.append("")
    return "\n".join(lines) + "\n"


def build() -> dict[str, Any]:
    if len(CALL_GROUPS) != 18:
        raise RuntimeError(
            f"root call group count {len(CALL_GROUPS)} != 18: {CALL_GROUPS!r}"
        )
    if len(CALL_GROUPS) != len(set(CALL_GROUPS)):
        raise RuntimeError(f"duplicate root call groups: {CALL_GROUPS!r}")
    return {
        "schema": "native-sol-root-contract-v1",
        "oracle": {
            "status": "native",
            "generator": "scripts/build_native_sol_root_contract.py",
        },
        "placeholders": {
            "journal": JOURNAL_PLACEHOLDER,
            "version": VERSION_PLACEHOLDER,
        },
        "header": HEADER,
        "usage": USAGE,
        "access_groups": ACCESS_GROUPS,
        "call_groups": CALL_GROUPS,
        "expected_bare_sol_stdout": render_stdout(
            header=HEADER, usage=USAGE, groups=ACCESS_GROUPS, apps=CALL_GROUPS
        ),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build native solstone root contract fixture."
    )
    parser.add_argument("--output", type=Path, default=OUTPUT)
    parser.add_argument("--check", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    output = args.output.resolve()
    rendered = json.dumps(build(), indent=2, sort_keys=True) + "\n"
    if args.check:
        if not output.is_file():
            print(f"{output} is missing")
            return 1
        if output.read_text() != rendered:
            print(f"{output} is stale; run make build-native-sol-root-contract")
            return 1
        print(f"{output} is current")
        return 0
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(rendered)
    print(f"wrote {output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
