# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from pathlib import Path

import scripts.check_local_server_argv_owner as checker


ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "scripts" / "fixtures"


def test_check_local_server_argv_owner_rejects_retired_builder_fixture():
    assert checker.scan_directory(FIXTURES) == [
        ("retired_local_argv_owner.py", 4, "_build_local_llama_cmd")
    ]


def test_check_local_server_argv_owner_accepts_production_tree():
    violations = checker.scan_production(ROOT)

    assert violations == []
