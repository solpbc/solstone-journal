#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
from __future__ import annotations

import hashlib
import json
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
ORACLE = REPO_ROOT / "core/fixtures/native-sol/sol-call-grammar-v1.json"
EXPECTED_SCHEMA = "sol-call-grammar-v1"
EXPECTED_SOURCE = "ce65d06ba67ca4fad85ba3b3f71a1eec359bc6e5"
EXPECTED_ENTRIES = 173
EXPECTED_BYTES = 119138
EXPECTED_SHA256 = "3a3f4ce14788a9902e2610f9216f8e97c348fb12b1fe81771569758f8dc96932"


def main() -> int:
    data = ORACLE.read_bytes()
    payload = json.loads(data)
    errors: list[str] = []

    if payload.get("schema") != EXPECTED_SCHEMA:
        errors.append(f"schema {payload.get('schema')!r} != {EXPECTED_SCHEMA!r}")
    if payload.get("source") != EXPECTED_SOURCE:
        errors.append(f"source {payload.get('source')!r} != {EXPECTED_SOURCE!r}")

    entries = payload.get("entries")
    if not isinstance(entries, list):
        errors.append("entries is not a list")
    elif len(entries) != EXPECTED_ENTRIES:
        errors.append(f"entries {len(entries)} != {EXPECTED_ENTRIES}")

    if len(data) != EXPECTED_BYTES:
        errors.append(f"bytes {len(data)} != {EXPECTED_BYTES}")

    digest = hashlib.sha256(data).hexdigest()
    if digest != EXPECTED_SHA256:
        errors.append(f"sha256 {digest} != {EXPECTED_SHA256}")

    if errors:
        print(f"{ORACLE} failed native sol grammar oracle check:")
        for error in errors:
            print(f"- {error}")
        return 1

    print(
        f"{ORACLE} ok: entries={EXPECTED_ENTRIES} "
        f"bytes={EXPECTED_BYTES} sha256={EXPECTED_SHA256}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
