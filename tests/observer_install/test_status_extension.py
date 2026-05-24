# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import subprocess

from solstone.apps.observer.utils import save_observer
from solstone.observe import observer_cli
from solstone.observe.observer_install import common, linux


def _save_status_observer() -> None:
    save_observer(
        {
            "key": "abcdefgh",
            "name": "archon",
            "created_at": None,
            "last_seen": None,
            "last_segment": None,
            "enabled": True,
            "stats": {"segments_received": 0, "bytes_received": 0},
        }
    )


def test_status_includes_install_marker(monkeypatch, capsys):
    _save_status_observer()
    common.write_marker(
        linux.INSTALL_NAME,
        {
            "name": "archon",
            "platform": "linux",
            "source": f"pypi:{linux.PACKAGE_NAME}",
            "installed_at": "2026-05-02T00:00:00Z",
            "last_run": "2026-05-02T00:00:00Z",
            "version": "0.1.0",
        },
    )
    monkeypatch.setattr(
        common,
        "run_probe",
        lambda cmd: subprocess.CompletedProcess(cmd, 0, "active\n", ""),
    )

    assert observer_cli._status_single("archon") == 0

    output = capsys.readouterr().out
    assert "  Installed: 2026-05-02T00:00:00Z (linux, version 0.1.0)" in output
    assert f"  Service:   {linux.UNIT_NAME} — active" in output


def test_status_without_marker_matches_baseline(capsys):
    _save_status_observer()

    assert observer_cli._status_single("archon") == 0

    assert capsys.readouterr().out == (
        "Observer: archon\n"
        "  Mode:       dl\n"
        "  Prefix:     abcdefgh\n"
        "  Status:     disconnected\n"
        "  Created:    never\n"
        "  Last seen:  never\n"
        "  Segments:   0\n"
        "  Bytes:      0 B\n"
    )
