#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Stdlib-only entry point for `make preflight`.

Runs before `.venv` exists, even when `uv` is absent. Delegates to
`solstone.think.preflight.main`; unlike `scripts/doctor.py`, this path imports
only the Python standard library.
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from solstone.think.preflight import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
