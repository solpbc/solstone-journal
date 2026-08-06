# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for observer app routes."""

from __future__ import annotations

import ast
import hashlib
import io
import json
import os
import shutil
import threading
import time
from pathlib import Path
from unittest.mock import MagicMock

import pytest

import solstone.apps.observer.routes as routes_module
import solstone.apps.observer.utils as observer_utils
import solstone.convey.bridge as convey_bridge
import solstone.observe.observer_cli as observer_cli_module
from solstone.apps.observer.routes import (
    ACTIVE_THRESHOLD_MS,
    FUTURE_CLOCK_DRIFT_TOLERANCE_MS,
    OBSERVER_STATE_LABELS,
    STALE_THRESHOLD_MS,
    _classify_observer_freshness,
)
from solstone.apps.observer.utils import (
    append_history_record,
    list_observers,
    load_history,
    sanitize_validation_summary,
    save_observer,
)
from solstone.convey.copy import OBSERVER_CALLOSUM_LIVE_LABEL
from solstone.convey.secure_listener import ConveyIdentity
from solstone.convey.sol_initiated.copy import KIND_SOL_CHAT_REQUEST
from solstone.observe.processing_record import (
    HANDLER_DESCRIBE,
    HANDLER_TRANSCRIBE,
    SCHEMA,
    STATE_EMPTY,
    STATE_FAILED,
)
from solstone.observe.protocol import OBSERVER_HANDLE_HEADER, OBSERVER_PROTOCOL_VERSION
from solstone.think.contract.journal import ContractIssue
from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.paths import authorized_clients_path
from solstone.think.streams import update_stream, write_segment_stream

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


def test_observer_state_labels_use_device_copy():
    assert OBSERVER_STATE_LABELS == {
        "connected": "connected",
        "stale": "not reporting",
        "disconnected": "offline",
        "revoked": "removed",
    }


def test_observer_index_serves_injected_spa_shell(observer_env):
    env = observer_env()

    response = env.client.get("/app/observer/")

    assert response.status_code == 200
    assert b'data-solstone-shell="spa"' in response.data


def _day_dir(env, day: str = "20250103"):
    return env.journal / "chronicle" / day


def _create_observer(env, name: str) -> str:
    resp = env.register_bound_observer(name)
    assert resp.status_code == 200
    return resp.get_json()["key"]


def _upload_audio(
    env,
    key: str,
    content: bytes,
    *,
    day: str = "20250103",
    segment: str = "120000_300",
    filename: str = "audio.flac",
):
    return env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": day,
            "segment": segment,
            "files": (io.BytesIO(content), filename),
        },
    )


def _listed_file_info(env, key: str, *, day: str = "20250103") -> dict:
    resp = env.client.get(
        f"/app/observer/ingest/segments/{day}",
        headers={"Authorization": f"Bearer {key}"},
    )
    assert resp.status_code == 200
    data = resp.get_json()
    assert len(data) == 1
    return data[0]["files"][0]


def _observer_record() -> dict:
    observers = list_observers()
    assert len(observers) == 1
    return observers[0]


def _string_constant(node: ast.AST) -> str | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    return None


def _dict_literal_keys(node: ast.Dict) -> set[str]:
    return {
        value
        for key_node in node.keys
        if (value := _string_constant(key_node)) is not None
    }


def _expression_identity(node: ast.AST) -> str:
    return ast.dump(node, include_attributes=False)


def _expression_label(node: ast.AST) -> str:
    try:
        return ast.unparse(node)
    except Exception:
        return _expression_identity(node)


def _body_nodes(body: list[ast.stmt]) -> list[ast.AST]:
    nodes: list[ast.AST] = []
    for stmt in body:
        if isinstance(stmt, (ast.AsyncFunctionDef, ast.ClassDef, ast.FunctionDef)):
            continue
        nodes.extend(ast.walk(stmt))
    return nodes


def _record_write(
    writes: dict[str, dict[str, list[tuple[int, str]]]],
    *,
    key: str,
    object_node: ast.AST,
    lineno: int,
) -> None:
    writes.setdefault(key, {}).setdefault(_expression_identity(object_node), []).append(
        (lineno, _expression_label(object_node))
    )


def _collect_last_segment_writes(
    function: ast.AsyncFunctionDef | ast.FunctionDef,
) -> dict[str, dict[str, list[tuple[int, str]]]]:
    writes: dict[str, dict[str, list[tuple[int, str]]]] = {}
    for node in _body_nodes(function.body):
        targets: list[ast.AST] = []
        if isinstance(node, ast.Assign):
            targets = list(node.targets)
        elif isinstance(node, (ast.AnnAssign, ast.AugAssign)):
            targets = [node.target]

        for target in targets:
            if not isinstance(target, ast.Subscript):
                continue
            key = _string_constant(target.slice)
            if key in {"last_segment", "last_segment_received_at"}:
                _record_write(
                    writes,
                    key=key,
                    object_node=target.value,
                    lineno=target.lineno,
                )

        if not (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and node.func.attr == "update"
        ):
            continue
        for arg in node.args:
            if not isinstance(arg, ast.Dict):
                continue
            keys = _dict_literal_keys(arg)
            for key in ("last_segment", "last_segment_received_at"):
                if key in keys:
                    _record_write(
                        writes,
                        key=key,
                        object_node=node.func.value,
                        lineno=node.lineno,
                    )
    return writes


def _assert_observer_data_creation_sites_are_coupled(
    function: ast.AsyncFunctionDef | ast.FunctionDef,
    *,
    source_name: str,
) -> None:
    for node in _body_nodes(function.body):
        if not isinstance(node, ast.Assign) or not isinstance(node.value, ast.Dict):
            continue
        if not any(
            isinstance(target, ast.Name) and target.id == "observer_data"
            for target in node.targets
        ):
            continue
        keys = _dict_literal_keys(node.value)
        if "last_segment" in keys:
            assert "last_segment_received_at" in keys, (
                f"{source_name}:{node.lineno}: observer_data last_segment creation "
                "must include last_segment_received_at"
            )


def _assert_last_segment_source_writes_are_coupled(
    source: str,
    *,
    source_name: str,
) -> None:
    tree = ast.parse(source, filename=source_name)
    functions = [
        node
        for node in ast.walk(tree)
        if isinstance(node, (ast.AsyncFunctionDef, ast.FunctionDef))
    ]
    for function in functions:
        _assert_observer_data_creation_sites_are_coupled(
            function,
            source_name=source_name,
        )
        writes = _collect_last_segment_writes(function)
        received_at_writes = set(writes.get("last_segment_received_at", {}))
        for object_id, locations in writes.get("last_segment", {}).items():
            if object_id in received_at_writes:
                continue
            lineno, label = locations[0]
            raise AssertionError(
                f"{source_name}:{lineno}: {label} last_segment write must also "
                "write last_segment_received_at in the same function"
            )


def _assert_last_segment_writes_are_coupled(path: Path) -> None:
    _assert_last_segment_source_writes_are_coupled(
        path.read_text(encoding="utf-8"),
        source_name=str(path),
    )


def _post_invalid_contract_audio(env, key: str):
    invalid_audio = b'{"raw":"audio.flac"}\n{"start":"00:00:00"}\n'
    return env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(invalid_audio), "120000_300_audio.jsonl"),
        },
    )


def _valid_contract_files() -> list[tuple[io.BytesIO, str]]:
    audio = b'{"raw":"audio.flac"}\n{"start":"00:00:00","text":"hello"}\n'
    screen = b'{"raw":"screen.mp4","qualified_count":1}\n{"timestamp":1.0}\n'
    stream = (
        b'{"stream":"contract-valid-test","prev_day":null,'
        b'"prev_segment":null,"seq":1}\n'
    )
    return [
        (io.BytesIO(audio), "120000_300_audio.jsonl"),
        (io.BytesIO(screen), "screen.jsonl"),
        (io.BytesIO(stream), "stream.json"),
    ]


def _post_valid_contract_triple(env, key: str):
    return env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": _valid_contract_files(),
        },
    )


def _post_raw_audio_screen_bundle(
    env,
    key: str,
    stream: str,
    audio_content: bytes,
    screen_content: bytes,
    *,
    day: str = "20250103",
    segment: str = "120000_300",
):
    stream_marker = json.dumps(
        {"stream": stream, "prev_day": None, "prev_segment": None, "seq": 1}
    ).encode("utf-8")
    return env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": day,
            "segment": segment,
            "files": [
                (io.BytesIO(audio_content), "audio.flac"),
                (io.BytesIO(screen_content), "screen.mp4"),
                (io.BytesIO(stream_marker + b"\n"), "stream.json"),
            ],
        },
    )


def _write_audio_processing_sidecar(
    segment_dir: Path,
    *,
    input_size: object,
    schema: object = SCHEMA,
    state: str = STATE_EMPTY,
    handler: str = HANDLER_TRANSCRIBE,
    include_schema: bool = True,
    include_input_size: bool = True,
) -> None:
    record = {
        "state": state,
        "handler": handler,
    }
    if include_schema:
        record["schema"] = schema
    if include_input_size:
        record["input_size"] = input_size
    row = {"raw": "audio.flac", "_solstone_processing": record}
    (segment_dir / "audio.jsonl").write_text(json.dumps(row) + "\n", encoding="utf-8")


def _write_screen_processing_sidecar(
    segment_dir: Path,
    *,
    input_size: object,
    state: str = STATE_EMPTY,
) -> None:
    record = {
        "schema": SCHEMA,
        "state": state,
        "handler": HANDLER_DESCRIBE,
        "input_size": input_size,
    }
    row = {"raw": "screen.mp4", "_solstone_processing": record}
    (segment_dir / "screen.jsonl").write_text(json.dumps(row) + "\n", encoding="utf-8")


def _listed_files_by_name(env, key: str, *, day: str = "20250103") -> dict[str, dict]:
    resp = env.client.get(
        f"/app/observer/ingest/segments/{day}",
        headers={"Authorization": f"Bearer {key}"},
    )
    assert resp.status_code == 200
    data = resp.get_json()
    assert len(data) == 1
    return {file_info["name"]: file_info for file_info in data[0]["files"]}


def _history_line_count(env, key_prefix: str, *, day: str = "20250103") -> int:
    hist_path = (
        env.journal
        / "apps"
        / "observer"
        / "observers"
        / key_prefix
        / "hist"
        / f"{day}.jsonl"
    )
    if not hist_path.exists():
        return 0
    return len(hist_path.read_text(encoding="utf-8").splitlines())


def _prepare_legacy_processed_segment(
    env,
    key: str,
    stream: str,
    audio_content: bytes,
    screen_content: bytes,
    *,
    day: str = "20250103",
    segment: str = "120000_300",
) -> Path:
    resp = _post_raw_audio_screen_bundle(
        env,
        key,
        stream,
        audio_content,
        screen_content,
        day=day,
        segment=segment,
    )
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "ok"
    segment_dir = _day_dir(env, day) / stream / segment
    _write_audio_processing_sidecar(segment_dir, input_size=len(audio_content))
    _write_screen_processing_sidecar(segment_dir, input_size=len(screen_content))
    (segment_dir / "audio.flac").unlink()
    (segment_dir / "screen.mp4").unlink()
    (segment_dir / "ingest.json").unlink()
    return segment_dir


def _plant_source_segment(
    env,
    *,
    day: str = "20250103",
    stream: str = "import.share",
    segment: str = "120000_300",
):
    seg_dir = _day_dir(env, day) / stream / segment
    seg_dir.mkdir(parents=True)
    (seg_dir / "doc.pdf").write_bytes(b"pdf")
    (seg_dir / "doc.jsonl").write_text('{"text": "derived"}\n', encoding="utf-8")
    (seg_dir / "item.json").write_text("{}\n", encoding="utf-8")
    state = update_stream(stream, day, segment, type="import")
    write_segment_stream(
        seg_dir,
        stream,
        state["prev_day"],
        state["prev_segment"],
        state["seq"],
    )
    return seg_dir


def _plant_location_segment(
    env,
    *,
    day: str = "20250103",
    segment: str = "120000_300",
):
    seg_dir = _day_dir(env, day) / "location" / segment
    seg_dir.mkdir(parents=True)
    (seg_dir / "location.jsonl").write_text(
        '{"lat": 12.34, "lon": 56.78}\n',
        encoding="utf-8",
    )
    state = update_stream("location", day, segment, type="observer")
    write_segment_stream(
        seg_dir,
        "location",
        state["prev_day"],
        state["prev_segment"],
        state["seq"],
    )
    return seg_dir


def _save_test_observer(
    key_prefix: str,
    name: str,
    *,
    created_at: int,
    last_seen: int | None,
    revoked: bool = False,
    enabled: bool = True,
    health: dict | None = None,
):
    key = key_prefix + ("f" * 56)
    record = {
        "key": key,
        "name": name,
        "created_at": created_at,
        "last_seen": last_seen,
        "last_segment": None,
        "enabled": enabled,
        "revoked": revoked,
        "revoked_at": created_at + 1 if revoked else None,
        "stats": {
            "segments_received": 0,
            "bytes_received": 0,
        },
    }
    if health is not None:
        record["health"] = health
    assert save_observer(record)
    return key


