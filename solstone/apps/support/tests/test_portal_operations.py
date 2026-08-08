# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import hmac
import json

import httpx
import pytest

from solstone.apps.support import operations
from solstone.apps.support.portal import PortalClient


def _client(tmp_path, monkeypatch, handler):
    client = PortalClient(
        portal_url="https://support.example.test",
        storage_dir=tmp_path,
        anonymous=True,
    )
    client._ensure_keypair()
    client._access_token = "test-token"
    monkeypatch.setattr(
        client, "_http", lambda: httpx.Client(transport=httpx.MockTransport(handler))
    )
    return client


def _operation_payload(client, action_id, verb="create"):
    child_id = operations.derive_child_action_id(action_id, verb)
    return json.loads(
        (client.storage_dir / "operations" / f"{child_id}.json").read_text()
    )


def test_retryable_5xx_reuses_the_same_outbound_key(tmp_path, monkeypatch):
    requests: list[httpx.Request] = []

    def handler(request):
        requests.append(request)
        if len(requests) == 1:
            return httpx.Response(503, json={"error": "temporarily_unavailable"})
        return httpx.Response(201, json={"ticket_id": "ticket-1"})

    client = _client(tmp_path, monkeypatch, handler)
    kwargs = {"subject": "Subject", "description": "Description", "action_id": "a1"}

    with pytest.raises(httpx.HTTPStatusError):
        client.create_ticket(**kwargs)
    result = client.create_ticket(**kwargs)

    assert result == {"ticket_id": "ticket-1"}
    assert len(requests) == 2
    assert (
        requests[0].headers["Idempotency-Key"] == requests[1].headers["Idempotency-Key"]
    )
    assert _operation_payload(client, "a1")["state"] == "completed"


def test_changed_retry_is_refused_before_transport(tmp_path, monkeypatch):
    requests: list[httpx.Request] = []

    def handler(request):
        requests.append(request)
        return httpx.Response(503, json={"error": "temporarily_unavailable"})

    client = _client(tmp_path, monkeypatch, handler)
    with pytest.raises(httpx.HTTPStatusError):
        client.create_ticket(subject="Subject", description="First", action_id="a1")

    with pytest.raises(operations.IdempotencyConflictError):
        client.create_ticket(subject="Subject", description="Changed", action_id="a1")
    assert len(requests) == 1


def test_remote_operation_in_progress_keeps_lease_live(tmp_path, monkeypatch):
    def handler(_request):
        return httpx.Response(409, json={"error": "operation_in_progress"})

    client = _client(tmp_path, monkeypatch, handler)
    with pytest.raises(operations.OperationInProgressError):
        client.create_ticket(
            subject="Subject", description="Description", action_id="a1"
        )

    with pytest.raises(operations.OperationInProgressError):
        operations.begin_operation(
            "a1",
            "create",
            {
                "product": "solstone",
                "subject": "Subject",
                "description": "Description",
                "severity": "medium",
                "anonymous": True,
            },
            principal="anonymous",
            storage_dir=client.storage_dir,
        )


def test_repeated_tos_change_marks_terminal_after_one_same_key_retry(
    tmp_path, monkeypatch
):
    requests: list[httpx.Request] = []

    def handler(request):
        requests.append(request)
        return httpx.Response(401, json={"error": "tos_changed"})

    client = _client(tmp_path, monkeypatch, handler)
    monkeypatch.setattr(client, "register", lambda: {"access_token": "refreshed"})
    kwargs = {"subject": "Subject", "description": "Description", "action_id": "a1"}

    with pytest.raises(operations.OperationTosChangedError):
        client.create_ticket(**kwargs)
    with pytest.raises(operations.OperationTosChangedError):
        client.create_ticket(**kwargs)

    assert len(requests) == 2
    assert (
        requests[0].headers["Idempotency-Key"] == requests[1].headers["Idempotency-Key"]
    )
    assert _operation_payload(client, "a1")["terminal_reason"] == "tos_changed"


