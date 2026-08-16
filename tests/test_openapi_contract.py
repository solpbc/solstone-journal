# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import importlib
import io
from copy import deepcopy
from pathlib import Path
from typing import Any

import pytest

import solstone.convey.chat as chat
from solstone.convey import create_app
from solstone.convey.chat import ChatSpawnResult
from solstone.convey.contract.assemble import (
    FRAGMENT_MODULES,
    build_document,
    rule_to_openapi_path,
)
from solstone.convey.contract.diff import (
    classify_changes,
    undeclared_top_level_fields,
)
from solstone.convey.secure_listener.identity import ConveyIdentity
from tests._baseline_harness import (
    isolated_app_env,
    mark_setup_complete,
    prepare_isolated_journal,
)

# These operations terminate in Rust Convey, so Flask is not their route owner.
# Keep this narrow list aligned with scripts/check_native_sol_conformance.py.
RUST_CONVEY_OPERATION_PREFIXES = (
    "body.",
    "import.",
    "settings.",
    "sol.",
    "speakers.",
    "transcripts.",
)


def _all_operations():
    operations = []
    for module_name in FRAGMENT_MODULES:
        operations.extend(importlib.import_module(module_name).OPERATIONS)
    return operations


CONTRACTED_PATHS = {
    rule_to_openapi_path(operation.rule) for operation in _all_operations()
}

CONTRACTED_INVENTORY_TRIPLES = {
    ("POST", "/api/chat", "chat.postMessage"),
    ("GET", "/api/chat/session", "chat.session"),
    ("POST", "/api/chat/sol_chat_request/open", "chat.openSolChatRequest"),
    ("POST", "/api/chat/offer/decline", "chat.declineOffer"),
    ("POST", "/api/chat/support/draft/confirm", "chat.supportDraftConfirm"),
    ("POST", "/api/chat/support/draft/cancel", "chat.supportDraftCancel"),
    ("GET", "/sse/events", "callosum.rootEvents"),
    ("POST", "/api/voice/session", "voice.session"),
    ("POST", "/api/voice/connect", "voice.connect"),
    ("GET", "/api/voice/status", "voice.status"),
    ("GET", "/app/home/api/pulse", "home.pulse"),
    ("POST", "/app/import/api/meta", "import.meta"),
    ("POST", "/app/import/api/save", "import.save"),
    ("POST", "/app/import/api/save-path", "import.savePath"),
    ("POST", "/app/import/api/start", "import.start"),
    ("GET", "/app/activities/api/day/{day}/records", "activities.list"),
    ("GET", "/app/activities/api/day/{day}/record/{span_id}", "activities.get"),
    ("POST", "/app/activities/api/day/{day}/records", "activities.create"),
    (
        "POST",
        "/app/activities/api/day/{day}/record/{span_id}/update",
        "activities.update",
    ),
    (
        "POST",
        "/app/activities/api/day/{day}/record/{span_id}/mute",
        "activities.mute",
    ),
    (
        "POST",
        "/app/activities/api/day/{day}/record/{span_id}/unmute",
        "activities.unmute",
    ),
    ("GET", "/app/support/api/config", "support.config"),
    ("POST", "/app/support/api/draft", "support.draft"),
    ("POST", "/app/support/api/register", "support.register"),
    ("GET", "/app/support/api/articles", "support.search"),
    ("GET", "/app/support/api/articles/{slug}", "support.article"),
    ("GET", "/app/support/api/tickets", "support.list"),
    ("GET", "/app/support/api/tickets/{ticket_id}", "support.show"),
    ("GET", "/app/support/api/tickets/closed", "support.history"),
    ("POST", "/app/support/api/tickets", "support.create"),
    ("POST", "/app/support/api/tickets/{ticket_id}/reply", "support.reply"),
    ("POST", "/app/support/api/tickets/{ticket_id}/close", "support.close"),
    (
        "POST",
        "/app/support/api/tickets/{ticket_id}/resolution/confirm",
        "support.resolved",
    ),
    (
        "POST",
        "/app/support/api/tickets/{ticket_id}/resolution/still-need-help",
        "support.still_need_help",
    ),
    (
        "POST",
        "/app/support/api/tickets/{ticket_id}/attachments",
        "support.attach",
    ),
    ("POST", "/app/support/api/feedback", "support.feedback"),
    ("GET", "/app/support/api/announcements", "support.announcements"),
    ("GET", "/app/support/api/diagnostics", "support.diagnose"),
    ("GET", "/api/health/summary", "health.summary"),
    ("GET", "/api/health/full", "health.full"),
    ("GET", "/api/health/range", "health.for_range"),
    ("GET", "/api/health/pipeline", "health.pipeline"),
    ("DELETE", "/app/observer/source/{stream}", "observer.deleteSource"),
}