def _raw_ingest_rejection() -> dict:
    return {
        "reason_code": "ingest_contract_invalid",
        "active_count": 79,
        "first_ts": 1_781_999_200_000,
        "latest_ts": 1_782_000_000_000,
        "summary": "screen.jsonl:2: value is invalid",
        "stream": "fedora",
        "version": "0.3.1",
        "segment": "20260622/120000_300",
    }


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


def test_register_bound_observer_helper_mints_cert_bound_record(observer_env):
    """Test creating a registered observer fixture."""
    env = observer_env()

    resp = env.register_bound_observer("test-laptop")

    assert resp.status_code == 200
    data = resp.get_json()

    assert "key" in data
    assert len(data["key"]) > 32  # 256 bits = 43 base64 chars
    assert data["prefix"] == data["key"][:8]
    assert "key_prefix" not in data
    assert data["name"] == "test-laptop"
    assert data["ingest_url"] == "/app/observer/ingest"
    assert data["key"] not in data["ingest_url"]
    assert data["protocol_version"] == OBSERVER_PROTOCOL_VERSION
    record = observer_utils.load_observer(data["key"])
    assert record is not None
    assert record["device_binding"]["kind"] == "cert"
    assert record["device_binding"]["device"] == env.pl_identity().fingerprint


def test_api_create_refuses_hand_mint(observer_env):
    env = observer_env()

    resp = env.client.post(
        "/app/observer/api/create",
        json={"name": "test-laptop"},
        content_type="application/json",
    )
    body = resp.get_json()
    assert resp.status_code == 410
    assert body["reason_code"] == "operation_no_longer_available"
    assert body["detail"] == (
        "Observer records are no longer created by hand. "
        "A device registers itself when you pair it."
    )


def test_api_list_shows_created_observer(observer_env):
    """Test that created observers appear in the list."""
    env = observer_env()

    # Create a observer
    resp = env.register_bound_observer("my-observer")
    assert resp.status_code == 200
    key_prefix = resp.get_json()["prefix"]

    # List should show it
    payload = _api_list_payload(env)
    observers = payload["observers"]

    assert len(observers) == 1
    assert payload["thresholds"] == {"active_ms": 30000, "stale_ms": 120000}
    assert observers[0]["prefix"] == key_prefix
    assert "key_prefix" not in observers[0]
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
    resp = env.register_bound_observer("my-observer")
    assert resp.status_code == 200
    key_prefix = resp.get_json()["prefix"]
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

    assert observers[0]["prefix"] == key_prefix
    assert observers[0]["last_chat_request_at"] == 9876


def test_api_delete_observer(observer_env):
    """Test revoking a observer (soft-delete)."""
    env = observer_env()

    # Create a observer
    resp = env.register_bound_observer("to-revoke")
    key_prefix = resp.get_json()["prefix"]

    # Revoke it
    resp = env.client.delete(f"/app/observer/api/{key_prefix}")
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "ok"

    # List should still show it, but marked as revoked
    observers = _api_list_observers(env)
    assert len(observers) == 1
    assert observers[0]["prefix"] == key_prefix
    assert observers[0]["revoked"] is True
    assert observers[0]["revoked_at"] is not None
    assert observers[0]["state"] == "revoked"
    assert observers[0]["group"] == "inactive"
    assert observers[0]["label"] == OBSERVER_STATE_LABELS["revoked"]
    assert observers[0]["elapsed_ms"] is None
    assert observers[0]["clock_skew"] is False


def test_api_delete_dl_observer_does_not_touch_authorized_clients(observer_env):
    env = observer_env()
    resp = env.register_unbound_observer("dl-delete")
    key_prefix = resp.get_json()["prefix"]
    fingerprint = "sha256:" + ("e" * 64)
    AuthorizedClients(authorized_clients_path()).add(
        fingerprint,
        "phone",
        "inst-1",
        paired_at="2026-05-20T00:00:00Z",
    )
    before = authorized_clients_path().read_bytes()

    resp = env.client.delete(f"/app/observer/api/{key_prefix}")

    assert resp.status_code == 200
    assert authorized_clients_path().read_bytes() == before
    assert (
        AuthorizedClients(authorized_clients_path()).is_authorized(fingerprint) is True
    )


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


def test_api_list_tie_breaks_by_prefix(observer_env, monkeypatch):
    """Observers with the same last_seen sort by prefix ascending."""
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
    assert [observer["prefix"] for observer in observers] == [
        "aaaa0000",
        "bbbb0000",
    ]
    assert all("key_prefix" not in observer for observer in observers)
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


def test_api_list_serializes_failing_ingest_rejection_without_segment(
    observer_env, monkeypatch
):
    env = observer_env()
    fixed_now = 6_000_000
    monkeypatch.setattr(routes_module, "now_ms", lambda: fixed_now)

    _save_test_observer(
        "feda0000",
        "fedora",
        created_at=10,
        last_seen=fixed_now - 5_000,
        health={"ingest_rejection": _raw_ingest_rejection()},
    )

    observer = _api_list_observers(env)[0]

    assert observer["failing"] is True
    assert set(observer["ingest_rejection"]) == {
        "reason_code",
        "active_count",
        "first_ts",
        "latest_ts",
        "summary",
        "stream",
        "version",
    }
    assert "segment" not in observer["ingest_rejection"]


def test_api_list_omits_ingest_rejection_when_not_failing(observer_env, monkeypatch):
    env = observer_env()
    fixed_now = 7_000_000
    monkeypatch.setattr(routes_module, "now_ms", lambda: fixed_now)

    _save_test_observer(
        "aaaa0000",
        "no-rejection",
        created_at=10,
        last_seen=fixed_now - 5_000,
    )
    _save_test_observer(
        "bbbb0000",
        "revoked-with-rejection",
        created_at=20,
        last_seen=fixed_now - 5_000,
        revoked=True,
        health={"ingest_rejection": _raw_ingest_rejection()},
    )
    _save_test_observer(
        "cccc0000",
        "disabled-with-rejection",
        created_at=30,
        last_seen=fixed_now - 5_000,
        enabled=False,
        health={"ingest_rejection": _raw_ingest_rejection()},
    )

    observers = {observer["name"]: observer for observer in _api_list_observers(env)}

    for name in [
        "no-rejection",
        "revoked-with-rejection",
        "disabled-with-rejection",
    ]:
        assert observers[name]["failing"] is False
        assert "ingest_rejection" not in observers[name]


def test_api_delete_nonexistent(observer_env):
    """Test deleting a nonexistent observer returns 404."""
    env = observer_env()

    resp = env.client.delete("/app/observer/api/nonexistent")
    assert resp.status_code == 404


def test_ingest_invalid_key(observer_env):
    """Test that ingest rejects invalid keys."""
    env = observer_env()
    before_observers = list_observers()

    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": "Bearer invalid-key-12345"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(b"should-not-be-parsed"), "audio.flac"),
        },
    )
    assert resp.status_code == 401
    assert "Invalid key" in resp.get_json()["detail"]
    assert list_observers() == before_observers
    assert not _day_dir(env).exists()


def test_delete_source_requires_auth(observer_env):
    """Deleting the location source requires observer auth."""
    env = observer_env()
    seg_dir = _plant_location_segment(env)

    resp = env.client.delete("/app/observer/source/location")
    assert resp.status_code == 401
    assert resp.get_json()["reason_code"] == "auth_required"
    assert seg_dir.exists()

    resp = env.client.delete(
        "/app/observer/source/location",
        headers={"Authorization": "Bearer invalid-key-12345"},
    )
    assert resp.status_code == 401
    assert "Invalid key" in resp.get_json()["detail"]
    assert seg_dir.exists()


def test_delete_source_hard_pin_rejects_other_stream(observer_env):
    """A valid observer key can only delete allowlisted source streams."""
    env = observer_env()
    create_resp = env.register_bound_observer("test-observer")
    key = create_resp.get_json()["key"]
    headers = {"Authorization": f"Bearer {key}"}

    other_seg = _plant_source_segment(
        env,
        stream="import.audio",
        segment="130000_300",
    )
    resp = env.client.delete("/app/observer/source/import.audio", headers=headers)
    assert resp.status_code == 400
    assert resp.get_json()["detail"] == "Only known source streams can be deleted"
    assert other_seg.exists()

    share_seg = _plant_source_segment(
        env,
        day="20250104",
        segment="140000_300",
    )
    resp = env.client.delete("/app/observer/source/import.share", headers=headers)
    assert resp.status_code == 400
    assert resp.get_json()["detail"] == "Only known source streams can be deleted"
    assert share_seg.exists()

    form_location_seg = _plant_location_segment(
        env,
        day="20250105",
        segment="120000_300",
    )
    resp = env.client.delete(
        "/app/observer/source/location",
        headers=headers,
        data={"stream": "import.share"},
    )
    assert resp.status_code == 400
    assert resp.get_json()["detail"] == "Only known source streams can be deleted"
    assert form_location_seg.exists()

    meta_location_seg = _plant_location_segment(
        env,
        day="20250106",
        segment="120000_300",
    )
    resp = env.client.delete(
        "/app/observer/source/location",
        headers=headers,
        data={"meta": json.dumps({"stream": "import.audio"})},
    )
    assert resp.status_code == 400
    assert resp.get_json()["detail"] == "Only known source streams can be deleted"
    assert meta_location_seg.exists()


def test_delete_source_location_happy_path(observer_env):
    env = observer_env()
    create_resp = env.register_bound_observer("test-observer")
    key = create_resp.get_json()["key"]
    seg_dir = _plant_location_segment(env)

    resp = env.client.delete(
        "/app/observer/source/location",
        headers={"Authorization": f"Bearer {key}"},
    )

    assert resp.status_code == 200
    receipt = resp.get_json()
    assert receipt["target"]["stream"] == "location"
    assert receipt["removed"]["segments"] == 1
    assert receipt["removed"]["originals"] == 1
    # ⛔ The segment is emptied and keeps its tombstone -- the owner's evidence that a
    # deletion happened. It no longer vanishes outright, per the founder ruling of
    # 2026-08-05 routing every removal through the retention executor.
    assert sorted(entry.name for entry in seg_dir.iterdir()) == ["tombstone.json"]


def test_ingest_missing_segment(observer_env):
    """Test that ingest requires segment."""
    env = observer_env()

    # Create a observer
    resp = env.register_bound_observer("test")
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
    resp = env.register_bound_observer("test")
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
    resp = env.register_bound_observer("test")
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
    resp = env.register_bound_observer("test")
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
    resp = env.register_bound_observer("test")
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
    resp = env.register_bound_observer("test-observer")
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


def test_ingest_unbound_record_authenticates_without_pl_identity(observer_env):
    env = observer_env()
    resp = env.register_unbound_observer("unbound-ingest")
    assert resp.status_code == 200
    key = resp.get_json()["key"]

    test_data = b"unbound audio content"
    resp = env.unbound_client.post(
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


def test_ingest_mixed_segment_stores_all_sources(observer_env):
    env = observer_env()
    key = _create_observer(env, "pixel")

    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(b"audio bytes"), "audio.m4a"),
                (io.BytesIO(b'{"lat": 1.0}\n'), "location.jsonl"),
                (io.BytesIO(b"screen bytes"), "screen.mp4"),
            ],
        },
    )

    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "ok"
    assert data["files"] == ["audio.m4a", "location.jsonl", "screen.mp4"]

    segment_dir = _day_dir(env) / "pixel" / data["segment"]
    audio_path = segment_dir / "audio.m4a"
    location_path = segment_dir / "location.jsonl"
    screen_path = segment_dir / "screen.mp4"
    assert audio_path.exists()
    assert location_path.exists()
    assert screen_path.exists()
    assert {audio_path.parent, location_path.parent, screen_path.parent} == {
        segment_dir
    }


def test_ingest_reuses_startup_contract_bundle(observer_env, monkeypatch):
    from solstone.think.contract import journal as contract_journal

    real = contract_journal.build_bundle
    calls = {"count": 0}

    def counting(*args, **kwargs):
        calls["count"] += 1
        return real(*args, **kwargs)

    monkeypatch.setattr(contract_journal, "build_bundle", counting)

    env = observer_env()

    resp = env.register_bound_observer("contract-cache-test")
    key = resp.get_json()["key"]

    for index in range(2):
        resp = env.client.post(
            "/app/observer/ingest",
            headers={"Authorization": f"Bearer {key}"},
            data={
                "day": "20250103",
                "segment": f"12000{index}_300",
                "files": (
                    io.BytesIO(f"test audio content {index}".encode("utf-8")),
                    f"test_audio_{index}.flac",
                ),
            },
        )
        assert resp.status_code == 200
        assert resp.get_json()["status"] == "ok"

    assert calls["count"] == 0
    assert (
        env.app.config["JOURNAL_CONTRACT_BUNDLE"]["contract"]
        == "solstone-journal-at-rest"
    )


def test_ingest_updates_stats(observer_env):
    """Test that ingest updates observer stats."""
    env = observer_env()

    # Create a observer
    resp = env.register_bound_observer("stats-test")
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


