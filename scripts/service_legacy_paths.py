#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared, explicit roots for the service-evidence capture transaction."""

from __future__ import annotations

import os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def evidence_root() -> Path:
    return Path(
        os.environ.get(
            "SERVICE_LEGACY_EVIDENCE_ROOT",
            ROOT / "core/fixtures/service_legacy_evidence",
        )
    ).resolve()


def python_cache_root() -> Path:
    return Path(
        os.environ.get(
            "SERVICE_LEGACY_PYTHON_CACHE_ROOT",
            ROOT / ".cache/service-legacy-evidence/python",
        )
    ).resolve()


def capture_input() -> str:
    value = os.environ.get("SERVICE_LEGACY_CAPTURE_INPUT", "")
    if len(value) != 40 or any(
        character not in "0123456789abcdef" for character in value
    ):
        raise RuntimeError(
            "SERVICE_LEGACY_CAPTURE_INPUT must be one exact lowercase commit id"
        )
    return value
