# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Regression tests for `sol call link pair` manual-code output."""

from __future__ import annotations

import re

from typer.testing import CliRunner

from solstone.apps.link import call


class _FakeTime:
    def __init__(self) -> None:
        self._calls = 0

    def time(self) -> float:
        self._calls += 1
        return 0.0 if self._calls == 1 else 2.0

    def sleep(self, _seconds: float) -> None:
        return None


def test_call_pair_prints_manual_code(tmp_path, monkeypatch) -> None:
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr(call, "require_solstone", lambda: None)
    monkeypatch.setattr(call, "_detect_lan_ip", lambda: None)
    monkeypatch.setattr(call, "time", _FakeTime())

    result = CliRunner().invoke(
        call.app,
        ["pair", "--device-label", "Test Phone", "--timeout", "1"],
    )

    assert result.exit_code == 2
    assert "manual code:" in result.stdout
    assert re.search(
        r"manual code: [0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}",
        result.stdout,
    )
