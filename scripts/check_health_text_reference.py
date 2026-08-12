#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Verify the immutable Lane AF retained text reference fixture."""

from __future__ import annotations

from capture_health_text_reference import DEFAULT_OUTPUT, self_test

EXPECTED_SHA256 = "fb9a2675cfa925fe1d49bfb306e83b10f1209d48912ae302c33aa8d61f0022d2"


def main() -> int:
    self_test(DEFAULT_OUTPUT.read_bytes(), EXPECTED_SHA256)
    print("health text reference verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