def test_ingest_persists_last_segment_receipt_freshness(
    observer_env,
    monkeypatch,
) -> None:
    env = observer_env()
    receipt_now = 1_800_000_000_000
    monkeypatch.setattr(routes_module, "now_ms", lambda: receipt_now)
    key = _create_observer(env, "freshness-test")

    resp = _upload_audio(
        env,
        key,
        b"test content",
        day="20260724",
        segment="120000_300",
    )

    assert resp.status_code == 200
    record = _observer_record()
    assert record["last_seen"] == receipt_now
    assert record["last_segment"] == "120000_300"
    assert record["last_segment_received_at"] == receipt_now
    assert isinstance(record["last_segment_received_at"], int)
    assert record["last_segment_day"] == "20260724"
    assert isinstance(record["last_segment_day"], str)


def test_last_segment_assignments_are_coupled_to_received_at() -> None:
    _assert_last_segment_writes_are_coupled(Path(routes_module.__file__))
    _assert_last_segment_writes_are_coupled(Path(observer_cli_module.__file__))


def test_last_segment_coupling_guard_rejects_uncoupled_subscript_write() -> None:
    source = """
def write_segment(observer, segment):
    observer["last_segment"] = segment
"""

    with pytest.raises(AssertionError, match="last_segment write"):
        _assert_last_segment_source_writes_are_coupled(
            source,
            source_name="synthetic_subscript.py",
        )


def test_last_segment_coupling_guard_rejects_uncoupled_update_write() -> None:
    source = """
def write_segment(observer, segment):
    observer.update({"last_segment": segment})
"""

    with pytest.raises(AssertionError, match="last_segment write"):
        _assert_last_segment_source_writes_are_coupled(
            source,
            source_name="synthetic_update.py",
        )


def test_status_event_refreshes_last_seen_without_segment_receipt(
    observer_env,
    monkeypatch,
) -> None:
    env = observer_env()
    key = _create_observer(env, "event-freshness-test")
    event_now = 1_800_000_060_000
    monkeypatch.setattr(observer_utils, "now_ms", lambda: event_now)

    resp = env.client.post(
        "/app/observer/ingest/event",
        headers={"Authorization": f"Bearer {key}"},
        json={"tract": "observe", "event": "status"},
        content_type="application/json",
    )

    assert resp.status_code == 200
    assert resp.get_json()["status"] == "ok"
    record = _observer_record()
    assert record["last_seen"] == event_now
    assert record["last_segment"] is None
    assert record["last_segment_received_at"] is None
    assert record["last_segment_day"] is None
    freshness = _classify_observer_freshness(
        record["last_seen"],
        record.get("revoked", False),
        event_now,
    )
    assert freshness["state"] == "connected"


def test_successful_ingest_upload_saves_observer_once(
    observer_env,
    monkeypatch,
) -> None:
    env = observer_env()
    key = _create_observer(env, "save-count-test")
    calls = 0
    original = routes_module.save_observer

    def counting_save_observer(observer: dict) -> bool:
        nonlocal calls
        calls += 1
        return original(observer)

    monkeypatch.setattr(routes_module, "save_observer", counting_save_observer)

    resp = _upload_audio(env, key, b"test content")

    assert resp.status_code == 200
    assert calls == 1


def test_ingest_event_relay(observer_env):
    """Test event relay endpoint."""
    env = observer_env()

    # Create a observer
    resp = env.register_bound_observer("event-test")
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


def test_ingest_event_pl_phone_identity_without_handle_returns_401(
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
    resp = env.register_bound_observer("test")
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
    resp = env.register_bound_observer("revoked-test")
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["prefix"]

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


def test_keyless_ingest_bearer_rejects_revoked_and_disabled_keys(observer_env):
    env = observer_env()

    resp = env.register_bound_observer("keyless-revoked-test")
    revoked_data = resp.get_json()
    revoked_key = revoked_data["key"]

    resp = env.client.delete(f"/app/observer/api/{revoked_data['prefix']}")
    assert resp.status_code == 200

    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {revoked_key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(b"revoked content"), "audio.flac"),
        },
    )
    assert resp.status_code == 403
    body = resp.get_json()
    assert body["reason_code"] == "pl_revoked"
    assert body["detail"] == "Observer revoked"

    resp = env.register_bound_observer("keyless-disabled-test")
    disabled_data = resp.get_json()
    disabled_key = disabled_data["key"]
    assert save_observer(
        {
            "key": disabled_key,
            "name": "keyless-disabled-test",
            "created_at": 0,
            "last_seen": None,
            "last_segment": None,
            "enabled": False,
            "revoked": False,
            "revoked_at": None,
            "stats": {
                "segments_received": 0,
                "bytes_received": 0,
            },
        }
    )

    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {disabled_key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(b"disabled content"), "audio.flac"),
        },
    )
    assert resp.status_code == 403
    body = resp.get_json()
    assert body["reason_code"] == "feature_unavailable"
    assert body["detail"] == "Observer disabled"


def test_ingest_event_revoked_key(observer_env):
    """Test that event relay rejects revoked keys."""
    env = observer_env()

    # Create and revoke a observer
    resp = env.register_bound_observer("revoked-event-test")
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["prefix"]

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


def test_observer_health_records_sanitized_beacon(observer_env):
    env = observer_env()
    key = _create_observer(env, "health-test")

    resp = env.client.post(
        "/app/observer/health",
        headers={OBSERVER_HANDLE_HEADER: key},
        json={
            "name": "phone",
            "stream_type": "phone",
            "version": "1.2.3",
            "uptime": 42,
            "last_successful_sync": 1_767_100_000_000,
            "pending_queue_depth": 3,
            "recent_error_count": 2,
            "last_error_reason": "x" * 250,
            "captured_path": "/Users/jer/private/audio.m4a",
            "response_body": "raw server body",
        },
        content_type="application/json",
    )

    assert resp.status_code == 200
    assert resp.get_json()["status"] == "ok"

    health = _observer_record()["health"]
    beacon = health["beacon"]
    assert set(beacon) == {
        "received_at",
        "name",
        "stream_type",
        "version",
        "uptime",
        "last_successful_sync",
        "pending_queue_depth",
        "recent_error_count",
        "last_error_reason",
    }
    assert beacon["name"] == "phone"
    assert beacon["stream_type"] == "phone"
    assert beacon["version"] == "1.2.3"
    assert beacon["uptime"] == 42
    assert beacon["last_successful_sync"] == 1_767_100_000_000
    assert beacon["pending_queue_depth"] == 3
    assert beacon["recent_error_count"] == 2
    assert len(beacon["last_error_reason"]) == 200
    assert "captured_path" not in beacon
    assert "response_body" not in beacon


def test_observer_health_missing_and_invalid_identity(observer_env):
    env = observer_env()

    resp = env.client.post(
        "/app/observer/health",
        json={"name": "phone"},
        content_type="application/json",
    )
    assert resp.status_code == 401
    assert resp.get_json()["reason_code"] == "auth_required"

    resp = env.client.post(
        "/app/observer/health",
        headers={"Authorization": "Bearer invalid-key"},
        json={"name": "phone"},
        content_type="application/json",
    )
    assert resp.status_code == 401
    assert resp.get_json()["reason_code"] == "auth_key_invalid"


def test_observer_health_revoked_key(observer_env):
    env = observer_env()
    resp = env.register_bound_observer("revoked-health-test")
    data = resp.get_json()
    key = data["key"]

    resp = env.client.delete(f"/app/observer/api/{data['prefix']}")
    assert resp.status_code == 200

    resp = env.client.post(
        "/app/observer/health",
        headers={"Authorization": f"Bearer {key}"},
        json={"name": "phone"},
        content_type="application/json",
    )
    assert resp.status_code == 403
    assert resp.get_json()["reason_code"] == "pl_revoked"


def test_api_get_key(observer_env):
    """Test retrieving full key for a observer."""
    env = observer_env()

    # Create a observer
    resp = env.register_bound_observer("key-test")
    create_data = resp.get_json()
    key = create_data["key"]
    key_prefix = create_data["prefix"]

    # Get the key
    resp = env.client.get(f"/app/observer/api/{key_prefix}/key")
    assert resp.status_code == 200

    data = resp.get_json()
    assert data["key"] == key
    assert data["name"] == "key-test"
    assert data["ingest_url"] == "/app/observer/ingest"
    assert key not in data["ingest_url"]
    assert data["protocol_version"] == OBSERVER_PROTOCOL_VERSION


def test_mint_responses_protocol_version_single_source_and_keyless_unconditional(
    observer_env, monkeypatch
):
    monkeypatch.setattr("solstone.observe.protocol.OBSERVER_PROTOCOL_VERSION", 99)
    env = observer_env()

    resp = env.register_bound_observer("mint-protocol-test")
    assert resp.status_code == 200
    create_data = resp.get_json()
    assert create_data["protocol_version"] == 99
    assert create_data["ingest_url"] == "/app/observer/ingest"

    resp = env.client.get(f"/app/observer/api/{create_data['prefix']}/key")
    assert resp.status_code == 200
    key_data = resp.get_json()
    assert key_data["protocol_version"] == 99
    assert key_data["ingest_url"] == "/app/observer/ingest"


def test_api_get_key_nonexistent(observer_env):
    """Test getting key for nonexistent observer returns 404."""
    env = observer_env()

    resp = env.client.get("/app/observer/api/nonexistent/key")
    assert resp.status_code == 404


def test_api_get_key_revoked(observer_env):
    """Test getting key for revoked observer returns 403."""
    env = observer_env()

    # Create then revoke
    resp = env.register_bound_observer("revoke-key-test")
    create_data = resp.get_json()
    key_prefix = create_data["prefix"]

    env.client.delete(f"/app/observer/api/{key_prefix}")

    # Try to get the key
    resp = env.client.get(f"/app/observer/api/{key_prefix}/key")
    assert resp.status_code == 403
    assert "revoked" in resp.get_json()["detail"]


def test_api_get_key_audit_log(observer_env):
    """Test that viewing a key logs an audit action."""
    from unittest.mock import patch

    env = observer_env()

    resp = env.register_bound_observer("audit-test")
    create_data = resp.get_json()
    key_prefix = create_data["prefix"]

    with patch("solstone.apps.observer.routes.log_app_action") as mock_log:
        resp = env.client.get(f"/app/observer/api/{key_prefix}/key")
        assert resp.status_code == 200

        mock_log.assert_called_once_with(
            app="observer",
            facet=None,
            action="observer_key_view",
            params={"name": "audit-test", "key_prefix": key_prefix},
        )


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
    resp = env.register_bound_observer("collision-test")
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
    resp = env.register_bound_observer("no-collision-test")
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
    resp = env.register_bound_observer("stats-adjust-test")
    key = resp.get_json()["key"]

    # Spec §5.2.6: the ladder fires on conflicting content at the requested
    # key, not on an occupied key; non-conflicting candidates are additive
    # writes (§5.2.4).
    day_dir = _day_dir(env)
    stream_dir = day_dir / "stats-adjust-test"
    stream_dir.mkdir(parents=True)
    (stream_dir / "120000_300").mkdir()
    (stream_dir / "120000_300" / "audio.flac").write_bytes(b"old audio")

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
    resp = env.register_bound_observer("history-test")
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["prefix"]

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


def test_ingest_history_with_collision(observer_env):
    """Test that sync history records collision adjustment."""
    env = observer_env()

    # Create a observer
    resp = env.register_bound_observer("collision-history-test")
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["prefix"]

    # Spec §5.2.6: the ladder fires on conflicting content at the requested
    # key, not on an occupied key; non-conflicting candidates are additive
    # writes (§5.2.4).
    day_dir = _day_dir(env)
    stream_dir = day_dir / "collision-history-test"
    stream_dir.mkdir(parents=True)
    (stream_dir / "120000_300").mkdir()
    (stream_dir / "120000_300" / "audio.flac").write_bytes(b"old audio")

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
    resp = env.register_bound_observer("segments-empty-test")
    key = resp.get_json()["key"]

    # Query segments - should be empty
    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": f"Bearer {key}"},
    )
    assert resp.status_code == 200
    data = resp.get_json()
    assert isinstance(data, list)
    assert data == []


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
    resp = env.register_bound_observer("segments-day-test")
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
    resp = env.register_bound_observer("segments-list-test")
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

    assert isinstance(data, list)
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


def test_segments_endpoint_omits_submitted_name_when_name_unchanged(observer_env):
    """Test segments endpoint omits submitted_name when no filename rewrite occurred."""
    env = observer_env()

    resp = env.register_bound_observer("segments-no-rewrite-test")
    key = resp.get_json()["key"]

    test_data = b"test audio content"
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

    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": f"Bearer {key}"},
    )
    assert resp.status_code == 200
    data = resp.get_json()
    file_info = data[0]["files"][0]
    assert "submitted_name" not in file_info


def test_segments_endpoint_v2_empty(observer_env):
    """Test v2 segments endpoint returns collection envelope for no uploads."""
    env = observer_env()

    resp = env.register_bound_observer("segments-v2-empty-test")
    key = resp.get_json()["key"]

    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={
            "Authorization": f"Bearer {key}",
            "X-Solstone-Protocol-Version": "2",
        },
    )
    assert resp.status_code == 200
    body = resp.get_json()
    assert isinstance(body, dict)
    assert "items" in body
    assert body["items"] == []
    assert body["protocol_version"] == 2
    assert body["total"] == 0


