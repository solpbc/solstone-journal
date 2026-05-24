# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for observer app routes."""

from __future__ import annotations

import io
import json

import solstone.apps.observer.routes as routes_module
import solstone.convey.bridge as convey_bridge
from solstone.apps.observer.routes import (
    ACTIVE_THRESHOLD_MS,
    FUTURE_CLOCK_DRIFT_TOLERANCE_MS,
    OBSERVER_STATE_LABELS,
    STALE_THRESHOLD_MS,
    _classify_observer_freshness,
)
from solstone.apps.observer.utils import mint_pl_observer_record, save_observer
from solstone.convey.copy import OBSERVER_CALLOSUM_LIVE_LABEL
from solstone.convey.secure_listener import ConveyIdentity
from solstone.convey.sol_initiated.copy import KIND_SOL_CHAT_REQUEST

PL_FINGERPRINT = "sha256:" + ("c" * 64)
PL_FINGERPRINT_2 = "sha256:" + ("d" * 64)


def _pl_identity(fingerprint: str = PL_FINGERPRINT) -> ConveyIdentity:
    return ConveyIdentity(
        mode="pl-via-spl",
        fingerprint=fingerprint,
        device_label="pl-observer",
        paired_at="2026-05-20T00:00:00Z",
        session_id="session-1",
    )


def _api_list_payload(env):
    resp = env.client.get("/app/observer/api/list")
    assert resp.status_code == 200
    return resp.get_json()


def _api_list_observers(env):
    return _api_list_payload(env)["observers"]


def _day_dir(env, day: str = "20250103"):
    return env.journal / "chronicle" / day


def _save_test_observer(
    key_prefix: str,
    name: str,
    *,
    created_at: int,
    last_seen: int | None,
    revoked: bool = False,
):
    key = key_prefix + ("f" * 56)
    assert save_observer(
        {
            "key": key,
            "name": name,
            "created_at": created_at,
            "last_seen": last_seen,
            "last_segment": None,
            "enabled": True,
            "revoked": revoked,
            "revoked_at": created_at + 1 if revoked else None,
            "stats": {
                "segments_received": 0,
                "bytes_received": 0,
            },
        }
    )
    return key


def test_classifier_last_seen_none_returns_disconnected():
    """Missing last_seen is classified as disconnected."""
    assert _classify_observer_freshness(None, False, 1_000_000) == {
        "state": "disconnected",
        "group": "inactive",
        "elapsed_ms": None,
        "clock_skew": False,
    }


def test_classifier_future_within_tolerance_returns_connected_no_skew():
    """Small future drift stays connected without clock skew."""
    current_now = 1_000_000
    assert 60_000 < FUTURE_CLOCK_DRIFT_TOLERANCE_MS

    assert _classify_observer_freshness(current_now + 60_000, False, current_now) == {
        "state": "connected",
        "group": "active",
        "elapsed_ms": 0,
        "clock_skew": False,
    }


def test_classifier_future_beyond_tolerance_returns_disconnected_with_skew():
    """Large future drift is disconnected and flagged for clock skew."""
    current_now = 1_000_000
    last_seen = current_now + (10 * 60_000)
    assert (10 * 60_000) > FUTURE_CLOCK_DRIFT_TOLERANCE_MS

    result = _classify_observer_freshness(last_seen, False, current_now)

    assert result["state"] == "disconnected"
    assert result["group"] == "inactive"
    assert result["clock_skew"] is True
    assert result["elapsed_ms"] == -600_000


def test_classifier_just_under_active_returns_connected():
    """Elapsed time just under the active threshold stays connected."""
    current_now = 1_000_000

    assert _classify_observer_freshness(
        current_now - (ACTIVE_THRESHOLD_MS - 1),
        False,
        current_now,
    ) == {
        "state": "connected",
        "group": "active",
        "elapsed_ms": ACTIVE_THRESHOLD_MS - 1,
        "clock_skew": False,
    }


def test_classifier_just_over_active_returns_stale():
    """Elapsed time at the active threshold enters the stale bucket."""
    current_now = 1_000_000

    assert _classify_observer_freshness(
        current_now - ACTIVE_THRESHOLD_MS,
        False,
        current_now,
    ) == {
        "state": "stale",
        "group": "stale",
        "elapsed_ms": ACTIVE_THRESHOLD_MS,
        "clock_skew": False,
    }


def test_classifier_beyond_stale_returns_disconnected():
    """Elapsed time at the stale threshold becomes disconnected."""
    current_now = 1_000_000

    assert _classify_observer_freshness(
        current_now - STALE_THRESHOLD_MS,
        False,
        current_now,
    ) == {
        "state": "disconnected",
        "group": "inactive",
        "elapsed_ms": STALE_THRESHOLD_MS,
        "clock_skew": False,
    }


def test_classifier_revoked_returns_revoked_regardless_of_last_seen():
    """Revoked observers stay revoked for both missing and recent last_seen."""
    current_now = 1_000_000
    expected = {
        "state": "revoked",
        "group": "inactive",
        "elapsed_ms": None,
        "clock_skew": False,
    }

    assert _classify_observer_freshness(None, True, current_now) == expected
    assert _classify_observer_freshness(current_now, True, current_now) == expected


def test_api_list_empty(observer_env):
    """Test listing observers when none exist."""
    env = observer_env()

    assert _api_list_payload(env) == {
        "thresholds": {
            "active_ms": 30000,
            "stale_ms": 120000,
        },
        "labels": dict(live=OBSERVER_CALLOSUM_LIVE_LABEL),
        "observers": [],
    }


def test_api_create_observer(observer_env):
    """Test creating a new observer."""
    env = observer_env()

    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "test-laptop"},
        content_type="application/json",
    )

    assert resp.status_code == 200
    data = resp.get_json()

    assert "key" in data
    assert len(data["key"]) > 32  # 256 bits = 43 base64 chars
    assert data["key_prefix"] == data["key"][:8]
    assert data["name"] == "test-laptop"
    assert "/app/observer/ingest/" in data["ingest_url"]


def test_api_create_requires_name(observer_env):
    """Test that creating a observer requires a name."""
    env = observer_env()

    # Missing name
    resp = env.client.post(
        "/app/observer/api/create",
        json={},
        content_type="application/json",
    )
    assert resp.status_code == 400
    assert "Name is required" in resp.get_json()["detail"]

    # Empty name
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "   "},
        content_type="application/json",
    )
    assert resp.status_code == 400


