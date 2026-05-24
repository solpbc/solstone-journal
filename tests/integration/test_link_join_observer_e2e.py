# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import argparse
import json
import stat
import urllib.request
from pathlib import Path

import pytest

from solstone.apps.link import call as link_call
from solstone.think.link import join_cli
from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.join_cli import BUNDLE_FILES
from solstone.think.link.paths import authorized_clients_path
from tests.link.live_helpers import running_convey_server

pytestmark = pytest.mark.integration


def test_link_join_observer_e2e(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    tmp_journal = tmp_path / "journal"
    tmp_journal.mkdir()
    config_home = tmp_path / "config"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_journal))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))

    with running_convey_server(tmp_journal) as base_url:
        pair_start = _post_json(
            f"{base_url}/app/link/pair-start",
            {"device_label": "test-laptop", "role": "observer"},
        )

        result = join_cli.main(
            argparse.Namespace(
                home=base_url,
                code=pair_start["manual_code"],
                as_role="observer",
                label="test-laptop",
            )
        )

        assert result == 0
        bundle = config_home / "solstone-observer" / "spl" / "test-laptop"
        assert stat.S_IMODE(bundle.stat().st_mode) == 0o700
        for name in BUNDLE_FILES:
            assert (bundle / name).exists()
            assert stat.S_IMODE((bundle / name).stat().st_mode) == 0o600
        peer = json.loads((bundle / "peer.json").read_text("utf-8"))
        assert peer["role"] == "observer"
        assert peer["label"] == "test-laptop"

        entries = AuthorizedClients(authorized_clients_path()).snapshot()
        assert len(entries) == 1
        assert entries[0].role == "observer"
        assert entries[0].device_label == "test-laptop"

        capsys.readouterr()
        link_call.list_devices()
        out = capsys.readouterr().out
        assert "Observers:" in out
        assert "test-laptop" in out


def _post_json(url: str, payload: dict[str, object]) -> dict[str, object]:
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        assert response.status == 200
        body = json.loads(response.read().decode("utf-8"))
    assert isinstance(body, dict)
    return body
