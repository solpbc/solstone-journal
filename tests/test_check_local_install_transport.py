# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from pathlib import Path

import scripts.check_local_install_transport as checker


ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "scripts" / "fixtures"


def test_check_local_install_transport_rejects_retired_fixture():
    findings = checker.scan_directory(FIXTURES)
    assert any(path == "retired_local_install_transport.py" for path, _line, _function in findings)


def test_check_local_install_transport_accepts_production_tree():
    assert checker.scan_production(ROOT) == []
