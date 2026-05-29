#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Entry shim for `sol doctor`.

Delegates to `solstone.think.doctor.main`, the canonical diagnostic. This
requires the installed package; for the stdlib-only pre-`.venv` readiness
battery use `scripts/preflight.py` or `make preflight`.
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from solstone.think.doctor import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
