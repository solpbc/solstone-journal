# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""AuthorizedClients add/remove/reload semantics for the solstone fork."""

from __future__ import annotations

import datetime as dt
import json
import threading
import time
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import pytest

import solstone.think.link.auth as auth_module
from solstone.think.link.auth import MAX_DEVICE_LABEL_LEN, AuthorizedClients

LEGACY_BROWSER_ROW = {
    "fingerprint": "sha256:" + "a" * 64,
    "device_label": "Browser",
    "paired_at": "2026-07-01T00:00:00Z",
    "instance_id": "inst-1",
    "role": "",
    "kind": "browser",
    "pubkey_spki": "30aa",
    "observer_handle": "handle123",
}

UNSUPPORTED_KIND_ROW = {
    "fingerprint": "sha256:" + "b" * 64,
    "device_label": "Legacy widget",
    "paired_at": "2026-07-01T00:01:00Z",
    "instance_id": "inst-1",
    "role": "",
    "kind": "legacy-widget",
}


@pytest.mark.parametrize(
    ("kind", "include_kind"),
    [(None, False), ("cert", True)],
    ids=["missing", "cert"],
)
def test_load_accepts_only_missing_or_exact_cert_kind(
    tmp_path: Path,
    kind: str | None,
    include_kind: bool,
) -> None:
    path = tmp_path / "auth.json"
    row = {
        "fingerprint": "sha256:accepted",
        "device_label": "Accepted",
        "paired_at": "2026-07-01T00:00:00Z",
        "instance_id": "inst-1",
    }
    if include_kind:
        row["kind"] = kind
    path.write_text(json.dumps([row], indent=2) + "\n", encoding="utf-8")

    store = AuthorizedClients(path)

    assert store.is_authorized(row["fingerprint"])
    entry = store.get(row["fingerprint"])
    assert entry is not None
    assert entry.kind == "cert"


@pytest.mark.parametrize(
    "kind",
    ["browser", "widget", "", None, True, 7, [], {}],
    ids=["browser", "widget", "empty", "null", "bool", "number", "list", "dict"],
)
def test_load_drops_every_present_non_cert_kind_with_redacted_warning(
    tmp_path: Path,
    caplog: pytest.LogCaptureFixture,
    kind: object,
) -> None:
    path = tmp_path / "auth.json"
    row = {
        "fingerprint": "sha256:malformed-kind",
        "device_label": "Sensitive device label",
        "paired_at": "2026-07-01T00:00:00Z",
        "instance_id": "inst-1",
        "kind": kind,
        "secret": "row-content-must-not-appear",
    }
    path.write_text(json.dumps([row], indent=2) + "\n", encoding="utf-8")
    before = path.read_bytes()
    caplog.set_level("WARNING", logger="solstone.think.link.auth")

    store = AuthorizedClients(path)

    assert store.get(row["fingerprint"]) is None
    assert store.snapshot() == []
    assert store.is_authorized(row["fingerprint"]) is False
    assert path.read_bytes() == before
    warnings = [
        record
        for record in caplog.records
        if record.name == "solstone.think.link.auth" and record.levelname == "WARNING"
    ]
    assert len(warnings) == 1
    message = warnings[0].getMessage()
    assert row["fingerprint"] not in message
    assert row["device_label"] not in message
    assert row["secret"] not in message