REGISTER_OBSERVER_PAYLOAD = {
    "platform": "linux",
    "hostname": "contract-host",
    "stream_type": "desktop",
    "version": "1",
}

PUSH_FINGERPRINT = "sha256:" + ("a" * 64)
OBSERVER_FINGERPRINT = "sha256:" + ("b" * 64)


@pytest.fixture
def contract_app(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    journal = prepare_isolated_journal(tmp_path / "journal")
    mark_setup_complete(journal)
    with isolated_app_env(journal):
        app = create_app(journal=str(journal.resolve()))
        app.config["TESTING"] = True
        from solstone.convey import root as convey_root
        from solstone.think.link.auth import AuthorizedClients
        from solstone.think.link.paths import authorized_clients_path

        authorized = AuthorizedClients(authorized_clients_path())
        authorized.add(
            OBSERVER_FINGERPRINT,
            "contract-observer",
            "instance-1",
            paired_at="2026-05-20T00:00:00Z",
        )
        monkeypatch.setattr(convey_root, "get_authorized_clients", lambda: authorized)
        yield app, app.test_client(), journal


def _operation(document: dict[str, Any], operation_id: str) -> dict[str, Any]:
    for path_item in document["paths"].values():
        for operation in path_item.values():
            if operation.get("operationId") == operation_id:
                return operation
    raise AssertionError(f"operation not found: {operation_id}")


def _response_schema(
    document: dict[str, Any],
    operation_id: str,
    status: int,
) -> dict[str, Any]:
    response = _operation(document, operation_id)["responses"][str(status)]
    content = response.get("content", {})
    if not content:
        return {}
    media = next(iter(content.values()))
    return media.get("schema", {})


def _declared_response_fields(
    document: dict[str, Any],
    operation_id: str,
    status: int = 200,
) -> set[str]:
    schema = _response_schema(document, operation_id, status)
    return set(schema.get("properties", {}))


def _global_reason_codes(document: dict[str, Any]) -> set[str]:
    reason_code = document["components"]["schemas"]["Error"]["properties"][
        "reason_code"
    ]
    return set(reason_code["enum"])


def _assert_structured_error(body: dict[str, Any], document: dict[str, Any]) -> None:
    assert {"error", "reason_code", "detail"}.issubset(body)
    assert body["reason_code"] in _global_reason_codes(document)


def _register_observer(client) -> str:
    response = client.post(
        "/app/observer/register",
        json=REGISTER_OBSERVER_PAYLOAD,
        environ_overrides={"pl.identity": _observer_identity()},
    )
    assert response.status_code == 200, response.get_data(as_text=True)
    body = response.get_json()
    assert isinstance(body, dict)
    return str(body["key"])


def _observer_identity() -> ConveyIdentity:
    return ConveyIdentity(
        mode="pl-via-spl",
        fingerprint=OBSERVER_FINGERPRINT,
        device_label="contract-observer",
        paired_at="2026-05-20T00:00:00Z",
        session_id="observer-contract-test",
    )


def _push_identity() -> ConveyIdentity:
    return ConveyIdentity(
        mode="dl",
        fingerprint=PUSH_FINGERPRINT,
        device_label="Owner phone",
        paired_at="2026-06-18T00:00:00Z",
        session_id="contract-test",
    )


def _reset_chat_state() -> None:
    chat.stop_all_chat_runtime()
    with chat._state_lock:
        chat._current_chat_use_id = None
        chat._current_chat_state = None
        chat._queued_triggers.clear()
        chat._active_talents.clear()
        chat._reserved_use_ids.clear()
        chat._thinking_buffers.clear()
        chat._thinking_providers.clear()
        for timer in chat._watchdog_timers.values():
            timer.cancel()
        chat._watchdog_timers.clear()
        chat._last_use_id = 0


def _patch_chat_post_dependencies(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "solstone.think.identity.ensure_identity_directory",
        lambda: None,
    )
    monkeypatch.setattr(
        "solstone.convey.chat._spawn_chat_generate",
        lambda _action: ChatSpawnResult(ok=True),
    )


def test_all_fragment_routes_resolve(contract_app):
    app, _client, _journal = contract_app
    assert build_document()["paths"]

    for operation in _all_operations():
        # These operations terminate in Rust Convey, so Flask is not their
        # route owner. Keep this exemption identical to the native-sol gates.
        if operation.operation_id.startswith(RUST_CONVEY_OPERATION_PREFIXES):
            continue
        matches = [
            rule
            for rule in app.url_map.iter_rules()
            if rule.rule == operation.rule and operation.method.upper() in rule.methods
        ]
        assert matches, f"{operation.method} {operation.rule} did not resolve"


def test_observer_auth_both_header_forms(contract_app):
    _app, client, _journal = contract_app
    document = build_document()
    allowed = _declared_response_fields(document, "observer.ingestManifest")
    key = _register_observer(client)

    responses = [
        client.get(
            "/app/observer/ingest/manifest",
            headers={"Authorization": f"Bearer {key}"},
            environ_overrides={"pl.identity": _observer_identity()},
        ),
        client.get(
            "/app/observer/ingest/manifest",
            headers={"X-Solstone-Observer": key},
            environ_overrides={"pl.identity": _observer_identity()},
        ),
    ]

    for response in responses:
        assert response.status_code == 200
        body = response.get_json()
        assert isinstance(body, dict)
        assert undeclared_top_level_fields(allowed, body) == []


def test_segments_protocol_version_shape(contract_app):
    _app, client, _journal = contract_app
    key = _register_observer(client)

    legacy = client.get(
        "/app/observer/ingest/segments/20250103",
        headers={"Authorization": f"Bearer {key}"},
        environ_overrides={"pl.identity": _observer_identity()},
    )
    assert legacy.status_code == 200
    assert isinstance(legacy.get_json(), list)

    current = client.get(
        "/app/observer/ingest/segments/20250103",
        headers={
            "Authorization": f"Bearer {key}",
            "X-Solstone-Protocol-Version": "2",
        },
        environ_overrides={"pl.identity": _observer_identity()},
    )
    assert current.status_code == 200
    body = current.get_json()
    assert isinstance(body, dict)
    assert {"items", "total", "protocol_version"}.issubset(body)


def test_segment_file_status_enum_matches_live_day_listing(contract_app):
    _app, client, _journal = contract_app
    document = build_document()
    status_enum = document["components"]["schemas"]["SegmentFile"]["properties"][
        "status"
    ]["enum"]
    assert status_enum == ["present", "missing", "processed"]
    key = _register_observer(client)

    upload = client.post(
        "/app/observer/ingest",
        headers={"X-Solstone-Observer": key},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(b"contract upload"), "audio.flac"),
        },
        content_type="multipart/form-data",
        environ_overrides={"pl.identity": _observer_identity()},
    )
    assert upload.status_code == 200

    listing = client.get(
        "/app/observer/ingest/segments/20250103",
        headers={
            "Authorization": f"Bearer {key}",
            "X-Solstone-Protocol-Version": "2",
        },
        environ_overrides={"pl.identity": _observer_identity()},
    )
    items = listing.get_json()["items"]
    statuses = [file_info["status"] for item in items for file_info in item["files"]]
    assert statuses and all(status in status_enum for status in statuses)