@pytest.mark.parametrize(
    ("status", "error", "exception"),
    [
        (409, "idempotency_conflict", operations.IdempotencyConflictError),
        (409, "invalid_state", operations.OperationInvalidStateError),
        (410, "operation_retired", operations.OperationRetiredError),
        (410, "operation_erased", operations.OperationErasedError),
    ],
)
def test_remote_terminal_errors_fail_once_and_refuse_retry(
    tmp_path, monkeypatch, status, error, exception
):
    calls: list[httpx.Request] = []

    def handler(request):
        calls.append(request)
        return httpx.Response(status, json={"error": error})

    client = _client(tmp_path, monkeypatch, handler)
    kwargs = {"subject": "Subject", "description": "Description", "action_id": "a1"}

    with pytest.raises(exception):
        client.create_ticket(**kwargs)
    assert _operation_payload(client, "a1")["terminal_reason"] == error
    with pytest.raises(exception):
        client.create_ticket(**kwargs)
    assert len(calls) == 1


def test_attachment_fingerprint_uses_the_uploaded_file_snapshot(tmp_path, monkeypatch):
    payload = b"one megabyte is not required for a chunked digest"
    file_path = tmp_path / "attachment.txt"
    file_path.write_bytes(payload)

    client = _client(
        tmp_path / "portal",
        monkeypatch,
        lambda _request: httpx.Response(201, json={"attachment_id": "att-1"}),
    )
    result = client.attach_file(7, file_path, action_id="a1")
    child_id = operations.derive_child_action_id("a1", "attach")
    canonical = operations.canonicalize_operation(
        "attach",
        {
            "ticket_id": 7,
            "filename": "attachment.txt",
            "content_type": "text/plain",
            "byte_size": len(payload),
            "content_sha256": hashlib.sha256(payload).hexdigest(),
        },
        principal="anonymous",
        child_action_id=child_id,
    )
    key = (client.storage_dir / "operation-fingerprint.key").read_bytes()
    expected_fingerprint = hmac.new(
        key, b"portal-fingerprint\x00" + canonical, hashlib.sha256
    ).hexdigest()

    assert result == {"attachment_id": "att-1"}
    assert _operation_payload(client, "a1", "attach")["canonical_fingerprint"] == (
        expected_fingerprint
    )


def test_feedback_uses_a_verb_distinct_operation_key(tmp_path, monkeypatch):
    requests: list[httpx.Request] = []

    def handler(request):
        requests.append(request)
        return httpx.Response(201, json={"ticket_id": str(len(requests))})

    client = _client(tmp_path, monkeypatch, handler)
    client.create_ticket(
        subject="feedback",
        description="Same body",
        action_id="a1",
        severity="low",
        category="feedback",
    )
    client.submit_feedback(body="Same body", action_id="a1")

    assert (
        requests[0].headers["Idempotency-Key"] != requests[1].headers["Idempotency-Key"]
    )


def test_closed_history_forwards_opaque_cursor_and_projects_tombstones(
    tmp_path, monkeypatch
):
    cursor = "opaque+/cursor==not-a-date"
    requests: list[httpx.Request] = []

    def handler(request):
        requests.append(request)
        return httpx.Response(
            200,
            json={
                "tickets": [
                    {
                        "ticket_id": 7,
                        "status": "closed",
                        "closed_at": "2026-08-01T00:00:00Z",
                        "reason_code": "owner_closed",
                        "subject": "poisoned",
                        "internal_notes": "poisoned",
                    }
                ],
                "next_cursor": cursor,
            },
        )

    client = _client(tmp_path, monkeypatch, handler)
    result = client.list_closed_history(cursor=cursor)

    assert requests[0].url.params["cursor"] == cursor
    assert result["next_cursor"] == cursor
    assert result["tickets"] == [
        {
            "ticket_id": 7,
            "status": "closed",
            "closed_at": "2026-08-01T00:00:00Z",
            "reason_code": "owner_closed",
        }
    ]