def test_segments_endpoint_v2_populated(observer_env):
    """Test v2 segments endpoint envelopes uploaded segments."""
    env = observer_env()

    resp = env.register_bound_observer("segments-v2-list-test")
    key = resp.get_json()["key"]

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

    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={
            "Authorization": f"Bearer {key}",
            "X-Solstone-Protocol-Version": "2",
        },
    )
    assert resp.status_code == 200
    body = resp.get_json()
    assert isinstance(body, dict)
    assert body["protocol_version"] == 2
    assert len(body["items"]) == 1
    segment = body["items"][0]
    assert segment["key"] == "120000_300"
    assert len(segment["files"]) == 1
    assert segment["files"][0]["status"] == "present"


def test_protocol_version_single_source(observer_env, monkeypatch):
    """A single protocol constant drives route gating and producer advertise."""
    monkeypatch.setattr("solstone.observe.protocol.OBSERVER_PROTOCOL_VERSION", 99)
    env = observer_env()

    resp = env.register_bound_observer("segments-patched-protocol-test")
    key = resp.get_json()["key"]

    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={
            "Authorization": f"Bearer {key}",
            "X-Solstone-Protocol-Version": "99",
        },
    )
    assert resp.status_code == 200
    body = resp.get_json()
    assert isinstance(body, dict)
    assert body["protocol_version"] == 99

    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={
            "Authorization": f"Bearer {key}",
            "X-Solstone-Protocol-Version": "2",
        },
    )
    assert resp.status_code == 200
    assert isinstance(resp.get_json(), list)


def test_segments_endpoint_does_not_create_missing_day_directory(observer_env):
    """A read-only listing must not create the chronicle day directory."""
    env = observer_env()
    key = _create_observer(env, "segments-no-day-create-test")
    observer = _observer_record()
    day = "20250104"
    day_dir = _day_dir(env, day)
    assert not day_dir.exists()
    append_history_record(
        observer["filename_prefix"],
        day,
        {"type": "observed", "segment": "120000_300"},
    )

    resp = env.client.get(
        f"/app/observer/ingest/segments/{day}",
        headers={
            "Authorization": f"Bearer {key}",
            "X-Solstone-Protocol-Version": str(OBSERVER_PROTOCOL_VERSION),
        },
    )

    assert resp.status_code == 200
    body = resp.get_json()
    assert body["items"] == []
    assert body["total"] == 0
    assert body["protocol_version"] == OBSERVER_PROTOCOL_VERSION
    assert not day_dir.exists()


def test_segments_endpoint_shows_collision(observer_env):
    """Test segments endpoint shows collision info."""
    env = observer_env()

    # Create a observer
    resp = env.register_bound_observer("segments-collision-test")
    key = resp.get_json()["key"]

    # Spec §5.2.6: the ladder fires on conflicting content at the requested
    # key, not on an occupied key; non-conflicting candidates are additive
    # writes (§5.2.4).
    day_dir = _day_dir(env)
    stream_dir = day_dir / "segments-collision-test"
    stream_dir.mkdir(parents=True)
    (stream_dir / "120000_300").mkdir()
    (stream_dir / "120000_300" / "audio.flac").write_bytes(b"old audio")

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
    resp = env.register_bound_observer("segments-missing-test")
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


def test_segments_endpoint_renamed_recorded_file_is_missing(observer_env):
    """A renamed file is missing because only the recorded path proves presence."""
    env = observer_env()
    observer_name = "segments-renamed-missing-test"
    key = _create_observer(env, observer_name)
    test_data = b"test audio for renamed path"

    resp = _upload_audio(env, key, test_data)
    assert resp.status_code == 200

    segment_dir = _day_dir(env) / observer_name / "120000_300"
    recorded_path = segment_dir / "audio.flac"
    recorded_path.rename(segment_dir / "renamed_audio.flac")

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "missing"


def test_segments_endpoint_same_content_elsewhere_is_missing(observer_env):
    """Same bytes elsewhere do not prove presence at the recorded path."""
    env = observer_env()
    observer_name = "segments-same-content-missing-test"
    key = _create_observer(env, observer_name)
    test_data = b"test audio for same content elsewhere"

    resp = _upload_audio(env, key, test_data)
    assert resp.status_code == 200

    day_dir = _day_dir(env)
    segment_dir = day_dir / observer_name / "120000_300"
    recorded_path = segment_dir / "audio.flac"
    (day_dir / observer_name / "same_bytes.flac").write_bytes(test_data)
    recorded_path.unlink()

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "missing"


def test_segments_endpoint_same_inode_elsewhere_is_missing(observer_env):
    """A hardlink elsewhere still leaves the recorded path missing."""
    env = observer_env()
    observer_name = "segments-same-inode-missing-test"
    key = _create_observer(env, observer_name)
    test_data = b"test audio for same inode elsewhere"

    resp = _upload_audio(env, key, test_data)
    assert resp.status_code == 200

    segment_dir = _day_dir(env) / observer_name / "120000_300"
    recorded_path = segment_dir / "audio.flac"
    os.link(recorded_path, segment_dir / "hardlinked_audio.flac")
    recorded_path.unlink()

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "missing"


def test_segments_endpoint_missing_path_does_not_scan_day_tree(
    observer_env, monkeypatch
):
    """Missing-path resolution must not descend through the day tree."""
    env = observer_env()
    observer_name = "segments-no-descent-test"
    key = _create_observer(env, observer_name)
    test_data = b"test audio for no descent"

    resp = _upload_audio(env, key, test_data)
    assert resp.status_code == 200

    recorded_path = _day_dir(env) / observer_name / "120000_300" / "audio.flac"
    recorded_path.unlink()

    def fail_rglob(self, *args, **kwargs):
        raise AssertionError("day-tree scan attempted")

    monkeypatch.setattr(Path, "rglob", fail_rglob)

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "missing"


def test_segments_endpoint_reports_processed_for_terminal_audio_sidecar(observer_env):
    env = observer_env()
    observer_name = "segments-processed-test"
    key = _create_observer(env, observer_name)
    audio_content = b"raw audio bytes for terminal processing"
    screen_content = b"raw screen bytes that remain present"

    resp = _post_raw_audio_screen_bundle(
        env, key, observer_name, audio_content, screen_content
    )
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "ok"

    segment_dir = _day_dir(env) / observer_name / "120000_300"
    _write_audio_processing_sidecar(segment_dir, input_size=len(audio_content))
    (segment_dir / "audio.flac").unlink()

    files = _listed_files_by_name(env, key)
    assert files["audio.flac"]["status"] == "processed"
    assert files["screen.mp4"]["status"] == "present"
    # D4 / AC-13: received-not-written files must never be enumerated as held.
    assert "stream.json" not in files


def test_ingest_processed_duplicate_does_not_create_collision_lattice(observer_env):
    env = observer_env()
    observer_name = "processed-lattice-test"
    key = _create_observer(env, observer_name)
    audio_content = b"raw audio bytes for processed duplicate gating"
    screen_content = b"raw screen bytes for processed duplicate gating"

    resp = _post_raw_audio_screen_bundle(
        env, key, observer_name, audio_content, screen_content
    )
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "ok"

    observer = _observer_record()
    key_prefix = observer["filename_prefix"]
    segment_dir = _day_dir(env) / observer_name / "120000_300"
    _write_audio_processing_sidecar(segment_dir, input_size=len(audio_content))
    (segment_dir / "audio.flac").unlink()

    stream_dir = env.journal / "chronicle" / "20250103" / observer_name
    initial_history_lines = _history_line_count(env, key_prefix)
    assert initial_history_lines == 1

    for index in range(3):
        resp = _post_raw_audio_screen_bundle(
            env, key, observer_name, audio_content, screen_content
        )
        body = resp.get_json()
        assert resp.status_code == 200
        assert body["status"] == "duplicate"
        assert len(list(stream_dir.iterdir())) == 1
        # Spec §5.2.5: every duplicate resolution appends a corroborating
        # audit record instead of relying on a history-as-index cache.
        assert _history_line_count(env, key_prefix) == initial_history_lines + index + 1
        assert not (_day_dir(env) / "observer" / "failed").exists()

        records = load_history(key_prefix, "20250103")
        latest = records[-1]
        assert latest["segment"] == "120000_300"
        dispositions = {
            file["written"]: file["disposition"] for file in latest["files"]
        }
        assert dispositions["audio.flac"] == "already_held"
        assert dispositions["screen.mp4"] == "already_held"


def test_segments_endpoint_no_sidecar_keeps_raw_file_missing(observer_env):
    env = observer_env()
    observer_name = "processed-no-sidecar-test"
    key = _create_observer(env, observer_name)
    audio_content = b"raw audio bytes with no processing sidecar"

    resp = _upload_audio(env, key, audio_content)
    assert resp.status_code == 200
    (_day_dir(env) / observer_name / "120000_300" / "audio.flac").unlink()

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "missing"


def test_segments_endpoint_malformed_sidecar_keeps_raw_file_missing(observer_env):
    env = observer_env()
    observer_name = "processed-malformed-sidecar-test"
    key = _create_observer(env, observer_name)
    audio_content = b"raw audio bytes with malformed sidecar"

    resp = _upload_audio(env, key, audio_content)
    assert resp.status_code == 200
    segment_dir = _day_dir(env) / observer_name / "120000_300"
    (segment_dir / "audio.jsonl").write_text("{not json\n", encoding="utf-8")
    (segment_dir / "audio.flac").unlink()

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "missing"


def test_segments_endpoint_absent_processing_schema_keeps_raw_file_missing(
    observer_env,
):
    env = observer_env()
    observer_name = "processed-absent-schema-test"
    key = _create_observer(env, observer_name)
    audio_content = b"raw audio bytes with absent processing schema"

    resp = _upload_audio(env, key, audio_content)
    assert resp.status_code == 200
    segment_dir = _day_dir(env) / observer_name / "120000_300"
    _write_audio_processing_sidecar(
        segment_dir,
        input_size=len(audio_content),
        include_schema=False,
    )
    (segment_dir / "audio.flac").unlink()

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "missing"


def test_segments_endpoint_wrong_processing_schema_keeps_raw_file_missing(
    observer_env,
):
    env = observer_env()
    observer_name = "processed-wrong-schema-test"
    key = _create_observer(env, observer_name)
    audio_content = b"raw audio bytes with wrong processing schema"

    resp = _upload_audio(env, key, audio_content)
    assert resp.status_code == 200
    segment_dir = _day_dir(env) / observer_name / "120000_300"
    _write_audio_processing_sidecar(
        segment_dir,
        input_size=len(audio_content),
        schema="solstone.processing.v0",
    )
    (segment_dir / "audio.flac").unlink()

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "missing"


def test_segments_endpoint_failed_processing_state_keeps_raw_file_missing(
    observer_env,
):
    env = observer_env()
    observer_name = "processed-failed-state-test"
    key = _create_observer(env, observer_name)
    audio_content = b"raw audio bytes with failed processing state"

    resp = _upload_audio(env, key, audio_content)
    assert resp.status_code == 200
    segment_dir = _day_dir(env) / observer_name / "120000_300"
    _write_audio_processing_sidecar(
        segment_dir,
        input_size=len(audio_content),
        state=STATE_FAILED,
    )
    (segment_dir / "audio.flac").unlink()

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "missing"


def test_segments_endpoint_handler_mismatch_keeps_raw_file_missing(observer_env):
    env = observer_env()
    observer_name = "processed-handler-mismatch-test"
    key = _create_observer(env, observer_name)
    audio_content = b"raw audio bytes with handler mismatch"

    resp = _upload_audio(env, key, audio_content)
    assert resp.status_code == 200
    segment_dir = _day_dir(env) / observer_name / "120000_300"
    _write_audio_processing_sidecar(
        segment_dir,
        input_size=len(audio_content),
        handler=HANDLER_DESCRIBE,
    )
    (segment_dir / "audio.flac").unlink()

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "missing"


def test_segments_endpoint_absent_processing_input_size_keeps_raw_file_missing(
    observer_env,
):
    env = observer_env()
    observer_name = "processed-absent-input-size-test"
    key = _create_observer(env, observer_name)
    audio_content = b"raw audio bytes with absent processing input size"

    resp = _upload_audio(env, key, audio_content)
    assert resp.status_code == 200
    segment_dir = _day_dir(env) / observer_name / "120000_300"
    _write_audio_processing_sidecar(
        segment_dir,
        input_size=len(audio_content),
        include_input_size=False,
    )
    (segment_dir / "audio.flac").unlink()

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "missing"


def test_segments_endpoint_non_int_processing_input_size_keeps_raw_file_missing(
    observer_env,
):
    env = observer_env()
    observer_name = "processed-non-int-input-size-test"
    key = _create_observer(env, observer_name)
    audio_content = b"raw audio bytes with non-int processing input size"

    resp = _upload_audio(env, key, audio_content)
    assert resp.status_code == 200
    segment_dir = _day_dir(env) / observer_name / "120000_300"
    _write_audio_processing_sidecar(
        segment_dir,
        input_size=str(len(audio_content)),
    )
    (segment_dir / "audio.flac").unlink()

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "missing"


