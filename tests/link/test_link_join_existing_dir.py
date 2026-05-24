# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import argparse

import pytest

from solstone.think.link import join_cli


def _args() -> argparse.Namespace:
    return argparse.Namespace(
        home="http://receiver",
        code="ABCD-EFGH",
        as_role="observer",
        label="laptop",
    )


def _bundle_dir(tmp_path, monkeypatch: pytest.MonkeyPatch):
    config_home = tmp_path / "config"
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    bundle = config_home / "solstone-observer" / "spl" / "laptop"
    bundle.mkdir(parents=True)
    return bundle


def test_existing_bundle_file_refuses_overwrite(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    bundle = _bundle_dir(tmp_path, monkeypatch)
    existing = bundle / "peer.json"
    existing.write_text("existing", encoding="utf-8")

    result = join_cli.main(_args())

    assert result == 1
    assert existing.read_text("utf-8") == "existing"


def test_existing_ds_store_only_proceeds_to_next_stage(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    bundle = _bundle_dir(tmp_path, monkeypatch)
    (bundle / ".DS_Store").write_text("", encoding="utf-8")
    calls = []

    def fake_urlopen(*args, **_kwargs):
        calls.append(args)
        raise join_cli.urllib.error.URLError("stop")

    monkeypatch.setattr(join_cli.urllib.request, "urlopen", fake_urlopen)

    result = join_cli.main(_args())

    assert result == 1
    assert len(calls) == 1


def test_existing_non_bundle_file_refuses_overwrite(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    bundle = _bundle_dir(tmp_path, monkeypatch)
    (bundle / "notes.txt").write_text("", encoding="utf-8")

    result = join_cli.main(_args())

    assert result == 1


def test_existing_hidden_bundle_file_refuses_overwrite(
    tmp_path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    bundle = _bundle_dir(tmp_path, monkeypatch)
    (bundle / ".private.pem").write_text("", encoding="utf-8")

    result = join_cli.main(_args())

    assert result == 1
