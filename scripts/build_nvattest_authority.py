#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Build the committed nvattest authority v1 JSON mirror."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from solstone.think.providers import nvattest_authority

ROOT = Path(__file__).resolve().parent.parent
ARTIFACT_PATH = ROOT / "core" / "fixtures" / "nvattest_authority_v1.json"


def render_nvattest_authority_json() -> str:
    return (
        json.dumps(nvattest_authority.authority_payload(), indent=2, sort_keys=True)
        + "\n"
    )


def write_outputs() -> None:
    ARTIFACT_PATH.parent.mkdir(parents=True, exist_ok=True)
    ARTIFACT_PATH.write_text(render_nvattest_authority_json(), encoding="utf-8")
    print(f"wrote {ARTIFACT_PATH.relative_to(ROOT)}")


def check_outputs() -> int:
    expected = render_nvattest_authority_json()
    try:
        actual = ARTIFACT_PATH.read_text(encoding="utf-8")
    except FileNotFoundError:
        actual = None
    if actual != expected:
        print(
            "nvattest authority is stale: "
            f"{ARTIFACT_PATH.relative_to(ROOT)}. Run: make nvattest-authority",
            file=sys.stderr,
        )
        return 1
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        return check_outputs()
    write_outputs()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