def test_api_list_shows_created_observer(observer_env):
    """Test that created observers appear in the list."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "my-observer"},
        content_type="application/json",
    )
    assert resp.status_code == 200
    key_prefix = resp.get_json()["key_prefix"]

    # List should show it
    payload = _api_list_payload(env)
    observers = payload["observers"]

    assert len(observers) == 1
    assert payload["thresholds"] == {"active_ms": 30000, "stale_ms": 120000}
    assert observers[0]["key_prefix"] == key_prefix
    assert observers[0]["name"] == "my-observer"
    assert observers[0]["enabled"] is True
    assert observers[0]["stats"]["segments_received"] == 0
    assert observers[0]["state"] == "disconnected"
    assert observers[0]["group"] == "inactive"
    assert observers[0]["label"] == OBSERVER_STATE_LABELS["disconnected"]
    assert observers[0]["elapsed_ms"] is None
    assert observers[0]["clock_skew"] is False
    assert observers[0]["last_chat_request_at"] is None


def test_api_list_includes_last_chat_request_at(observer_env):
    env = observer_env()
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "my-observer"},
        content_type="application/json",
    )
    assert resp.status_code == 200
    key_prefix = resp.get_json()["key_prefix"]
    handle = convey_bridge.register_sse_subscriber(key_prefix)
    try:
        convey_bridge._broadcast_to_sse_clients(
            {"tract": "chat", "event": KIND_SOL_CHAT_REQUEST, "ts": 9876}
        )
        observers = _api_list_observers(env)
    finally:
        convey_bridge.unregister_sse_subscriber(handle)
        with convey_bridge._SSE_LOCK:
            convey_bridge._SSE_LAST_CHAT_REQUEST_AT_BY_KEY.pop(key_prefix, None)

    assert observers[0]["key_prefix"] == key_prefix
    assert observers[0]["last_chat_request_at"] == 9876


def test_api_delete_observer(observer_env):
    """Test revoking a observer (soft-delete)."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "to-revoke"},
        content_type="application/json",
    )
    key_prefix = resp.get_json()["key_prefix"]

    # Revoke it
    resp = env.client.delete(f"/app/observer/api/{key_prefix}")
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "ok"

    # List should still show it, but marked as revoked
    observers = _api_list_observers(env)
    assert len(observers) == 1
    assert observers[0]["key_prefix"] == key_prefix
    assert observers[0]["revoked"] is True
    assert observers[0]["revoked_at"] is not None
    assert observers[0]["state"] == "revoked"
    assert observers[0]["group"] == "inactive"
    assert observers[0]["label"] == OBSERVER_STATE_LABELS["revoked"]
    assert observers[0]["elapsed_ms"] is None
    assert observers[0]["clock_skew"] is False


def test_api_list_sorts_by_group_and_last_seen(observer_env, monkeypatch):
    """api_list orders active, then stale, then inactive with freshest first."""
    env = observer_env()
    fixed_now = 2_000_000
    monkeypatch.setattr(routes_module, "now_ms", lambda: fixed_now)

    _save_test_observer(
        "cccc0000",
        "inactive-disconnected",
        created_at=10,
        last_seen=fixed_now - 600_000,
    )
    _save_test_observer(
        "bbbb0000",
        "stale-observer",
        created_at=20,
        last_seen=fixed_now - 60_000,
    )
    _save_test_observer(
        "aaaa0000",
        "active-observer",
        created_at=30,
        last_seen=fixed_now - 5_000,
    )
    _save_test_observer(
        "dddd0000",
        "inactive-never",
        created_at=40,
        last_seen=None,
    )

    observers = _api_list_observers(env)
    assert [observer["name"] for observer in observers] == [
        "active-observer",
        "stale-observer",
        "inactive-disconnected",
        "inactive-never",
    ]
    assert [
        (
            observer["state"],
            observer["group"],
            observer["label"],
            observer["elapsed_ms"],
            observer["clock_skew"],
        )
        for observer in observers
    ] == [
        ("connected", "active", OBSERVER_STATE_LABELS["connected"], 5_000, False),
        ("stale", "stale", OBSERVER_STATE_LABELS["stale"], 60_000, False),
        (
            "disconnected",
            "inactive",
            OBSERVER_STATE_LABELS["disconnected"],
            600_000,
            False,
        ),
        (
            "disconnected",
            "inactive",
            OBSERVER_STATE_LABELS["disconnected"],
            None,
            False,
        ),
    ]


def test_api_list_tie_breaks_by_key_prefix(observer_env, monkeypatch):
    """Observers with the same last_seen sort by key_prefix ascending."""
    env = observer_env()
    fixed_now = 3_000_000
    monkeypatch.setattr(routes_module, "now_ms", lambda: fixed_now)

    _save_test_observer(
        "bbbb0000",
        "active-b",
        created_at=10,
        last_seen=fixed_now - 5_000,
    )
    _save_test_observer(
        "aaaa0000",
        "active-a",
        created_at=20,
        last_seen=fixed_now - 5_000,
    )

    observers = _api_list_observers(env)
    assert [observer["key_prefix"] for observer in observers] == [
        "aaaa0000",
        "bbbb0000",
    ]
    assert all(observer["state"] == "connected" for observer in observers)
    assert all(observer["group"] == "active" for observer in observers)
    assert all(
        observer["label"] == OBSERVER_STATE_LABELS["connected"]
        for observer in observers
    )


def test_api_list_revoked_observer_buckets_inactive(observer_env, monkeypatch):
    """Revoked observers sort in the inactive bucket regardless of last_seen."""
    env = observer_env()
    fixed_now = 4_000_000
    monkeypatch.setattr(routes_module, "now_ms", lambda: fixed_now)

    _save_test_observer(
        "bbbb0000",
        "revoked-observer",
        created_at=10,
        last_seen=fixed_now - 1_000,
        revoked=True,
    )
    _save_test_observer(
        "aaaa0000",
        "stale-observer",
        created_at=20,
        last_seen=fixed_now - 60_000,
    )

    observers = _api_list_observers(env)
    assert [observer["name"] for observer in observers] == [
        "stale-observer",
        "revoked-observer",
    ]
    assert observers[0]["state"] == "stale"
    assert observers[0]["group"] == "stale"
    assert observers[0]["label"] == OBSERVER_STATE_LABELS["stale"]
    assert observers[0]["elapsed_ms"] == 60_000
    assert observers[0]["clock_skew"] is False
    assert observers[1]["state"] == "revoked"
    assert observers[1]["group"] == "inactive"
    assert observers[1]["label"] == OBSERVER_STATE_LABELS["revoked"]
    assert observers[1]["elapsed_ms"] is None
    assert observers[1]["clock_skew"] is False


def test_api_list_includes_state_and_group_per_observer(observer_env, monkeypatch):
    """api_list includes freshness state, grouping, label, and skew metadata."""
    env = observer_env()
    fixed_now = 5_000_000
    monkeypatch.setattr(routes_module, "now_ms", lambda: fixed_now)

    _save_test_observer(
        "aaaa0000",
        "active-observer",
        created_at=10,
        last_seen=fixed_now - 5_000,
    )

    observer = _api_list_observers(env)[0]

    assert observer["state"] == "connected"
    assert observer["group"] == "active"
    assert observer["label"] == OBSERVER_STATE_LABELS["connected"]
    assert isinstance(observer["elapsed_ms"], int)
    assert observer["elapsed_ms"] == 5_000
    assert observer["clock_skew"] is False