def test_multipart_and_json_parsing(contract_app):
    _app, client, _journal = contract_app
    document = build_document()
    key = _register_observer(client)

    upload = client.post(
        "/app/observer/ingest",
        headers={"X-Solstone-Observer": key},
        data={
            "day": "20250103",
            "segment": "120000_300",
            "files": (io.BytesIO(b"contract upload"), "audio.flac"),
        },
        content_type="multipart/form-data",
        environ_overrides={"pl.identity": _observer_identity()},
    )
    assert upload.status_code != 415
    upload_body = upload.get_json()
    assert isinstance(upload_body, dict)
    if upload.status_code == 200:
        assert {"status", "segment", "files", "bytes"}.issubset(upload_body)
        allowed = _declared_response_fields(document, "observer.ingestUpload")
        assert undeclared_top_level_fields(allowed, upload_body) == []
    else:
        _assert_structured_error(upload_body, document)

    push = client.post(
        "/api/push/register",
        json={
            "device_token": "A" * 64,
            "bundle_id": "org.solpbc.solstone-swift",
            "environment": "development",
            "platform": "ios",
        },
        environ_overrides={"pl.identity": _push_identity()},
    )
    assert push.status_code == 200
    push_body = push.get_json()
    assert isinstance(push_body, dict)
    assert push_body == {"registered": True, "device_count": 1}


