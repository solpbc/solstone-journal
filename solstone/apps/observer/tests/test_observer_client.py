# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for ObserverClient with mocked HTTP calls."""

from __future__ import annotations

import json
from unittest.mock import MagicMock, patch

import pytest
import requests


@pytest.fixture
def mock_session():
    with patch("solstone.observe.observer_client.requests.Session") as mock:
        session = MagicMock()
        mock.return_value = session
        yield session


@pytest.fixture
def mock_config():
    with (
        patch("solstone.observe.observer_client.get_config") as mock,
        patch("solstone.observe.observer_client.read_service_port") as mock_port,
    ):
        mock.return_value = {}
        mock_port.return_value = 8000
        yield mock


@pytest.fixture
def mock_journal(tmp_path):
    with patch("solstone.observe.observer_client.get_journal") as mock:
        mock.return_value = str(tmp_path)
        yield tmp_path


def test_observer_client_init(mock_session, mock_config):
    from solstone.observe.observer_client import ObserverClient

    client = ObserverClient("main-stream")

    assert client._url == "http://localhost:8000"
    assert client._key is None
    assert client._name == "main-stream"
    assert client._stream == "main-stream"
    assert client._auto_register is True


def test_observer_client_init_no_port(mock_session):
    """When no config URL and no convey.port file, _url is empty."""
    from solstone.observe.observer_client import ObserverClient

    with (
        patch("solstone.observe.observer_client.get_config") as cfg,
        patch("solstone.observe.observer_client.read_service_port") as port,
    ):
        cfg.return_value = {}
        port.return_value = None
        client = ObserverClient("main-stream")

    assert client._url == ""


def test_observer_client_init_with_config(mock_session, mock_config):
    from solstone.observe.observer_client import ObserverClient

    mock_config.return_value = {
        "observe": {
            "observer": {
                "url": "https://example.test/",
                "key": "abc123",
                "name": "named-observer",
                "auto_register": False,
            }
        }
    }

    client = ObserverClient("main-stream")

    assert client._url == "https://example.test"
    assert client._key == "abc123"
    assert client._name == "named-observer"
    assert client._auto_register is False


@pytest.mark.parametrize("pair_mode", ["", "PL", "http"])
def test_observer_client_rejects_invalid_pair_mode(
    mock_session,
    mock_config,
    pair_mode,
):
    from solstone.observe.observer_client import ObserverClient

    mock_config.return_value = {"observe": {"observer": {"pair_mode": pair_mode}}}

    with pytest.raises(ValueError, match="pair_mode"):
        ObserverClient("main-stream")


def test_observer_client_pl_rejects_dual_key_config(mock_session, mock_config):
    from solstone.observe.observer_client import ObserverClient

    mock_config.return_value = {
        "observe": {
            "observer": {
                "pair_mode": "pl",
                "key": "testkey123",
                "spl_label": "laptop",
                "spl_relay_url": "https://relay.test",
            }
        }
    }

    with pytest.raises(ValueError, match="pair_mode=pl.*observe.observer.key"):
        ObserverClient("main-stream")


def test_observer_client_pl_requires_label_and_relay(mock_session, mock_config):
    from solstone.observe.observer_client import ObserverClient

    mock_config.return_value = {
        "observe": {"observer": {"pair_mode": "pl", "spl_relay_url": "https://relay"}}
    }
    with pytest.raises(ValueError, match="spl_label"):
        ObserverClient("main-stream")

    mock_config.return_value = {
        "observe": {"observer": {"pair_mode": "pl", "spl_label": "laptop"}}
    }
    with pytest.raises(ValueError, match="spl_relay_url"):
        ObserverClient("main-stream")


def test_observer_client_pl_requires_bundle(
    mock_session, mock_config, tmp_path, monkeypatch
):
    from solstone.observe.observer_client import ObserverClient

    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
    mock_config.return_value = {
        "observe": {
            "observer": {
                "pair_mode": "pl",
                "spl_label": "missing",
                "spl_relay_url": "https://relay.test",
            }
        }
    }

    with pytest.raises(ValueError, match="bundle not found"):
        ObserverClient("main-stream")