def test_api_delete_nonexistent(observer_env):
    """Test deleting a nonexistent observer returns 404."""
    env = observer_env()

    resp = env.client.delete("/app/observer/api/nonexistent")
    assert resp.status_code == 404


def test_ingest_invalid_key(observer_env):
    """Test that ingest rejects invalid keys."""
    env = observer_env()

    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": "Bearer invalid-key-12345"},
        data={"day": "20250103", "segment": "120000_300"},
    )
    assert resp.status_code == 401
    assert "Invalid key" in resp.get_json()["detail"]


def test_ingest_missing_segment(observer_env):
    """Test that ingest requires segment."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Upload without segment
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={"day": "20250103"},
    )
    assert resp.status_code == 400
    assert "Missing segment" in resp.get_json()["detail"]


def test_ingest_missing_day(observer_env):
    """Test that ingest requires day."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Upload without day
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={"segment": "120000_300"},
    )
    assert resp.status_code == 400
    assert "Missing day" in resp.get_json()["detail"]


def test_ingest_invalid_segment_format(observer_env):
    """Test that ingest validates segment format."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Invalid segment format
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={"day": "20250103", "segment": "invalid"},
    )
    assert resp.status_code == 400
    assert "Invalid segment format" in resp.get_json()["detail"]


def test_ingest_invalid_day_format(observer_env):
    """Test that ingest validates day format."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Invalid day format
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={"day": "2025-01-03", "segment": "120000_300"},
    )
    assert resp.status_code == 400
    assert "Invalid day format" in resp.get_json()["detail"]


def test_ingest_no_files(observer_env):
    """Test that ingest requires files."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Upload without files
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={"day": "20250103", "segment": "120000_300"},
    )
    assert resp.status_code == 400
    assert "No files uploaded" in resp.get_json()["detail"]


def test_ingest_success(observer_env):
    """Test successful file ingest."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "test-observer"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Upload a file
    test_data = b"test audio content"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "test_audio.flac"),
        },
    )
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "ok"
    assert data["files"] == ["test_audio.flac"]
    assert data["bytes"] == len(test_data)

    # Verify file was written (in stream/segment directory)
    expected_file = _day_dir(env) / "test-observer" / "120000_300" / "test_audio.flac"
    assert expected_file.exists()
    assert expected_file.read_bytes() == test_data


def test_ingest_updates_stats(observer_env):
    """Test that ingest updates observer stats."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "stats-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Upload a file
    test_data = b"test content"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "audio.flac"),
        },
    )
    assert resp.status_code == 200

    # Check stats updated
    observers = _api_list_observers(env)
    assert len(observers) == 1
    assert observers[0]["stats"]["segments_received"] == 1
    assert observers[0]["stats"]["bytes_received"] == len(test_data)
    assert observers[0]["last_segment"] == "120000_300"
    assert observers[0]["last_seen"] is not None


def test_ingest_pl_uses_fingerprint_identity(observer_env):
    env = observer_env()
    prefix = PL_FINGERPRINT.removeprefix("sha256:")[:16]
    mint_pl_observer_record(
        fingerprint=PL_FINGERPRINT,
        device_label="pl-observer",
        paired_at="2026-05-20T00:00:00Z",
    )

    resp = env.client.post(
        "/app/observer/ingest",
        environ_overrides={"pl.identity": _pl_identity()},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(b"pl content"), "audio.flac"),
        },
    )

    assert resp.status_code == 200
    assert (
        env.journal
        / "apps"
        / "observer"
        / "observers"
        / prefix
        / "hist"
        / "20250103.jsonl"
    ).exists()


def test_ingest_event_relay(observer_env):
    """Test event relay endpoint."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "event-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Send an event
    resp = env.client.post(
        "/app/observer/ingest/event",
        headers={"Authorization": f"Bearer {key}"},
        json={"tract": "observe", "event": "status", "mode": "screencast"},
        content_type="application/json",
    )
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "ok"


def test_ingest_event_pl_ignores_url_key(observer_env, monkeypatch):
    env = observer_env()
    other_key = _save_test_observer(
        "deadbeef",
        "other-dl",
        created_at=100,
        last_seen=None,
    )
    mint_pl_observer_record(
        fingerprint=PL_FINGERPRINT,
        device_label="pl-event",
        paired_at="2026-05-20T00:00:00Z",
    )
    emitted: list[tuple[str, str, dict]] = []
    monkeypatch.setattr(
        routes_module,
        "emit",
        lambda tract, event, **kwargs: emitted.append((tract, event, kwargs)),
    )

    resp = env.client.post(
        f"/app/observer/ingest/{other_key[:8]}/event",
        environ_overrides={"pl.identity": _pl_identity()},
        json={"tract": "observe", "event": "status", "mode": "screencast"},
        content_type="application/json",
    )

    assert resp.status_code == 200
    assert resp.get_json()["status"] == "ok"
    assert emitted == [
        (
            "observe",
            "status",
            {"mode": "screencast", "observer": "pl-event"},
        )
    ]


def test_dl_and_pl_observers_coexist_and_ingest(observer_env):
    env = observer_env()
    dl_resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "dl-coexist"},
        content_type="application/json",
    )
    assert dl_resp.status_code == 200
    dl_key = dl_resp.get_json()["key"]
    dl_prefix = dl_key[:8]
    pl_prefix = PL_FINGERPRINT.removeprefix("sha256:")[:16]
    mint_pl_observer_record(
        fingerprint=PL_FINGERPRINT,
        device_label="pl-coexist",
        paired_at="2026-05-20T00:00:00Z",
    )

    dl_upload = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {dl_key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(b"dl content"), "dl.txt"),
        },
    )
    pl_upload = env.client.post(
        "/app/observer/ingest",
        environ_overrides={"pl.identity": _pl_identity()},
        data={
            "day": "20250103",
            "segment": "120500_300",
            "files": (io.BytesIO(b"pl content"), "pl.txt"),
        },
    )

    assert dl_upload.status_code == 200
    assert pl_upload.status_code == 200
    observer_files = {
        path.name
        for path in (env.journal / "apps" / "observer" / "observers").glob("*.json")
    }
    assert f"{dl_prefix}.json" in observer_files
    assert f"{pl_prefix}.json" in observer_files
    assert (
        env.journal
        / "apps"
        / "observer"
        / "observers"
        / dl_prefix
        / "hist"
        / "20250103.jsonl"
    ).exists()
    assert (
        env.journal
        / "apps"
        / "observer"
        / "observers"
        / pl_prefix
        / "hist"
        / "20250103.jsonl"
    ).exists()