def test_load_drops_multiple_unsupported_kind_rows_with_single_redacted_warning(
    tmp_path: Path,
    caplog: pytest.LogCaptureFixture,
) -> None:
    path = tmp_path / "auth.json"
    unsupported_rows = [
        {
            "fingerprint": "sha256:null-kind-marker",
            "device_label": "Null kind marker",
            "paired_at": "2026-07-01T00:00:00Z",
            "instance_id": "inst-1",
            "kind": None,
        },
        {
            "fingerprint": "sha256:browser-kind-marker",
            "device_label": "Browser kind marker",
            "paired_at": "2026-07-01T00:01:00Z",
            "instance_id": "inst-2",
            "kind": "browser",
        },
        {
            "fingerprint": "sha256:number-kind-marker",
            "device_label": "Number kind marker",
            "paired_at": "2026-07-01T00:02:00Z",
            "instance_id": "inst-3",
            "kind": 5,
        },
    ]
    valid_cert_row = {
        "fingerprint": "sha256:valid-cert",
        "device_label": "Valid cert",
        "paired_at": "2026-07-01T00:03:00Z",
        "instance_id": "inst-4",
        "kind": "cert",
    }
    path.write_text(
        json.dumps([*unsupported_rows, valid_cert_row], indent=2) + "\n",
        encoding="utf-8",
    )
    before = path.read_bytes()
    caplog.set_level("WARNING", logger="solstone.think.link.auth")

    store = AuthorizedClients(path)

    for row in unsupported_rows:
        assert store.get(row["fingerprint"]) is None
        assert store.is_authorized(row["fingerprint"]) is False
    assert [entry.fingerprint for entry in store.snapshot()] == [
        valid_cert_row["fingerprint"]
    ]
    assert store.is_authorized(valid_cert_row["fingerprint"]) is True
    assert path.read_bytes() == before
    warnings = [
        record
        for record in caplog.records
        if record.name == "solstone.think.link.auth" and record.levelname == "WARNING"
    ]
    assert len(warnings) == 1
    message = warnings[0].getMessage()
    for row in unsupported_rows:
        assert row["fingerprint"] not in message
        assert row["device_label"] not in message


def test_empty_file_is_empty(tmp_path: Path) -> None:
    store = AuthorizedClients(tmp_path / "auth.json")

    assert not store.is_authorized("sha256:abc")


def test_add_and_authorized(tmp_path: Path) -> None:
    store = AuthorizedClients(tmp_path / "auth.json")

    store.add("sha256:abc", "Rae's phone", "inst-1")

    assert store.is_authorized("sha256:abc")
    assert not store.is_authorized("sha256:xyz")


def test_remove(tmp_path: Path) -> None:
    store = AuthorizedClients(tmp_path / "auth.json")
    store.add("sha256:abc", "Rae", "inst-1")

    assert store.remove("sha256:abc") is True
    assert not store.is_authorized("sha256:abc")
    assert store.remove("sha256:abc") is False


def test_external_edit_reloads_on_mtime_change(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)
    store.add("sha256:abc", "Rae", "inst-1")
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
    assert {entry.role for entry in snapshot} == {""}


