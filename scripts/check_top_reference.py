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
FIXTURE_SHA256 = "10a424f5de70b4a009f0b13f07d0e4e8c093b92471453d594d9efdaab69ac9b9"
CAPTURE_BLOB = "27410efa7983f5485a7eb7954dce8525472c5ac9"
CAPTURE_SHA256 = "3c5b7bab2db495c449043c8fc8b433dc98c9725e36df31e47a4d0f6fe1d4faec"


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