def test_ingest_event_pl_phone_identity_without_observer_record_returns_401(
    observer_env,
):
    env = observer_env()

    resp = env.client.post(
        "/app/observer/ingest/event",
        environ_overrides={"pl.identity": _pl_identity(PL_FINGERPRINT_2)},
        json={"tract": "observe", "event": "status"},
        content_type="application/json",
    )

    assert resp.status_code == 401
    assert resp.get_json()["reason_code"] == "auth_required"


def test_ingest_event_missing_tract(observer_env):
    """Test that event relay requires tract."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Missing tract
    resp = env.client.post(
        "/app/observer/ingest/event",
        headers={"Authorization": f"Bearer {key}"},
        json={"event": "status"},
        content_type="application/json",
    )
    assert resp.status_code == 400
    assert "Missing tract or event" in resp.get_json()["detail"]


def test_ingest_revoked_key(observer_env):
    """Test that ingest rejects revoked keys."""
    env = observer_env()

    # Create and revoke a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "revoked-test"},
        content_type="application/json",
    )
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["key_prefix"]

    resp = env.client.delete(f"/app/observer/api/{key_prefix}")
    assert resp.status_code == 200

    # Try to upload - should fail
    test_data = b"test content"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "audio.flac"),
        },
    )
    assert resp.status_code == 403
    assert "Observer revoked" in resp.get_json()["detail"]


def test_ingest_event_revoked_key(observer_env):
    """Test that event relay rejects revoked keys."""
    env = observer_env()

    # Create and revoke a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "revoked-event-test"},
        content_type="application/json",
    )
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["key_prefix"]

    resp = env.client.delete(f"/app/observer/api/{key_prefix}")
    assert resp.status_code == 200

    # Try to send event - should fail
    resp = env.client.post(
        "/app/observer/ingest/event",
        headers={"Authorization": f"Bearer {key}"},
        json={"tract": "observe", "event": "status"},
        content_type="application/json",
    )
    assert resp.status_code == 403
    assert "Observer revoked" in resp.get_json()["detail"]


def test_api_get_key(observer_env):
    """Test retrieving full key for a observer."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "key-test"},
        content_type="application/json",
    )
    create_data = resp.get_json()
    key = create_data["key"]
    key_prefix = create_data["key_prefix"]

    # Get the key
    resp = env.client.get(f"/app/observer/api/{key_prefix}/key")
    assert resp.status_code == 200

    data = resp.get_json()
    assert data["key"] == key
    assert data["name"] == "key-test"
    assert data["ingest_url"] == f"/app/observer/ingest/{key}"


def test_api_get_key_nonexistent(observer_env):
    """Test getting key for nonexistent observer returns 404."""
    env = observer_env()

    resp = env.client.get("/app/observer/api/nonexistent/key")
    assert resp.status_code == 404


def test_api_get_key_revoked(observer_env):
    """Test getting key for revoked observer returns 403."""
    env = observer_env()

    # Create then revoke
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "revoke-key-test"},
        content_type="application/json",
    )
    create_data = resp.get_json()
    key_prefix = create_data["key_prefix"]

    env.client.delete(f"/app/observer/api/{key_prefix}")

    # Try to get the key
    resp = env.client.get(f"/app/observer/api/{key_prefix}/key")
    assert resp.status_code == 403
    assert "revoked" in resp.get_json()["detail"]


def test_api_get_key_audit_log(observer_env):
    """Test that viewing a key logs an audit action."""
    from unittest.mock import patch

    env = observer_env()

    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "audit-test"},
        content_type="application/json",
    )
    create_data = resp.get_json()
    key_prefix = create_data["key_prefix"]

    with patch("solstone.apps.observer.routes.log_app_action") as mock_log:
        resp = env.client.get(f"/app/observer/api/{key_prefix}/key")
        assert resp.status_code == 200

        mock_log.assert_called_once_with(
            app="observer",
            facet=None,
            action="observer_key_view",
            params={"name": "audit-test", "key_prefix": key_prefix},
        )


# === Segment collision helper tests ===


def test_find_available_segment_no_conflict(observer_env):
    """Test find_available_segment returns original when no conflict."""
    from solstone.observe.utils import find_available_segment

    env = observer_env()
    day_dir = _day_dir(env)
    day_dir.mkdir(parents=True)

    result = find_available_segment(day_dir, "120000_300")
    assert result == "120000_300"


def test_find_available_segment_with_conflict(observer_env):
    """Test find_available_segment finds alternative when conflict exists."""
    from solstone.observe.utils import find_available_segment

    env = observer_env()
    day_dir = _day_dir(env)
    day_dir.mkdir(parents=True)

    # Create conflicting segment directory
    (day_dir / "120000_300").mkdir()

    result = find_available_segment(day_dir, "120000_300")

    # Should find a different segment
    assert result is not None
    assert result != "120000_300"
    # Should be a valid segment format
    assert "_" in result
    time_part, dur_part = result.split("_")
    assert len(time_part) == 6
    assert dur_part.isdigit()


def test_find_available_segment_with_limited_attempts(observer_env):
    """Test find_available_segment respects max_attempts limit."""
    from solstone.observe.utils import find_available_segment

    env = observer_env()
    day_dir = _day_dir(env)
    day_dir.mkdir(parents=True)

    # Create conflicting segment directory
    (day_dir / "120000_300").mkdir()

    # With max_attempts=0, should return None immediately (no attempts allowed)
    result = find_available_segment(day_dir, "120000_300", max_attempts=0)
    assert result is None


def test_save_to_failed_creates_directory(observer_env):
    """Test _save_to_failed creates failed directory structure."""
    from solstone.apps.observer.routes import _save_to_failed

    env = observer_env()
    day_dir = _day_dir(env)
    day_dir.mkdir(parents=True)

    # Create mock file_data tuples: (submitted_filename, simple_filename, content, sha256)
    file_data = [
        ("120000_300_audio.flac", "audio.flac", b"audio data", "sha256_audio"),
        ("120000_300_screen.webm", "screen.webm", b"video data", "sha256_video"),
    ]

    failed_dir = _save_to_failed(day_dir, file_data, "120000_300")

    # Verify structure includes segment key
    assert failed_dir.exists()
    assert "observer/failed/120000_300/" in str(failed_dir)
    assert (failed_dir / "120000_300_audio.flac").exists()
    assert (failed_dir / "120000_300_screen.webm").exists()
    # Verify actual content was written
    assert (failed_dir / "120000_300_audio.flac").read_bytes() == b"audio data"
    assert (failed_dir / "120000_300_screen.webm").read_bytes() == b"video data"


# === Integration tests for collision handling ===


