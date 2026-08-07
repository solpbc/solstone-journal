# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from pathlib import Path

import scripts.check_local_generate_cutover as checker

ROOT = Path(__file__).resolve().parent.parent


def test_rejects_planted_fixture():
    assert checker.scan_directory(ROOT / "scripts/fixtures") == [
        (
            "retired_local_generate_cutover.py",
            7,
            "bundled branch owns local transport/admission",
        )
    ]


def test_accepts_production():
    assert checker.scan_production(ROOT) == []


def test_comment_is_not_a_violation():
    assert (
        checker.scan_source(
            "# resolve_context_window\ndef okay(): pass\n", "x.py", "x.py"
        )
        == []
    )