def test_add_then_last_seen_key_absent_in_payload(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    store.add("sha256:abc", "Rae", "inst-1")

    payload = _load_payload(path)
    assert payload[0]["role"] == ""
    assert "last_seen_at" not in payload[0]
    assert "client_label" not in payload[0]
    assert "label_ordinal" not in payload[0]


def test_network_round_trips(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    store.add("sha256:abc", "Rae", "inst-1", network="anywhere")

    payload = _load_payload(path)
    assert payload[0]["network"] == "anywhere"

    reloaded = AuthorizedClients(path).get("sha256:abc")
    assert reloaded is not None
    assert reloaded.network == "anywhere"


def test_client_label_round_trips(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    store.add("sha256:abc", "Assigned", "inst-1", client_label="client-host")

    payload = _load_payload(path)
    assert payload[0]["client_label"] == "client-host"

    reloaded = AuthorizedClients(path).get("sha256:abc")
    assert reloaded is not None
    assert reloaded.client_label == "client-host"


def test_label_ordinal_round_trips_and_invalid_values_default(
    tmp_path: Path,
) -> None:
    path = tmp_path / "auth.json"
    path.write_text(
        json.dumps(
            [
                {
                    "fingerprint": "sha256:good",
                    "device_label": "phone",
                    "paired_at": "2026-04-19T00:00:00Z",
                    "instance_id": "inst-1",
                    "label_ordinal": 3,
                },
                {
                    "fingerprint": "sha256:bool",
                    "device_label": "phone",
                    "paired_at": "2026-04-19T00:00:01Z",
                    "instance_id": "inst-1",
                    "label_ordinal": True,
                },
                {
                    "fingerprint": "sha256:zero",
                    "device_label": "phone",
                    "paired_at": "2026-04-19T00:00:02Z",
                    "instance_id": "inst-1",
                    "label_ordinal": 0,
                },
                {
                    "fingerprint": "sha256:string",
                    "device_label": "phone",
                    "paired_at": "2026-04-19T00:00:03Z",
                    "instance_id": "inst-1",
                    "label_ordinal": "2",
                },
            ],
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    entries = {entry.fingerprint: entry for entry in AuthorizedClients(path).snapshot()}

    assert entries["sha256:good"].label_ordinal == 3
    assert entries["sha256:good"].display_label == "phone (3)"
    assert entries["sha256:bool"].label_ordinal == 1
    assert entries["sha256:zero"].label_ordinal == 1
    assert entries["sha256:string"].label_ordinal == 1


def test_missing_network_defaults_to_none(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    path.write_text(
        json.dumps(
            [
                {
                    "fingerprint": "sha256:abc",
                    "device_label": "Rae",
                    "paired_at": "2026-04-19T00:00:00Z",
                    "instance_id": "inst-1",
                }
            ],
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    entry = AuthorizedClients(path).get("sha256:abc")

    assert entry is not None
    assert entry.network is None


def test_missing_client_label_defaults_to_empty(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    path.write_text(
        json.dumps(
            [
                {
                    "fingerprint": "sha256:abc",
                    "device_label": "Rae",
                    "paired_at": "2026-04-19T00:00:00Z",
                    "instance_id": "inst-1",
                }
            ],
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    entry = AuthorizedClients(path).get("sha256:abc")

    assert entry is not None
    assert entry.client_label == ""


def test_touch_last_seen_unknown_fp_returns_false(tmp_path: Path) -> None:
    store = AuthorizedClients(tmp_path / "auth.json")

    assert store.touch_last_seen("sha256:deadbeef") is False

    store.add("sha256:abc", "Rae", "inst-1")

    assert store.touch_last_seen("sha256:deadbeef") is False


def test_touch_last_seen_updates_timestamp(tmp_path: Path) -> None:
    store = AuthorizedClients(tmp_path / "auth.json")
    fingerprint = "sha256:abc"
    later = dt.datetime(2026, 4, 19, 18, 3, 12, tzinfo=dt.UTC)

    store.add(fingerprint, "Rae", "inst-1")

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

    store.add("sha256:abc", "Rae", "inst-1")
    assert store.touch_last_seen("sha256:abc") is True

    payload = _load_payload(path)
    assert payload[0]["role"] == ""
    assert payload[0]["last_seen_at"]


def test_touch_last_seen_preserves_network(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    store.add("sha256:abc", "Rae", "inst-1", network="anywhere")
    assert store.touch_last_seen("sha256:abc") is True

    entry = store.get("sha256:abc")
    assert entry is not None
    assert entry.network == "anywhere"
    assert _load_payload(path)[0]["network"] == "anywhere"


def test_add_allocates_ordinals_and_touch_preserves_after_reload(
    tmp_path: Path,
) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    first = store.add(
        "sha256:a",
        "iPhone",
        "inst-1",
        paired_at="2026-04-19T00:00:00Z",
    )
    second = store.add(
        "sha256:b",
        "iPhone",
        "inst-1",
        paired_at="2026-04-19T00:00:01Z",
    )

    assert first.display_label == "iPhone"
    assert second.label_ordinal == 2
    assert second.display_label == "iPhone (2)"
    payload = {item["fingerprint"]: item for item in _load_payload(path)}
    assert "label_ordinal" not in payload["sha256:a"]
    assert payload["sha256:b"]["label_ordinal"] == 2

    assert store.touch_last_seen(
        "sha256:b",
        now=dt.datetime(2026, 4, 19, 18, 3, 12, tzinfo=dt.UTC),
    )
    reloaded = AuthorizedClients(path).get("sha256:b")

    assert reloaded is not None
    assert reloaded.label_ordinal == 2
    assert reloaded.display_label == "iPhone (2)"


def test_blank_base_entries_keep_ordinal_one(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    first = store.add("sha256:a", "", "inst-1")
    second = store.add("sha256:b", "", "inst-1")

    assert first.display_label == ""
    assert second.display_label == ""
    assert {entry.label_ordinal for entry in AuthorizedClients(path).snapshot()} == {1}
    assert all("label_ordinal" not in item for item in _load_payload(path))


def test_removed_sibling_does_not_renumber_sticky_ordinal(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)
    store.add("sha256:a", "iPhone", "inst-1")
    store.add("sha256:b", "iPhone", "inst-1")

    assert store.remove("sha256:a") is True
    reloaded = AuthorizedClients(path).get("sha256:b")

    assert reloaded is not None
    assert reloaded.label_ordinal == 2
    assert reloaded.display_label == "iPhone (2)"


def test_update_label_updates_and_persists(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)
    fingerprint = "sha256:abc"

    store.add(fingerprint, "old name", "inst-1")

    updated = store.update_label(fingerprint, "  new name  ")
    assert updated is not None
    assert updated.device_label == "new name"
    assert updated.display_label == "new name"
    entry = store.get(fingerprint)
    assert entry is not None
    assert entry.device_label == "new name"

    payload = _load_payload(path)
    assert payload[0]["device_label"] == "new name"


def test_update_label_preserves_network(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    store.add("sha256:abc", "old name", "inst-1", network="anywhere")
    assert store.update_label("sha256:abc", "new name") is not None

    entry = store.get("sha256:abc")
    assert entry is not None
    assert entry.device_label == "new name"
    assert entry.network == "anywhere"
    assert _load_payload(path)[0]["network"] == "anywhere"


def test_update_label_preserves_client_label(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    store.add("sha256:abc", "old name", "inst-1", client_label="client-host")
    assert store.update_label("sha256:abc", "new name") is not None

    entry = store.get("sha256:abc")
    assert entry is not None
    assert entry.device_label == "new name"
    assert entry.client_label == "client-host"
    assert _load_payload(path)[0]["client_label"] == "client-host"


def test_update_label_allocates_for_new_base_not_old_ordinal(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)
    store.add("sha256:a", "iPhone", "inst-1")
    store.add("sha256:b", "iPhone", "inst-1")

    updated = store.update_label("sha256:b", "Work Phone")

    assert updated is not None
    assert updated.label_ordinal == 1
    assert updated.display_label == "Work Phone"
    payload = {item["fingerprint"]: item for item in _load_payload(path)}
    assert "label_ordinal" not in payload["sha256:b"]


def test_update_label_display_round_trip_promotes_base_label(
    tmp_path: Path,
) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)
    base = "x" * MAX_DEVICE_LABEL_LEN
    store.add("sha256:a", base, "inst-1")
    target = store.add("sha256:b", "", "inst-1", client_label=base)
    assert len(target.display_label) == MAX_DEVICE_LABEL_LEN + 4

    updated = store.update_label("sha256:b", target.display_label)

    assert updated is not None
    assert updated.device_label == base
    assert updated.label_ordinal == 2
    assert updated.display_label == target.display_label
    reloaded = AuthorizedClients(path).get("sha256:b")
    assert reloaded is not None
    assert reloaded.device_label == base


@pytest.mark.parametrize(
    ("label", "message"),
    [
        ("", "label must not be empty"),
        ("   ", "label must not be empty"),
        ("x" * (MAX_DEVICE_LABEL_LEN + 1), "label too long"),
    ],
)
def test_update_label_rejects_invalid_labels(
    tmp_path: Path,
    label: str,
    message: str,
) -> None:
    store = AuthorizedClients(tmp_path / "auth.json")
    store.add("sha256:abc", "old name", "inst-1")

    with pytest.raises(ValueError, match=message):
        store.update_label("sha256:abc", label)


def test_update_label_unknown_fp_returns_false(tmp_path: Path) -> None:
    store = AuthorizedClients(tmp_path / "auth.json")

    assert store.update_label("sha256:deadbeef", "new name") is None

    store.add("sha256:abc", "old name", "inst-1")

    assert store.update_label("sha256:deadbeef", "new name") is None


def test_update_label_rereads_file_and_preserves_interleaved_last_seen(
    tmp_path: Path,
) -> None:
    path = tmp_path / "auth.json"
    first = AuthorizedClients(path)
    second = AuthorizedClients(path)
    fingerprint = "sha256:abc"
    seen_at = dt.datetime(2026, 4, 19, 18, 3, 12, tzinfo=dt.UTC)

    first.add(fingerprint, "old name", "inst-1")
    assert second.touch_last_seen(fingerprint, now=seen_at) is True
    assert first.update_label(fingerprint, "new name") is not None

    final = AuthorizedClients(path).get(fingerprint)
    assert final is not None
    assert final.device_label == "new name"
    assert final.last_seen_at == "2026-04-19T18:03:12Z"


def test_backfill_label_ordinals_repairs_duplicates_idempotently(
    tmp_path: Path,
) -> None:
    path = tmp_path / "auth.json"
    path.write_text(
        json.dumps(
            [
                {
                    "fingerprint": "sha256:c",
                    "device_label": "iPhone",
                    "paired_at": "2026-04-19T00:00:03Z",
                    "instance_id": "inst-1",
                },
                {
                    "fingerprint": "sha256:a",
                    "device_label": "iPhone",
                    "paired_at": "2026-04-19T00:00:01Z",
                    "instance_id": "inst-1",
                },
                {
                    "fingerprint": "sha256:b",
                    "device_label": "iPhone",
                    "paired_at": "2026-04-19T00:00:02Z",
                    "instance_id": "inst-1",
                },
                {
                    "fingerprint": "sha256:blank-a",
                    "device_label": "",
                    "paired_at": "2026-04-19T00:00:01Z",
                    "instance_id": "inst-1",
                },
                {
                    "fingerprint": "sha256:blank-b",
                    "device_label": "",
                    "paired_at": "2026-04-19T00:00:02Z",
                    "instance_id": "inst-1",
                },
            ],
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    before_load = path.read_bytes()
    store = AuthorizedClients(path)

    assert path.read_bytes() == before_load
    assert store.backfill_label_ordinals() is True
    payload = {item["fingerprint"]: item for item in _load_payload(path)}
    assert "label_ordinal" not in payload["sha256:a"]
    assert payload["sha256:b"]["label_ordinal"] == 2
    assert payload["sha256:c"]["label_ordinal"] == 3
    assert "label_ordinal" not in payload["sha256:blank-a"]
    assert "label_ordinal" not in payload["sha256:blank-b"]

    after_backfill = path.read_bytes()
    assert store.backfill_label_ordinals() is False
    assert path.read_bytes() == after_backfill

    assert store.touch_last_seen(
        "sha256:b",
        now=dt.datetime(2026, 4, 19, 18, 3, 12, tzinfo=dt.UTC),
    )
    after_touch = path.read_bytes()
    assert store.backfill_label_ordinals() is False
    assert path.read_bytes() == after_touch
    reloaded = AuthorizedClients(path).get("sha256:b")
    assert reloaded is not None
    assert reloaded.label_ordinal == 2


def test_backfill_label_ordinals_preserves_sticky_non_duplicates(
    tmp_path: Path,
) -> None:
    path = tmp_path / "auth.json"
    path.write_text(
        json.dumps(
            [
                {
                    "fingerprint": "sha256:a",
                    "device_label": "iPhone",
                    "paired_at": "2026-04-19T00:00:01Z",
                    "instance_id": "inst-1",
                    "label_ordinal": 1,
                },
                {
                    "fingerprint": "sha256:b",
                    "device_label": "iPhone",
                    "paired_at": "2026-04-19T00:00:02Z",
                    "instance_id": "inst-1",
                    "label_ordinal": 3,
                },
                {
                    "fingerprint": "sha256:c",
                    "device_label": "iPad",
                    "paired_at": "2026-04-19T00:00:03Z",
                    "instance_id": "inst-1",
                    "label_ordinal": 2,
                },
            ],
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    before = path.read_bytes()

    assert AuthorizedClients(path).backfill_label_ordinals() is False
    assert path.read_bytes() == before


def test_concurrent_add_allocates_distinct_ordinals(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "auth.json"
    first = AuthorizedClients(path)
    second = AuthorizedClients(path)
    barrier = threading.Barrier(2)
    real_hold_lock = auth_module.hold_lock

    @contextmanager
    def hold_lock_with_barrier(path_arg: Path, **kwargs: object) -> Iterator[None]:
        barrier.wait(timeout=5)
        with real_hold_lock(path_arg, **kwargs):
            yield

    monkeypatch.setattr(auth_module, "hold_lock", hold_lock_with_barrier)
    errors: list[Exception] = []

    def add(store: AuthorizedClients, fingerprint: str) -> None:
        try:
            store.add(fingerprint, "iPhone", "inst-1")
        except Exception as exc:
            errors.append(exc)

    threads = [
        threading.Thread(target=add, args=(first, "sha256:a")),
        threading.Thread(target=add, args=(second, "sha256:b")),
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=10)

    assert not any(thread.is_alive() for thread in threads)
    assert not errors
    entries = {entry.fingerprint: entry for entry in AuthorizedClients(path).snapshot()}
    assert {entry.label_ordinal for entry in entries.values()} == {1, 2}
    assert sorted(entry.display_label for entry in entries.values()) == [
        "iPhone",
        "iPhone (2)",
    ]


def test_find_all_by_display_label(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    store = AuthorizedClients(path)

    assert store.find_all_by_display_label("Rae") == []

    store.add("sha256:abc", "Rae", "inst-1")
    store.add("sha256:empty", "", "inst-1", client_label="client-host")
    entries = store.find_all_by_display_label("Rae")
    assert len(entries) == 1
    entry = entries[0]
    assert entry.fingerprint == "sha256:abc"
    assert entry.role == ""
    assert store.find_all_by_display_label("Nope") == []
    assert store.find_all_by_display_label("") == []
    client_entries = store.find_all_by_display_label("client-host")
    assert len(client_entries) == 1
    assert client_entries[0].fingerprint == "sha256:empty"

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

    reloaded_entries = store.find_all_by_display_label("External")
    assert len(reloaded_entries) == 1
    reloaded = reloaded_entries[0]
    assert reloaded.fingerprint == "sha256:xyz"
    assert reloaded.role == ""
    assert reloaded.last_seen_at == "2026-04-19T18:03:12Z"
    assert store.find_all_by_display_label("Rae") == []


def test_old_cert_entry_defaults_to_cert_kind(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    path.write_text(
        json.dumps(
            [
                {
                    "fingerprint": "sha256:legacy",
                    "device_label": "legacy",
                    "paired_at": "2026-07-01T00:00:00Z",
                    "instance_id": "inst-1",
                }
            ],
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    entry = AuthorizedClients(path).get("sha256:legacy")

    assert entry is not None
    assert entry.kind == "cert"
    assert entry.observer_handle is None


def test_load_drops_legacy_browser_row_with_redacted_warning(
    tmp_path: Path, caplog: pytest.LogCaptureFixture
) -> None:
    path = tmp_path / "auth.json"
    cert_row = {
        "fingerprint": "sha256:cert",
        "device_label": "Cert",
        "paired_at": "2026-07-01T00:02:00Z",
        "instance_id": "inst-1",
        "role": "",
        "kind": "cert",
    }
    payload = json.dumps([LEGACY_BROWSER_ROW, cert_row], indent=2) + "\n"
    path.write_text(payload, encoding="utf-8")
    before = path.read_bytes()
    caplog.set_level("WARNING", logger="solstone.think.link.auth")

    store = AuthorizedClients(path)

    assert [entry.fingerprint for entry in store.snapshot()] == [
        cert_row["fingerprint"]
    ]
    assert store.get(LEGACY_BROWSER_ROW["fingerprint"]) is None
    assert store.is_authorized(LEGACY_BROWSER_ROW["fingerprint"]) is False
    cert_entry = store.get(cert_row["fingerprint"])
    assert cert_entry is not None
    assert cert_entry.kind == "cert"
    assert path.read_bytes() == before
    warnings = [
        record
        for record in caplog.records
        if record.name == "solstone.think.link.auth" and record.levelname == "WARNING"
    ]
    assert len(warnings) == 1
    message = warnings[0].getMessage()
    assert LEGACY_BROWSER_ROW["fingerprint"] not in message
    assert "30aa" not in message
    assert "handle123" not in message


def test_cert_mutation_rewrites_ledger_dropping_legacy_browser_row(
    tmp_path: Path,
) -> None:
    path = tmp_path / "auth.json"
    missing_kind_cert_row = {
        "fingerprint": "sha256:missing-kind-cert",
        "device_label": "Old label",
        "paired_at": "2026-07-01T00:02:00Z",
        "instance_id": "inst-1",
        "role": "peer",
        "last_seen_at": "2026-07-01T00:03:00Z",
        "network": "anywhere",
        "client_label": "missing-kind-client",
        "label_ordinal": 2,
    }
    explicit_cert_row = {
        "fingerprint": "sha256:explicit-cert",
        "device_label": "Renamed",
        "paired_at": "2026-07-01T00:04:00Z",
        "instance_id": "inst-2",
        "role": "",
        "kind": "cert",
        "last_seen_at": "2026-07-01T00:05:00Z",
        "network": "local",
        "client_label": "explicit-cert-client",
        "label_ordinal": 1,
    }
    null_kind_row = {
        "fingerprint": "sha256:null-kind",
        "device_label": "Null kind",
        "paired_at": "2026-07-01T00:06:00Z",
        "instance_id": "inst-3",
        "kind": None,
    }
    path.write_text(
        json.dumps(
            [
                LEGACY_BROWSER_ROW,
                null_kind_row,
                missing_kind_cert_row,
                explicit_cert_row,
            ],
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    store = AuthorizedClients(path)

    assert (
        store.update_label(missing_kind_cert_row["fingerprint"], "Renamed") is not None
    )

    payload = _load_payload(path)
    assert payload == [
        {
            "fingerprint": missing_kind_cert_row["fingerprint"],
            "device_label": "Renamed",
            "paired_at": missing_kind_cert_row["paired_at"],
            "instance_id": missing_kind_cert_row["instance_id"],
            "role": "peer",
            "kind": "cert",
            "network": "anywhere",
            "last_seen_at": "2026-07-01T00:03:00Z",
            "client_label": "missing-kind-client",
            "label_ordinal": 2,
        },
        {
            "fingerprint": explicit_cert_row["fingerprint"],
            "device_label": explicit_cert_row["device_label"],
            "paired_at": explicit_cert_row["paired_at"],
            "instance_id": explicit_cert_row["instance_id"],
            "role": "",
            "kind": "cert",
            "network": "local",
            "last_seen_at": "2026-07-01T00:05:00Z",
            "client_label": "explicit-cert-client",
        },
    ]
    entries = {entry.fingerprint: entry for entry in AuthorizedClients(path).snapshot()}
    assert entries[missing_kind_cert_row["fingerprint"]].label_ordinal == 2
    assert entries[explicit_cert_row["fingerprint"]].label_ordinal == 1
    assert all(
        item.get("kind") == "cert"
        and "pubkey_spki" not in item
        and "observer_handle" not in item
        for item in payload
    )


def test_legacy_cert_kind_forms_both_authorize(tmp_path: Path) -> None:
    path = tmp_path / "auth.json"
    explicit = {
        "fingerprint": "sha256:explicit",
        "device_label": "Explicit",
        "paired_at": "2026-07-01T00:00:00Z",
        "instance_id": "inst-1",
        "kind": "cert",
    }
    missing = {
        "fingerprint": "sha256:missing",
        "device_label": "Missing",
        "paired_at": "2026-07-01T00:01:00Z",
        "instance_id": "inst-1",
    }
    path.write_text(json.dumps([explicit, missing], indent=2) + "\n", encoding="utf-8")

    store = AuthorizedClients(path)

    assert store.is_authorized(explicit["fingerprint"])
    assert store.is_authorized(missing["fingerprint"])
    assert store.get(explicit["fingerprint"]).kind == "cert"
    assert store.get(missing["fingerprint"]).kind == "cert"


def test_load_drops_any_unsupported_kind(
    tmp_path: Path, caplog: pytest.LogCaptureFixture
) -> None:
    path = tmp_path / "auth.json"
    path.write_text(
        json.dumps([UNSUPPORTED_KIND_ROW], indent=2) + "\n", encoding="utf-8"
    )
    caplog.set_level("WARNING", logger="solstone.think.link.auth")

    store = AuthorizedClients(path)

    assert store.get(UNSUPPORTED_KIND_ROW["fingerprint"]) is None
    assert store.snapshot() == []
    assert store.is_authorized(UNSUPPORTED_KIND_ROW["fingerprint"]) is False
    warnings = [
        record
        for record in caplog.records
        if record.name == "solstone.think.link.auth" and record.levelname == "WARNING"
    ]
    assert len(warnings) == 1
    assert UNSUPPORTED_KIND_ROW["fingerprint"] not in warnings[0].getMessage()


def _load_payload(path: Path) -> list[dict[str, str]]:
    return json.loads(path.read_text(encoding="utf-8"))
