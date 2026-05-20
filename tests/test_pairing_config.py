# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from pathlib import Path
from unittest.mock import Mock, patch

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
        "host_url": " https://example.test/base ",
    }
    _write_config(journal_copy, payload)

    assert config.get_host_url() == "https://example.test/base"


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
