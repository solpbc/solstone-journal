#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Verify the immutable Lane AF retained text reference fixture."""

from __future__ import annotations

from capture_health_text_reference import DEFAULT_OUTPUT, self_test

EXPECTED_SHA256 = "b0c3ac7312aea7e017c5807c2f531b7463b8a416f78ca3a1d7c63cd6536f664d"


def main() -> int:
    self_test(DEFAULT_OUTPUT.read_bytes(), EXPECTED_SHA256)
    print("health text reference verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