def test_ingest_collision_adjusts_segment(observer_env):
    """Test that ingest adjusts segment key on collision."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "collision-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Create a conflicting segment directory under the stream
    day_dir = _day_dir(env)
    stream_dir = day_dir / "collision-test"
    stream_dir.mkdir(parents=True)
    (stream_dir / "120000_300").mkdir()
    (stream_dir / "120000_300" / "audio.flac").write_bytes(b"existing")

    # Upload with same segment key
    test_data = b"new audio content"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "120000_300_audio.flac"),
        },
    )

    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "collision"  # New status indicates adjustment

    # The segment key should have been adjusted, file is stripped of prefix
    saved_file = data["files"][0]
    assert saved_file == "audio.flac"

    # Verify both segments exist
    assert (stream_dir / "120000_300" / "audio.flac").exists()  # Original
    # New one is in adjusted segment directory (not 120000_300)
    adjusted_segments = [
        d for d in stream_dir.iterdir() if d.is_dir() and d.name != "120000_300"
    ]
    assert len(adjusted_segments) == 1
    assert (adjusted_segments[0] / "audio.flac").exists()


def test_ingest_no_collision_preserves_segment(observer_env):
    """Test that ingest preserves segment key when no collision."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "no-collision-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Upload without any conflicting segment directory
    test_data = b"audio content"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "120000_300_audio.flac"),
        },
    )

    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "ok"
    assert data["files"] == ["audio.flac"]  # Segment prefix stripped

    # Verify file saved in stream/segment directory
    expected_file = _day_dir(env) / "no-collision-test" / "120000_300" / "audio.flac"
    assert expected_file.exists()


def test_ingest_stats_use_adjusted_segment(observer_env):
    """Test that observer stats record the adjusted segment key."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "stats-adjust-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Create a conflicting segment directory under the stream
    day_dir = _day_dir(env)
    stream_dir = day_dir / "stats-adjust-test"
    stream_dir.mkdir(parents=True)
    (stream_dir / "120000_300").mkdir()

    # Upload with same segment key
    test_data = b"new audio"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "120000_300_audio.flac"),
        },
    )

    assert resp.status_code == 200

    # Check stats - last_segment should be the adjusted one
    observers = _api_list_observers(env)
    assert len(observers) == 1
    last_segment = observers[0]["last_segment"]
    assert last_segment is not None
    # It should be adjusted (not the original conflicting one)
    assert last_segment != "120000_300"
    # The adjusted segment directory should exist
    assert (stream_dir / last_segment).exists()


# === Sync history tests ===


def test_ingest_creates_sync_history(observer_env):
    """Test that ingest creates sync history record."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "history-test"},
        content_type="application/json",
    )
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["key_prefix"]

    # Upload a file
    test_data = b"test audio content for history"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "120000_300_audio.flac"),
        },
    )
    assert resp.status_code == 200

    # Check history file exists
    hist_path = (
        env.journal
        / "apps"
        / "observer"
        / "observers"
        / key_prefix
        / "hist"
        / "20250103.jsonl"
    )
    assert hist_path.exists()

    # Load and verify history
    with open(hist_path) as f:
        record = json.loads(f.readline())

    assert record["segment"] == "120000_300"
    assert record["stream"] == "history-test"
    assert "segment_original" not in record  # No collision
    assert len(record["files"]) == 1

    file_rec = record["files"][0]
    assert file_rec["submitted"] == "120000_300_audio.flac"
    assert file_rec["written"] == "audio.flac"  # Segment prefix stripped
    assert file_rec["size"] == len(test_data)
    assert len(file_rec["sha256"]) == 64  # SHA256 hex length
    assert file_rec["inode"] > 0


def test_ingest_history_with_collision(observer_env):
    """Test that sync history records collision adjustment."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "collision-history-test"},
        content_type="application/json",
    )
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["key_prefix"]

    # Create conflicting segment directory under the stream
    day_dir = _day_dir(env)
    stream_dir = day_dir / "collision-history-test"
    stream_dir.mkdir(parents=True)
    (stream_dir / "120000_300").mkdir()

    # Upload with same segment key
    test_data = b"new audio content"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "120000_300_audio.flac"),
        },
    )
    assert resp.status_code == 200

    # Load history
    hist_path = (
        env.journal
        / "apps"
        / "observer"
        / "observers"
        / key_prefix
        / "hist"
        / "20250103.jsonl"
    )
    with open(hist_path) as f:
        record = json.loads(f.readline())

    # Should record original segment
    assert record["segment_original"] == "120000_300"
    assert record["segment"] != "120000_300"

    # File names should reflect stripping of segment prefix
    file_rec = record["files"][0]
    assert file_rec["submitted"] == "120000_300_audio.flac"
    assert file_rec["written"] == "audio.flac"  # Segment prefix stripped


def test_segments_endpoint_empty(observer_env):
    """Test segments endpoint returns empty for no uploads."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "segments-empty-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Query segments - should be empty
    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": f"Bearer {key}"},
    )
    assert resp.status_code == 200
    assert resp.get_json() == []


def test_segments_endpoint_invalid_key(observer_env):
    """Test segments endpoint rejects invalid key."""
    env = observer_env()

    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": "Bearer invalid-key"},
    )
    assert resp.status_code == 401


def test_segments_endpoint_invalid_day(observer_env):
    """Test segments endpoint validates day format."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "segments-day-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    resp = env.client.get(
        "/app/observer/ingest/segments/2025-01-03",
        headers={"Authorization": f"Bearer {key}"},
    )
    assert resp.status_code == 400
    assert "Invalid day format" in resp.get_json()["detail"]


def test_segments_endpoint_lists_uploads(observer_env):
    """Test segments endpoint lists uploaded segments."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "segments-list-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Upload a file
    test_data = b"test audio content"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "120000_300_audio.flac"),
        },
    )
    assert resp.status_code == 200

    # Query segments
    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": f"Bearer {key}"},
    )
    assert resp.status_code == 200
    data = resp.get_json()

    assert len(data) == 1
    segment = data[0]
    assert segment["key"] == "120000_300"
    assert segment["observed"] is False  # Not yet processed
    assert "original_key" not in segment  # No collision
    assert len(segment["files"]) == 1

    file_info = segment["files"][0]
    assert file_info["name"] == "audio.flac"  # Segment prefix stripped
    assert file_info["size"] == len(test_data)
    assert len(file_info["sha256"]) == 64
    assert file_info["status"] == "present"
    assert (
        file_info["submitted_name"] == "120000_300_audio.flac"
    )  # Original name preserved