def test_auto_registration(mock_session, mock_config, mock_journal, tmp_path):
    from solstone.observe.observer_client import ObserverClient

    file1 = tmp_path / "audio.flac"
    file1.write_bytes(b"audio")

    create_response = MagicMock()
    create_response.status_code = 200
    create_response.json.return_value = {"key": "registered-key"}

    upload_response = MagicMock()
    upload_response.status_code = 200
    upload_response.json.return_value = {"files": ["audio.flac"], "bytes": 5}

    mock_session.post.side_effect = [create_response, upload_response]

    client = ObserverClient("main-stream")
    result = client.upload_segment("20250103", "120000_300", [file1])

    assert result.success is True
    assert client._key == "registered-key"
    assert mock_session.post.call_args_list[0][0][0].endswith(
        "/app/observer/api/create"
    )
    config = json.loads((mock_journal / "config" / "journal.json").read_text())
    assert config["observe"]["observer"]["key"] == "registered-key"


def test_existing_key_skips_registration(mock_session, mock_config, tmp_path):
    from solstone.observe.observer_client import ObserverClient

    mock_config.return_value = {"observe": {"observer": {"key": "testkey123"}}}

    file1 = tmp_path / "audio.flac"
    file1.write_bytes(b"audio")

    upload_response = MagicMock()
    upload_response.status_code = 200
    upload_response.json.return_value = {"files": ["audio.flac"], "bytes": 5}
    mock_session.post.return_value = upload_response

    client = ObserverClient("main-stream")
    result = client.upload_segment("20250103", "120000_300", [file1])

    assert result.success is True
    assert mock_session.post.call_count == 1
    assert mock_session.post.call_args[0][0].endswith("/app/observer/ingest")
    assert mock_session.post.call_args[1]["headers"] == {
        "Authorization": "Bearer testkey123"
    }


def test_registration_retry(mock_session, mock_config, mock_journal, tmp_path):
    from solstone.observe.observer_client import ObserverClient

    file1 = tmp_path / "audio.flac"
    file1.write_bytes(b"audio")

    create_response = MagicMock()
    create_response.status_code = 200
    create_response.json.return_value = {"key": "registered-key"}

    upload_response = MagicMock()
    upload_response.status_code = 200
    upload_response.json.return_value = {"files": ["audio.flac"], "bytes": 5}

    mock_session.post.side_effect = [
        requests.ConnectionError("no route"),
        create_response,
        upload_response,
    ]

    with patch("solstone.observe.observer_client.time.sleep"):
        client = ObserverClient("main-stream")
        result = client.upload_segment("20250103", "120000_300", [file1])

    assert result.success is True
    assert mock_session.post.call_count == 3


def test_registration_403(mock_session, mock_config):
    from solstone.observe.observer_client import ObserverClient

    response = MagicMock()
    response.status_code = 403
    mock_session.post.return_value = response

    client = ObserverClient("main-stream")
    client._ensure_registered()

    assert client._revoked is True
    assert client._key is None


def test_upload_segment_success(mock_session, mock_config, tmp_path):
    from solstone.observe.observer_client import ObserverClient

    mock_config.return_value = {"observe": {"observer": {"key": "testkey123"}}}

    file1 = tmp_path / "audio.flac"
    file1.write_bytes(b"audio data")

    mock_response = MagicMock()
    mock_response.status_code = 200
    mock_response.json.return_value = {"files": ["audio.flac"], "bytes": 10}
    mock_session.post.return_value = mock_response

    client = ObserverClient("main-stream")
    result = client.upload_segment("20250103", "120000_300", [file1])

    assert result.success is True
    assert result.duplicate is False


def test_upload_segment_retry(mock_session, mock_config, tmp_path):
    from solstone.observe.observer_client import ObserverClient

    mock_config.return_value = {"observe": {"observer": {"key": "testkey123"}}}

    file1 = tmp_path / "audio.flac"
    file1.write_bytes(b"audio data")

    failure = MagicMock()
    failure.status_code = 500
    failure.text = "Server error"

    success = MagicMock()
    success.status_code = 200
    success.json.return_value = {"files": ["audio.flac"], "bytes": 10}

    mock_session.post.side_effect = [failure, success]

    with patch("solstone.observe.observer_client.time.sleep"):
        client = ObserverClient("main-stream")
        result = client.upload_segment("20250103", "120000_300", [file1])

    assert result.success is True
    assert mock_session.post.call_count == 2


def test_upload_segment_403(mock_session, mock_config, tmp_path):
    from solstone.observe.observer_client import ObserverClient

    mock_config.return_value = {"observe": {"observer": {"key": "testkey123"}}}

    file1 = tmp_path / "audio.flac"
    file1.write_bytes(b"audio data")

    response = MagicMock()
    response.status_code = 403
    response.text = "Forbidden"
    mock_session.post.return_value = response

    client = ObserverClient("main-stream")
    result = client.upload_segment("20250103", "120000_300", [file1])

    assert result.success is False
    assert client._revoked is True


