# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from pathlib import Path

from solstone.think.link.nonces import NONCE_TTL_SECONDS, Nonce, NonceStore


def test_add_and_consume(tmp_path: Path) -> None:
    store = NonceStore(tmp_path / "nonces.json")

    added = store.add("abc123", "phone", now=1000)
    consumed = store.consume("abc123", now=1001)

    assert added == Nonce(
        value="abc123",
        device_label="phone",
        issued_at=1000,
        expires_at=1000 + NONCE_TTL_SECONDS,
        used=False,
        manual_code=None,
        role="phone",
    )
    assert consumed == Nonce(
        value="abc123",
        device_label="phone",
        issued_at=1000,
        expires_at=1000 + NONCE_TTL_SECONDS,
        used=True,
        manual_code=None,
        role="phone",
    )


def test_consume_is_single_use(tmp_path: Path) -> None:
    store = NonceStore(tmp_path / "nonces.json")

    store.add("abc123", "phone", now=1000)

    assert store.consume("abc123", now=1001) is not None
    assert store.consume("abc123", now=1002) is None


def test_expired_nonce_rejected(tmp_path: Path) -> None:
    store = NonceStore(tmp_path / "nonces.json")

    store.add("abc123", "phone", now=1000)

    assert store.consume("abc123", now=1000 + NONCE_TTL_SECONDS + 1) is None


def test_unknown_nonce_returns_none(tmp_path: Path) -> None:
    store = NonceStore(tmp_path / "nonces.json")

    assert store.consume("never-added") is None


def test_gc_removes_expired_and_used(tmp_path: Path) -> None:
    store = NonceStore(tmp_path / "nonces.json")

    store.add("live", "device", now=1000)
    store.add("used", "device", now=1000)
    store.consume("used", now=1001)

    removed = store.gc(now=1001)

    assert removed == 1
    assert [entry.value for entry in store.snapshot()] == ["live"]

    store.add("fresh", "device", now=2000)
    store.add("also_expired", "device", now=2000 - NONCE_TTL_SECONDS - 10)
    store.gc(now=2000)

    assert {entry.value for entry in store.snapshot()} == {"fresh"}


def test_persistence_across_store_instances(tmp_path: Path) -> None:
    path = tmp_path / "nonces.json"
    first = NonceStore(path)
    first.add("shared", "device", now=1000)

    second = NonceStore(path)
    entry = second.consume("shared", now=1001)

    assert entry is not None
    assert entry.value == "shared"


def test_consume_by_code_round_trip(tmp_path: Path) -> None:
    store = NonceStore(tmp_path / "nonces.json")
    store.add("abc123", "phone", manual_code="K7M3X9PW", now=1000)

    consumed = store.consume_by_code("K7M3X9PW", now=1001)

    assert consumed == Nonce(
        value="abc123",
        device_label="phone",
        issued_at=1000,
        expires_at=1000 + NONCE_TTL_SECONDS,
        used=True,
        manual_code="K7M3X9PW",
        role="phone",
    )
    assert store.peek("abc123") == consumed


def test_consume_by_code_single_use(tmp_path: Path) -> None:
    store = NonceStore(tmp_path / "nonces.json")
    store.add("abc123", "phone", manual_code="K7M3X9PW", now=1000)

    assert store.consume_by_code("K7M3X9PW", now=1001) is not None
    assert store.consume_by_code("K7M3X9PW", now=1002) is None


def test_consume_by_code_unknown_returns_none(tmp_path: Path) -> None:
    store = NonceStore(tmp_path / "nonces.json")
    store.add("abc123", "phone", manual_code="K7M3X9PW", now=1000)

    assert store.consume_by_code("AAAAAAAA", now=1001) is None


def test_consume_by_code_expired_returns_none(tmp_path: Path) -> None:
    store = NonceStore(tmp_path / "nonces.json")
    store.add("abc123", "phone", manual_code="K7M3X9PW", now=1000)

    assert store.consume_by_code("K7M3X9PW", now=1000 + NONCE_TTL_SECONDS + 1) is None


def test_persistence_preserves_manual_code(tmp_path: Path) -> None:
    path = tmp_path / "nonces.json"
    first = NonceStore(path)
    first.add("abc123", "phone", manual_code="K7M3X9PW", now=1000)

    second = NonceStore(path)
    entry = second.peek("abc123")

    assert entry is not None
    assert entry.manual_code == "K7M3X9PW"


def test_read_tolerates_missing_manual_code(tmp_path: Path) -> None:
    path = tmp_path / "nonces.json"
    path.write_text(
        json.dumps(
            [
                {
                    "value": "abc123",
                    "device_label": "phone",
                    "issued_at": 1000,
                    "expires_at": 1000 + NONCE_TTL_SECONDS,
                    "used": False,
                }
            ]
        ),
        encoding="utf-8",
    )
    store = NonceStore(path)

    entry = store.peek("abc123")
    consumed = store.consume("abc123", now=1001)

    assert entry is not None
    assert entry.manual_code is None
    assert consumed is not None
    assert consumed.manual_code is None