def test_segments_endpoint_shows_collision(observer_env):
    """Test segments endpoint shows collision info."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "segments-collision-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Create conflicting segment directory under the stream
    day_dir = _day_dir(env)
    stream_dir = day_dir / "segments-collision-test"
    stream_dir.mkdir(parents=True)
    (stream_dir / "120000_300").mkdir()

    # Upload with collision
    test_data = b"new audio"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "120000_300_audio.flac"),
        },
    )
    assert resp.status_code == 200

    # Query segments
    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": f"Bearer {key}"},
    )
    data = resp.get_json()

    assert len(data) == 1
    segment = data[0]
    assert segment["key"] != "120000_300"
    assert segment["original_key"] == "120000_300"

    file_info = segment["files"][0]
    assert file_info["submitted_name"] == "120000_300_audio.flac"
    assert file_info["name"] == "audio.flac"  # Segment prefix stripped
    assert file_info["status"] == "present"


def test_segments_endpoint_missing_file(observer_env):
    """Test segments endpoint reports missing files."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "segments-missing-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Upload a file
    test_data = b"test audio"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "120000_300_audio.flac"),
        },
    )
    assert resp.status_code == 200

    # Delete the file (now in stream/segment directory with stripped name)
    (_day_dir(env) / "segments-missing-test" / "120000_300" / "audio.flac").unlink()

    # Query segments
    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": f"Bearer {key}"},
    )
    data = resp.get_json()

    assert len(data) == 1
    file_info = data[0]["files"][0]
    assert file_info["status"] == "missing"


def test_segments_endpoint_relocated_file(observer_env):
    """Test segments endpoint detects relocated files by inode."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "segments-relocate-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Upload a file
    test_data = b"test audio for relocation"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "120000_300_audio.flac"),
        },
    )
    assert resp.status_code == 200

    # Move the file to a different name (simulating some file reorganization)
    day_dir = _day_dir(env)
    segment_dir = day_dir / "segments-relocate-test" / "120000_300"
    original_path = segment_dir / "audio.flac"
    new_path = segment_dir / "renamed_audio.flac"
    original_path.rename(new_path)

    # Query segments - should detect relocation by inode
    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": f"Bearer {key}"},
    )
    data = resp.get_json()

    assert len(data) == 1
    file_info = data[0]["files"][0]
    assert file_info["status"] == "relocated"
    assert (
        file_info["current_path"]
        == "segments-relocate-test/120000_300/renamed_audio.flac"
    )


def test_find_by_inode(observer_env):
    """Test _find_by_inode helper."""
    from solstone.apps.observer.routes import _find_by_inode

    env = observer_env()
    day_dir = _day_dir(env)
    day_dir.mkdir(parents=True)

    # Create a file and get its inode
    test_file = day_dir / "test.txt"
    test_file.write_bytes(b"hello")
    inode = test_file.stat().st_ino

    # Should find it at original location
    found = _find_by_inode(day_dir, inode)
    assert found == test_file

    # Move to subdirectory
    subdir = day_dir / "subdir"
    subdir.mkdir()
    new_path = subdir / "renamed.txt"
    test_file.rename(new_path)

    # Should still find by inode
    found = _find_by_inode(day_dir, inode)
    assert found == new_path

    # Non-existent inode returns None
    found = _find_by_inode(day_dir, 999999999)
    assert found is None


def test_segments_endpoint_revoked_key(observer_env):
    """Test segments endpoint rejects revoked key."""
    env = observer_env()

    # Create and revoke a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "segments-revoked-test"},
        content_type="application/json",
    )
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["key_prefix"]

    env.client.delete(f"/app/observer/api/{key_prefix}")

    # Query segments - should be rejected
    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": f"Bearer {key}"},
    )
    assert resp.status_code == 403
    assert "Observer revoked" in resp.get_json()["detail"]


def test_segments_endpoint_deduplicates_by_sha256(observer_env):
    """Test that duplicate file uploads are rejected (not duplicated on disk).

    With duplicate detection enabled, re-uploading the same content returns
    status='duplicate' and the segment is not written again.
    """
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "segments-dedup-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Upload a file
    test_data = b"test audio content"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "120000_300_audio.flac"),
        },
    )
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "ok"

    # Upload the same file again (same content = same sha256)
    # With duplicate detection, this should be rejected
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "120000_300_audio.flac"),
        },
    )
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "duplicate"

    # Query segments - should have only one segment (duplicate was rejected)
    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": f"Bearer {key}"},
    )
    data = resp.get_json()

    # Should have 1 segment (duplicate rejected, not 2 segments)
    assert len(data) == 1
    assert data[0]["key"] == "120000_300"
    assert len(data[0]["files"]) == 1
    assert data[0]["files"][0]["status"] == "present"


def test_segments_endpoint_shows_observed_status(observer_env):
    """Test that segments endpoint includes observed status."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "observed-test"},
        content_type="application/json",
    )
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["key_prefix"]

    # Upload a file
    test_data = b"test audio content"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "120000_300_audio.flac"),
        },
    )
    assert resp.status_code == 200

    # Query segments - should show observed: false
    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": f"Bearer {key}"},
    )
    data = resp.get_json()
    assert len(data) == 1
    assert data[0]["observed"] is False

    # Manually add an observed record to simulate event handler
    hist_dir = env.journal / "apps" / "observer" / "observers" / key_prefix / "hist"
    hist_dir.mkdir(parents=True, exist_ok=True)
    hist_path = hist_dir / "20250103.jsonl"
    with open(hist_path, "a") as f:
        f.write('{"ts": 1704312345000, "type": "observed", "segment": "120000_300"}\n')

    # Query again - should now show observed: true
    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": f"Bearer {key}"},
    )
    data = resp.get_json()
    assert len(data) == 1
    assert data[0]["observed"] is True


def test_api_list_includes_segments_observed_stat(observer_env):
    """Test that api_list includes segments_observed stat."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "stats-test"},
        content_type="application/json",
    )
    data = resp.get_json()
    key_prefix = data["key_prefix"]

    # Initially no segments_observed
    data = _api_list_observers(env)
    assert len(data) == 1
    assert "segments_observed" not in data[0]["stats"]

    # Manually add segments_observed stat
    observer_path = (
        env.journal / "apps" / "observer" / "observers" / f"{key_prefix}.json"
    )
    with open(observer_path) as f:
        observer_data = json.load(f)
    observer_data["stats"]["segments_observed"] = 5
    with open(observer_path, "w") as f:
        json.dump(observer_data, f)

    # Should now show in list
    data = _api_list_observers(env)
    assert data[0]["stats"]["segments_observed"] == 5


# === Duplicate detection tests ===


def test_ingest_duplicate_segment_returns_duplicate_status(observer_env):
    """Test that re-submitting identical files returns duplicate status."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "duplicate-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # First upload
    test_data = b"test audio content for duplicate test"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "audio.flac"),
        },
    )
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "ok"
    first_segment = data["segment"]

    # Second upload with identical content
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "audio.flac"),
        },
    )
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "duplicate"
    assert data["existing_segment"] == first_segment
    assert "message" in data


def test_ingest_duplicate_does_not_emit_event(observer_env, monkeypatch):
    """Test that duplicate submission does not emit observe.observing event."""
    from unittest.mock import MagicMock

    env = observer_env()

    # Mock emit
    import solstone.apps.observer.routes as routes_module

    emit_mock = MagicMock()
    monkeypatch.setattr(routes_module, "emit", emit_mock)

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "no-event-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    test_data = b"test audio for event test"

    # First upload - should emit
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "audio.flac"),
        },
    )
    assert resp.status_code == 200
    assert emit_mock.call_count == 1

    # Second upload - should NOT emit
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "audio.flac"),
        },
    )
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "duplicate"
    assert emit_mock.call_count == 1  # No new emit


