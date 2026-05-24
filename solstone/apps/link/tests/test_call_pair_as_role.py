# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import pytest
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


def _prepare_pair_env(tmp_path, monkeypatch) -> None:
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr(call, "require_solstone", lambda: None)
    monkeypatch.setattr(call, "_detect_lan_ip", lambda: None)
    monkeypatch.setattr(call, "time", _FakeTime())


@pytest.mark.parametrize("role", ["observer", "peer"])
def test_pair_invokes_with_role(tmp_path, monkeypatch, role: str) -> None:
    _prepare_pair_env(tmp_path, monkeypatch)

    result = CliRunner().invoke(
        call.app,
        ["pair", "--device-label", "test-laptop", "--as", role, "--timeout", "1"],
    )

    assert result.exit_code == 2
    nonces = call._nonces().snapshot()
    assert len(nonces) == 1
    assert nonces[0].role == role


@pytest.mark.parametrize("role", ["bogus", ""])
def test_pair_rejects_invalid_role(tmp_path, monkeypatch, role: str) -> None:
    _prepare_pair_env(tmp_path, monkeypatch)

    result = CliRunner().invoke(
        call.app,
        ["pair", "--device-label", "test-laptop", "--as", role, "--timeout", "1"],
    )

    assert result.exit_code == 2
    assert "invalid role" in result.stderr
    assert call._nonces().snapshot() == []


def test_pair_default_role_phone(tmp_path, monkeypatch) -> None:
    _prepare_pair_env(tmp_path, monkeypatch)

    result = CliRunner().invoke(
        call.app,
        ["pair", "--device-label", "test-laptop", "--timeout", "1"],
    )

    assert result.exit_code == 2
    nonces = call._nonces().snapshot()
    assert len(nonces) == 1
    assert nonces[0].role == "phone"


def test_pair_prints_role_in_device_line(tmp_path, monkeypatch) -> None:
    _prepare_pair_env(tmp_path, monkeypatch)

    result = CliRunner().invoke(
        call.app,
        ["pair", "--device-label", "test-laptop", "--as", "observer", "--timeout", "1"],
    )

    assert result.exit_code == 2
    assert "Device: test-laptop (role: observer)" in result.stdout
