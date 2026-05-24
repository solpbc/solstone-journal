# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import argparse

import pytest

from solstone.think.link import join_cli


def test_peer_role_is_rejected_without_writing(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    config_home = tmp_path / "config"
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    args = argparse.Namespace(
        home="http://receiver",
        code="ABCD-EFGH",
        as_role="peer",
        label="laptop",
    )

    result = join_cli.main(args)

    assert result == 2
    assert join_cli.PEER_UNSUPPORTED in capsys.readouterr().err
    assert not (config_home / "solstone-observer" / "spl" / "laptop").exists()
