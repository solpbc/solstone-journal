# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Record observer-client wire behavior fixtures and vectors."""

from __future__ import annotations

import copy
import json
import os
import tempfile
from collections.abc import Iterable, Iterator
from contextlib import contextmanager
from io import BytesIO
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator

from solstone.convey.contract.observer_bundle import (
    BundleVerificationError,
    ObserverBundleError,
    _iter_operations,
    _schema_name_from_ref,
    _sha256_text,
    render_json,
)
from solstone.convey.secure_listener import ConveyIdentity
from solstone.observe import protocol
from solstone.observe.processing_record import HANDLER_TRANSCRIBE, STATE_EMPTY
from solstone.observe.processing_record import SCHEMA as PROCESSING_SCHEMA
from solstone.think.journal_config import JournalConfigMutation, mutate_journal_config
from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.paths import authorized_clients_path

_RECORDING_FINGERPRINT = "sha256:" + ("1" * 64)
_RECORDING_LABEL = "Observer Contract Recorder"


def build_fixture_and_vector_payloads(
    projection: dict[str, Any],
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Build fixture and vector payloads, validating fixture schemas."""

    fixtures = _example_fixtures(projection)
    recorded_fixtures, vectors = _record_behavior_vectors()
    fixtures.extend(recorded_fixtures)
    declared_fixtures = _declared_negative_fixtures()
    fixtures.extend(declared_fixtures)
    vectors.extend(_declared_negative_vectors(declared_fixtures))

    fixture_payload = {
        "fixtures": sorted(fixtures, key=lambda item: item["id"]),
        "schema": "solstone.observer-client-contract-fixtures.v1",
    }
    _validate_fixtures(projection, fixture_payload["fixtures"])

    vector_payload = {
        "schema": "solstone.observer-client-contract-vectors.v1",
        "tmp_token": "$TMP",
        "vectors": sorted(vectors, key=lambda item: item["id"]),
    }
    return fixture_payload, vector_payload


def _example_fixtures(projection: dict[str, Any]) -> list[dict[str, Any]]:
    fixtures: list[dict[str, Any]] = []
    for _path, _method, operation in _iter_operations(projection):
        operation_id = operation["operationId"]
        request_body = operation.get("requestBody", {})
        if isinstance(request_body, dict):
            for media_type, media in request_body.get("content", {}).items():
                if isinstance(media, dict) and "example" in media:
                    fixtures.append(
                        _fixture(
                            fixture_id=_fixture_id(
                                "example",
                                operation_id,
                                "request",
                                "body",
                                media_type,
                                "default",
                            ),
                            kind="openapi-example",
                            operation_id=operation_id,
                            direction="request",
                            status=None,
                            media_type=media_type,
                            variant="default",
                            payload=copy.deepcopy(media["example"]),
                            validates=True,
                        )
                    )

        for status, response in operation.get("responses", {}).items():
            if not isinstance(response, dict):
                continue
            for media_type, media in response.get("content", {}).items():
                if not isinstance(media, dict):
                    continue
                if "example" in media:
                    fixtures.append(
                        _fixture(
                            fixture_id=_fixture_id(
                                "example",
                                operation_id,
                                "response",
                                status,
                                media_type,
                                "default",
                            ),
                            kind="openapi-example",
                            operation_id=operation_id,
                            direction="response",
                            status=int(status),
                            media_type=media_type,
                            variant="default",
                            payload=copy.deepcopy(media["example"]),
                            validates=True,
                        )
                    )
                examples = media.get("examples", {})
                if isinstance(examples, dict):
                    for variant, example in sorted(examples.items()):
                        if not isinstance(example, dict) or "value" not in example:
                            continue
                        fixtures.append(
                            _fixture(
                                fixture_id=_fixture_id(
                                    "example",
                                    operation_id,
                                    "response",
                                    status,
                                    media_type,
                                    variant,
                                ),
                                kind="openapi-example",
                                operation_id=operation_id,
                                direction="response",
                                status=int(status),
                                media_type=media_type,
                                variant=variant,
                                payload=copy.deepcopy(example["value"]),
                                validates=True,
                            )
                        )
    return fixtures


def _record_behavior_vectors() -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    fixtures: list[dict[str, Any]] = []
    vectors: list[dict[str, Any]] = []

    with tempfile.TemporaryDirectory(prefix="solstone-observer-bundle-") as tmp_text:
        tmp = Path(tmp_text)
        journal = _prepare_recording_journal(tmp / "journal")
        with _recording_env(journal):
            from solstone.apps.observer import routes as observer_routes
            from solstone.convey import bridge as convey_bridge
            from solstone.convey import chat_stream as convey_chat_stream
            from solstone.convey import create_app
            from solstone.convey import root as convey_root

            authorized = AuthorizedClients(authorized_clients_path())
            with (
                _temporary_attr(observer_routes, "now_ms", lambda: 1700000000000),
                _temporary_attr(
                    convey_root, "get_authorized_clients", lambda: authorized
                ),
                # Fixture recording owns a temporary journal and needs only the
                # endpoint's wire response. Production finalization would try
                # to index and broadcast the synthetic chat event, making the
                # deterministic generator depend on a running native install.
                _temporary_attr(
                    convey_chat_stream,
                    "_finalize_chat_event_appends",
                    lambda _events: None,
                ),
            ):
                app = create_app(journal=str(journal.resolve()))
                app.config["TESTING"] = True
                client = app.test_client()

                auth_key = _register_observer(client, "vector-auth")
                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.auth.bearer.segments",
                    vector_id="observer.auth.bearer",
                    operation_id="observer.ingestSegments",
                    response=_observer_get(
                        client,
                        "/app/observer/ingest/segments/20250103",
                        headers={
                            "Authorization": f"Bearer {auth_key}",
                            protocol.OBSERVER_PROTOCOL_VERSION_HEADER: "2",
                        },
                    ),
                    variant="bearer",
                    pointers=["/items", "/total", "/protocol_version"],
                )
                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.auth.handle.segments",
                    vector_id="observer.auth.handle",
                    operation_id="observer.ingestSegments",
                    response=_observer_get(
                        client,
                        "/app/observer/ingest/segments/20250103",
                        headers={
                            protocol.OBSERVER_HANDLE_HEADER: auth_key,
                            protocol.OBSERVER_PROTOCOL_VERSION_HEADER: "2",
                        },
                    ),
                    variant="observer_handle",
                    pointers=["/items", "/total", "/protocol_version"],
                )

                segments_key = _register_observer(client, "vector-segments")
                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.segments.legacy.absent_header",
                    vector_id="observer.ingestSegments.legacy_array.absent_header",
                    operation_id="observer.ingestSegments",
                    response=_observer_get(
                        client,
                        "/app/observer/ingest/segments/20250103",
                        headers={"Authorization": f"Bearer {segments_key}"},
                    ),
                    variant="legacy_array_absent_header",
                    pointers=[""],
                )
                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.segments.legacy.unparseable_header",
                    vector_id=(
                        "observer.ingestSegments.legacy_array.unparseable_header"
                    ),
                    operation_id="observer.ingestSegments",
                    response=_observer_get(
                        client,
                        "/app/observer/ingest/segments/20250103",
                        headers={
                            "Authorization": f"Bearer {segments_key}",
                            protocol.OBSERVER_PROTOCOL_VERSION_HEADER: "bogus",
                        },
                    ),
                    variant="legacy_array_unparseable_header",
                    pointers=[""],
                )
                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.segments.v2.envelope",
                    vector_id="observer.ingestSegments.v2_envelope",
                    operation_id="observer.ingestSegments",
                    response=_observer_get(
                        client,
                        "/app/observer/ingest/segments/20250103",
                        headers={
                            "Authorization": f"Bearer {segments_key}",
                            protocol.OBSERVER_PROTOCOL_VERSION_HEADER: "2",
                        },
                    ),
                    variant="v2_envelope",
                    pointers=["/items", "/total", "/protocol_version"],
                )

                upload_key = _register_observer(client, "vector-upload")
                ok_response = _upload(
                    client,
                    upload_key,
                    "20250104",
                    "120000_300",
                    [(b"ok audio", "120000_300_audio.flac")],
                )
                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.ingestUpload.ok",
                    vector_id="observer.ingestUpload.status.ok",
                    operation_id="observer.ingestUpload",
                    response=ok_response,
                    variant="ok",
                    pointers=["/status", "/segment", "/files", "/bytes"],
                )
                duplicate_response = _upload(
                    client,
                    upload_key,
                    "20250104",
                    "120000_300",
                    [(b"ok audio", "120000_300_audio.flac")],
                )
                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.ingestUpload.duplicate",
                    vector_id="observer.ingestUpload.status.duplicate",
                    operation_id="observer.ingestUpload",
                    response=duplicate_response,
                    variant="duplicate",
                    pointers=["/status", "/existing_segment", "/message"],
                )

                collision_key = _register_observer(client, "vector-collision")
                _upload(
                    client,
                    collision_key,
                    "20250105",
                    "120000_300",
                    [
                        (b"collision audio", "audio.flac"),
                        (b"screen v1", "screen.mp4"),
                    ],
                )
                collision_response = _upload(
                    client,
                    collision_key,
                    "20250105",
                    "120000_300",
                    [
                        (b"collision audio", "audio.flac"),
                        (b"screen v2", "screen.mp4"),
                    ],
                )
                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.ingestUpload.collision",
                    vector_id="observer.ingestUpload.status.collision",
                    operation_id="observer.ingestUpload",
                    response=collision_response,
                    variant="collision",
                    pointers=["/status", "/segment", "/segment_original"],
                )

                conflict_key = _register_observer(client, "vector-conflict")
                _upload(
                    client,
                    conflict_key,
                    "20250106",
                    "120000_300",
                    [(b"held audio", "audio.flac"), (b"notes v1", "notes.txt")],
                )
                conflict_response = _upload(
                    client,
                    conflict_key,
                    "20250106",
                    "120000_300",
                    [(b"held audio", "audio.flac"), (b"notes v2", "notes.txt")],
                )
                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.ingestUpload.conflict",
                    vector_id="observer.ingestUpload.status.conflict",
                    operation_id="observer.ingestUpload",
                    response=conflict_response,
                    variant="conflict",
                    pointers=[
                        "/status",
                        "/reason_code",
                        "/conflicting_files",
                        "/existing_segment",
                    ],
                )

                failed_key = _register_observer(client, "vector-failed")
                failed_response = _upload(
                    client,
                    failed_key,
                    "20250107",
                    "120000_300",
                    [
                        (
                            b'{"raw":"audio.flac"}\n{"start":"00:00:00"}\n',
                            "120000_300_audio.jsonl",
                        )
                    ],
                )
                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.ingestUpload.failed",
                    vector_id="observer.ingestUpload.status.failed",
                    operation_id="observer.ingestUpload",
                    response=failed_response,
                    variant="failed",
                    pointers=["/status", "/reason_code", "/failed_path"],
                )

                fallback_key = _register_observer(client, "vector-submitted")
                _upload(
                    client,
                    fallback_key,
                    "20250108",
                    "120000_300",
                    [(b"same-name audio", "audio.flac")],
                )
                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.segments.submitted_name_omitted",
                    vector_id="observer.ingestSegments.submitted_name_fallback",
                    operation_id="observer.ingestSegments",
                    response=_observer_get(
                        client,
                        "/app/observer/ingest/segments/20250108",
                        headers={
                            "Authorization": f"Bearer {fallback_key}",
                            protocol.OBSERVER_PROTOCOL_VERSION_HEADER: "2",
                        },
                    ),
                    variant="submitted_name_omitted",
                    pointers=["/items/0/files/0/name"],
                )

                custody_key = _register_observer(client, "vector-custody")
                _upload(
                    client,
                    custody_key,
                    "20250109",
                    "120000_300",
                    [
                        (b"custody audio", "audio.flac"),
                        (b"custody screen", "screen.mp4"),
                        (b"custody notes", "notes.txt"),
                    ],
                )
                segment_dir = (
                    journal / "chronicle" / "20250109" / "vector-custody" / "120000_300"
                )
                _write_processing_sidecar(segment_dir, input_size=len(b"custody audio"))
                (segment_dir / "audio.flac").unlink()
                (segment_dir / "screen.mp4").unlink()
                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.segments.custody_statuses",
                    vector_id="observer.ingestSegments.custody_statuses",
                    operation_id="observer.ingestSegments",
                    response=_observer_get(
                        client,
                        "/app/observer/ingest/segments/20250109",
                        headers={
                            "Authorization": f"Bearer {custody_key}",
                            protocol.OBSERVER_PROTOCOL_VERSION_HEADER: "2",
                        },
                    ),
                    variant="custody_statuses",
                    pointers=[
                        "/items/0/files/0/status",
                        "/items/0/files/1/status",
                        "/items/0/files/2/status",
                    ],
                )

                _record_sse_vectors(
                    client,
                    observer_routes,
                    convey_bridge,
                    fixtures,
                    vectors,
                )

                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.chat.openSolChatRequest.ok",
                    vector_id="chat.openSolChatRequest.ok",
                    operation_id="chat.openSolChatRequest",
                    response=client.post(
                        "/api/chat/sol_chat_request/open",
                        json={"request_id": "request-1"},
                    ),
                    variant="ok",
                    pointers=["/ok"],
                )
                _record_response(
                    fixtures,
                    vectors,
                    fixture_id="recorded.chat.openSolChatRequest.missing",
                    vector_id="chat.openSolChatRequest.missing_required_field",
                    operation_id="chat.openSolChatRequest",
                    response=client.post(
                        "/api/chat/sol_chat_request/open",
                        json={"request_id": "   "},
                    ),
                    variant="missing_required_field",
                    pointers=["/reason_code"],
                )

    return fixtures, vectors


def _record_sse_vectors(
    client: Any,
    observer_routes: Any,
    convey_bridge: Any,
    fixtures: list[dict[str, Any]],
    vectors: list[dict[str, Any]],
) -> None:
    root_response = client.get("/sse/events", buffered=False)
    try:
        root_heartbeat = _next_sse_chunk(root_response)
        _record_sse_fixture(
            fixtures,
            vectors,
            fixture_id="recorded.sse.root.heartbeat",
            vector_id="callosum.rootEvents.sse.heartbeat",
            operation_id="callosum.rootEvents",
            variant="heartbeat",
            payload=root_heartbeat,
            frame_kind="heartbeat",
            validates=None,
        )
        convey_bridge._broadcast_to_sse_clients(
            {"tract": "future", "event": "unknown", "ts": 0, "extra": "value"}
        )
        root_data = _parse_sse_data(_next_sse_chunk(root_response))
        _record_sse_fixture(
            fixtures,
            vectors,
            fixture_id="recorded.sse.root.data_unknown_event",
            vector_id="callosum.rootEvents.sse.data_unknown_event",
            operation_id="callosum.rootEvents",
            variant="data_unknown_event",
            payload=root_data,
            frame_kind="data",
            validates=True,
        )
    finally:
        root_response.close()

    sse_key = _register_observer(client, "vector-sse")
    observer_response = _observer_get(
        client,
        "/app/observer/callosum",
        headers={"Authorization": f"Bearer {sse_key}"},
        buffered=False,
    )
    try:
        observer_heartbeat = _next_sse_chunk(observer_response)
        _record_sse_fixture(
            fixtures,
            vectors,
            fixture_id="recorded.sse.observer.heartbeat",
            vector_id="observer.callosumStream.sse.heartbeat",
            operation_id="observer.callosumStream",
            variant="heartbeat",
            payload=observer_heartbeat,
            frame_kind="heartbeat",
            validates=None,
        )
        convey_bridge._broadcast_callosum_event(
            {"tract": "observe", "event": "status", "ts": 0, "extra": "value"}
        )
        observer_data = _parse_sse_data(_next_sse_chunk(observer_response))
        _record_sse_fixture(
            fixtures,
            vectors,
            fixture_id="recorded.sse.observer.data",
            vector_id="observer.callosumStream.sse.data",
            operation_id="observer.callosumStream",
            variant="data",
            payload=observer_data,
            frame_kind="data",
            validates=True,
        )
    finally:
        observer_response.close()

    error_key = _register_observer(client, "vector-sse-error")
    error_response = _observer_get(
        client,
        "/app/observer/callosum",
        headers={protocol.OBSERVER_HANDLE_HEADER: error_key},
        buffered=False,
    )
    try:
        _next_sse_chunk(error_response)
        from solstone.apps.observer.utils import load_observer, save_observer

        observer = load_observer(error_key)
        if observer is None:
            raise ObserverBundleError("failed to reload observer for SSE error vector")
        observer["revoked"] = True
        if not save_observer(observer):
            raise ObserverBundleError("failed to save revoked observer for SSE vector")
        with _temporary_attr(observer_routes, "_SSE_HEARTBEAT_SECONDS", 0.01):
            error_chunk = _next_sse_chunk(error_response)
        error_payload = _parse_sse_error(error_chunk)
        _record_sse_fixture(
            fixtures,
            vectors,
            fixture_id="recorded.sse.observer.error",
            vector_id="observer.callosumStream.sse.error",
            operation_id="observer.callosumStream",
            variant="error",
            payload=error_payload,
            frame_kind="error",
            validates=True,
        )
    finally:
        error_response.close()


def _declared_negative_fixtures() -> list[dict[str, Any]]:
    return [
        _fixture(
            fixture_id="declared.observer.ingestSegments.envelope_total_mismatch",
            kind="declared-negative",
            operation_id="observer.ingestSegments",
            direction="response",
            status=200,
            media_type="application/json",
            variant="envelope_total_mismatch",
            payload={
                "items": [],
                "protocol_version": protocol.OBSERVER_PROTOCOL_VERSION,
                "total": 1,
            },
            validates=True,
        ),
        _fixture(
            fixture_id="declared.observer.ingestSegments.custody_unknown_rejected",
            kind="declared-negative",
            operation_id="observer.ingestSegments",
            direction="response",
            status=200,
            media_type="application/json",
            variant="custody_unknown",
            payload={
                "items": [
                    {
                        "files": [
                            {
                                "name": "audio.flac",
                                "sha256": "0" * 64,
                                "size": 1,
                                "status": "unknown",
                            }
                        ],
                        "key": "120000_300",
                        "observed": False,
                    }
                ],
                "protocol_version": protocol.OBSERVER_PROTOCOL_VERSION,
                "total": 1,
            },
            validates=False,
        ),
        _fixture(
            fixture_id="declared.observer.ingestUpload.status_unknown_rejected",
            kind="declared-negative",
            operation_id="observer.ingestUpload",
            direction="response",
            status=200,
            media_type="application/json",
            variant="status_unknown",
            payload={"status": "unknown"},
            validates=False,
        ),
    ]


def _declared_negative_vectors(fixtures: list[dict[str, Any]]) -> list[dict[str, Any]]:
    pointers_by_fixture = {
        "declared.observer.ingestSegments.custody_unknown_rejected": [
            "/items/0/files/0/status"
        ],
        "declared.observer.ingestSegments.envelope_total_mismatch": ["/total"],
        "declared.observer.ingestUpload.status_unknown_rejected": ["/status"],
    }
    vectors: list[dict[str, Any]] = []
    fixtures_by_id = {fixture["id"]: fixture for fixture in fixtures}
    for fixture_id, pointers in sorted(pointers_by_fixture.items()):
        fixture = fixtures_by_id[fixture_id]
        vector_id = fixture_id.removeprefix("declared.")
        vectors.append(
            {
                "decision": _decision_for_declared_vector(vector_id),
                "fixture_id": fixture_id,
                "id": vector_id,
                "kind": "declared",
                "pointer_hashes": _pointer_hashes(fixture["payload"], pointers),
                "pointers": pointers,
            }
        )
    return vectors


def _validate_fixtures(
    projection: dict[str, Any], fixtures: Iterable[dict[str, Any]]
) -> None:
    components = projection.get("components", {}).get("schemas", {})
    for fixture in fixtures:
        validates = fixture.get("schema_validation", {}).get("validates")
        if validates is None:
            continue
        schema = _schema_for_fixture(projection, fixture)
        schema = _inline_refs(schema, components)
        Draft202012Validator.check_schema(schema)
        errors = list(Draft202012Validator(schema).iter_errors(fixture["payload"]))
        if validates and errors:
            raise ObserverBundleError(
                f"fixture {fixture['id']} failed schema validation: {errors[0].message}"
            )
        if not validates and not errors:
            raise ObserverBundleError(
                f"fixture {fixture['id']} unexpectedly passed schema validation"
            )


def _schema_for_fixture(projection: dict[str, Any], fixture: dict[str, Any]) -> dict:
    operation = _operation_by_id(projection, fixture["provenance"]["operation_id"])
    direction = fixture["provenance"]["direction"]
    media_type = fixture["provenance"]["media_type"]
    if direction == "request":
        content = operation["requestBody"]["content"]
    elif direction == "response":
        status = str(fixture["provenance"]["status"])
        response = operation["responses"][status]
        if (
            fixture["provenance"]["media_type"] == "text/event-stream"
            and fixture["provenance"]["named_variant"] == "error"
        ):
            frame = response.get("x-sse-error-frame", {})
            schema = frame.get("schema") if isinstance(frame, dict) else None
            if not isinstance(schema, dict):
                raise ObserverBundleError(
                    f"SSE error fixture {fixture['id']} has no error schema"
                )
            return copy.deepcopy(schema)
        content = response["content"]
    else:
        raise ObserverBundleError(f"unknown fixture direction: {direction}")
    return copy.deepcopy(content[media_type]["schema"])


def _operation_by_id(projection: dict[str, Any], operation_id: str) -> dict[str, Any]:
    for _path, _method, operation in _iter_operations(projection):
        if operation["operationId"] == operation_id:
            return operation
    raise ObserverBundleError(f"operation not found in projection: {operation_id}")


def _inline_refs(schema: Any, components: dict[str, Any]) -> Any:
    if isinstance(schema, dict):
        ref = schema.get("$ref")
        if isinstance(ref, str):
            name = _schema_name_from_ref(ref)
            if name is None or name not in components:
                raise ObserverBundleError(f"cannot inline ref: {ref}")
            return _inline_refs(components[name], components)
        return {key: _inline_refs(value, components) for key, value in schema.items()}
    if isinstance(schema, list):
        return [_inline_refs(item, components) for item in schema]
    return schema


def _record_response(
    fixtures: list[dict[str, Any]],
    vectors: list[dict[str, Any]],
    *,
    fixture_id: str,
    vector_id: str,
    operation_id: str,
    response: Any,
    variant: str,
    pointers: list[str],
) -> None:
    payload = response.get_json()
    if payload is None:
        raise ObserverBundleError(f"{vector_id} did not return JSON")
    stable_payload = _stable_payload(payload)
    fixtures.append(
        _fixture(
            fixture_id=fixture_id,
            kind="recorded-response",
            operation_id=operation_id,
            direction="response",
            status=response.status_code,
            media_type="application/json",
            variant=variant,
            payload=stable_payload,
            validates=True,
        )
    )
    vectors.append(
        {
            "decision": _decision_for_response_vector(
                vector_id,
                stable_payload,
                response.status_code,
            ),
            "fixture_id": fixture_id,
            "id": vector_id,
            "kind": "recorded",
            "observed_status": response.status_code,
            "pointer_hashes": _pointer_hashes(stable_payload, pointers),
            "pointers": pointers,
        }
    )


def _record_sse_fixture(
    fixtures: list[dict[str, Any]],
    vectors: list[dict[str, Any]],
    *,
    fixture_id: str,
    vector_id: str,
    operation_id: str,
    variant: str,
    payload: object,
    frame_kind: str,
    validates: bool | None,
) -> None:
    stable_payload = _stable_payload(payload)
    if isinstance(payload, str):
        pointers = [""]
    elif frame_kind == "error":
        pointers = ["/reason_code"]
    else:
        pointers = ["/tract", "/event"]
    fixtures.append(
        _fixture(
            fixture_id=fixture_id,
            kind="recorded-sse-frame",
            operation_id=operation_id,
            direction="response",
            status=200,
            media_type="text/event-stream",
            variant=variant,
            payload=stable_payload,
            validates=validates,
        )
    )
    vectors.append(
        {
            "decision": _decision_for_sse_vector(vector_id, frame_kind, stable_payload),
            "fixture_id": fixture_id,
            "frame_kind": frame_kind,
            "id": vector_id,
            "kind": "recorded",
            "pointer_hashes": _pointer_hashes(stable_payload, pointers),
            "pointers": pointers,
        }
    )


def _decision_for_response_vector(
    vector_id: str,
    payload: object,
    observed_status: int,
) -> dict[str, Any]:
    if vector_id.startswith("observer.ingestSegments.legacy_array."):
        header = "absent" if vector_id.endswith(".absent_header") else "unparseable"
        return {
            "absent_or_unparseable_uses": 1,
            "header": header,
            "kind": "protocol_variant",
            "parsed_version": 1,
            "response_variant": "legacy_array",
        }
    if not isinstance(payload, dict):
        raise ObserverBundleError(f"{vector_id} response payload is not an object")
    if vector_id.startswith("observer.ingestUpload.status."):
        status = str(payload.get("status"))
        decisions = {
            "ok": {
                "accepted": True,
                "client_action": "adopt_segment",
                "http_status": 200,
                "kind": "ingest_status",
                "status": "ok",
                "stored_key_source": "segment",
                "stored_key_precedence": ["segment"],
            },
            "duplicate": {
                "accepted": True,
                "client_action": "adopt_existing_segment_without_reupload",
                "http_status": 200,
                "kind": "ingest_status",
                "status": "duplicate",
                "stored_key_source": "existing_segment",
                "stored_key_precedence": ["existing_segment"],
            },
            "collision": {
                "accepted": True,
                "client_action": "adopt_remapped_segment",
                "http_status": 200,
                "kind": "ingest_status",
                "original_key_source": "segment_original",
                "status": "collision",
                "stored_key_source": "segment",
                "stored_key_precedence": ["segment", "segment_original"],
            },
            "conflict": {
                "accepted": False,
                "client_action": "preserve_local_and_surface_conflict",
                "http_status": 409,
                "kind": "ingest_status",
                "status": "conflict",
                "stored_key_source": "existing_segment",
                "stored_key_precedence": ["existing_segment"],
            },
            "failed": {
                "accepted": False,
                "client_action": "preserve_local_and_surface_failure",
                "http_status": 422,
                "kind": "ingest_status",
                "status": "failed",
                "stored_key_source": None,
                "stored_key_precedence": [],
            },
        }
        decision = decisions.get(status)
        if decision is None:
            raise ObserverBundleError(f"{vector_id} has unknown ingest status {status}")
        if decision["http_status"] != observed_status:
            raise ObserverBundleError(
                f"{vector_id} expected HTTP {decision['http_status']}, got {observed_status}"
            )
        return decision
    if vector_id == "observer.ingestSegments.submitted_name_fallback":
        return {
            "fallback": "name",
            "kind": "submitted_name_fallback",
            "submitted_name_present": False,
        }
    if vector_id == "observer.ingestSegments.custody_statuses":
        return {
            "holding_by_status": {
                "missing": "not_held",
                "present": "held",
                "processed": "held",
            },
            "kind": "custody_status",
            "unknown_status": "reject",
        }
    if vector_id.startswith("observer.auth."):
        auth_form = (
            "authorization_bearer"
            if vector_id == "observer.auth.bearer"
            else "x_solstone_observer"
        )
        return {
            "accepted": True,
            "auth_form": auth_form,
            "kind": "auth_header_form",
            "precedence": "x_solstone_observer_preferred_when_both_present",
        }
    if vector_id == "observer.ingestSegments.v2_envelope":
        return {
            "current_protocol_version": protocol.OBSERVER_PROTOCOL_VERSION,
            "header": "2",
            "kind": "protocol_variant",
            "parsed_version": 2,
            "response_variant": "v2_envelope",
        }
    if vector_id == "chat.openSolChatRequest.ok":
        return {
            "accepted": True,
            "kind": "chat_open_request",
            "missing_field_behavior": "non_empty_trimmed_request_id_required",
            "result": "ok_true",
        }
    if vector_id == "chat.openSolChatRequest.missing_required_field":
        return {
            "accepted": False,
            "kind": "chat_open_request",
            "missing_field_behavior": "absent_malformed_empty_or_blank_rejected",
            "reason_code": "missing_required_field",
        }
    raise ObserverBundleError(f"no response decision declared for vector {vector_id}")


def _decision_for_sse_vector(
    vector_id: str,
    frame_kind: str,
    payload: object,
) -> dict[str, Any]:
    if frame_kind == "heartbeat":
        return {
            "action": "ignore_keepalive",
            "frame_kind": "heartbeat",
            "kind": "sse_frame",
        }
    if vector_id == "callosum.rootEvents.sse.data_unknown_event":
        return {
            "action": "pass_through",
            "frame_kind": "data",
            "kind": "sse_frame",
            "unknown_event_behavior": "preserve",
        }
    if vector_id == "observer.callosumStream.sse.data":
        return {
            "action": "dispatch_callosum_event",
            "frame_kind": "data",
            "kind": "sse_frame",
            "unknown_event_behavior": "preserve",
        }
    if vector_id == "observer.callosumStream.sse.error":
        return {
            "action": "surface_error_and_close",
            "frame_kind": "error",
            "kind": "sse_frame",
            "reason_code": _required_payload_str(payload, "reason_code", vector_id),
        }
    raise ObserverBundleError(f"no SSE decision declared for vector {vector_id}")


def _decision_for_declared_vector(vector_id: str) -> dict[str, Any]:
    if vector_id == "observer.ingestSegments.envelope_total_mismatch":
        return {
            "expected": "total_equals_items_length",
            "kind": "envelope_integrity",
            "valid": False,
        }
    if vector_id == "observer.ingestSegments.custody_unknown_rejected":
        return {
            "kind": "custody_unknown",
            "status": "unknown",
            "unknown_status": "reject",
        }
    if vector_id == "observer.ingestUpload.status_unknown_rejected":
        return {
            "kind": "closed_vocabulary_unknown",
            "status": "unknown",
            "unknown_value_behavior": "reject",
            "vocabulary": "observer.ingestUpload.status",
        }
    raise ObserverBundleError(f"no declared decision for vector {vector_id}")


def _required_payload_str(payload: object, field: str, label: str) -> str:
    if not isinstance(payload, dict) or not isinstance(payload.get(field), str):
        raise ObserverBundleError(f"{label} payload missing string {field}")
    return payload[field]


def _fixture(
    *,
    fixture_id: str,
    kind: str,
    operation_id: str,
    direction: str,
    status: int | None,
    media_type: str,
    variant: str,
    payload: object,
    validates: bool | None,
) -> dict[str, Any]:
    validation = {"validates": validates}
    if validates is None:
        validation["reason"] = "SSE heartbeat comments are not JSON schema payloads"
    return {
        "id": fixture_id,
        "kind": kind,
        "payload": payload,
        "provenance": {
            "direction": direction,
            "media_type": media_type,
            "named_variant": variant,
            "operation_id": operation_id,
            "status": status,
        },
        "schema_validation": validation,
    }


def _fixture_id(
    prefix: str,
    operation_id: str,
    direction: str,
    status: str,
    media_type: str,
    variant: str,
) -> str:
    media = media_type.replace("/", "-").replace("+", "-")
    return f"{prefix}.{operation_id}.{direction}.{status}.{media}.{variant}"


def _register_observer(client: Any, name: str) -> str:
    AuthorizedClients(authorized_clients_path()).add(
        _RECORDING_FINGERPRINT,
        _RECORDING_LABEL,
        "observer-contract-recorder",
        paired_at="2026-01-01T00:00:00Z",
        client_label=name,
    )
    response = client.post(
        "/app/observer/register",
        environ_base={"pl.identity": _recording_pl_identity()},
        json={
            "hostname": name,
            "platform": "linux",
            "stream_type": "desktop",
            "version": "1",
        },
    )
    if response.status_code != 200:
        raise ObserverBundleError(
            f"observer registration failed: {response.get_data(as_text=True)}"
        )
    body = response.get_json()
    if not isinstance(body, dict) or not body.get("key"):
        raise ObserverBundleError("observer registration response missing key")
    return str(body["key"])


def _recording_pl_identity() -> ConveyIdentity:
    return ConveyIdentity(
        mode="pl-via-spl",
        fingerprint=_RECORDING_FINGERPRINT,
        device_label=_RECORDING_LABEL,
        paired_at="2026-01-01T00:00:00Z",
        session_id="observer-contract-recorder",
    )


def _observer_get(client: Any, path: str, **kwargs: Any) -> Any:
    return client.get(
        path,
        environ_overrides={"pl.identity": _recording_pl_identity()},
        **kwargs,
    )


def _upload(
    client: Any,
    key: str,
    day: str,
    segment: str,
    files: list[tuple[bytes, str]],
) -> Any:
    return client.post(
        "/app/observer/ingest",
        environ_overrides={"pl.identity": _recording_pl_identity()},
        headers={"Authorization": f"Bearer {key}"},
        data={
            "day": day,
            "segment": segment,
            "files": [(BytesIO(content), filename) for content, filename in files],
        },
    )


def _write_processing_sidecar(segment_dir: Path, *, input_size: int) -> None:
    record = {
        "handler": HANDLER_TRANSCRIBE,
        "input_size": input_size,
        "schema": PROCESSING_SCHEMA,
        "state": STATE_EMPTY,
    }
    row = {"_solstone_processing": record, "raw": "audio.flac"}
    (segment_dir / "audio.jsonl").write_text(
        json.dumps(row, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def _next_sse_chunk(response: Any) -> str:
    chunk = next(response.response)
    if isinstance(chunk, bytes):
        return chunk.decode("utf-8")
    return str(chunk)


def _parse_sse_data(chunk: str) -> dict[str, Any]:
    prefix = "data: "
    if not chunk.startswith(prefix):
        raise ObserverBundleError(f"expected SSE data frame, got {chunk!r}")
    return json.loads(chunk[len(prefix) :].strip())


def _parse_sse_error(chunk: str) -> dict[str, Any]:
    lines = chunk.splitlines()
    if lines[:1] != ["event: error"] or len(lines) < 2:
        raise ObserverBundleError(f"expected SSE error frame, got {chunk!r}")
    return json.loads(lines[1].removeprefix("data: "))


def _prepare_recording_journal(destination: Path) -> Path:
    destination.mkdir(parents=True, exist_ok=False)
    _mark_setup_complete(destination)
    return destination.resolve()


def _mark_setup_complete(journal: Path) -> None:
    def apply(config: dict) -> JournalConfigMutation[None]:
        config["setup"] = {"completed_at": 1700000000000}
        return JournalConfigMutation(changed=True, value=None)

    mutate_journal_config(apply, journal_path=journal)


@contextmanager
def _recording_env(journal: Path) -> Iterator[None]:
    overrides = {
        "SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES": "1",
        "SOLSTONE_JOURNAL": str(journal),
        "SOL_SKIP_SUPERVISOR_CHECK": "1",
    }
    previous = {key: os.environ.get(key) for key in overrides}
    os.environ.update(overrides)
    try:
        yield
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


@contextmanager
def _temporary_attr(target: object, name: str, value: object) -> Iterator[None]:
    previous = getattr(target, name)
    setattr(target, name, value)
    try:
        yield
    finally:
        setattr(target, name, previous)


def _stable_payload(payload: object) -> object:
    if isinstance(payload, dict):
        return {key: _stable_payload(payload[key]) for key in sorted(payload)}
    if isinstance(payload, list):
        return [_stable_payload(item) for item in payload]
    return payload


def _pointer_hashes(payload: object, pointers: Iterable[str]) -> dict[str, str]:
    return {
        pointer: _sha256_text(render_json(_resolve_json_pointer(payload, pointer)))
        for pointer in pointers
    }


def _resolve_json_pointer(payload: Any, pointer: str) -> Any:
    if pointer == "":
        return payload
    if not pointer.startswith("/"):
        raise BundleVerificationError(f"invalid JSON pointer: {pointer}")
    current = payload
    for token in pointer[1:].split("/"):
        token = token.replace("~1", "/").replace("~0", "~")
        if isinstance(current, list):
            try:
                current = current[int(token)]
            except (ValueError, IndexError) as exc:
                raise BundleVerificationError(
                    f"JSON pointer does not resolve: {pointer}"
                ) from exc
        elif isinstance(current, dict):
            if token not in current:
                raise BundleVerificationError(
                    f"JSON pointer does not resolve: {pointer}"
                )
            current = current[token]
        else:
            raise BundleVerificationError(f"JSON pointer does not resolve: {pointer}")
    return current
