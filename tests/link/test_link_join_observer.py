# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import argparse
import json
import stat
from pathlib import Path
from typing import Any

import pytest
from cryptography.hazmat.primitives import serialization

from solstone.apps.link.routes import _build_pair_link
from solstone.think.link import join_cli
from solstone.think.link.ca import generate_ca


class _FakeResponse:
    def __init__(self, body: bytes, *, status: int = 200) -> None:
        self._body = body
        self.status = status

    def __enter__(self) -> _FakeResponse:
        return self

    def __exit__(self, *_: object) -> None:
        return None

    def read(self) -> bytes:
        return self._body

    def getcode(self) -> int:
        return self.status


def _args(
    *,
    home: str | None = "http://receiver",
    code: str = "ABCD-EFGH",
    as_role: str = "observer",
    label: str = "laptop",
) -> argparse.Namespace:
    return argparse.Namespace(home=home, code=code, as_role=as_role, label=label)


def _success_payload(tmp_path: Path) -> dict[str, Any]:
    ca = generate_ca(tmp_path / "ca")
    ca_pem = ca.cert.public_bytes(serialization.Encoding.PEM).decode("ascii")
    return {
        "client_cert": "-----BEGIN CERTIFICATE-----\nclient\n-----END CERTIFICATE-----\n",
        "ca_chain": [ca_pem],
        "instance_id": "inst-1",
        "home_label": "solstone",
        "home_attestation": "header.payload.signature",
        "local_endpoints": [{"host": "127.0.0.1", "port": 7657}],
        "fingerprint": "sha256:client",
    }


def _mock_urlopen(
    monkeypatch: pytest.MonkeyPatch,
    payload: dict[str, Any] | bytes,
    *,
    status: int = 200,
    calls: list[tuple[str, dict[str, Any]]] | None = None,
) -> None:
    body = (
        payload if isinstance(payload, bytes) else json.dumps(payload).encode("utf-8")
    )

    def fake_urlopen(request, **_kwargs):
        if calls is not None:
            calls.append(
                (
                    request.full_url,
                    json.loads(request.data.decode("utf-8")),
                )
            )
        return _FakeResponse(body, status=status)

    monkeypatch.setattr(join_cli.urllib.request, "urlopen", fake_urlopen)


def _configure_home(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    config_home = tmp_path / "config"
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    return config_home


def test_short_code_happy_path_writes_bundle(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    config_home = _configure_home(tmp_path, monkeypatch)
    calls: list[tuple[str, dict[str, Any]]] = []
    _mock_urlopen(monkeypatch, _success_payload(tmp_path), calls=calls)

    result = join_cli.main(_args())

    assert result == 0
    assert calls[0][0] == "http://receiver/app/link/by-code"
    assert calls[0][1]["code"] == "ABCDEFGH"
    bundle = config_home / "solstone-observer" / "spl" / "laptop"
    assert stat.S_IMODE(bundle.stat().st_mode) == 0o700
    for name in join_cli.BUNDLE_FILES:
        assert (bundle / name).exists()
        assert stat.S_IMODE((bundle / name).stat().st_mode) == 0o600
    peer = json.loads((bundle / "peer.json").read_text("utf-8"))
    assert list(peer.keys()) == [
        "label",
        "paired_at",
        "instance_id",
        "home_label",
        "fingerprint",
        "local_endpoints",
        "role",
    ]
    assert peer["label"] == "laptop"
    assert peer["instance_id"] == "inst-1"
    assert peer["home_label"] == "solstone"
    assert peer["fingerprint"].startswith("sha256:")
    assert peer["local_endpoints"] == [{"host": "127.0.0.1", "port": 7657}]
    assert peer["role"] == "observer"


def test_url_happy_path_posts_to_pair_token(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _configure_home(tmp_path, monkeypatch)
    calls: list[tuple[str, dict[str, Any]]] = []
    _mock_urlopen(monkeypatch, _success_payload(tmp_path), calls=calls)
    pair_link = _build_pair_link(
        "192.0.2.42",
        7070,
        "a1b2c3d4e5f607181122334455667788",
        "deadbeefcafebabe0123456789abcdef",
    )

    result = join_cli.main(_args(code=pair_link, home="http://receiver"))

    assert result == 0
    assert (
        calls[0][0]
        == "http://receiver/app/link/pair?token=a1b2c3d4e5f607181122334455667788"
    )
    assert "code" not in calls[0][1]
    assert calls[0][1]["device_label"] == "laptop"


def test_missing_required_response_field_exits_1(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _configure_home(tmp_path, monkeypatch)
    payload = _success_payload(tmp_path)
    del payload["client_cert"]
    _mock_urlopen(monkeypatch, payload)

    result = join_cli.main(_args())

    assert result == 1


def test_non_200_exits_1(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _configure_home(tmp_path, monkeypatch)
    _mock_urlopen(monkeypatch, b"nope", status=500)

    result = join_cli.main(_args())

    assert result == 1


def test_malformed_json_exits_1(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure_home(tmp_path, monkeypatch)
    _mock_urlopen(monkeypatch, b"{")

    result = join_cli.main(_args())

    assert result == 1


def test_partial_write_failure_cleans_created_directory(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    config_home = _configure_home(tmp_path, monkeypatch)
    _mock_urlopen(monkeypatch, _success_payload(tmp_path))
    original_write = join_cli._write_bytes

    def fail_on_chain(path: Path, content: bytes) -> None:
        if path.name == "chain.pem":
            raise OSError("failed to write chain.pem")
        original_write(path, content)

    monkeypatch.setattr(join_cli, "_write_bytes", fail_on_chain)

    result = join_cli.main(_args())

    assert result == 1
    assert not (config_home / "solstone-observer" / "spl" / "laptop").exists()