def test_ingest_duplicate_increments_duplicates_rejected_stat(observer_env):
    """Test that duplicate submission increments duplicates_rejected stat."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "dup-stat-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    test_data = b"test audio for stat test"

    # First upload
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "audio.flac"),
        },
    )
    assert resp.status_code == 200

    # Check stats - no duplicates_rejected yet
    stats = _api_list_observers(env)[0]["stats"]
    assert stats.get("duplicates_rejected", 0) == 0

    # Submit duplicate
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "audio.flac"),
        },
    )
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "duplicate"

    # Check stats - should have 1 duplicate rejected
    stats = _api_list_observers(env)[0]["stats"]
    assert stats["duplicates_rejected"] == 1


def test_ingest_partial_duplicate_creates_new_segment(observer_env):
    """Test that partial duplicate (some files match) creates new segment."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "partial-dup-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    audio_data = b"test audio content"
    screen_data = b"test screen content"
    new_screen_data = b"different screen content"

    # First upload with audio and screen
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
        },
        content_type="multipart/form-data",
    )
    # Add files manually for multipart
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio_data), "audio.flac"),
                (io.BytesIO(screen_data), "screen.mp4"),
            ],
        },
    )
    assert resp.status_code == 200
    first_data = resp.get_json()
    assert first_data["status"] == "ok"
    first_segment = first_data["segment"]

    # Second upload with same audio but different screen
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio_data), "audio.flac"),
                (io.BytesIO(new_screen_data), "screen.mp4"),
            ],
        },
    )
    assert resp.status_code == 200
    second_data = resp.get_json()
    # Should be collision (new segment) not duplicate
    assert second_data["status"] in ("ok", "collision")
    # Should be a different segment (collision resolution)
    assert second_data["segment"] != first_segment


def test_ingest_partial_match_logged_in_history(observer_env):
    """Test that partial SHA256 matches are logged in history record."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "partial-log-test"},
        content_type="application/json",
    )
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["key_prefix"]

    audio_data = b"test audio for partial log"

    # First upload
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(audio_data), "audio.flac"),
        },
    )
    assert resp.status_code == 200

    # Second upload with same audio but new additional file
    new_data = b"brand new file"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio_data), "audio.flac"),
                (io.BytesIO(new_data), "new_file.txt"),
            ],
        },
    )
    assert resp.status_code == 200

    # Load history and check for partial_match_sha256s in latest record
    hist_path = (
        env.journal
        / "apps"
        / "observer"
        / "observers"
        / key_prefix
        / "hist"
        / "20250103.jsonl"
    )
    with open(hist_path) as f:
        records = [json.loads(line) for line in f if line.strip()]

    # Should have 2 upload records
    upload_records = [r for r in records if "type" not in r]
    assert len(upload_records) == 2

    # The second record should have partial_match_sha256s
    assert "partial_match_sha256s" in upload_records[1]
    assert len(upload_records[1]["partial_match_sha256s"]) == 1


def test_ingest_returns_collision_status_when_adjusted(observer_env):
    """Test that collision resolution returns status='collision'."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "collision-status-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Create existing segment directory under the stream
    day_dir = _day_dir(env)
    stream_dir = day_dir / "collision-status-test"
    stream_dir.mkdir(parents=True)
    (stream_dir / "120000_300").mkdir()
    (stream_dir / "120000_300" / "existing.txt").write_bytes(b"existing content")

    # Upload - will need collision resolution
    test_data = b"new content"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(test_data), "audio.flac"),
        },
    )
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "collision"
    assert data["segment"] != "120000_300"  # Adjusted