def test_segments_endpoint_mismatched_processing_input_size_keeps_raw_file_missing(
    observer_env,
):
    env = observer_env()
    observer_name = "processed-mismatched-input-size-test"
    key = _create_observer(env, observer_name)
    audio_content = b"raw audio bytes with mismatched processing input size"

    resp = _upload_audio(env, key, audio_content)
    assert resp.status_code == 200
    segment_dir = _day_dir(env) / observer_name / "120000_300"
    _write_audio_processing_sidecar(
        segment_dir,
        input_size=len(audio_content) + 1,
    )
    (segment_dir / "audio.flac").unlink()

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "missing"


def test_segments_endpoint_revoked_key(observer_env):
    """Test segments endpoint rejects revoked key."""
    env = observer_env()

    # Create and revoke a observer
    resp = env.register_bound_observer("segments-revoked-test")
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["prefix"]

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
    resp = env.register_bound_observer("segments-dedup-test")
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
    resp = env.register_bound_observer("observed-test")
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["prefix"]

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
    resp = env.register_bound_observer("stats-test")
    data = resp.get_json()
    key_prefix = data["prefix"]

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
    resp = env.register_bound_observer("duplicate-test")
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


def test_ingest_reuploads_deleted_file_heals_in_place(observer_env):
    """Spec AC-6: deleted bytes are restored at the recorded path."""
    env = observer_env()

    observer_name = "dedup-heal-test"
    key = _create_observer(env, observer_name)
    test_data = b"test audio content for deleted file healing"
    sha256 = hashlib.sha256(test_data).hexdigest()

    def upload():
        return env.client.post(
            "/app/observer/ingest",
            headers={"Authorization": f"Bearer {key}"},
            data={
                "day": "20250103",
                "segment": "120000_300",
                "files": (io.BytesIO(test_data), "audio.flac"),
            },
        )

    resp = upload()
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "ok"
    original_segment = data["segment"]

    original_path = _day_dir(env) / observer_name / original_segment / "audio.flac"
    assert original_path.exists()
    original_path.unlink()

    resp = upload()
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "ok"
    assert data["segment"] == original_segment
    assert original_path.read_bytes() == test_data

    resp = env.client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": f"Bearer {key}"},
    )
    assert resp.status_code == 200
    segments = {segment["key"]: segment for segment in resp.get_json()}

    original_file = next(
        file_info
        for file_info in segments[original_segment]["files"]
        if file_info["sha256"] == sha256
    )
    assert original_file["status"] == "present"

    resp = upload()
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "duplicate"
    assert data["existing_segment"]


def test_ingest_reupload_after_removed_segment_restores_recorded_path(observer_env):
    """Re-uploading after freeing the segment slot restores the recorded path."""
    env = observer_env()

    observer_name = "dedup-restored-path-test"
    key = _create_observer(env, observer_name)
    test_data = b"test audio content for recorded path restoration"

    def upload():
        return _upload_audio(env, key, test_data)

    resp = upload()
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "ok"
    original_segment = data["segment"]
    assert original_segment == "120000_300"

    segment_dir = _day_dir(env) / observer_name / original_segment
    # Removing the whole segment dir frees the slot, so the existing heal path
    # restores the re-upload at the original recorded path without remapping.
    shutil.rmtree(segment_dir)

    resp = upload()
    assert resp.status_code == 200
    data = resp.get_json()
    assert data["status"] == "ok"
    assert data["segment"] == original_segment

    file_info = _listed_file_info(env, key)
    assert file_info["status"] == "present"


def test_ingest_duplicate_does_not_emit_event(observer_env, monkeypatch):
    """Test that duplicate submission does not emit observe.observing event."""
    env = observer_env()

    # Mock emit
    import solstone.apps.observer.routes as routes_module

    emit_mock = MagicMock()
    monkeypatch.setattr(routes_module, "emit", emit_mock)

    # Create a observer
    resp = env.register_bound_observer("no-event-test")
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
    resp = env.register_bound_observer("dup-stat-test")
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


def test_ingest_conflicting_content_uses_deterministic_ladder(observer_env):
    """Spec AC-10: same key plus conflicting content mints LEN+1 deterministically."""
    env = observer_env()

    # Create a observer
    resp = env.register_bound_observer("partial-dup-test")
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
    assert second_data["status"] == "collision"
    assert second_data["segment"] == "120000_301"
    assert second_data["segment_original"] == "120000_300"
    assert second_data["segment"] != first_segment


def test_ingest_same_segment_addition_records_written_file(observer_env):
    """Spec AC-3: identical held media plus a new non-media file lands in place."""
    env = observer_env()

    # Create a observer
    resp = env.register_bound_observer("partial-log-test")
    data = resp.get_json()
    key = data["key"]
    key_prefix = data["prefix"]

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
    body = resp.get_json()
    assert body["status"] == "ok"
    assert body["segment"] == "120000_300"
    segment_dir = _day_dir(env) / "partial-log-test" / "120000_300"
    assert (segment_dir / "new_file.txt").read_bytes() == new_data

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

    latest_files = {file["written"]: file for file in upload_records[1]["files"]}
    assert latest_files["audio.flac"]["disposition"] == "already_held"
    assert latest_files["new_file.txt"]["disposition"] == "written"
    assert "partial_match_sha256s" not in upload_records[1]


def test_ingest_reupload_deleted_duplicate_dirs_resolves_to_survivor(observer_env):
    """AC-2: old duplicate-dir lattices collapse back to the surviving segment."""
    env = observer_env()
    key = _create_observer(env, "ceo-repro-test")
    audio = b"ceo repro identical audio"

    first = _upload_audio(env, key, audio, segment="120000_300")
    assert first.status_code == 200
    assert first.get_json()["status"] == "ok"

    stream_dir = _day_dir(env) / "ceo-repro-test"
    for segment in ("120000_301", "120000_302"):
        dup_dir = stream_dir / segment
        dup_dir.mkdir()
        (dup_dir / "audio.flac").write_bytes(audio)

    shutil.rmtree(stream_dir / "120000_301")
    shutil.rmtree(stream_dir / "120000_302")

    retry = _upload_audio(env, key, audio, segment="120000_302")
    assert retry.status_code == 200
    body = retry.get_json()
    assert body["status"] == "duplicate"
    assert body["existing_segment"] == "120000_300"
    assert sorted(path.name for path in stream_dir.iterdir()) == ["120000_300"]


def test_ingest_sidecar_conflict_returns_409_without_writes(observer_env):
    """AC-1: metadata churn conflicts without writing or minting a segment."""
    env = observer_env()
    key = _create_observer(env, "sidecar-conflict-test")
    audio = b"held audio"
    sidecar = b"sidecar v1"
    changed_sidecar = b"sidecar v2"

    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(sidecar), "notes.txt"),
            ],
        },
    )
    assert resp.status_code == 200

    segment_dir = _day_dir(env) / "sidecar-conflict-test" / "120000_300"
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(changed_sidecar), "notes.txt"),
            ],
        },
    )
    body = resp.get_json()

    assert resp.status_code == 409
    assert body["reason_code"] == "ingest_sidecar_conflict"
    assert body["conflicting_files"] == ["notes.txt"]
    assert body["existing_segment"] == "120000_300"
    assert (segment_dir / "audio.flac").read_bytes() == audio
    assert (segment_dir / "notes.txt").read_bytes() == sidecar
    assert not (segment_dir.parent / "120000_301").exists()
    assert not (_day_dir(env) / "observer" / "failed").exists()

    observer = _observer_record()
    latest = load_history(observer["filename_prefix"], "20250103")[-1]
    latest_files = {file["written"]: file for file in latest["files"]}
    assert latest_files["audio.flac"]["disposition"] == "already_held"
    assert latest_files["notes.txt"]["disposition"] == "received_not_written"

    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(sidecar), "notes.txt"),
            ],
        },
    )
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "duplicate"

    stream_marker = (
        b'{"stream":"third-party","prev_day":null,"prev_segment":null,"seq":9}\n'
    )
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(stream_marker), "stream.json"),
            ],
        },
    )
    assert resp.status_code == 200
    assert resp.get_json()["status"] == "duplicate"


def test_ingest_conflict_blocks_new_sidecar_until_stripped(observer_env):
    """AC-1a: conflict wins over additive sidecar writes."""
    env = observer_env()
    key = _create_observer(env, "conflict-blocks-new-test")
    audio = b"held audio"
    notes = b"notes v1"
    changed_notes = b"notes v2"
    extra = b"extra sidecar"

    first = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(notes), "notes.txt"),
            ],
        },
    )
    assert first.status_code == 200

    segment_dir = _day_dir(env) / "conflict-blocks-new-test" / "120000_300"
    conflict = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(extra), "extra.txt"),
                (io.BytesIO(changed_notes), "notes.txt"),
            ],
        },
    )
    assert conflict.status_code == 409
    assert not (segment_dir / "extra.txt").exists()

    stripped = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(extra), "extra.txt"),
            ],
        },
    )
    assert stripped.status_code == 200
    assert stripped.get_json()["status"] == "ok"
    assert (segment_dir / "extra.txt").read_bytes() == extra


def test_ingest_heal_precedes_sidecar_conflict(observer_env):
    """AC-1b: missing content heals before sidecar conflict signaling."""
    env = observer_env()
    key = _create_observer(env, "heal-before-conflict-test")
    audio = b"held audio"
    screen = b"missing screen"
    notes = b"notes v1"
    changed_notes = b"notes v2"

    first = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(screen), "screen.mp4"),
                (io.BytesIO(notes), "notes.txt"),
            ],
        },
    )
    assert first.status_code == 200

    segment_dir = _day_dir(env) / "heal-before-conflict-test" / "120000_300"
    (segment_dir / "screen.mp4").unlink()

    heal = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(screen), "screen.mp4"),
                (io.BytesIO(changed_notes), "notes.txt"),
            ],
        },
    )
    assert heal.status_code == 200
    assert heal.get_json()["status"] == "ok"
    assert (segment_dir / "screen.mp4").read_bytes() == screen
    assert (segment_dir / "notes.txt").read_bytes() == notes

    observer = _observer_record()
    latest = load_history(observer["filename_prefix"], "20250103")[-1]
    latest_files = {file["written"]: file for file in latest["files"]}
    assert latest_files["screen.mp4"]["disposition"] == "written"
    assert latest_files["notes.txt"]["disposition"] == "received_not_written"

    followup = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(screen), "screen.mp4"),
                (io.BytesIO(changed_notes), "notes.txt"),
            ],
        },
    )
    assert followup.status_code == 409


def test_ingest_media_less_bundle_identity(observer_env):
    """AC-4: media-less bundles match all uploaded non-reserved files."""
    env = observer_env()
    key = _create_observer(env, "media-less-test")
    first_bytes = b'{"line":1}\n'
    second_bytes = b'{"line":2}\n'

    first = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(first_bytes), "tmux.jsonl"),
        },
    )
    assert first.status_code == 200
    assert first.get_json()["status"] == "ok"

    duplicate = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(first_bytes), "tmux.jsonl"),
        },
    )
    assert duplicate.status_code == 200
    assert duplicate.get_json()["status"] == "duplicate"

    changed = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(second_bytes), "tmux.jsonl"),
        },
    )
    assert changed.status_code == 200
    body = changed.get_json()
    assert body["status"] == "collision"
    assert body["segment"] == "120000_301"
    assert (
        _day_dir(env) / "media-less-test" / "120000_301" / "tmux.jsonl"
    ).read_bytes() == second_bytes


def test_ingest_same_media_different_start_times_resolve_to_their_own_segments(
    observer_env,
):
    """AC-5: byte-identical media at different starts remain distinct."""
    env = observer_env()
    key = _create_observer(env, "same-media-start-test")
    audio = b"same audio different starts"

    first = _upload_audio(env, key, audio, segment="120000_300")
    second = _upload_audio(env, key, audio, segment="121000_300")
    assert first.status_code == 200
    assert second.status_code == 200
    assert first.get_json()["segment"] == "120000_300"
    assert second.get_json()["segment"] == "121000_300"

    first_dup = _upload_audio(env, key, audio, segment="120000_300")
    second_dup = _upload_audio(env, key, audio, segment="121000_300")
    assert first_dup.get_json()["existing_segment"] == "120000_300"
    assert second_dup.get_json()["existing_segment"] == "121000_300"


