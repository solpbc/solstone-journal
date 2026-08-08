# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import json
from concurrent.futures import ThreadPoolExecutor
from datetime import UTC, datetime, timedelta

import pytest

from solstone.apps.support.operations import (
    IdempotencyConflictError,
    OperationRetiredError,
    OperationStateUnavailableError,
    OperationSupersededError,
    begin_operation,
    canonicalize_operation,
    compact_expired_terminal_records,
    derive_child_action_id,
    mark_acknowledged,
    mark_completed,
    mark_failed,
    mark_in_progress,
)


NOW = datetime(2026, 8, 7, 12, 0, tzinfo=UTC)
PRINCIPAL = "jkt:thumbprint"


def _begin(storage, parent="draft-1", verb="reply", fields=None, *, index=0, now=NOW):
    return begin_operation(
        parent,
        verb,
        fields or {"ticket_id": "T1", "content": "Need help"},
        principal=PRINCIPAL,
        index=index,
        now=now,
        storage_dir=storage,
    )


def test_retry_reuses_same_operation_key(tmp_path):
    first = _begin(tmp_path)
    second = _begin(tmp_path)

    assert first.child_action_id == second.child_action_id
    assert first.operation_key == second.operation_key
    assert first.generation == second.generation == 1


def test_identical_payload_with_different_action_id_has_different_key(tmp_path):
    first = _begin(tmp_path, parent="draft-one")
    second = _begin(tmp_path, parent="draft-two")

    assert first.child_action_id != second.child_action_id
    assert first.operation_key != second.operation_key


def test_changed_payload_for_same_action_id_fails_locally(tmp_path):
    _begin(tmp_path)

    with pytest.raises(IdempotencyConflictError):
        _begin(tmp_path, fields={"ticket_id": "T1", "content": "Changed"})


def test_fingerprint_and_key_are_keyed_not_raw_sha256(tmp_path):
    record = _begin(tmp_path, fields={"ticket_id": "7", "content": "x"})
    canonical = canonicalize_operation(
        "reply",
        {"ticket_id": "7", "content": "x"},
        principal=PRINCIPAL,
        child_action_id=record.child_action_id,
    )

    assert bytes.fromhex(record.canonical_fingerprint) != hashlib.sha256(canonical).digest()
    assert record.operation_key != "spk1_" + hashlib.sha256(canonical).hexdigest()


def test_serialized_ledger_never_contains_source_payload_or_operation_key(tmp_path):
    fields = {
        "product": "desktop",
        "subject": "subject-secret",
        "description": "diagnostics-secret attachment-bytes-secret",
        "severity": "low",
        "category": "bug",
        "user_email": "owner@example.test",
        "user_context": {"path": "/private/secret-path"},
        "anonymous": False,
    }
    record = _begin(tmp_path, verb="create", fields=fields)
    serialized = (tmp_path / "operations" / f"{record.child_action_id}.json").read_text()

    for forbidden in (
        "subject-secret",
        "diagnostics-secret",
        "attachment-bytes-secret",
        "/private/secret-path",
        "owner@example.test",
        record.operation_key,
    ):
        assert forbidden not in serialized
    assert "canonical_fingerprint" in serialized
    assert "principal_tag" in serialized


def test_missing_key_with_existing_record_fails_closed_without_new_record(tmp_path):
    record = _begin(tmp_path)
    key_path = tmp_path / "operation-fingerprint.key"
    key_path.unlink()
    before = sorted(path.name for path in (tmp_path / "operations").glob("*.json"))

    with pytest.raises(OperationStateUnavailableError):
        _begin(tmp_path, parent="another-draft")

    assert not key_path.exists()
    assert sorted(path.name for path in (tmp_path / "operations").glob("*.json")) == before
    assert (tmp_path / "operations" / f"{record.child_action_id}.json").exists()


def test_stale_generation_cannot_overwrite_winning_terminal_result(tmp_path):
    first = mark_in_progress(_begin(tmp_path), now=NOW, storage_dir=tmp_path)
    second = _begin(tmp_path, now=NOW + timedelta(seconds=61))
    completed = mark_completed(
        second,
        remote_operation_id="remote-2",
        now=NOW + timedelta(seconds=62),
        storage_dir=tmp_path,
    )

    assert completed.generation == 2
    for stale_call in (
        lambda: mark_completed(
            first,
            remote_operation_id="remote-stale",
            now=NOW + timedelta(seconds=62),
            storage_dir=tmp_path,
        ),
        lambda: mark_failed(
            first,
            reason="stale",
            now=NOW + timedelta(seconds=62),
            storage_dir=tmp_path,
        ),
        lambda: mark_acknowledged(first, storage_dir=tmp_path),
    ):
        with pytest.raises(OperationSupersededError):
            stale_call()

    payload = json.loads(
        (tmp_path / "operations" / f"{completed.child_action_id}.json").read_text()
    )
    assert payload["generation"] == 2
    assert payload["state"] == "completed"
    assert payload["remote_operation_id"] == "remote-2"
    assert payload["terminal_reason"] is None
    assert payload["ack_state"] == "unacknowledged"


def test_old_terminal_record_compacts_and_refuses_resume(tmp_path):
    record = mark_in_progress(_begin(tmp_path), now=NOW, storage_dir=tmp_path)
    completed = mark_completed(
        record, remote_operation_id="remote", now=NOW, storage_dir=tmp_path
    )
    compact_expired_terminal_records(NOW + timedelta(days=46), storage_dir=tmp_path)
    path = tmp_path / "operations" / f"{completed.child_action_id}.json"

    assert json.loads(path.read_text()) == {
        "schema_version": 1,
        "child_action_id": completed.child_action_id,
        "terminal_reason": "completed",
    }
    with pytest.raises(OperationRetiredError):
        _begin(tmp_path, now=NOW + timedelta(days=46))
    assert json.loads(path.read_text())["child_action_id"] == completed.child_action_id


def test_attachment_batch_has_one_action_and_key_per_file(tmp_path):
    first = _begin(
        tmp_path,
        parent="attach-draft",
        verb="attach",
        index=0,
        fields={
            "ticket_id": "T1",
            "filename": "private-one.txt",
            "content_type": "text/plain",
            "byte_size": 1,
            "content_sha256": "attachment-bytes-secret",
        },
    )
    second = _begin(
        tmp_path,
        parent="attach-draft",
        verb="attach",
        index=1,
        fields={
            "ticket_id": "T1",
            "filename": "two.txt",
            "content_type": "text/plain",
            "byte_size": 1,
            "content_sha256": "b" * 64,
        },
    )

    assert first.child_action_id != second.child_action_id
    assert first.operation_key != second.operation_key
    assert len(list((tmp_path / "operations").glob("*.json"))) == 2
    serialized = (
        tmp_path / "operations" / f"{first.child_action_id}.json"
    ).read_text()
    assert "private-one.txt" not in serialized
    assert "attachment-bytes-secret" not in serialized


def test_concurrent_starts_converge_on_one_record_and_key(tmp_path):
    with ThreadPoolExecutor(max_workers=2) as executor:
        records = list(executor.map(lambda _: _begin(tmp_path), range(2)))

    assert {record.child_action_id for record in records} == {
        derive_child_action_id("draft-1", "reply")
    }
    assert len({record.operation_key for record in records}) == 1
    assert len(list((tmp_path / "operations").glob("*.json"))) == 1
