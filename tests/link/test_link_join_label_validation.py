# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import argparse

import pytest

from solstone.think.link import join_cli


def _args(label: str) -> argparse.Namespace:
    return argparse.Namespace(
        home="http://receiver",
        code="ABCD-EFGH",
        as_role="observer",
        label=label,
    )


@pytest.mark.parametrize(
    "label",
    ["", "a" * 81, "a/b", "a\\b", "a..b", ".hidden", "foo bar", "foo*", "foo!"],
)
def test_invalid_labels_exit_2_without_writing(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
    label: str,
) -> None:
    config_home = tmp_path / "config"
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    calls = []
    monkeypatch.setattr(
        join_cli.urllib.request, "urlopen", lambda *a, **k: calls.append(a)
    )

    result = join_cli.main(_args(label))

    assert result == 2
    assert calls == []
    assert not (config_home / "solstone-observer" / "spl").exists()


@pytest.mark.parametrize(
    "label",
    ["laptop", "my-laptop", "my_laptop", "laptop.v2", "a", "a" * 80],
)
def test_valid_labels_reach_http_stage(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
    label: str,
) -> None:
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
    calls = []

    def fake_urlopen(*args, **_kwargs):
        calls.append(args)
        raise join_cli.urllib.error.URLError("stop")

    monkeypatch.setattr(join_cli.urllib.request, "urlopen", fake_urlopen)

    result = join_cli.main(_args(label))

    assert result == 1
    assert len(calls) == 1