def test_ingest_manifest_absence_or_corruption_uses_processing_proof(
    observer_env,
):
    """AC-8: absent/corrupt manifests heal without proof and dedupe with proof."""
    env = observer_env()
    key = _create_observer(env, "manifest-fallback-test")
    audio = b"manifest fallback audio"

    first = _upload_audio(env, key, audio)
    assert first.status_code == 200
    segment_dir = _day_dir(env) / "manifest-fallback-test" / "120000_300"

    (segment_dir / "audio.flac").unlink()
    heal = _upload_audio(env, key, audio)
    assert heal.status_code == 200
    assert heal.get_json()["status"] == "ok"
    assert (segment_dir / "audio.flac").read_bytes() == audio

    (segment_dir / "ingest.json").write_text("{not json\n", encoding="utf-8")
    _write_audio_processing_sidecar(segment_dir, input_size=len(audio))
    (segment_dir / "audio.flac").unlink()
    corrupt = _upload_audio(env, key, audio)
    assert corrupt.status_code == 200
    assert corrupt.get_json()["status"] == "duplicate"
    assert not (segment_dir / "audio.flac").exists()
    manifest = json.loads((segment_dir / "ingest.json").read_text(encoding="utf-8"))
    assert manifest["files"]["audio.flac"] == {
        "sha256": hashlib.sha256(audio).hexdigest(),
        "size": len(audio),
    }

    variants = [
        ("unknown-version", {"schema_version": 999}),
        ("missing-version", {}),
        ("string-version", {"schema_version": "1"}),
        ("bad-file-entry", {"schema_version": 1, "entry": "not-a-dict"}),
    ]
    for suffix, manifest_shape in variants:
        variant_audio = f"manifest {suffix} audio".encode()
        variant_key = _create_observer(env, f"manifest-{suffix}-test")
        first = _upload_audio(env, variant_key, variant_audio)
        assert first.status_code == 200
        variant_dir = _day_dir(env) / f"manifest-{suffix}-test" / "120000_300"
        _write_audio_processing_sidecar(variant_dir, input_size=len(variant_audio))
        (variant_dir / "audio.flac").unlink()
        manifest_files = {
            "audio.flac": {
                "sha256": hashlib.sha256(variant_audio).hexdigest(),
                "size": len(variant_audio),
            }
        }
        manifest = {
            "requested_segment": "120000_300",
            "files": manifest_files,
            **manifest_shape,
        }
        if suffix == "bad-file-entry":
            manifest["files"] = {"audio.flac": manifest_shape["entry"]}
        (variant_dir / "ingest.json").write_text(
            json.dumps(manifest) + "\n", encoding="utf-8"
        )

        retry = _upload_audio(env, variant_key, variant_audio)
        body = retry.get_json()
        assert retry.status_code == 200
        assert body["status"] == "duplicate"
        assert not (variant_dir / "audio.flac").exists()
        manifest = json.loads((variant_dir / "ingest.json").read_text("utf-8"))
        assert manifest["files"]["audio.flac"] == {
            "sha256": hashlib.sha256(variant_audio).hexdigest(),
            "size": len(variant_audio),
        }


def test_ingest_manifest_different_hash_with_terminal_proof_disqualifies_candidate(
    observer_env,
):
    """Differing manifest hash on absent content mints a sibling, not a heal."""
    env = observer_env()
    key = _create_observer(env, "manifest-diff-proof-test")
    original = b"manifest original audio"
    changed = b"manifest changed audio"

    first = _upload_audio(env, key, original)
    assert first.status_code == 200
    segment_dir = _day_dir(env) / "manifest-diff-proof-test" / "120000_300"
    manifest_before = (segment_dir / "ingest.json").read_bytes()
    _write_audio_processing_sidecar(segment_dir, input_size=len(original))
    proof_before = (segment_dir / "audio.jsonl").read_bytes()
    (segment_dir / "audio.flac").unlink()

    resp = _upload_audio(env, key, changed)
    body = resp.get_json()
    assert resp.status_code == 200
    assert body["status"] == "collision"
    assert body["segment"] == "120000_301"
    assert not (segment_dir / "audio.flac").exists()
    assert (segment_dir / "ingest.json").read_bytes() == manifest_before
    assert (segment_dir / "audio.jsonl").read_bytes() == proof_before
    assert (
        _day_dir(env) / "manifest-diff-proof-test" / "120000_301" / "audio.flac"
    ).read_bytes() == changed


def test_ingest_manifest_different_hash_without_terminal_proof_disqualifies_candidate(
    observer_env,
):
    """Differing manifest hash disqualifies absent content even without proof."""
    env = observer_env()
    key = _create_observer(env, "manifest-diff-no-proof-test")
    original = b"manifest original no proof"
    changed = b"manifest changed no proof"

    first = _upload_audio(env, key, original)
    assert first.status_code == 200
    segment_dir = _day_dir(env) / "manifest-diff-no-proof-test" / "120000_300"
    manifest_before = (segment_dir / "ingest.json").read_bytes()
    (segment_dir / "audio.flac").unlink()

    resp = _upload_audio(env, key, changed)
    body = resp.get_json()
    assert resp.status_code == 200
    assert body["status"] == "collision"
    assert body["segment"] == "120000_301"
    assert not (segment_dir / "audio.flac").exists()
    assert (segment_dir / "ingest.json").read_bytes() == manifest_before
    assert (
        _day_dir(env) / "manifest-diff-no-proof-test" / "120000_301" / "audio.flac"
    ).read_bytes() == changed


def test_ingest_adds_missing_media_to_candidate_without_fragmenting(observer_env):
    """Candidate with one media file accepts another media file for the same segment."""
    env = observer_env()
    key = _create_observer(env, "anti-fragment-test")
    screen = b"existing screen bytes"
    audio = b"new audio bytes"
    segment_dir = _day_dir(env) / "anti-fragment-test" / "120000_300"
    segment_dir.mkdir(parents=True)
    (segment_dir / "screen.mp4").write_bytes(screen)

    resp = _upload_audio(env, key, audio)
    body = resp.get_json()
    assert resp.status_code == 200
    assert body["status"] == "ok"
    assert body["segment"] == "120000_300"
    assert (segment_dir / "screen.mp4").read_bytes() == screen
    assert (segment_dir / "audio.flac").read_bytes() == audio
    assert not (segment_dir.parent / "120000_301").exists()


def test_ingest_unrelated_exact_key_candidate_accepts_additive_media(observer_env):
    """Occupied-but-non-conflicting exact-key dirs take additive writes."""
    env = observer_env()
    key = _create_observer(env, "unrelated-additive-test")
    existing = b"existing sidecar content"
    audio = b"new unrelated audio"
    segment_dir = _day_dir(env) / "unrelated-additive-test" / "120000_300"
    segment_dir.mkdir(parents=True)
    (segment_dir / "existing.txt").write_bytes(existing)

    # Spec §5.2.6: the ladder fires on conflicting content at the requested
    # key, not on an occupied key; non-conflicting candidates are additive
    # writes (§5.2.4).
    resp = _upload_audio(env, key, audio)
    body = resp.get_json()

    assert resp.status_code == 200
    assert body["status"] == "ok"
    assert body["segment"] == "120000_300"
    assert (segment_dir / "existing.txt").read_bytes() == existing
    assert (segment_dir / "audio.flac").read_bytes() == audio
    assert not (segment_dir.parent / "120000_301").exists()


def test_ingest_stream_marker_only_candidate_accepts_media_retry(observer_env):
    """Half-minted stream marker dirs accept media retry in place."""
    env = observer_env()
    key = _create_observer(env, "half-minted-test")
    audio = b"retry audio bytes"
    segment_dir = _day_dir(env) / "half-minted-test" / "120000_300"
    segment_dir.mkdir(parents=True)
    write_segment_stream(segment_dir, "half-minted-test", None, None, 1)
    marker_before = (segment_dir / "stream.json").read_bytes()

    resp = _upload_audio(env, key, audio)
    body = resp.get_json()

    assert resp.status_code == 200
    assert body["status"] == "ok"
    assert body["segment"] == "120000_300"
    assert (segment_dir / "audio.flac").read_bytes() == audio
    assert (segment_dir / "stream.json").read_bytes() == marker_before
    assert not (segment_dir.parent / "120000_301").exists()


def test_ingest_manifest_accumulates_and_duplicates_after_media_deleted(
    observer_env,
):
    """Manifest keeps prior media identity when later bundles add sidecars."""
    env = observer_env()
    key = _create_observer(env, "manifest-accumulates-test")
    audio = b"accumulating audio"
    screen = b"accumulating screen"
    notes = b"later sidecar"

    first = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(screen), "screen.mp4"),
            ],
        },
    )
    assert first.status_code == 200
    segment_dir = _day_dir(env) / "manifest-accumulates-test" / "120000_300"

    sidecar = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(notes), "notes.txt"),
        },
    )
    assert sidecar.status_code == 200
    manifest = json.loads((segment_dir / "ingest.json").read_text(encoding="utf-8"))
    assert (
        manifest["files"]["audio.flac"]["sha256"] == hashlib.sha256(audio).hexdigest()
    )
    assert (
        manifest["files"]["screen.mp4"]["sha256"] == hashlib.sha256(screen).hexdigest()
    )
    assert manifest["files"]["notes.txt"]["sha256"] == hashlib.sha256(notes).hexdigest()

    _write_audio_processing_sidecar(segment_dir, input_size=len(audio))
    _write_screen_processing_sidecar(segment_dir, input_size=len(screen))
    (segment_dir / "audio.flac").unlink()
    (segment_dir / "screen.mp4").unlink()

    duplicate = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(screen), "screen.mp4"),
            ],
        },
    )
    assert duplicate.status_code == 200
    assert duplicate.get_json()["status"] == "duplicate"
    assert not (segment_dir / "audio.flac").exists()
    assert not (segment_dir / "screen.mp4").exists()


def test_ingest_legacy_processed_duplicate_graduates_manifest_and_reuploads(
    observer_env,
):
    env = observer_env()
    observer_name = "legacy-processed-graduates-test"
    key = _create_observer(env, observer_name)
    audio = b"legacy processed audio"
    screen = b"legacy processed screen"
    segment_dir = _prepare_legacy_processed_segment(
        env, key, observer_name, audio, screen
    )
    stream_dir = segment_dir.parent
    observer = _observer_record()
    key_prefix = observer["filename_prefix"]
    initial_history_lines = _history_line_count(env, key_prefix)
    assert initial_history_lines == 1
    assert not (segment_dir / "ingest.json").exists()

    for index in range(3):
        resp = _post_raw_audio_screen_bundle(env, key, observer_name, audio, screen)
        body = resp.get_json()

        assert resp.status_code == 200
        assert body["status"] == "duplicate"
        assert not (segment_dir / "audio.flac").exists()
        assert not (segment_dir / "screen.mp4").exists()
        assert len(list(stream_dir.iterdir())) == 1
        assert _history_line_count(env, key_prefix) == initial_history_lines + index + 1

        latest = load_history(key_prefix, "20250103")[-1]
        dispositions = {
            file["written"]: file["disposition"] for file in latest["files"]
        }
        assert dispositions["audio.flac"] == "already_held"
        assert dispositions["screen.mp4"] == "already_held"

        manifest = json.loads((segment_dir / "ingest.json").read_text("utf-8"))
        assert manifest["files"]["audio.flac"] == {
            "sha256": hashlib.sha256(audio).hexdigest(),
            "size": len(audio),
        }
        assert manifest["files"]["screen.mp4"] == {
            "sha256": hashlib.sha256(screen).hexdigest(),
            "size": len(screen),
        }


def test_segments_endpoint_reports_processed_for_legacy_duplicate(observer_env):
    env = observer_env()
    observer_name = "legacy-processed-listing-test"
    key = _create_observer(env, observer_name)
    audio = b"legacy processed listing audio"
    screen = b"legacy processed listing screen"
    _prepare_legacy_processed_segment(env, key, observer_name, audio, screen)

    duplicate = _post_raw_audio_screen_bundle(env, key, observer_name, audio, screen)
    assert duplicate.status_code == 200
    assert duplicate.get_json()["status"] == "duplicate"

    files = _listed_files_by_name(env, key)
    assert files["audio.flac"]["status"] == "processed"
    assert files["screen.mp4"]["status"] == "processed"


@pytest.mark.parametrize(
    "proof_variant",
    ["size_mismatch", "failed_state", "handler_mismatch"],
)
def test_ingest_legacy_processed_proof_mismatch_heals(
    observer_env,
    proof_variant: str,
):
    env = observer_env()
    observer_name = f"legacy-proof-mismatch-{proof_variant}"
    key = _create_observer(env, observer_name)
    audio = b"legacy mismatch audio"
    screen = b"legacy mismatch screen"
    segment_dir = _prepare_legacy_processed_segment(
        env, key, observer_name, audio, screen
    )

    if proof_variant == "size_mismatch":
        _write_audio_processing_sidecar(segment_dir, input_size=len(audio) + 1)
    elif proof_variant == "failed_state":
        _write_audio_processing_sidecar(
            segment_dir, input_size=len(audio), state=STATE_FAILED
        )
    else:
        _write_audio_processing_sidecar(
            segment_dir, input_size=len(audio), handler=HANDLER_DESCRIBE
        )

    resp = _post_raw_audio_screen_bundle(env, key, observer_name, audio, screen)
    body = resp.get_json()

    assert resp.status_code == 200
    assert body["status"] == "ok"
    assert (segment_dir / "audio.flac").read_bytes() == audio
    assert not (segment_dir.parent / "120000_301").exists()


def test_ingest_legacy_processed_sidecar_conflict_returns_409(observer_env):
    env = observer_env()
    observer_name = "legacy-processed-sidecar-conflict-test"
    key = _create_observer(env, observer_name)
    audio = b"legacy sidecar conflict audio"
    screen = b"legacy sidecar conflict screen"
    segment_dir = _prepare_legacy_processed_segment(
        env, key, observer_name, audio, screen
    )
    proof_before = (segment_dir / "audio.jsonl").read_bytes()
    changed_sidecar = b'{"raw":"audio.flac"}\n{"start":"00:00:00","text":"changed"}\n'
    stream_marker = json.dumps(
        {"stream": observer_name, "prev_day": None, "prev_segment": None, "seq": 1}
    ).encode("utf-8")

    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(screen), "screen.mp4"),
                (io.BytesIO(changed_sidecar), "audio.jsonl"),
                (io.BytesIO(stream_marker + b"\n"), "stream.json"),
            ],
        },
    )
    body = resp.get_json()

    assert resp.status_code == 409
    assert body["reason_code"] == "ingest_sidecar_conflict"
    assert body["conflicting_files"] == ["audio.jsonl"]
    assert not (segment_dir / "audio.flac").exists()
    assert not (segment_dir / "screen.mp4").exists()
    assert (segment_dir / "audio.jsonl").read_bytes() == proof_before
    assert not (segment_dir.parent / "120000_301").exists()


