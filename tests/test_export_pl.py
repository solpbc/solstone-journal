# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import io
import json
import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest
import requests
from cryptography.hazmat.primitives import serialization

import solstone.think.utils as think_utils
from solstone.observe.peer_lookup import PeerInfo
from solstone.observe.peer_unpair import maybe_prompt_unpair
from solstone.observe.pl_http import PlHttpResponse
from solstone.think.link.ca import cert_fingerprint, generate_ca


def _set_journal(monkeypatch: pytest.MonkeyPatch, journal: Path) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
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


def _patch_exporters(monkeypatch: pytest.MonkeyPatch, calls: list[tuple]) -> None:
    from solstone.observe import export

    def make_exporter(area: str):
        def fake_export(base_url, key, *args, session=None):
            session.post(f"{base_url}/app/import/journal/{key[:8]}/ingest/{area}")
            calls.append(("area", area, session))
            return export.ExportResult(area=area, sent=1)

        return fake_export

    monkeypatch.setattr(export, "export_segments", make_exporter("segments"))
    monkeypatch.setattr(export, "export_imports", make_exporter("imports"))
    monkeypatch.setattr(export, "export_entities", make_exporter("entities"))
    monkeypatch.setattr(export, "export_facets", make_exporter("facets"))
    monkeypatch.setattr(export, "export_config", make_exporter("config"))


def _patch_tunnel(
    monkeypatch: pytest.MonkeyPatch,
    requests_seen: list[tuple[str, str, dict[str, str], bytes]],
) -> None:
    class FakeTunnelSession:
        async def request(self, method, path, *, headers=None, body=b""):
            requests_seen.append((method, path, headers or {}, body))
            return (200, {}, b'{"ok": true}')

        async def close(self) -> None:
            return None

    async def fake_open_tunnel(_identity, _relay_url):
        return FakeTunnelSession()

    monkeypatch.setattr("solstone.think.link.dialer.open_tunnel", fake_open_tunnel)
    monkeypatch.setenv("SOL_LINK_RELAY_URL", "https://relay.test")


def test_export_pl_single_area_uses_pl_url_and_no_authorization(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.observe import export

    journal = tmp_path / "journal"
    journal.mkdir()
    _set_journal(monkeypatch, journal)
    _write_peer(journal)
    area_calls: list[tuple] = []
    requests_seen: list[tuple[str, str, dict[str, str], bytes]] = []
    _patch_exporters(monkeypatch, area_calls)
    _patch_tunnel(monkeypatch, requests_seen)
    prompt_called = False

    def prompt(*_args, **_kwargs):
        nonlocal prompt_called
        prompt_called = True

    monkeypatch.setattr(export, "maybe_prompt_unpair", prompt)
    monkeypatch.setattr(
        sys,
        "argv",
        ["sol", "--to", "host-a", "--only", "entities", "--day", "20260520"],
    )

    export.main()

    assert [call[1] for call in area_calls] == ["entities"]
    assert requests_seen[0][1] == "/app/import/journal/12345678/ingest/entities"
    assert "Authorization" not in requests_seen[0][2]
    assert prompt_called is False


@pytest.mark.parametrize(
    "only_arg",
    [
        None,
        "segments,imports,entities,facets,config",
        "entities,facets,segments,imports,config",
    ],
)
def test_export_pl_full_pipeline_reuses_single_tunnel_and_prompts(
    only_arg: str | None,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.observe import export

    journal = tmp_path / "journal"
    journal.mkdir()
    _set_journal(monkeypatch, journal)
    _write_peer(journal)
    area_calls: list[tuple] = []
    requests_seen: list[tuple[str, str, dict[str, str], bytes]] = []
    _patch_exporters(monkeypatch, area_calls)
    _patch_tunnel(monkeypatch, requests_seen)
    prompt_calls = 0

    def prompt(*_args, **_kwargs):
        nonlocal prompt_calls
        prompt_calls += 1

    monkeypatch.setattr(export, "maybe_prompt_unpair", prompt)
    argv = ["sol", "--to", "host-a", "--day", "20260520"]
    if only_arg is not None:
        argv.extend(["--only", only_arg])
    monkeypatch.setattr(sys, "argv", argv)

    export.main()

    assert [call[1] for call in area_calls] == [
        "segments",
        "imports",
        "entities",
        "facets",
        "config",
    ]
    assert [call[1] for call in requests_seen] == [
        "/app/import/journal/12345678/ingest/segments",
        "/app/import/journal/12345678/ingest/imports",
        "/app/import/journal/12345678/ingest/entities",
        "/app/import/journal/12345678/ingest/facets",
        "/app/import/journal/12345678/ingest/config",
    ]
    assert prompt_calls == 1


def test_export_pl_partial_and_dry_run_do_not_prompt(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.observe import export

    journal = tmp_path / "journal"
    journal.mkdir()
    _set_journal(monkeypatch, journal)
    _write_peer(journal)
    area_calls: list[tuple] = []
    requests_seen: list[tuple[str, str, dict[str, str], bytes]] = []
    _patch_exporters(monkeypatch, area_calls)
    _patch_tunnel(monkeypatch, requests_seen)
    prompt_calls = 0

    def prompt(*_args, **_kwargs):
        nonlocal prompt_calls
        prompt_calls += 1

    monkeypatch.setattr(export, "maybe_prompt_unpair", prompt)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "sol",
            "--to",
            "host-a",
            "--only",
            "segments,entities",
            "--day",
            "20260520",
        ],
    )

    export.main()

    assert [call[1] for call in area_calls] == ["segments", "entities"]
    assert prompt_calls == 0

    monkeypatch.setattr(
        sys,
        "argv",
        ["sol", "--to", "host-a", "--dry-run", "--day", "20260520"],
    )
    export.main()

    assert prompt_calls == 0


def test_export_dl_never_prompts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.observe import export

    journal = tmp_path / "journal"
    journal.mkdir()
    _set_journal(monkeypatch, journal)
    area_calls: list[tuple] = []
    _patch_exporters(monkeypatch, area_calls)
    prompt_calls = 0

    class FakeSession:
        def __init__(self) -> None:
            self.headers = {}

        def post(self, *_args, **_kwargs):
            return PlHttpResponse(200, {}, b'{"ok": true}')

        def close(self) -> None:
            return None

    monkeypatch.setattr(export.requests, "Session", FakeSession)

    def prompt(*_args, **_kwargs):
        nonlocal prompt_calls
        prompt_calls += 1

    monkeypatch.setattr(export, "maybe_prompt_unpair", prompt)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "sol",
            "--to",
            "https://receiver.test",
            "--key",
            "api-key",
            "--day",
            "20260520",
        ],
    )

    export.main()

    assert prompt_calls == 0
    assert [call[1] for call in area_calls] == [
        "segments",
        "imports",
        "entities",
        "facets",
        "config",
    ]