@pytest.mark.parametrize(
    ("method_name", "path"),
    [
        ("close_ticket", "/api/tickets/7/close"),
        (
            "confirm_resolution",
            "/api/tickets/7/resolution/confirm",
        ),
    ],
)
def test_closing_actions_project_poisoned_response(
    tmp_path, monkeypatch, method_name, path
):
    def handler(request):
        assert request.url.path == path
        return httpx.Response(
            201,
            json={
                "ticket_id": 7,
                "status": "closed",
                "closed_at": "2026-08-01T00:00:00Z",
                "close_scheduled_at": "2026-09-01T00:00:00Z",
                "reason_code": "owner_closed",
                "subject": "secret",
                "user_context": {"secret": True},
                "internal_notes": "secret",
            },
        )

    client = _client(tmp_path, monkeypatch, handler)
    result = getattr(client, method_name)(7, action_id="a1")

    assert result == {
        "ticket_id": 7,
        "status": "closed",
        "closed_at": "2026-08-01T00:00:00Z",
        "close_scheduled_at": "2026-09-01T00:00:00Z",
        "reason_code": "owner_closed",
    }


def test_still_need_help_preserves_active_ticket_detail(tmp_path, monkeypatch):
    active = {
        "ticket_id": 7,
        "status": "open",
        "subject": "keep this",
        "user_context": {"keep": True},
    }
    client = _client(
        tmp_path, monkeypatch, lambda _request: httpx.Response(201, json=active)
    )

    assert client.still_need_help(7, action_id="a1") == active


def test_post_close_reply_is_tombstone_projected(tmp_path, monkeypatch):
    client = _client(
        tmp_path,
        monkeypatch,
        lambda _request: httpx.Response(
            200,
            json={
                "ticket_id": 7,
                "status": "closed",
                "closed_at": "2026-08-01T00:00:00Z",
                "subject": "poisoned",
                "messages": [{"content": "poisoned"}],
            },
        ),
    )

    assert client.reply_to_ticket(7, "reply", action_id="a1") == {
        "ticket_id": 7,
        "status": "closed",
        "closed_at": "2026-08-01T00:00:00Z",
    }


@pytest.mark.parametrize(
    ("status", "expected"),
    [(200, True), (404, False)],
)
def test_acknowledge_operation_handles_success_and_absence(
    tmp_path, monkeypatch, status, expected
):
    client = _client(
        tmp_path, monkeypatch, lambda _request: httpx.Response(status, json={})
    )

    assert client.acknowledge_operation(remote_operation_id="remote-1") is expected


def test_acknowledge_operation_propagates_failure(tmp_path, monkeypatch):
    client = _client(
        tmp_path, monkeypatch, lambda _request: httpx.Response(503, json={})
    )

    with pytest.raises(httpx.HTTPStatusError):
        client.acknowledge_operation(remote_operation_id="remote-1")


def test_acknowledge_operation_propagates_malformed_success_body(tmp_path, monkeypatch):
    client = _client(
        tmp_path,
        monkeypatch,
        lambda _request: httpx.Response(200, content=b"not json"),
    )

    with pytest.raises(ValueError):
        client.acknowledge_operation(remote_operation_id="remote-1")


def _completed_record(storage):
    record = operations.begin_operation(
        "ack-action",
        "reply",
        {"ticket_id": 7, "content": "reply"},
        principal="anonymous",
        storage_dir=storage,
    )
    record = operations.mark_in_progress(record, storage_dir=storage)
    return operations.mark_completed(
        record, remote_operation_id="remote-1", storage_dir=storage
    )


def test_ack_drain_does_not_touch_network_when_empty(tmp_path, monkeypatch):
    def fail_handler(_request):
        raise AssertionError("empty acknowledgement drain must not use the network")

    client = _client(tmp_path, monkeypatch, fail_handler)
    client.drain_pending_acknowledgements()


def test_ack_drain_marks_successful_acknowledgement(tmp_path, monkeypatch):
    _completed_record(tmp_path)
    client = _client(
        tmp_path, monkeypatch, lambda _request: httpx.Response(200, json={})
    )

    client.drain_pending_acknowledgements()

    assert operations.list_pending_acknowledgements(storage_dir=tmp_path) == []


def test_ack_drain_preserves_record_on_failure(tmp_path, monkeypatch):
    _completed_record(tmp_path)
    client = _client(
        tmp_path, monkeypatch, lambda _request: httpx.Response(503, json={})
    )

    client.drain_pending_acknowledgements()

    pending = operations.list_pending_acknowledgements(storage_dir=tmp_path)
    assert len(pending) == 1
    assert pending[0].ack_state == "unacknowledged"
