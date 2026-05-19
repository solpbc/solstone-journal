# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import pytest

from solstone.observe.observer_install import common


def test_detect_platform_override():
    assert common.detect_platform("linux") == "linux"


@pytest.mark.parametrize(
    ("sys_platform", "expected"),
    [
        ("linux", "linux"),
        ("linux2", "linux"),
        ("darwin", "macos"),
        ("win32", "unsupported"),
    ],
)
def test_detect_platform_honors_sys_platform(
    monkeypatch: pytest.MonkeyPatch, sys_platform: str, expected: str
):
    monkeypatch.setattr(common.sys, "platform", sys_platform)
    assert common.detect_platform() == expected


def test_default_server_url_explicit():
    assert common.default_server_url("http://example.com") == "http://example.com"


def test_default_server_url_reads_port_file(observer_install_env):
    health = observer_install_env.journal / "health"
    health.mkdir()
    (health / "convey.port").write_text("5015\n", encoding="utf-8")

    assert common.default_server_url(None) == "http://127.0.0.1:5015"


def test_default_server_url_missing_port_raises():
    with pytest.raises(common.InstallError) as exc_info:
        common.default_server_url(None)

    assert "could not determine solstone server URL" in str(exc_info.value)
    assert "pass --server-url" in exc_info.value.hint


def test_normalize_stream_name():
    assert common.normalize_stream_name("My-Host.local") == "my-host.local"


def test_marker_path(observer_install_env):
    assert common.marker_path("solstone-linux") == (
        observer_install_env.home
        / ".local"
        / "share"
        / "solstone"
        / "observers"
        / "solstone-linux"
        / ".installed.json"
    )


def test_marker_round_trip():
    data = {
        "name": "archon",
        "platform": "linux",
        "source": "pypi:solstone-linux",
        "installed_at": "2026-05-02T00:00:00Z",
        "last_run": "2026-05-02T00:00:00Z",
        "version": "0.1.0",
    }

    common.write_marker("solstone-linux", data)

    assert common.read_marker("solstone-linux") == data


def test_find_marker_for_observer_matches_name():
    common.write_marker(
        "solstone-linux",
        {
            "name": "archon",
            "platform": "linux",
            "source": "pypi:solstone-linux",
            "installed_at": "2026-05-02T00:00:00Z",
            "last_run": "2026-05-02T00:00:00Z",
            "version": "0.1.0",
        },
    )

    result = common.find_marker_for_observer("archon")

    assert result is not None
    path, data = result
    assert path == common.marker_path("solstone-linux")
    assert data["name"] == "archon"
