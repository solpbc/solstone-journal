# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for native speakers-analyze wire constants."""

from __future__ import annotations

import re
from pathlib import Path

import pytest

from solstone.apps.speakers.encoder_config import (
    SPEAKERS_ANALYZE_DTYPE,
    SPEAKERS_ANALYZE_PAYLOAD_FORMAT,
)

REPO_ROOT = Path(__file__).resolve().parents[1]
RUST_SOURCE = REPO_ROOT / "core/crates/solstone-core-speakers-analyze/src/lib.rs"


def _rust_str_const(source_path: Path, source: str, name: str) -> str:
    match = re.search(
        rf"^\s*(?:pub\s+)?const\s+{re.escape(name)}\s*:\s*&str\s*=\s*\"([^\"]+)\"\s*;",
        source,
        re.MULTILINE,
    )
    assert match is not None, f"{source_path} does not declare Rust string const {name}"
    return match.group(1)


def _extract_rust_speakers_analyze_wire_constants(
    source_path: Path,
) -> tuple[str, str]:
    try:
        source = source_path.read_text(encoding="utf-8")
    except OSError as exc:
        raise AssertionError(
            f"unable to read Rust speakers-analyze source: {source_path}"
        ) from exc

    return (
        _rust_str_const(source_path, source, "PAYLOAD_FORMAT"),
        _rust_str_const(source_path, source, "DTYPE_F32LE"),
    )


def _assert_speakers_analyze_wire_constants_match(
    source_path: Path,
) -> tuple[str, str]:
    payload_format, dtype = _extract_rust_speakers_analyze_wire_constants(source_path)

    assert payload_format == SPEAKERS_ANALYZE_PAYLOAD_FORMAT, (
        f"{source_path} PAYLOAD_FORMAT={payload_format!r} does not match "
        "SPEAKERS_ANALYZE_PAYLOAD_FORMAT"
    )
    assert dtype == SPEAKERS_ANALYZE_DTYPE, (
        f"{source_path} DTYPE_F32LE={dtype!r} does not match SPEAKERS_ANALYZE_DTYPE"
    )

    return payload_format, dtype


def test_rust_and_python_speakers_analyze_wire_constants_match() -> None:
    _assert_speakers_analyze_wire_constants_match(RUST_SOURCE)


def test_speakers_analyze_wire_constant_pin_rejects_mismatched_source(
    tmp_path: Path,
) -> None:
    source_path = tmp_path / "lib.rs"
    source_path.write_text(
        "\n".join(
            [
                'const PAYLOAD_FORMAT: &str = "deliberately-wrong-payload-format";',
                'const DTYPE_F32LE: &str = "deliberately-wrong-dtype";',
                "",
            ]
        ),
        encoding="utf-8",
    )

    with pytest.raises(AssertionError, match=re.escape(str(source_path))):
        _assert_speakers_analyze_wire_constants_match(source_path)
