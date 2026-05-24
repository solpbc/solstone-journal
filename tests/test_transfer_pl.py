# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest
import requests
from cryptography.hazmat.primitives import serialization

import solstone.think.utils as think_utils
from solstone.observe.peer_lookup import PeerInfo
from solstone.observe.transfer import send_segments_pl
from solstone.think.link.ca import cert_fingerprint, generate_ca


def _set_journal(monkeypatch: pytest.MonkeyPatch, journal: Path) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
    think_utils._journal_path_cache = None


def _write_peer(journal: Path) -> PeerInfo:
    peer_dir = journal / "peers" / "12345678-1234-1234-1234-123456789abc"
    peer_dir.mkdir(parents=True)
    ca = generate_ca(peer_dir / "ca")
    cert_pem = ca.cert.public_bytes(serialization.Encoding.PEM).decode("ascii")
    peer = {
        "label": "host-a",
        "instance_id": "12345678-1234-1234-1234-123456789abc",
        "home_label": "solstone",
        "local_endpoints": [],
    }
    (peer_dir / "private.pem").write_text("private", encoding="utf-8")
    (peer_dir / "cert.pem").write_text(cert_pem, encoding="utf-8")
    (peer_dir / "chain.pem").write_text(cert_pem, encoding="utf-8")
    (peer_dir / "home_attestation.jwt").write_text("jwt", encoding="utf-8")
    (peer_dir / "peer.json").write_text(json.dumps(peer), encoding="utf-8")
    return PeerInfo(
        dir=peer_dir,
        instance_id=peer["instance_id"],
        label=peer["label"],
        local_endpoints=[],
        cert_fingerprint=cert_fingerprint(cert_pem),
    )


def _write_segment(journal: Path) -> None:
    segment = journal / "chronicle" / "20260520" / "laptop" / "143022_300"
    segment.mkdir(parents=True)
    (segment / "audio.flac").write_bytes(b"audio")
    (segment / "transcript.jsonl").write_bytes(b"transcript")
    (segment / "stream.json").write_bytes(b'{"name": "laptop"}')


def test_transfer_send_pl_posts_journal_segment_day_path(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    journal = tmp_path / "journal"
    _set_journal(monkeypatch, journal)
    peer = _write_peer(journal)
    _write_segment(journal)
    calls: list[tuple[str, str, dict[str, str], bytes]] = []

    class FakeTunnelSession:
        async def request(self, method, path, *, headers=None, body=b""):
            calls.append((method, path, headers or {}, body))
            if method == "GET":
                return (200, {}, b"{}")
            return (200, {}, b'{"ok": true}')

        async def close(self) -> None:
            return None

    async def fake_open_tunnel(_identity, _relay_url):
        return FakeTunnelSession()

    monkeypatch.setattr("solstone.think.link.dialer.open_tunnel", fake_open_tunnel)
    monkeypatch.setenv("SOL_LINK_RELAY_URL", "https://relay.test")

    send_segments_pl(peer, ["20260520"], dry_run=False)

    post = next(call for call in calls if call[0] == "POST")
    assert post[1] == "/app/import/journal/12345678/ingest/segments/20260520"
    assert "Authorization" not in post[2]
    assert b'"day": "20260520"' in post[3]
    assert b'"stream": "laptop"' in post[3]
    assert b"audio" in post[3]


def test_transfer_send_dl_regression_url_headers_and_body(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.observe.transfer import send_segments

    journal = tmp_path / "journal"
    _set_journal(monkeypatch, journal)
    _write_segment(journal)

    mock_session = MagicMock(spec=requests.Session)
    mock_session.headers = {}
    get_response = MagicMock()
    get_response.status_code = 200
    get_response.json.return_value = {}
    mock_session.get.return_value = get_response
    post_response = MagicMock()
    post_response.status_code = 200
    post_response.json.return_value = {"bytes": 15}
    mock_session.post.return_value = post_response

    with patch("solstone.observe.transfer.requests.Session", return_value=mock_session):
        send_segments("https://receiver.test", "test-key", ["20260520"], False)

    assert mock_session.headers["Authorization"] == "Bearer test-key"
    assert mock_session.get.call_args.args[0] == (
        "https://receiver.test/app/observer/ingest/segments/20260520"
    )
    assert mock_session.post.call_args.args[0] == (
        "https://receiver.test/app/observer/ingest"
    )
    post_kwargs = mock_session.post.call_args.kwargs
    assert post_kwargs["data"] == {
        "day": "20260520",
        "segment": "143022_300",
        "meta": json.dumps({"stream": "laptop"}),
    }
    assert [entry[1][0] for entry in post_kwargs["files"]] == [
        "audio.flac",
        "transcript.jsonl",
    ]


@pytest.mark.parametrize(
    ("argv", "message"),
    [
        (
            ["sol", "send", "--to", "host-a", "--key", "x"],
            "'--key' is only valid with '--to <URL>'",
        ),
        (
            ["sol", "send", "--to", "https://receiver.test"],
            "'--to <URL>' requires '--key <KEY>'",
        ),
    ],
)
def test_transfer_send_destination_validation(
    argv: list[str],
    message: str,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    from solstone.observe import transfer

    _set_journal(monkeypatch, tmp_path / "journal")
    monkeypatch.setattr(sys, "argv", argv)

    with pytest.raises(SystemExit):
        transfer.main()

    assert message in capsys.readouterr().err
