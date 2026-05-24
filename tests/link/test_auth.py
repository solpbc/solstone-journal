# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""AuthorizedClients add/remove/reload semantics for the solstone fork."""

from __future__ import annotations

import datetime as dt
import json
import time
from pathlib import Path

from solstone.think.link.auth import AuthorizedClients


def test_empty_file_is_empty(tmp_path: Path) -> None:
    store = AuthorizedClients(tmp_path / "auth.json")

    assert not store.is_authorized("sha256:abc")


def test_add_and_authorized(tmp_path: Path) -> None:
    store = AuthorizedClients(tmp_path / "auth.json")

    store.add("sha256:abc", "Jer's phone", "inst-1")

    assert store.is_authorized("sha256:abc")
    assert not store.is_authorized("sha256:xyz")


def test_remove(tmp_path: Path) -> None:
    store = AuthorizedClients(tmp_path / "auth.json")
    store.add("sha256:abc", "Jer", "inst-1")

    assert store.remove("sha256:abc") is True
    assert not store.is_authorized("sha256:abc")
    assert store.remove("sha256:abc") is False


def test_external_edit_reloads_on_mtime_change(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)
    store.add("sha256:abc", "Jer", "inst-1")
    assert store.is_authorized("sha256:abc")

    time.sleep(0.02)
    path.write_text(json.dumps([], indent=2) + "\n", encoding="utf-8")

    assert store.reload_if_stale() is True
    assert not store.is_authorized("sha256:abc")


def test_is_authorized_reloads_automatically(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    time.sleep(0.02)
    path.write_text(
        json.dumps(
            [
                {
                    "fingerprint": "sha256:zzz",
                    "device_label": "external",
                    "paired_at": "2026-04-19T00:00:00Z",
                    "instance_id": "inst-1",
                }
            ],
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    assert store.is_authorized("sha256:zzz")


def test_snapshot_returns_entries(tmp_path: Path) -> None:
    store = AuthorizedClients(tmp_path / "auth.json")
    store.add("sha256:a", "d1", "inst-1")
    store.add("sha256:b", "d2", "inst-1")

    snapshot = store.snapshot()
    fingerprints = sorted(entry.fingerprint for entry in snapshot)

    assert fingerprints == ["sha256:a", "sha256:b"]
    assert {entry.role for entry in snapshot} == {"phone"}


def test_add_then_last_seen_key_absent_in_payload(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    store.add("sha256:abc", "Jer", "inst-1")

    payload = _load_payload(path)
    assert payload[0]["role"] == "phone"
    assert "last_seen_at" not in payload[0]


def test_touch_last_seen_unknown_fp_returns_false(tmp_path: Path) -> None:
    store = AuthorizedClients(tmp_path / "auth.json")

    assert store.touch_last_seen("sha256:deadbeef") is False

    store.add("sha256:abc", "Jer", "inst-1")

    assert store.touch_last_seen("sha256:deadbeef") is False


def test_touch_last_seen_updates_timestamp(tmp_path: Path) -> None:
    store = AuthorizedClients(tmp_path / "auth.json")
    fingerprint = "sha256:abc"
    later = dt.datetime(2026, 4, 19, 18, 3, 12, tzinfo=dt.UTC)

    store.add(fingerprint, "Jer", "inst-1")

    assert store.touch_last_seen(fingerprint) is True
    first_entry = next(
        entry for entry in store.snapshot() if entry.fingerprint == fingerprint
    )
    assert first_entry.last_seen_at is not None

    assert store.touch_last_seen(fingerprint, now=later) is True
    second_entry = next(
        entry for entry in store.snapshot() if entry.fingerprint == fingerprint
    )
    assert second_entry.last_seen_at == "2026-04-19T18:03:12Z"


def test_touch_last_seen_persists_key_in_payload(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    store.add("sha256:abc", "Jer", "inst-1")
    assert store.touch_last_seen("sha256:abc") is True

    payload = _load_payload(path)
    assert payload[0]["role"] == "phone"
    assert payload[0]["last_seen_at"]


def test_find_by_label(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    assert store.find_by_label("Jer") is None

    store.add("sha256:abc", "Jer", "inst-1")
    entry = store.find_by_label("Jer")
    assert entry is not None
    assert entry.fingerprint == "sha256:abc"
    assert entry.role == "phone"
    assert store.find_by_label("Nope") is None

    time.sleep(0.02)
    path.write_text(
        json.dumps(
            [
                {
                    "fingerprint": "sha256:xyz",
                    "device_label": "External",
                    "paired_at": "2026-04-19T00:00:00Z",
                    "instance_id": "inst-2",
                    "last_seen_at": "2026-04-19T18:03:12Z",
                }
            ],
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    reloaded = store.find_by_label("External")
    assert reloaded is not None
    assert reloaded.fingerprint == "sha256:xyz"
    assert reloaded.role == "phone"
    assert reloaded.last_seen_at == "2026-04-19T18:03:12Z"
    assert store.find_by_label("Jer") is None


def _load_payload(path: Path) -> list[dict[str, str]]:
    return json.loads(path.read_text(encoding="utf-8"))
