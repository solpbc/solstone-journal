#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Verify the immutable retained ``journal top`` reference fixture."""

from __future__ import annotations

import hashlib
import importlib.util
from pathlib import Path
from types import ModuleType

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "core/fixtures/top_reference.json"
CAPTURE = ROOT / "scripts/capture_top_reference.py"
FIXTURE_SHA256 = "c69e605a8f6e8bfb9e8768647568527a6a79621ba292431595a49a570c9bf40a"
CAPTURE_BLOB = "247c63bf13923e7dab20838d7025a889b16f39a6"
CAPTURE_SHA256 = "e73b7cee9c4f4497c76e0b67a7de968ed413636e150d3566247b5e8657f56896"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_capture() -> ModuleType:
    spec = importlib.util.spec_from_file_location("_top_reference_capture", CAPTURE)
    if spec is None or spec.loader is None:
        raise RuntimeError("top-reference-tool: cannot load capture tool")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main() -> None:
    fixture_sha = sha256(FIXTURE)
    if fixture_sha != FIXTURE_SHA256:
        raise RuntimeError(
            f"top-reference-digest: expected {FIXTURE_SHA256}, got {fixture_sha}"
        )
    capture_sha = sha256(CAPTURE)
    if capture_sha != CAPTURE_SHA256:
        raise RuntimeError(
            f"top-reference-tool: expected {CAPTURE_SHA256}, got {capture_sha}"
        )

    capture = load_capture()
    blob, committed_sha = capture.committed_blob(capture.TOOL_PATH)
    if blob != CAPTURE_BLOB or committed_sha != CAPTURE_SHA256:
        raise RuntimeError(
            "top-reference-tool: committed capture identity differs from checker"
        )
    capture.self_test()
    value = capture.load_json(FIXTURE.read_bytes())
    capture.semantic_verify(value)
    rebuilt = capture.canonical_bytes(capture.build_reference())
    if rebuilt != FIXTURE.read_bytes():
        raise RuntimeError("top-reference-canonical: fresh capture differs")


if __name__ == "__main__":
    main()