def test_ingest_held_content_never_mints_for_request_shape(observer_env):
    """AC-9: held content resolves in place across request shapes."""
    env = observer_env()
    audio = b"held media content"
    tmux = b'{"line": 1}\n'
    cases = [
        (
            "media-only-exact",
            [(audio, "audio.flac")],
            [(audio, "audio.flac")],
            "120000_300",
            "duplicate",
        ),
        (
            "media-new-sidecar",
            [(audio, "audio.flac")],
            [(audio, "audio.flac"), (b"notes", "notes.txt")],
            "120000_300",
            "ok",
        ),
        (
            "media-reserved",
            [(audio, "audio.flac")],
            [
                (audio, "audio.flac"),
                (
                    b'{"stream":"held-shape-media-reserved","prev_day":null,'
                    b'"prev_segment":null,"seq":1}\n',
                    "stream.json",
                ),
            ],
            "120000_300",
            "duplicate",
        ),
        (
            "media-laddered-key",
            [(audio, "audio.flac")],
            [(audio, "audio.flac")],
            "120000_301",
            "duplicate",
        ),
        (
            "media-different-len",
            [(audio, "audio.flac")],
            [(audio, "audio.flac")],
            "120000_999",
            "duplicate",
        ),
        (
            "media-less-exact",
            [(tmux, "tmux.jsonl")],
            [(tmux, "tmux.jsonl")],
            "120000_300",
            "duplicate",
        ),
        (
            "media-less-laddered-key",
            [(tmux, "tmux.jsonl")],
            [(tmux, "tmux.jsonl")],
            "120000_301",
            "duplicate",
        ),
    ]

    for suffix, initial_files, retry_files, requested_segment, expected_status in cases:
        observer_name = f"held-shape-{suffix}"
        key = _create_observer(env, observer_name)
        first = env.client.post(
            "/app/observer/ingest",
            headers={"Authorization": f"Bearer {key}"},
            data={
                "day": "20250103",
                "segment": "120000_300",
                "files": [
                    (io.BytesIO(content), name) for content, name in initial_files
                ],
            },
        )
        assert first.status_code == 200

        retry = env.client.post(
            "/app/observer/ingest",
            headers={"Authorization": f"Bearer {key}"},
            data={
                "day": "20250103",
                "segment": requested_segment,
                "files": [(io.BytesIO(content), name) for content, name in retry_files],
            },
        )
        body = retry.get_json()
        assert retry.status_code == 200
        assert body["status"] == expected_status
        if expected_status == "duplicate":
            assert body["existing_segment"] == "120000_300"
        else:
            assert body["segment"] == "120000_300"
        assert sorted(
            path.name for path in (_day_dir(env) / observer_name).iterdir()
        ) == ["120000_300"]


def test_ingest_create_collision_reenters_once(observer_env, monkeypatch):
    """AC-11: create-exclusive races re-enter once without random siblings."""
    env = observer_env()
    key = _create_observer(env, "race-test")
    audio = b"loser audio"
    real_write = observer_utils.write_bytes_exclusive
    calls = {"count": 0}

    def race_once(path: Path, data: bytes, *, mode=None):
        calls["count"] += 1
        if calls["count"] == 1:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"winner audio")
            raise FileExistsError(path)
        return real_write(path, data, mode=mode)

    monkeypatch.setattr(observer_utils, "write_bytes_exclusive", race_once)

    resp = _upload_audio(env, key, audio)
    assert resp.status_code == 200
    body = resp.get_json()
    assert body["status"] == "collision"
    assert body["segment"] == "120000_301"

    stream_dir = _day_dir(env) / "race-test"
    assert (stream_dir / "120000_300" / "audio.flac").read_bytes() == b"winner audio"
    assert (stream_dir / "120000_301" / "audio.flac").read_bytes() == audio
    assert not (stream_dir / "120000_302").exists()


def test_ingest_create_collision_identical_bytes_stays_on_single_dir(
    observer_env, monkeypatch
):
    """AC-11: identical create races succeed without ladder siblings."""
    env = observer_env()
    key = _create_observer(env, "identical-race-test")
    audio = b"same raced audio"
    real_write = observer_utils.write_bytes_exclusive
    calls = {"count": 0}

    def race_same_once(path: Path, data: bytes, *, mode=None):
        calls["count"] += 1
        if calls["count"] == 1:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            raise FileExistsError(path)
        return real_write(path, data, mode=mode)

    monkeypatch.setattr(observer_utils, "write_bytes_exclusive", race_same_once)

    first = _upload_audio(env, key, audio)
    assert first.status_code == 200
    assert first.get_json()["status"] == "ok"

    second = _upload_audio(env, key, audio)
    assert second.status_code == 200
    assert second.get_json()["status"] == "duplicate"

    stream_dir = _day_dir(env) / "identical-race-test"
    assert sorted(path.name for path in stream_dir.iterdir()) == ["120000_300"]


def test_ingest_multifile_create_collision_reenters_once(observer_env, monkeypatch):
    """AC-11: a second-file race re-enters once and preserves landed bytes."""
    env = observer_env()
    key = _create_observer(env, "multifile-race-test")
    audio = b"loser audio"
    screen = b"loser screen"
    winner_screen = b"winner screen"
    real_write = observer_utils.write_bytes_exclusive
    calls = {"count": 0}

    def race_on_second_file(path: Path, data: bytes, *, mode=None):
        calls["count"] += 1
        if calls["count"] == 2:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(winner_screen)
            raise FileExistsError(path)
        return real_write(path, data, mode=mode)

    monkeypatch.setattr(observer_utils, "write_bytes_exclusive", race_on_second_file)

    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(screen), "screen.mp4"),
            ],
        },
    )
    body = resp.get_json()

    assert resp.status_code == 200
    assert body["status"] == "collision"
    assert body["segment"] == "120000_301"
    stream_dir = _day_dir(env) / "multifile-race-test"
    assert (stream_dir / "120000_300" / "audio.flac").read_bytes() == audio
    assert (stream_dir / "120000_300" / "screen.mp4").read_bytes() == winner_screen
    assert (stream_dir / "120000_301" / "audio.flac").read_bytes() == audio
    assert (stream_dir / "120000_301" / "screen.mp4").read_bytes() == screen
    assert not (stream_dir / "120000_302").exists()


def test_ingest_duplicate_rechecks_held_files_before_response(
    observer_env, monkeypatch
):
    """Held files that vanish after planning heal instead of returning duplicate."""
    env = observer_env()
    key = _create_observer(env, "held-recheck-test")
    audio = b"held recheck audio"

    first = _upload_audio(env, key, audio)
    assert first.status_code == 200
    segment_dir = _day_dir(env) / "held-recheck-test" / "120000_300"
    real_recheck = observer_utils._held_files_are_current
    calls = {"count": 0}

    def delete_once_then_check(path: Path, files):
        calls["count"] += 1
        if calls["count"] == 1:
            (path / files[0].written).unlink()
        return real_recheck(path, files)

    monkeypatch.setattr(
        observer_utils, "_held_files_are_current", delete_once_then_check
    )

    retry = _upload_audio(env, key, audio)
    body = retry.get_json()

    assert retry.status_code == 200
    assert body["status"] == "ok"
    assert body["segment"] == "120000_300"
    assert (segment_dir / "audio.flac").read_bytes() == audio


def test_ingest_manifest_locked_merge_preserves_concurrent_additions(
    observer_env, monkeypatch
):
    """Concurrent manifest read-merge-write keeps both additive entries."""
    env = observer_env()
    segment_dir = _day_dir(env) / "manifest-lock-test" / "120000_300"
    segment_dir.mkdir(parents=True)
    base = observer_utils.IngestFile(
        submitted="audio.flac",
        written="audio.flac",
        content=b"base audio",
        sha256=hashlib.sha256(b"base audio").hexdigest(),
    )
    observer_utils.write_ingest_manifest(
        segment_dir, requested_segment="120000_300", files=[base]
    )

    real_atomic_replace = observer_utils.atomic_replace
    first_replace_entered = threading.Event()
    release_first_replace = threading.Event()
    calls = {"count": 0}

    def slow_first_replace(path: Path, data, *, mode=None):
        if path.name == "ingest.json":
            calls["count"] += 1
            if calls["count"] == 1:
                first_replace_entered.set()
                assert release_first_replace.wait(timeout=5)
        return real_atomic_replace(path, data, mode=mode)

    monkeypatch.setattr(observer_utils, "atomic_replace", slow_first_replace)

    def ingest_file(name: str, content: bytes) -> observer_utils.IngestFile:
        return observer_utils.IngestFile(
            submitted=name,
            written=name,
            content=content,
            sha256=hashlib.sha256(content).hexdigest(),
        )

    errors = []

    def write_file(file: observer_utils.IngestFile) -> None:
        try:
            observer_utils.write_ingest_manifest(
                segment_dir, requested_segment="120000_300", files=[file]
            )
        except Exception as exc:  # pragma: no cover - surfaced by assertion below
            errors.append(exc)

    first_file = ingest_file("notes-a.txt", b"notes a")
    second_file = ingest_file("notes-b.txt", b"notes b")
    first_thread = threading.Thread(target=write_file, args=(first_file,))
    second_thread = threading.Thread(target=write_file, args=(second_file,))
    first_thread.start()
    assert first_replace_entered.wait(timeout=5)
    second_thread.start()
    time.sleep(0.1)
    release_first_replace.set()
    first_thread.join(timeout=5)
    second_thread.join(timeout=5)

    assert not first_thread.is_alive()
    assert not second_thread.is_alive()
    assert errors == []
    manifest = json.loads((segment_dir / "ingest.json").read_text(encoding="utf-8"))
    assert set(manifest["files"]) == {"audio.flac", "notes-a.txt", "notes-b.txt"}


def test_ingest_stream_chain_unchanged_for_existing_candidate_resolutions(observer_env):
    """AC-12: duplicate/additive/heal do not rewrite stream state or marker."""
    env = observer_env()
    key = _create_observer(env, "stream-chain-test")
    audio = b"stream chain audio"

    first = _upload_audio(env, key, audio)
    assert first.status_code == 200
    segment_dir = _day_dir(env) / "stream-chain-test" / "120000_300"
    marker_before = (segment_dir / "stream.json").read_bytes()
    state_path = env.journal / "streams" / "stream-chain-test.json"
    state_before = state_path.read_bytes()

    duplicate = _upload_audio(env, key, audio)
    assert duplicate.get_json()["status"] == "duplicate"

    (segment_dir / "audio.flac").unlink()
    heal = _upload_audio(env, key, audio)
    assert heal.get_json()["status"] == "ok"

    additive = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "audio.flac"),
                (io.BytesIO(b"sidecar"), "sidecar.txt"),
            ],
        },
    )
    assert additive.get_json()["status"] == "ok"

    assert (segment_dir / "stream.json").read_bytes() == marker_before
    assert state_path.read_bytes() == state_before


def test_ingest_duplicate_against_import_created_segment_is_corroborated(
    observer_env,
):
    """AC-13: duplicate against no-history disk content creates listing proof."""
    env = observer_env()
    key = _create_observer(env, "import-candidate-test")
    audio = b"import-created audio"
    segment_dir = _day_dir(env) / "import-candidate-test" / "120000_300"
    segment_dir.mkdir(parents=True)
    (segment_dir / "audio.flac").write_bytes(audio)

    resp = _upload_audio(env, key, audio)
    assert resp.status_code == 200
    body = resp.get_json()
    assert body["status"] == "duplicate"
    assert body["existing_segment"] == "120000_300"

    file_info = _listed_file_info(env, key)
    assert file_info["name"] == "audio.flac"
    assert file_info["status"] == "present"
    assert file_info["sha256"] == hashlib.sha256(audio).hexdigest()
    manifest = json.loads((segment_dir / "ingest.json").read_text(encoding="utf-8"))
    assert manifest == {
        "files": {
            "audio.flac": {
                "sha256": hashlib.sha256(audio).hexdigest(),
                "size": len(audio),
            }
        },
        "requested_segment": "120000_300",
        "schema_version": 1,
    }


def test_ingest_duplicate_listing_keeps_same_sha_under_different_names(observer_env):
    """AC-13: listing corroborates every submitted name, not only every sha."""
    env = observer_env()
    key = _create_observer(env, "same-sha-names-test")
    content = b"same bytes under two names"
    sha256 = hashlib.sha256(content).hexdigest()

    first = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(content), "left.flac"),
                (io.BytesIO(content), "right.flac"),
            ],
        },
    )
    assert first.status_code == 200

    duplicate = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(content), "left.flac"),
                (io.BytesIO(content), "right.flac"),
            ],
        },
    )
    assert duplicate.status_code == 200
    assert duplicate.get_json()["status"] == "duplicate"

    files = _listed_files_by_name(env, key)
    assert files["left.flac"]["sha256"] == sha256
    assert files["left.flac"]["status"] == "present"
    assert files["right.flac"]["sha256"] == sha256
    assert files["right.flac"]["status"] == "present"


