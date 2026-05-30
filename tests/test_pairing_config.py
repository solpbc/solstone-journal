# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import Mock, patch

import pytest

from solstone.think.pairing import config


def _write_config(journal: Path, payload: dict) -> None:
    config_path = journal / "config" / "journal.json"
    config_path.write_text(json.dumps(payload), encoding="utf-8")


def _read_config(journal: Path) -> dict:
    return json.loads((journal / "config" / "journal.json").read_text(encoding="utf-8"))


def test_pairing_config_defaults(journal_copy):
    payload = _read_config(journal_copy)
    payload.pop("pairing", None)
    payload["identity"] = {"name": "", "preferred": ""}
    _write_config(journal_copy, payload)

    assert config.get_host_url() == "http://localhost:5015"


def test_pairing_host_url_reads_trimmed_value(journal_copy):
    payload = _read_config(journal_copy)
    payload["pairing"] = {
        "host_url": " http://192.168.1.44:6123 ",
    }
    _write_config(journal_copy, payload)

    assert config.get_host_url() == "http://192.168.1.44:6123"


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("192.168.1.44:5015", "http://192.168.1.44:5015"),
        (" http://192.168.1.44:5015 ", "http://192.168.1.44:5015"),
        ("http://192.168.1.44:5015/", "http://192.168.1.44:5015"),
    ],
)
def test_validate_host_url_accepts_ipv4_port(raw: str, expected: str) -> None:
    assert config.validate_host_url(raw) == expected


@pytest.mark.parametrize(
    "raw",
    [
        "",
        "   ",
        "http://",
        "192.168.1.44",
        "http://192.168.1.44",
        "192.168.1.44:0",
        "192.168.1.44:65536",
        "192.168.1.44:notaport",
        "https://192.168.1.44:5015",
        "http://user@192.168.1.44:5015",
        "http://192.168.1.44:5015/path",
        "http://192.168.1.44:5015?x=1",
        "http://192.168.1.44:5015#frag",
        "http://[::1]:5015",
        "http://[fe80::1]:5015",
    ],
)
def test_validate_host_url_rejects_invalid_values(raw: str) -> None:
    with pytest.raises(config.InvalidHostUrl) as excinfo:
        config.validate_host_url(raw)

    assert str(excinfo.value) == config.HOST_URL_INVALID


@pytest.mark.parametrize("raw", ["mylab.local:5015", "http://home.local:5015"])
def test_validate_host_url_rejects_hostname_with_sol_private_link_message(
    raw: str,
) -> None:
    with pytest.raises(config.InvalidHostUrl) as excinfo:
        config.validate_host_url(raw)

    assert str(excinfo.value) == config.HOST_URL_HOSTNAME_UNSUPPORTED


def test_host_url_override_round_trip(journal_copy) -> None:
    canonical = config.validate_host_url("192.168.1.44:5015")

    config.set_host_url(canonical)

    assert _read_config(journal_copy)["pairing"]["host_url"] == canonical
    assert config.get_host_url_override() == canonical
    assert config.get_host_url() == canonical
    assert config.override_host_port() == "192.168.1.44:5015"

    config.clear_host_url()

    assert _read_config(journal_copy)["pairing"]["host_url"] is None
    assert config.get_host_url_override() is None
    assert config.override_host_port() is None


def test_pairing_host_url_uses_detected_lan_ip_when_network_access_enabled(
    journal_copy,
):
    payload = _read_config(journal_copy)
    payload["pairing"] = {"host_url": None}
    payload["convey"]["allow_network_access"] = True
    _write_config(journal_copy, payload)
    health_dir = journal_copy / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    (health_dir / "convey.port").write_text("6123", encoding="utf-8")

    with patch(
        "solstone.think.pairing.config._detect_lan_ipv4", return_value="192.168.1.44"
    ):
        assert config.get_host_url() == "http://192.168.1.44:6123"


def test_pairing_host_url_uses_localhost_when_network_access_disabled(journal_copy):
    payload = _read_config(journal_copy)
    payload["pairing"] = {"host_url": None}
    payload["convey"]["allow_network_access"] = False
    _write_config(journal_copy, payload)
    health_dir = journal_copy / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    (health_dir / "convey.port").write_text("6123", encoding="utf-8")

    assert config.get_host_url() == "http://localhost:6123"


def test_pairing_host_url_falls_back_to_localhost_when_lan_detect_fails(journal_copy):
    payload = _read_config(journal_copy)
    payload["pairing"] = {"host_url": None}
    payload["convey"]["allow_network_access"] = True
    _write_config(journal_copy, payload)
    health_dir = journal_copy / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    (health_dir / "convey.port").write_text("6123", encoding="utf-8")

    mock_socket = Mock()
    mock_socket.__enter__ = Mock(return_value=mock_socket)
    mock_socket.__exit__ = Mock(return_value=None)
    mock_socket.connect.side_effect = OSError("boom")
    with patch("solstone.think.pairing.config.socket.socket", return_value=mock_socket):
        assert config.get_host_url() == "http://localhost:6123"