def test_export_dl_regression_config_url_headers_and_body(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from solstone.observe.export import export_config

    journal = tmp_path / "journal"
    _set_journal(monkeypatch, journal)
    config_dir = journal / "config"
    config_dir.mkdir(parents=True)
    config = {
        "identity": {"name": "Test"},
        "convey": {
            "password_hash": "secret",
            "secret": "secret",
            "trust_localhost": True,
        },
    }
    (config_dir / "journal.json").write_text(json.dumps(config), encoding="utf-8")

    mock_session = MagicMock(spec=requests.Session)
    mock_session.headers = {}
    get_response = MagicMock()
    get_response.status_code = 200
    get_response.json.return_value = {"last_hash": "different"}
    mock_session.get.return_value = get_response
    post_response = MagicMock()
    post_response.status_code = 200
    post_response.json.return_value = {"staged": True, "diff_fields": 1}
    mock_session.post.return_value = post_response

    with patch("solstone.observe.export.requests.Session", return_value=mock_session):
        export_config("https://receiver.test", "abcdef123456", False)

    assert mock_session.headers["Authorization"] == "Bearer abcdef123456"
    assert mock_session.get.call_args.args[0] == (
        "https://receiver.test/app/import/journal/abcdef12/manifest/config"
    )
    assert mock_session.post.call_args.args[0] == (
        "https://receiver.test/app/import/journal/abcdef12/ingest/config"
    )
    payload = mock_session.post.call_args.kwargs["json"]["config"]
    assert payload["identity"] == {"name": "Test"}
    assert payload["convey"] == {"trust_localhost": True}


class _FakeInput(io.StringIO):
    def __init__(self, value: str, *, tty: bool) -> None:
        super().__init__(value)
        self._tty = tty

    def isatty(self) -> bool:
        return self._tty


def test_unpair_prompt_yes_deletes_peer(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    peer_dir = tmp_path / "peer"
    peer_dir.mkdir()
    peer = PeerInfo(peer_dir, "iid", "host-a", [], "sha256:" + ("a" * 64))
    posts: list[dict] = []

    class FakeSession:
        def post(self, _url, *, json=None):
            posts.append(json)
            return PlHttpResponse(200, {}, b"{}")

    stdout = io.StringIO()
    maybe_prompt_unpair(
        peer,
        FakeSession(),
        stdin=_FakeInput("y\n", tty=True),
        stdout=stdout,
    )

    assert posts == [{"fingerprint": peer.cert_fingerprint}]
    assert not peer_dir.exists()
    assert 'Unpaired "host-a".' in stdout.getvalue()


def test_unpair_prompt_decline_and_non_tty_timeout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    peer = PeerInfo(tmp_path / "peer", "iid", "host-a", [], "sha256:" + ("a" * 64))
    peer.dir.mkdir()

    class FakeSession:
        def post(self, *_args, **_kwargs):
            raise AssertionError("unpair should not post")

    stdout = io.StringIO()
    maybe_prompt_unpair(
        peer,
        FakeSession(),
        stdin=_FakeInput("N\n", tty=True),
        stdout=stdout,
    )
    assert 'Keeping peer "host-a".' in stdout.getvalue()
    assert peer.dir.exists()

    stdout = io.StringIO()
    monkeypatch.setattr(
        "solstone.observe.peer_unpair.select.select", lambda *args: ([], [], [])
    )
    maybe_prompt_unpair(
        peer,
        FakeSession(),
        stdin=_FakeInput("", tty=False),
        stdout=stdout,
    )
    assert 'Keeping peer "host-a" (non-interactive default).' in stdout.getvalue()