def test_structured_error_shape(contract_app):
    _app, client, _journal = contract_app
    document = build_document()

    response = client.get("/app/observer/ingest/manifest")

    assert response.status_code == 401
    body = response.get_json()
    assert isinstance(body, dict)
    _assert_structured_error(body, document)


def test_named_response_no_drift(contract_app):
    _app, client, _journal = contract_app
    document = build_document()
    allowed = _declared_response_fields(document, "link.status")

    response = client.get("/app/network/api/status")

    assert response.status_code == 200
    body = response.get_json()
    assert isinstance(body, dict)
    assert undeclared_top_level_fields(allowed, body) == []
    assert allowed <= set(body)


def test_link_devices_declares_device_item_schema():
    document = build_document()
    schema = _response_schema(document, "link.devices", 200)

    devices = schema["properties"]["devices"]
    item = devices["items"]
    expected = {
        "fingerprint",
        "fingerprint_short",
        "device_label",
        "display_label",
        "client_label",
        "paired_at",
        "last_seen_at",
        "role",
        "network",
        "kind",
        "observer_handle",
    }

    assert item["type"] == "object"
    assert expected <= set(item["properties"])
    assert expected <= set(item["required"])


def test_link_rename_contract_declares_persisted_response_and_errors():
    document = build_document()
    operation = _operation(document, "link.rename")
    request_schema = operation["requestBody"]["content"]["application/json"]["schema"]
    response_schema = _response_schema(document, "link.rename", 200)

    assert request_schema["required"] == ["fingerprint", "label"]
    assert set(response_schema["properties"]) == {
        "fingerprint",
        "device_label",
        "display_label",
    }
    assert response_schema["required"] == [
        "fingerprint",
        "device_label",
        "display_label",
    ]
    assert operation["responses"]["400"]["x-reason-codes"] == [
        "invalid_request_value",
        "missing_required_field",
    ]
    assert operation["responses"]["404"]["x-reason-codes"] == [
        "paired_device_not_found"
    ]
    assert operation["responses"]["500"]["x-reason-codes"] == [
        "convey_operation_failed"
    ]


def test_no_r0_routes_in_artifact():
    document = build_document()

    assert "/api/config/convey" not in document["paths"]
    assert "/api/system/status" not in document["paths"]
    assert set(document["paths"]) == CONTRACTED_PATHS
    assert len(document["paths"]) == len(CONTRACTED_PATHS)