def test_ingest_returns_collision_status_when_adjusted(observer_env):
    """Test that collision resolution returns status='collision'."""
    env = observer_env()

    # Create a observer
    resp = env.register_bound_observer("collision-status-test")
    key = resp.get_json()["key"]

    # Spec §5.2.6: the ladder fires on conflicting content at the requested
    # key, not on an occupied key; non-conflicting candidates are additive
    # writes (§5.2.4).
    day_dir = _day_dir(env)
    stream_dir = day_dir / "collision-status-test"
    stream_dir.mkdir(parents=True)
    (stream_dir / "120000_300").mkdir()
    (stream_dir / "120000_300" / "audio.flac").write_bytes(b"old content")

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
    resp = env.register_bound_observer("test-observer")
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
    resp = env.register_bound_observer("test-observer")
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


def test_ingest_contract_sidecar_invalid_quarantined_without_emit(
    observer_env, monkeypatch
):
    env = observer_env()
    emitted = []
    monkeypatch.setattr(
        routes_module,
        "emit",
        lambda tract, event, **fields: emitted.append((tract, event, fields)),
    )

    resp = env.register_bound_observer("contract-test")
    key = resp.get_json()["key"]

    invalid_audio = b'{"raw":"audio.flac"}\n{"start":"00:00:00"}\n'
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(invalid_audio), "120000_300_audio.jsonl"),
        },
    )

    body = resp.get_json()
    assert resp.status_code == 422
    assert body["status"] == "failed"
    assert body["reason_code"] == "ingest_contract_invalid"
    assert any(
        "audio.jsonl" in item and "text" in item for item in body["invalid_files"]
    )
    assert emitted == []

    assert not (_day_dir(env) / "contract-test" / "120000_300").exists()
    failed_dir = env.journal / "chronicle" / body["failed_path"]
    assert failed_dir.exists()
    assert (failed_dir / "120000_300_audio.jsonl").read_bytes() == invalid_audio


def test_ingest_contract_invalid_records_rejection(observer_env):
    env = observer_env()
    key = _create_observer(env, "contract-rejection-test")

    resp = _post_invalid_contract_audio(env, key)

    body = resp.get_json()
    assert resp.status_code == 422
    assert body["status"] == "failed"
    assert body["reason_code"] == "ingest_contract_invalid"
    assert any(
        "audio.jsonl" in item and "text" in item for item in body["invalid_files"]
    )
    assert "failed_path" in body

    rejection = _observer_record()["health"]["ingest_rejection"]
    assert rejection["reason_code"] == "ingest_contract_invalid"
    assert rejection["active_count"] == 1
    assert rejection["segment"] == "120000_300"
    assert rejection["stream"] == "contract-rejection-test"
    assert rejection["summary"]
    assert rejection["version"] == "test"


def test_repeated_invalid_keeps_first_ts_increments_count(observer_env, monkeypatch):
    env = observer_env()
    key = _create_observer(env, "contract-repeat-test")
    ticks = iter([1000, 2000])
    monkeypatch.setattr("solstone.apps.observer.utils.now_ms", lambda: next(ticks))

    first = _post_invalid_contract_audio(env, key)
    assert first.status_code == 422
    first_rejection = _observer_record()["health"]["ingest_rejection"]

    second = _post_invalid_contract_audio(env, key)
    assert second.status_code == 422
    second_rejection = _observer_record()["health"]["ingest_rejection"]

    assert second_rejection["first_ts"] == first_rejection["first_ts"]
    assert second_rejection["latest_ts"] > first_rejection["latest_ts"]
    assert second_rejection["active_count"] == 2


def test_valid_upload_clears_rejection(observer_env):
    env = observer_env()
    key = _create_observer(env, "valid-clear-test")

    invalid = _post_invalid_contract_audio(env, key)
    assert invalid.status_code == 422
    assert "ingest_rejection" in _observer_record()["health"]

    valid = _post_valid_contract_triple(env, key)
    assert valid.status_code == 200
    assert valid.get_json()["status"] == "ok"

    record = _observer_record()
    assert "ingest_rejection" not in record.get("health", {})
    assert record["stats"]["segments_received"] == 1


def test_duplicate_after_validation_clears_rejection(observer_env):
    env = observer_env()
    key = _create_observer(env, "duplicate-clear-test")

    first = _post_valid_contract_triple(env, key)
    assert first.status_code == 200
    assert first.get_json()["status"] == "ok"

    invalid = _post_invalid_contract_audio(env, key)
    assert invalid.status_code == 422
    assert "ingest_rejection" in _observer_record()["health"]

    duplicate = _post_valid_contract_triple(env, key)
    assert duplicate.status_code == 200
    assert duplicate.get_json()["status"] == "duplicate"
    assert "ingest_rejection" not in _observer_record().get("health", {})


def test_bounded_summary_no_content_leak(observer_env):
    summary = sanitize_validation_summary(
        [
            ContractIssue(
                "screen.jsonl:2:timestamp",
                "'LEAKING_OCR_CONTENT_12345' is not of type 'number'",
            )
        ]
    )
    assert "LEAKING_OCR_CONTENT_12345" not in summary
    assert "value is not of type 'number'" in summary
    assert len(summary) <= 240

    required_summary = sanitize_validation_summary(
        [ContractIssue("screen.jsonl:2", "'timestamp' is a required property")]
    )
    assert "is a required property" in required_summary

    env = observer_env()
    key = _create_observer(env, "leak-test")
    leaking_screen = (
        b'{"raw":"screen.mp4"}\n{"timestamp":"LEAKING_OCR_CONTENT_12345"}\n'
    )
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(leaking_screen), "120000_300_screen.jsonl"),
        },
    )

    assert resp.status_code == 422
    persisted_summary = _observer_record()["health"]["ingest_rejection"]["summary"]
    assert "LEAKING_OCR_CONTENT_12345" not in persisted_summary
    assert "value is not of type 'number'" in persisted_summary


def test_disabled_observer_no_ingest_state(observer_env):
    env = observer_env()
    key = _create_observer(env, "disabled-ingest-test")
    observer = _observer_record()
    observer["enabled"] = False
    assert save_observer(observer)

    resp = _post_invalid_contract_audio(env, key)

    body = resp.get_json()
    assert resp.status_code == 403
    assert body["reason_code"] == "feature_unavailable"
    assert "ingest_rejection" not in _observer_record().get("health", {})


def test_ingest_contract_sidecars_valid_are_accepted(observer_env, monkeypatch):
    env = observer_env()
    emitted = []
    monkeypatch.setattr(
        routes_module,
        "emit",
        lambda tract, event, **fields: emitted.append((tract, event, fields)),
    )

    resp = env.register_bound_observer("contract-valid-test")
    key = resp.get_json()["key"]

    audio = b'{"raw":"audio.flac"}\n{"start":"00:00:00","text":"hello"}\n'
    screen = b'{"raw":"screen.mp4","qualified_count":1}\n{"timestamp":1.0}\n'
    stream = b'{"stream":"contract-valid-test","prev_day":null,"prev_segment":null,"seq":1}\n'
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "120000_300_audio.jsonl"),
                (io.BytesIO(screen), "screen.jsonl"),
                (io.BytesIO(stream), "stream.json"),
            ],
        },
    )

    body = resp.get_json()
    assert resp.status_code == 200
    assert body["status"] == "ok"
    # Spec §5.2.1: reserved names are never written from client bytes.
    assert body["files"] == ["audio.jsonl", "screen.jsonl"]
    assert len(emitted) == 1

    segment_dir = _day_dir(env) / "contract-valid-test" / "120000_300"
    assert (segment_dir / "stream.json").read_bytes() != stream
    assert (
        b'"stream": "contract-valid-test"' in (segment_dir / "stream.json").read_bytes()
    )

    observer = _observer_record()
    records = load_history(observer["filename_prefix"], "20250103")
    latest_files = {file["written"]: file for file in records[-1]["files"]}
    assert latest_files["stream.json"]["disposition"] == "received_not_written"


def test_ingest_malformed_reserved_contract_file_is_bundle_422(observer_env):
    """AC-14 / D3: malformed reserved files still fail whole-bundle validation."""
    env = observer_env()
    key = _create_observer(env, "reserved-invalid-test")
    invalid_ingest = (
        b'{"schema_version":2,"requested_segment":"120000_300","files":{}}\n'
    )

    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(invalid_ingest), "ingest.json"),
        },
    )

    body = resp.get_json()
    assert resp.status_code == 422
    assert body["reason_code"] == "ingest_contract_invalid"
    assert any("ingest.json" in item for item in body["invalid_files"])
    assert not (_day_dir(env) / "reserved-invalid-test" / "120000_300").exists()


def test_ingest_contract_sidecars_without_raw_are_accepted(observer_env, monkeypatch):
    env = observer_env()
    emitted = []
    monkeypatch.setattr(
        routes_module,
        "emit",
        lambda tract, event, **fields: emitted.append((tract, event, fields)),
    )

    resp = env.register_bound_observer("contract-no-raw-test")
    key = resp.get_json()["key"]

    audio = b'{"observer":"external"}\n{"start":"00:00:00","text":"hi"}\n'
    screen = b'{"observer":"tmux"}\n{"timestamp":1.0}\n'
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": [
                (io.BytesIO(audio), "120000_300_audio.jsonl"),
                (io.BytesIO(screen), "screen.jsonl"),
            ],
        },
    )

    body = resp.get_json()
    assert resp.status_code == 200
    assert body["status"] == "ok"
    assert body["files"] == ["audio.jsonl", "screen.jsonl"]
    assert len(emitted) == 1
    segment_dir = _day_dir(env) / "contract-no-raw-test" / "120000_300"
    assert (segment_dir / "audio.jsonl").read_bytes() == audio
    assert (segment_dir / "screen.jsonl").read_bytes() == screen
    assert not (_day_dir(env) / "observer" / "failed").exists()


def test_ingest_contract_screen_floor_violation_quarantined_without_emit(
    observer_env, monkeypatch
):
    env = observer_env()
    emitted = []
    monkeypatch.setattr(
        routes_module,
        "emit",
        lambda tract, event, **fields: emitted.append((tract, event, fields)),
    )

    resp = env.register_bound_observer("contract-screen-invalid-test")
    key = resp.get_json()["key"]

    invalid_screen = b'{"observer":"tmux"}\n{"content":{}}\n'
    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(invalid_screen), "screen.jsonl"),
        },
    )

    body = resp.get_json()
    assert resp.status_code == 422
    assert body["status"] == "failed"
    assert body["reason_code"] == "ingest_contract_invalid"
    assert any(
        "screen.jsonl" in item and "timestamp" in item for item in body["invalid_files"]
    )
    assert emitted == []

    assert not (_day_dir(env) / "contract-screen-invalid-test" / "120000_300").exists()
    failed_dir = env.journal / "chronicle" / body["failed_path"]
    assert failed_dir.exists()
    assert (failed_dir / "screen.jsonl").read_bytes() == invalid_screen


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
    resp = env.register_bound_observer("fedora.tmux")
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


def test_manifest_day_listing(observer_env):
    """Test manifest day listing from observer history."""
    env = observer_env()

    resp = env.register_bound_observer("manifest-list-test")
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

    resp = env.client.get(
        "/app/observer/ingest/manifest",
        headers={"Authorization": f"Bearer {key}"},
    )
    assert resp.status_code == 200
    assert resp.get_json() == {"days": {"20250103": {"segments": 1}}}


def test_manifest_per_day(observer_env):
    """Test per-day manifest format matches transfer manifest v1."""
    env = observer_env()

    resp = env.register_bound_observer("manifest-day-test")
    key = resp.get_json()["key"]

    resp = env.client.post(
        "/app/observer/ingest",
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "meta": json.dumps({"stream": "remote.host"}),
            "files": [
                (io.BytesIO(b"audio bytes"), "audio.flac"),
                (io.BytesIO(b"screen bytes"), "screen.webm"),
            ],
        },
    )
    assert resp.status_code == 200

    resp = env.client.get(
        "/app/observer/ingest/manifest/20250103",
        headers={"Authorization": f"Bearer {key}"},
    )
    assert resp.status_code == 200
    data = resp.get_json()

    assert data["version"] == 1
    assert data["day"] == "20250103"
    assert isinstance(data["created_at"], int)
    assert "host" in data
    assert "manifest-day-test/120000_300" in data["segments"]

    files = data["segments"]["manifest-day-test/120000_300"]["files"]
    names = {file_info["name"] for file_info in files}
    assert {"audio.flac", "screen.webm"}.issubset(names)
    for file_info in files:
        assert set(file_info) == {"name", "sha256", "size"}
        assert len(file_info["sha256"]) == 64


def test_manifest_auth_required(observer_env):
    """Test manifest endpoint rejects invalid key."""
    env = observer_env()

    resp = env.client.get("/app/observer/ingest/manifest")
    assert resp.status_code == 401