def test_upload_segment_all_retries_fail(mock_session, mock_config, tmp_path):
    from solstone.observe.observer_client import ObserverClient

    mock_config.return_value = {"observe": {"observer": {"key": "testkey123"}}}

    file1 = tmp_path / "audio.flac"
    file1.write_bytes(b"audio data")

    failure = MagicMock()
    failure.status_code = 500
    failure.text = "Server error"
    mock_session.post.return_value = failure

    with patch("solstone.observe.observer_client.time.sleep"):
        client = ObserverClient("main-stream")
        result = client.upload_segment("20250103", "120000_300", [file1])

    assert result.success is False
    assert mock_session.post.call_count == 3


def test_relay_event_success(mock_session, mock_config):
    from solstone.observe.observer_client import ObserverClient

    mock_config.return_value = {"observe": {"observer": {"key": "testkey123"}}}

    response = MagicMock()
    response.status_code = 200
    mock_session.post.return_value = response

    client = ObserverClient("main-stream")
    result = client.relay_event("observe", "status", mode="idle")

    assert result is True
    assert mock_session.post.call_args[1]["json"] == {
        "tract": "observe",
        "event": "status",
        "mode": "idle",
    }


def test_relay_event_403(mock_session, mock_config):
    from solstone.observe.observer_client import ObserverClient

    mock_config.return_value = {"observe": {"observer": {"key": "testkey123"}}}

    response = MagicMock()
    response.status_code = 403
    response.text = "Forbidden"
    mock_session.post.return_value = response

    client = ObserverClient("main-stream")
    result = client.relay_event("observe", "status", mode="idle")

    assert result is False
    assert client._revoked is True


def test_key_persistence(mock_session, mock_config, mock_journal):
    from solstone.observe.observer_client import ObserverClient

    client = ObserverClient("main-stream")
    client._persist_key("persisted-key")

    config = json.loads((mock_journal / "config" / "journal.json").read_text())
    assert config == {"observe": {"observer": {"key": "persisted-key"}}}


def test_key_persistence_preserves_existing(mock_session, mock_config, mock_journal):
    from solstone.observe.observer_client import ObserverClient

    config_dir = mock_journal / "config"
    config_dir.mkdir()
    config_path = config_dir / "journal.json"
    config_path.write_text(
        json.dumps(
            {"identity": {"name": "Jer"}, "observe": {"tmux": {"enabled": True}}}
        )
    )

    client = ObserverClient("main-stream")
    client._persist_key("persisted-key")

    config = json.loads(config_path.read_text())
    assert config["identity"]["name"] == "Jer"
    assert config["observe"]["tmux"]["enabled"] is True
    assert config["observe"]["observer"]["key"] == "persisted-key"


def test_cleanup_draft(tmp_path):
    from solstone.observe.observer_client import cleanup_draft

    draft_dir = tmp_path / "draft"
    draft_dir.mkdir()
    (draft_dir / "a.txt").write_text("a")
    (draft_dir / "b.txt").write_text("b")

    cleanup_draft(str(draft_dir))

    assert not draft_dir.exists()


def test_finalize_draft(tmp_path):
    from solstone.observe.observer_client import finalize_draft

    draft_dir = tmp_path / "091551_draft"
    draft_dir.mkdir()
    (draft_dir / "screen.webm").write_text("video")
    (draft_dir / "audio.flac").write_text("audio")

    result = finalize_draft(str(draft_dir), "091551_300")

    assert result == str(tmp_path / "091551_300")
    assert not draft_dir.exists()
    final = tmp_path / "091551_300"
    assert final.exists()
    assert (final / "screen.webm").read_text() == "video"
    assert (final / "audio.flac").read_text() == "audio"


def test_upload_duplicate_response(mock_session, mock_config, tmp_path):
    from solstone.observe.observer_client import ObserverClient

    mock_config.return_value = {"observe": {"observer": {"key": "testkey123"}}}

    file1 = tmp_path / "audio.flac"
    file1.write_bytes(b"audio data")

    response = MagicMock()
    response.status_code = 200
    response.json.return_value = {
        "status": "duplicate",
        "existing_segment": "120000_300",
        "message": "All files already received",
    }
    mock_session.post.return_value = response

    client = ObserverClient("main-stream")
    result = client.upload_segment("20250103", "120000_300", [file1])

    assert result.success is True
    assert result.duplicate is True