def test_home_pulse_named_fields_present(contract_app):
    _app, client, _journal = contract_app
    document = build_document()

    response = client.get("/app/home/api/pulse")
    assert response.status_code == 200, response.get_data(as_text=True)
    body = response.get_json()
    assert isinstance(body, dict)
    named = {"journal_age_days", "home_state", "welcome_framing"}
    assert named <= set(body)

    schema = _response_schema(document, "home.pulse", 200)
    properties = schema["properties"]
    assert named <= set(properties)
    for field in named:
        assert properties[field].get("description")
    assert schema.get("additionalProperties") is True


def test_post_chat_accepted_named_fields_present(contract_app, monkeypatch):
    _reset_chat_state()
    _patch_chat_post_dependencies(monkeypatch)
    _app, client, _journal = contract_app
    document = build_document()

    response = client.post("/api/chat", json={"message": "hi"})

    assert response.status_code == 200, response.get_data(as_text=True)
    body = response.get_json()
    assert isinstance(body, dict)
    assert isinstance(body.get("use_id"), str) and body["use_id"]
    assert isinstance(body.get("queued"), bool)
    assert isinstance(body.get("queue_depth"), int)
    allowed = _declared_response_fields(document, "chat.postMessage", 200)
    assert allowed <= set(body)
    assert undeclared_top_level_fields(allowed, body) == []


def test_post_chat_queue_full_carries_depth(contract_app, monkeypatch):
    _reset_chat_state()
    _app, client, _journal = contract_app
    document = build_document()
    monkeypatch.setattr(
        "solstone.think.identity.ensure_identity_directory",
        lambda: None,
    )
    with chat._state_lock:
        chat._current_chat_use_id = "current"
        chat._current_chat_state = {
            "raw_use_id": "raw-current",
            "raw_use_ids_seen": {"raw-current"},
            "trigger": {"type": "owner_message", "message": "busy"},
            "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
            "retry_count": 0,
        }
        for index in range(10):
            chat._queued_triggers.append(
                {
                    "use_id": str(index + 1),
                    "trigger": {
                        "type": "owner_message",
                        "message": f"queued {index}",
                    },
                    "location": {"app": "sol", "path": "/app/sol", "facet": "work"},
                }
            )

    response = client.post("/api/chat", json={"message": "x"})

    assert response.status_code == 429
    body = response.get_json()
    assert isinstance(body, dict)
    _assert_structured_error(body, document)
    assert body["reason_code"] == "chat_queue_full"
    assert isinstance(body["queue_depth"], int) and body["queue_depth"] == 10


def test_post_chat_missing_message_reason_code(contract_app, monkeypatch):
    _reset_chat_state()
    _patch_chat_post_dependencies(monkeypatch)
    _app, client, _journal = contract_app
    document = build_document()

    response = client.post("/api/chat", json={})

    assert response.status_code == 400
    body = response.get_json()
    assert isinstance(body, dict)
    _assert_structured_error(body, document)
    assert body["reason_code"] == "missing_required_field"


@pytest.mark.parametrize("payload", [{"request_id": ""}, {"request_id": "   "}])
def test_open_sol_chat_request_rejects_blank_request_id(contract_app, payload):
    _app, client, _journal = contract_app
    document = build_document()

    response = client.post("/api/chat/sol_chat_request/open", json=payload)

    assert response.status_code == 400
    body = response.get_json()
    assert isinstance(body, dict)
    _assert_structured_error(body, document)
    assert body["reason_code"] == "missing_required_field"


def test_chat_session_empty_state_named_fields(contract_app):
    _reset_chat_state()
    _app, client, _journal = contract_app
    document = build_document()

    response = client.get("/api/chat/session")

    assert response.status_code == 200, response.get_data(as_text=True)
    body = response.get_json()
    assert isinstance(body, dict)
    allowed = _declared_response_fields(document, "chat.session", 200)
    assert allowed <= set(body)
    assert undeclared_top_level_fields(allowed, body) == []
    assert body["chat_error"] is None
    assert body["latest_sol_message"] is None
    for key in (
        "active_talents",
        "queued_talents",
        "completed_talents",
        "errored_talents",
    ):
        assert body[key] == []
    assert isinstance(body["queue_depth"], int)


