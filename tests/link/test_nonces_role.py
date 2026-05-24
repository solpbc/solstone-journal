# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from pathlib import Path

from solstone.think.link.nonces import NONCE_TTL_SECONDS, NonceStore


def test_add_persists_role(tmp_path: Path) -> None:
    path = tmp_path / "nonces.json"
    store = NonceStore(path)

    store.add("abc123", "Observer", role="observer", now=1000)

    payload = json.loads(path.read_text("utf-8"))
    assert payload[0]["role"] == "observer"


def test_add_default_role_phone(tmp_path: Path) -> None:
    path = tmp_path / "nonces.json"
    store = NonceStore(path)

    store.add("abc123", "Phone", now=1000)

    payload = json.loads(path.read_text("utf-8"))
    assert payload[0]["role"] == "phone"


def test_consume_preserves_role(tmp_path: Path) -> None:
    store = NonceStore(tmp_path / "nonces.json")
    store.add("abc123", "Observer", role="observer", now=1000)

    consumed = store.consume("abc123", now=1001)

    assert consumed is not None
    assert consumed.role == "observer"


def test_consume_by_code_preserves_role(tmp_path: Path) -> None:
    store = NonceStore(tmp_path / "nonces.json")
    store.add(
        "abc123",
        "Observer",
        role="observer",
        manual_code="K7M3X9PW",
        now=1000,
    )

    consumed = store.consume_by_code("K7M3X9PW", now=1001)

    assert consumed is not None
    assert consumed.role == "observer"


def test_read_legacy_nonce_defaults_phone(tmp_path: Path) -> None:
    path = tmp_path / "nonces.json"
    path.write_text(
        json.dumps(
            [
                {
                    "value": "abc123",
                    "device_label": "Legacy",
                    "issued_at": 1000,
                    "expires_at": 1000 + NONCE_TTL_SECONDS,
                    "used": False,
                    "manual_code": None,
                }
            ],
        )
        + "\n",
        encoding="utf-8",
    )
    store = NonceStore(path)

    consumed = store.consume("abc123", now=1001)

    assert consumed is not None
    assert consumed.role == "phone"
