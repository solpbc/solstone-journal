# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from pathlib import Path

from solstone.think.link.auth import AuthorizedClients


def test_add_persists_role(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    store.add("sha256:abc", "Observer", "inst-1", role="observer")

    payload = json.loads(path.read_text("utf-8"))
    assert payload[0]["role"] == "observer"


def test_add_default_role_phone(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    store.add("sha256:abc", "Phone", "inst-1")

    payload = json.loads(path.read_text("utf-8"))
    assert payload[0]["role"] == "phone"


def test_load_legacy_entry_defaults_phone(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    path.write_text(
        json.dumps(
            [
                {
                    "fingerprint": "sha256:abc",
                    "device_label": "Legacy",
                    "paired_at": "2026-04-19T00:00:00Z",
                    "instance_id": "inst-1",
                }
            ],
        )
        + "\n",
        encoding="utf-8",
    )

    store = AuthorizedClients(path)

    assert store.snapshot()[0].role == "phone"


def test_role_round_trips_through_write_then_read(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)
    store.add("sha256:observer", "Observer", "inst-1", role="observer")
    store.add("sha256:peer", "Peer", "inst-1", role="peer")
    store.add("sha256:phone", "Phone", "inst-1")

    reloaded = AuthorizedClients(path)
    roles = {entry.fingerprint: entry.role for entry in reloaded.snapshot()}

    assert roles == {
        "sha256:observer": "observer",
        "sha256:peer": "peer",
        "sha256:phone": "phone",
    }


def test_invalid_role_string_still_persists(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    store.add("sha256:abc", "Bogus", "inst-1", role="bogus")

    payload = json.loads(path.read_text("utf-8"))
    assert payload[0]["role"] == "bogus"