def test_contracted_inventory_triples():
    document = build_document()
    expected_operation_ids = {
        operation_id for _method, _path, operation_id in CONTRACTED_INVENTORY_TRIPLES
    }

    for method, path, operation_id in CONTRACTED_INVENTORY_TRIPLES:
        assert document["paths"][path][method.lower()]["operationId"] == operation_id

    actual = {
        (method.upper(), path, operation["operationId"])
        for path, methods in document["paths"].items()
        for method, operation in methods.items()
        if operation.get("operationId") in expected_operation_ids
    }
    assert actual == CONTRACTED_INVENTORY_TRIPLES


def test_root_sse_event_stream():
    document = build_document()

    response = document["paths"]["/sse/events"]["get"]["responses"]["200"]
    content = response["content"]

    assert "text/event-stream" in content
    assert content["text/event-stream"]["schema"] == {
        "$ref": "#/components/schemas/CallosumEvent"
    }
    assert "x-sse-error-frame" not in response
    assert set(response["x-chat-events"]["kinds"]) == {
        "owner_message",
        "sol_message",
        "talent_queued",
        "talent_spawned",
        "talent_finished",
        "talent_errored",
        "chat_queue_depth",
        "result",
        "chat_error",
    }


def test_all_referenced_reason_codes_are_global():
    document = build_document()
    global_codes = _global_reason_codes(document)
    referenced_codes: set[str] = set()

    for path_item in document["paths"].values():
        for operation in path_item.values():
            for response in operation.get("responses", {}).values():
                referenced_codes.update(response.get("x-reason-codes", []))
                sse_error_frame = response.get("x-sse-error-frame", {})
                referenced_codes.update(sse_error_frame.get("x-reason-codes", []))

    assert referenced_codes - global_codes == set()


def test_scenario_removed_named_field_is_breaking():
    committed = build_document()
    current = deepcopy(committed)
    properties = _response_schema(current, "observer.register", 200)["properties"]
    properties.pop("prefix")

    breaking = classify_changes(current, committed)

    assert any(
        "observer.register: removed response field 'prefix'" in item
        for item in breaking
    )


def test_scenario_added_optional_field_is_silent():
    committed = build_document()
    current = deepcopy(committed)
    properties = _response_schema(current, "observer.register", 200)["properties"]
    properties["optional_future"] = {"type": "string"}

    assert classify_changes(current, committed) == []


def test_scenario_new_required_request_field_is_breaking():
    committed = build_document()
    current = deepcopy(committed)
    schema = _operation(current, "observer.register")["requestBody"]["content"][
        "application/json"
    ]["schema"]
    schema["properties"]["new_required"] = {"type": "string"}
    schema["required"].append("new_required")

    breaking = classify_changes(current, committed)

    assert any(
        "observer.register: new required request field 'new_required'" in item
        for item in breaking
    )


def test_scenario_removed_referenced_reason_code_is_breaking():
    committed = build_document()
    current = deepcopy(committed)
    response = _operation(current, "link.status")["responses"]["403"]
    response["x-reason-codes"].remove("pl_revoked")

    breaking = classify_changes(current, committed)

    assert any(
        "link.status: removed referenced reason code 'pl_revoked'" in item
        for item in breaking
    )


def test_scenario_global_enum_addition_is_not_breaking():
    committed = build_document()
    current = deepcopy(committed)
    enum = current["components"]["schemas"]["Error"]["properties"]["reason_code"][
        "enum"
    ]
    enum.append("future_unreferenced_code")

    assert classify_changes(current, committed) == []


def test_scenario_undeclared_field_then_declared():
    assert undeclared_top_level_fields({"a", "b"}, {"a": 1, "c": 2}) == ["c"]
    assert undeclared_top_level_fields({"a", "b", "c"}, {"a": 1, "c": 2}) == []