def test_ingest_zero_byte_file_rejected(observer_env):
    """Test that uploading only 0-byte files returns 400."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "test-observer"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Upload a 0-byte file
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(b""), "empty_audio.flac"),
        },
    )
    assert resp.status_code == 400
    assert "No valid files" in resp.get_json()["detail"]


def test_ingest_mixed_zero_byte_files(observer_env):
    """Test that 0-byte files are skipped but valid files are accepted."""
    env = observer_env()

    # Create a observer
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "test-observer"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    # Upload one valid file and one 0-byte file
    valid_data = b"real audio content"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(b""), "empty.flac"),
                (io.BytesIO(valid_data), "audio.flac"),
            ],
        },
    )
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "ok"
    assert data["files"] == ["audio.flac"]
    assert data["bytes"] == len(valid_data)

    # Verify only valid file was written
    expected_file = _day_dir(env) / "test-observer" / "120000_300" / "audio.flac"
    assert expected_file.exists()
    assert expected_file.read_bytes() == valid_data


def test_ingest_stream_qualifier_preserved(observer_env):
    """Regression: tmux observer must land in host.tmux, not host stream.

    When a client registers as "fedora.tmux" and uploads with
    meta={"stream": "fedora.tmux"}, the server was calling
    stream_name(observer="fedora.tmux") which strips the qualifier via
    _strip_hostname, collapsing both desktop and tmux observers into
    the same "fedora" stream.  The fix: trust meta["stream"] when present.
    """
    env = observer_env()

    # Register as the tmux observer would (name = stream name with qualifier)
    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "fedora.tmux"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    test_data = b"tmux capture content"
    meta = json.dumps({"host": "fedora", "platform": "linux", "stream": "fedora.tmux"})
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "meta": meta,
            "files": (io.BytesIO(test_data), "tmux.jsonl"),
        },
    )
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "ok"

    # Must land under fedora.tmux/, NOT fedora/
    assert (_day_dir(env) / "fedora.tmux" / "120000_300" / "tmux.jsonl").exists()
    assert not (_day_dir(env) / "fedora" / "120000_300" / "tmux.jsonl").exists()


def test_transfer_success(observer_env):
    """Test successful transfer upload."""
    env = observer_env()

    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "transfer-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    test_data = b"transferred audio content"
    resp = env.client.post(
        f"/app/observer/ingest/{key}/transfer",
        data={
            "day": "20250103",
            "segment": "120000_300",
            "stream": "remote.host",
            "files": (io.BytesIO(test_data), "audio.flac"),
        },
    )
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "ok"
    assert data["segment"] == "120000_300"
    assert data["files"] == ["audio.flac"]
    assert data["bytes"] == len(test_data)

    expected_file = _day_dir(env) / "remote.host" / "120000_300" / "audio.flac"
    assert expected_file.exists()
    assert expected_file.read_bytes() == test_data


def test_transfer_requires_stream(observer_env):
    """Test that transfer requires stream."""
    env = observer_env()

    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "transfer-stream-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    resp = env.client.post(
        f"/app/observer/ingest/{key}/transfer",
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(b"content"), "audio.flac"),
        },
    )
    assert resp.status_code == 400
    assert resp.get_json()["detail"] == "Missing stream"


def test_transfer_invalid_stream(observer_env):
    """Test that transfer validates stream format."""
    env = observer_env()

    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "transfer-invalid-stream"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    resp = env.client.post(
        f"/app/observer/ingest/{key}/transfer",
        data={
            "day": "20250103",
            "segment": "120000_300",
            "stream": "INVALID!",
            "files": (io.BytesIO(b"content"), "audio.flac"),
        },
    )
    assert resp.status_code == 400
    assert resp.get_json()["detail"] == "Invalid stream format"


def test_transfer_duplicate_detection(observer_env):
    """Test transfer duplicate detection."""
    env = observer_env()

    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "transfer-duplicate-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    test_data = b"duplicate transfer content"
    resp = env.client.post(
        f"/app/observer/ingest/{key}/transfer",
        data={
            "day": "20250103",
            "segment": "120000_300",
            "stream": "remote.host",
            "files": (io.BytesIO(test_data), "audio.flac"),
        },
    )
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "ok"

    resp = env.client.post(
        f"/app/observer/ingest/{key}/transfer",
        data={
            "day": "20250103",
            "segment": "120000_300",
            "stream": "remote.host",
            "files": (io.BytesIO(test_data), "audio.flac"),
        },
    )
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "duplicate"
    assert data["existing_segment"] == "120000_300"


def test_transfer_deconfliction(observer_env):
    """Test transfer deconflicts existing segment directories."""
    env = observer_env()

    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "transfer-collision-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    stream_dir = _day_dir(env) / "remote.host"
    stream_dir.mkdir(parents=True)
    (stream_dir / "120000_300").mkdir()

    resp = env.client.post(
        f"/app/observer/ingest/{key}/transfer",
        data={
            "day": "20250103",
            "segment": "120000_300",
            "stream": "remote.host",
            "files": (io.BytesIO(b"collision content"), "audio.flac"),
        },
    )
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "collision"
    assert data["segment"] != "120000_300"
    assert (stream_dir / data["segment"] / "audio.flac").exists()


def test_transfer_emits_transferred_event(observer_env, monkeypatch):
    """Test transfer emits observe.transferred."""
    env = observer_env()

    import solstone.apps.observer.routes as routes_module

    calls = []

    def mock_emit(*args, **kwargs):
        calls.append((args, kwargs))

    monkeypatch.setattr(routes_module, "emit", mock_emit)

    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "transfer-event-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    resp = env.client.post(
        f"/app/observer/ingest/{key}/transfer",
        data={
            "day": "20250103",
            "segment": "120000_300",
            "stream": "remote.host",
            "files": (io.BytesIO(b"event content"), "audio.flac"),
        },
    )
    assert resp.status_code == 200
    assert len(calls) == 1
    assert calls[0][0] == ("observe", "transferred")


def test_transfer_does_not_emit_observing(observer_env, monkeypatch):
    """Test transfer does not emit observe.observing."""
    env = observer_env()

    import solstone.apps.observer.routes as routes_module

    calls = []

    def mock_emit(*args, **kwargs):
        calls.append((args, kwargs))

    monkeypatch.setattr(routes_module, "emit", mock_emit)

    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "transfer-no-observing-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    resp = env.client.post(
        f"/app/observer/ingest/{key}/transfer",
        data={
            "day": "20250103",
            "segment": "120000_300",
            "stream": "remote.host",
            "files": (io.BytesIO(b"event content"), "audio.flac"),
        },
    )
    assert resp.status_code == 200
    assert all(args[1] != "observing" for args, _kwargs in calls)


def test_transfer_history_record(observer_env):
    """Test transfer upload history records source='transfer'."""
    from solstone.apps.observer.utils import load_history

    env = observer_env()

    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "transfer-history-test"},
        content_type="application/json",
    )
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["key_prefix"]

    resp = env.client.post(
        f"/app/observer/ingest/{key}/transfer",
        data={
            "day": "20250103",
            "segment": "120000_300",
            "stream": "remote.host",
            "files": (io.BytesIO(b"history content"), "audio.flac"),
        },
    )
    assert resp.status_code == 200

    records = load_history(key_prefix, "20250103")
    upload_record = next(record for record in records if not record.get("type"))
    assert upload_record["source"] == "transfer"


def test_transfer_auth_required(observer_env):
    """Test transfer rejects invalid path key without auth header."""
    env = observer_env()

    resp = env.client.post(
        "/app/observer/ingest/badkey/transfer",
        data={
            "day": "20250103",
            "segment": "120000_300",
            "stream": "remote.host",
            "files": (io.BytesIO(b"content"), "audio.flac"),
        },
    )
    assert resp.status_code == 401


def test_transfer_invalid_key(observer_env):
    """Test transfer rejects invalid key."""
    env = observer_env()

    resp = env.client.post(
        "/app/observer/ingest/not-a-real-key/transfer",
        data={
            "day": "20250103",
            "segment": "120000_300",
            "stream": "remote.host",
            "files": (io.BytesIO(b"content"), "audio.flac"),
        },
    )
    assert resp.status_code == 401


def test_manifest_day_listing(observer_env):
    """Test manifest day listing from observer history."""
    env = observer_env()

    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "manifest-list-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(b"manifest content"), "audio.flac"),
        },
    )
    assert resp.status_code == 200

    resp = env.client.get(f"/app/observer/ingest/{key}/manifest")
    assert resp.status_code == 200
    assert resp.get_json() == {"days": {"20250103": {"segments": 1}}}


def test_manifest_per_day(observer_env):
    """Test per-day manifest format matches transfer manifest v1."""
    env = observer_env()

    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "manifest-day-test"},
        content_type="application/json",
    )
    key = resp.get_json()["key"]

    resp = env.client.post(
        f"/app/observer/ingest/{key}/transfer",
        data={
            "day": "20250103",
            "segment": "120000_300",
            "stream": "remote.host",
            "files": [
                (io.BytesIO(b"audio bytes"), "audio.flac"),
                (io.BytesIO(b"screen bytes"), "screen.webm"),
            ],
        },
    )
    assert resp.status_code == 200

    resp = env.client.get(f"/app/observer/ingest/{key}/manifest/20250103")
    assert resp.status_code == 200
    data = resp.get_json()

    assert data["version"] == 1
    assert data["day"] == "20250103"
    assert isinstance(data["created_at"], int)
    assert "host" in data
    assert "remote.host/120000_300" in data["segments"]

    files = data["segments"]["remote.host/120000_300"]["files"]
    assert len(files) == 2
    for file_info in files:
        assert set(file_info) == {"name", "sha256", "size"}
        assert len(file_info["sha256"]) == 64


def test_manifest_auth_required(observer_env):
    """Test manifest endpoint rejects invalid key."""
    env = observer_env()

    resp = env.client.get("/app/observer/ingest/badkey/manifest")
    assert resp.status_code == 401
