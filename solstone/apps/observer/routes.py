# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Observer app wire protocol.

Provides endpoints for:
- Registering observer connections
- Receiving file uploads from observers (ingest)
- Serving segment manifests for transfer diffing
- Relaying events from observers to local Callosum
- Retrieving segment upload history for sync verification
"""

from __future__ import annotations

import base64
import json
import logging
import platform
import queue
import re
import secrets
import time
from pathlib import Path
from typing import Any

from flask import (
    Blueprint,
    Response,
    current_app,
    g,
    jsonify,
    request,
    stream_with_context,
)
from werkzeug.utils import secure_filename

import solstone.convey.bridge as convey_bridge
from solstone.apps.utils import log_app_action
from solstone.convey import emit
from solstone.convey.bridge import _SSE_HEARTBEAT_SECONDS
from solstone.convey.reasons import (
    AUTH_REQUIRED,
    FEATURE_UNAVAILABLE,
    INGEST_CONTRACT_INVALID,
    INGEST_NO_FILES,
    INGEST_SIDECAR_CONFLICT,
    INGEST_STORAGE_FAILED,
    INVALID_DAY,
    INVALID_SEGMENT_OR_STREAM,
    LOCAL_REQUEST_ONLY,
    MISSING_REQUIRED_FIELD,
    PL_REVOKED,
    SETTINGS_OPERATION_FAILED,
    Reason,
)
from solstone.convey.utils import error_response, respond_collection
from solstone.observe import protocol
from solstone.observe.utils import (
    compute_bytes_sha256,
    compute_file_sha256,
)
from solstone.think.contract.journal import (
    ContractIssue,
    schema_for_filename,
    validate_contract_file,
)
from solstone.think.link.auth import AuthorizedClients
from solstone.think.link.paths import authorized_clients_path
from solstone.think.segment_files import RESERVED_SEGMENT_FILENAMES
from solstone.think.streams import stream_name, update_stream, write_segment_stream
from solstone.think.utils import day_path, iter_segments, now_ms, segment_path

from .processing_proof import has_terminal_processing_proof
from .share_delete import DELETABLE_SOURCE_STREAMS, delete_source_stream
from .utils import (
    DEVICE_BINDING_FIELD,
    DEVICE_BINDING_KIND_CERT,
    DISPOSITION_RECEIVED_NOT_WRITTEN,
    MAX_INGEST_SEGMENT_ATTEMPTS,
    IngestFile,
    ObserverRegistry,
    append_history_record,
    clear_ingest_rejection,
    find_oldest_unrevoked_by_name,
    get_hist_dir,
    load_history,
    observer_device_binding,
    pruned_segments,
    record_ingest_rejection,
    record_status_beacon,
    resolve_ingest_identity,
    resolve_ingest_plan,
    resolve_observer_identity,
    save_ingest_plan,
    save_observer,
)

logger = logging.getLogger(__name__)

observer_bp = Blueprint(
    "app:observer",
    __name__,
    url_prefix="/app/observer",
)
OBSERVER_CALLOSUM_SSE_ROUTE = "/app/observer/callosum"
_OBSERVER_CALLOSUM_SSE_RULE = OBSERVER_CALLOSUM_SSE_ROUTE.removeprefix(
    observer_bp.url_prefix or ""
)

# Key length in bytes (256 bits = 32 bytes)
KEY_BYTES = 32
_SSE_DEVICE_RECHECK_SECONDS = 5.0


def _error_body(reason: Reason, *, detail: str | None = None) -> dict[str, str]:
    return {
        "error": reason.message,
        "reason_code": reason.code,
        "detail": detail or "",
    }


def _sse_error_event(reason: Reason, *, detail: str) -> str:
    return f"event: error\ndata: {json.dumps(_error_body(reason, detail=detail))}\n\n"


def _validate_ingest_contract(
    *,
    observer: dict,
    key_prefix: str,
    segment: str,
    day: str,
    stream: str,
    file_data: list[tuple[str, str, bytes, str]],
    bundle: dict[str, Any],
    meta: dict[str, Any] | None = None,
) -> list[ContractIssue]:
    schema_entries = bundle.get("schemas", {})
    ingest_entry = (
        schema_entries.get("observer-ingest-envelope", {})
        if isinstance(schema_entries, dict)
        else {}
    )
    ingest_schema = (
        ingest_entry.get("schema") if isinstance(ingest_entry, dict) else None
    )
    issues: list[ContractIssue] = []
    if isinstance(ingest_schema, dict):
        envelope = {
            "day": day,
            "segment": segment,
            "stream": stream,
            "observer": str(observer.get("name") or key_prefix),
            "files": [
                {
                    "submitted": submitted,
                    "written": written,
                    "size": len(content),
                    "sha256": sha256,
                }
                for submitted, written, content, sha256 in file_data
            ],
        }
        if isinstance(meta, dict):
            for key in ("host", "platform"):
                if isinstance(meta.get(key), str):
                    envelope[key] = meta[key]
            envelope["meta"] = meta
        issues.extend(
            validate_contract_file(
                "observer-ingest-envelope",
                json.dumps(envelope).encode("utf-8"),
                ingest_schema,
            )
        )

    for _submitted, simple_filename, content, _sha256 in file_data:
        file_schema = schema_for_filename(simple_filename, bundle)
        if file_schema is None:
            continue
        issues.extend(validate_contract_file(simple_filename, content, file_schema))
    return issues


def _generate_key() -> str:
    """Generate a URL-safe key for observer authentication."""
    return base64.urlsafe_b64encode(secrets.token_bytes(KEY_BYTES)).decode().rstrip("=")


# LOCKED — wire format observer clients depend on. Field names and presence are the
# downstream contract. Adding a new field to a callosum event is permitted; renaming
# or removing existing fields requires a spec revision.
#
# Each SSE message body is a JSON object with at minimum:
#   {
#     "tract": str,        # e.g. "chat", "observe", "cortex", "supervisor", ...
#     "event": str,        # the event name within the tract
#     "ts":    int,        # millisecond timestamp
#     ... event-specific fields, passed through as emitted by the bus
#   }
#
# The feed does NOT add or remove fields relative to the bus payload.
# The feed does NOT filter events.
# The feed does NOT redact fields (v1 trust call; same trust boundary as the existing
# Convey SSE bridge — observers are inside it).
@observer_bp.route(_OBSERVER_CALLOSUM_SSE_RULE, methods=["GET"])
def callosum_sse() -> Any:
    """Stream Callosum events to an authenticated observer process."""
    observer, key_prefix, error = resolve_observer_identity()
    if error is not None:
        return error

    binding = observer_device_binding(observer)
    handle = convey_bridge.register_sse_subscriber(key_prefix)

    def current_observer() -> dict | None:
        return ObserverRegistry.singleton().by_prefix(key_prefix)

    def current_observer_rejection() -> tuple[Reason, str] | None:
        observer_now = current_observer()
        if not observer_now:
            return AUTH_REQUIRED, "Authorization required"
        if observer_now.get("revoked", False):
            return PL_REVOKED, "Observer revoked"
        if not observer_now.get("enabled", True):
            return FEATURE_UNAVAILABLE, "Observer disabled"
        return None

    def current_device_rejection() -> tuple[Reason, str] | None:
        if binding is None:
            return None
        if binding["kind"] != DEVICE_BINDING_KIND_CERT:
            return PL_REVOKED, "Paired device revoked"
        entry = AuthorizedClients(authorized_clients_path()).get(binding["device"])
        if entry is None:
            return PL_REVOKED, "Paired device revoked"
        return None

    def current_rejection() -> tuple[Reason, str] | None:
        return current_observer_rejection() or current_device_rejection()

    def generate():
        try:
            next_heartbeat_at = time.monotonic() + _SSE_HEARTBEAT_SECONDS
            next_device_check_at = time.monotonic() + _SSE_DEVICE_RECHECK_SECONDS
            yield ": heartbeat\n\n"
            while True:
                if handle.dropped.is_set():
                    return
                now = time.monotonic()
                if now >= next_device_check_at:
                    rejection = current_rejection()
                    if rejection is not None:
                        reason, detail = rejection
                        yield _sse_error_event(reason, detail=detail)
                        return
                    next_device_check_at = now + _SSE_DEVICE_RECHECK_SECONDS
                timeout = max(
                    0.0,
                    min(next_heartbeat_at, next_device_check_at) - time.monotonic(),
                )
                try:
                    serialized_message = handle.queue.get(timeout=timeout)
                except queue.Empty:
                    now = time.monotonic()
                    if now >= next_device_check_at:
                        rejection = current_rejection()
                        if rejection is not None:
                            reason, detail = rejection
                            yield _sse_error_event(reason, detail=detail)
                            return
                        next_device_check_at = now + _SSE_DEVICE_RECHECK_SECONDS
                    if now >= next_heartbeat_at:
                        yield ": heartbeat\n\n"
                        next_heartbeat_at = now + _SSE_HEARTBEAT_SECONDS
                    continue

                if handle.dropped.is_set():
                    return
                now = time.monotonic()
                if now >= next_device_check_at:
                    rejection = current_rejection()
                    if rejection is not None:
                        reason, detail = rejection
                        yield _sse_error_event(reason, detail=detail)
                        return
                    next_device_check_at = now + _SSE_DEVICE_RECHECK_SECONDS
                yield f"data: {serialized_message}\n\n"
        finally:
            convey_bridge.unregister_sse_subscriber(handle)

    response = Response(
        stream_with_context(generate()),
        mimetype="text/event-stream",
    )
    response.headers["Cache-Control"] = "no-cache"
    response.headers["X-Accel-Buffering"] = "no"
    return response




_REGISTER_REQUIRED_FIELDS = ("platform", "hostname", "stream_type", "version")
_REGISTER_LOOPBACK_REMOTE_ADDRS = frozenset({"127.0.0.1", "::1", "localhost"})


def _is_trusted_localhost() -> bool:
    """Direct-loopback check for observer-local endpoints."""
    is_localhost = request.remote_addr in _REGISTER_LOOPBACK_REMOTE_ADDRS
    proxy_headers = (
        request.headers.get("X-Forwarded-For")
        or request.headers.get("X-Real-IP")
        or request.headers.get("X-Forwarded-Host")
    )
    return is_localhost and not proxy_headers


def _is_authorized_pl_identity() -> bool:
    """Return True when the request arrived through a currently authorized PL."""
    identity = getattr(g, "identity", None)
    if getattr(identity, "mode", None) not in {"pl-direct", "pl-via-spl"}:
        return False
    fingerprint = getattr(identity, "fingerprint", None)
    if not isinstance(fingerprint, str) or not fingerprint:
        return False
    return AuthorizedClients(authorized_clients_path()).is_authorized(fingerprint)


def _is_trusted_register_caller() -> bool:
    return _is_trusted_localhost() or _is_authorized_pl_identity()


def _authorized_pl_entry():
    """Return the authorized cert-class PL entry for this request, if any."""
    identity = getattr(g, "identity", None)
    if getattr(identity, "mode", None) not in {"pl-direct", "pl-via-spl"}:
        return None
    fingerprint = getattr(identity, "fingerprint", None)
    if not isinstance(fingerprint, str) or not fingerprint:
        return None
    entry = AuthorizedClients(authorized_clients_path()).get(fingerprint)
    if entry is None or entry.kind != DEVICE_BINDING_KIND_CERT:
        return None
    return entry


def _device_binding_for_entry(entry) -> dict[str, str]:
    return {"device": entry.fingerprint, "kind": entry.kind}


def _resolve_register_device_binding() -> dict[str, str] | None:
    entry = _authorized_pl_entry()
    if entry is not None:
        return _device_binding_for_entry(entry)
    return None


def _register_descriptor(record: dict) -> dict:
    """Build the pinned register/ingest response body from a saved record."""
    key = record["key"]
    return {
        "key": key,
        "prefix": key[:8],
        "name": record["name"],
        "ingest_url": "/app/observer/ingest",
        "protocol_version": protocol.OBSERVER_PROTOCOL_VERSION,
    }


@observer_bp.route("/register", methods=["POST"])
def register() -> Any:
    """Self-register an observer after local or device admission.

    The in-handler guard admits trusted loopback callers and currently
    authorized PL identities. A device binding is attached only when the caller
    resolves to a cert-class entry.
    The route is require_access-exempt so an observer can register before setup
    completes. Mints the DL handle, locks a stream onto the record, and returns
    the pinned descriptor response.
    """
    parsed = request.get_json(force=True, silent=True)
    data = parsed if isinstance(parsed, dict) else {}

    # Register guard runs before field-specific validation; untrusted callers mint nothing.
    if not _is_trusted_register_caller():
        return error_response(
            LOCAL_REQUEST_ONLY,
            detail=(
                "Observer registration requires a trusted local caller or "
                "authorized device identity."
            ),
        )

    device_binding = _resolve_register_device_binding()

    for field in _REGISTER_REQUIRED_FIELDS:
        value = data.get(field)
        if not isinstance(value, str) or not value.strip():
            return error_response(MISSING_REQUIRED_FIELD, detail=f"{field} is required")

    hostname = data["hostname"].strip()
    stream_type = data["stream_type"].strip()
    try:
        if stream_type == "desktop":
            stream = stream_name(host=hostname)
        else:
            stream = stream_name(host=hostname, qualifier=stream_type)
    except ValueError as exc:
        return error_response(INVALID_SEGMENT_OR_STREAM, detail=str(exc))

    existing = find_oldest_unrevoked_by_name(stream)
    if existing is not None:
        existing_binding = observer_device_binding(existing)
        if existing_binding is None and device_binding is not None:
            existing[DEVICE_BINDING_FIELD] = device_binding
        elif existing_binding != device_binding:
            return error_response(
                LOCAL_REQUEST_ONLY,
                detail="Observer stream is bound to a different device.",
            )
        # Idempotent re-register: reuse the prior key/record, refresh the
        # mutable descriptor fields, preserve key/created_at/stats/last_seen.
        existing["platform"] = data["platform"].strip()
        existing["stream_type"] = stream_type
        existing["label"] = data.get("label")
        existing["version"] = data["version"].strip()
        if not save_observer(existing):
            return error_response(
                SETTINGS_OPERATION_FAILED,
                detail="Failed to save observer",
            )
        log_app_action(
            app="observer",
            facet=None,
            action="observer_register_reused",
            params={"name": stream, "key_prefix": existing["key"][:8]},
        )
        return jsonify(_register_descriptor(existing))

    key = _generate_key()
    observer_data = {
        "key": key,
        "name": stream,
        "platform": data["platform"].strip(),
        "hostname": hostname,
        "stream_type": stream_type,
        "label": data.get("label"),
        "version": data["version"].strip(),
        "stream": stream,
        "created_at": now_ms(),
        "last_seen": None,
        "last_segment": None,
        "last_segment_received_at": None,
        "last_segment_day": None,
        "enabled": True,
        "stats": {
            "segments_received": 0,
            "bytes_received": 0,
        },
    }
    if device_binding is not None:
        observer_data[DEVICE_BINDING_FIELD] = device_binding

    if not save_observer(observer_data):
        return error_response(
            SETTINGS_OPERATION_FAILED,
            detail="Failed to save observer",
        )

    log_app_action(
        app="observer",
        facet=None,
        action="observer_register",
        params={"name": stream, "key_prefix": key[:8]},
    )

    return jsonify(_register_descriptor(observer_data))






# === Sync history helpers ===


def resolve_file_status(
    day_dir: Path,
    stream: str,
    segment: str,
    written: str,
    size: object,
) -> str:
    """Read-only status check for a recorded file on disk.

    The recorded day/stream/segment/filename path is the only proof the
    journal still holds an uploaded file. Returns "present" when the file
    exists at that exact path, "processed" when a same-stem sidecar at that
    exact segment path carries terminal processing proof for the recorded raw
    media and size, else "missing". Does not scan descendants.
    """
    recorded_path = day_dir / stream / segment / written
    if recorded_path.exists():
        return "present"
    if has_terminal_processing_proof(recorded_path, size):
        return "processed"
    return "missing"


# === Segment collision helpers ===


def _strip_segment_prefix(filename: str, segment: str) -> str:
    """Strip segment prefix from filename if present.

    Handles old-style prefixed filenames (e.g., "143022_300_audio.flac")
    and returns simple names (e.g., "audio.flac").

    Args:
        filename: Original filename (may have segment prefix)
        segment: Segment key (HHMMSS_LEN)

    Returns:
        Simple filename without segment prefix
    """
    prefix = f"{segment}_"
    if filename.startswith(prefix):
        return filename[len(prefix) :]
    return filename


def _save_to_failed(
    day_dir: Path, file_data: list[tuple[str, str, bytes, str]], segment: str
) -> Path:
    """Save files to failed directory for manual review.

    Files are saved with their original segment key (not adjusted) since
    the collision resolution failed.

    Args:
        day_dir: Path to day directory
        file_data: List of (submitted_filename, simple_filename, content, sha256) tuples
        segment: Original segment key (used in directory name)

    Returns:
        Path to the failed directory where files were saved
    """
    # Use segment in path for easier identification of failed uploads
    failed_dir = day_dir / "observer" / "failed" / segment / str(now_ms())
    failed_dir.mkdir(parents=True, exist_ok=True)

    for submitted_filename, _simple_filename, content, _sha256 in file_data:
        target_path = failed_dir / submitted_filename
        target_path.write_bytes(content)

    return failed_dir


# === Ingest API (key-protected) ===


@observer_bp.route("/source/<stream>", methods=["DELETE"])
def delete_source(stream: str) -> Any:
    """Delete an allowed source stream for an authenticated observer."""
    observer, key_prefix, error = resolve_observer_identity()
    if error is not None:
        return error

    if stream not in DELETABLE_SOURCE_STREAMS:
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Only known source streams can be deleted",
        )

    form_stream = request.form.get("stream", "").strip()
    meta_stream = ""
    meta_str = request.form.get("meta", "").strip()
    if meta_str:
        try:
            parsed = json.loads(meta_str)
            if isinstance(parsed, dict):
                meta_stream = str(parsed.get("stream", "")).strip()
        except json.JSONDecodeError:
            meta_stream = ""

    for candidate in (form_stream, meta_stream):
        if candidate and candidate not in DELETABLE_SOURCE_STREAMS:
            return error_response(
                INVALID_SEGMENT_OR_STREAM,
                detail="Only known source streams can be deleted",
            )

    receipt = delete_source_stream(stream)
    logger.info(
        "Deleted %s source (observer=%s)", stream, observer.get("name", key_prefix)
    )
    return jsonify(receipt), 200


def _process_ingest_files(
    observer: dict,
    key_prefix: str,
    segment: str,
    day: str,
    stream: str,
    uploaded_files,
    *,
    bundle: dict[str, Any],
    meta: dict[str, Any] | None = None,
) -> tuple[dict, int]:
    """Shared ingest pipeline: read/hash, validate, resolve, apply, record history."""
    # Read file contents into memory and compute SHA256 before saving
    # This allows duplicate detection without writing to disk
    file_data = []  # List of (submitted_filename, simple_filename, content, sha256)
    for upload in uploaded_files:
        if not upload.filename:
            continue

        submitted_filename = secure_filename(upload.filename)
        if not submitted_filename:
            continue

        # Strip segment prefix from filename if present
        simple_filename = _strip_segment_prefix(submitted_filename, segment)

        # Read content and compute SHA256
        content = upload.read()
        if len(content) == 0:
            logger.warning(f"Skipping 0-byte file: {submitted_filename}")
            continue
        sha256 = compute_bytes_sha256(content)

        file_data.append((submitted_filename, simple_filename, content, sha256))

    if not file_data:
        return _error_body(INGEST_NO_FILES, detail="No valid files uploaded"), 400

    contract_issues = _validate_ingest_contract(
        observer=observer,
        key_prefix=key_prefix,
        segment=segment,
        day=day,
        stream=stream,
        file_data=file_data,
        bundle=bundle,
        meta=meta,
    )
    if contract_issues:
        day_dir = day_path(day)
        day_dir.mkdir(parents=True, exist_ok=True)
        failed_dir = _save_to_failed(day_dir, file_data, segment)
        try:
            record_ingest_rejection(
                observer,
                reason_code=INGEST_CONTRACT_INVALID.code,
                segment=segment,
                stream=stream,
                version=observer.get("version"),
                issues=contract_issues,
            )
            save_observer(observer)
        except Exception:
            logger.exception(
                "Failed to record ingest rejection for %s", observer.get("name")
            )
        return (
            {
                "status": "failed",
                **_error_body(
                    INGEST_CONTRACT_INVALID,
                    detail="Uploaded file did not match the journal contract",
                ),
                "failed_path": str(failed_dir.relative_to(day_dir.parent)),
                "invalid_files": [str(issue) for issue in contract_issues],
            },
            INGEST_CONTRACT_INVALID.status,
        )

    ingest_files = [
        IngestFile(
            submitted=submitted,
            written=written,
            content=content,
            sha256=sha256,
        )
        for submitted, written, content, sha256 in file_data
    ]
    plan = resolve_ingest_plan(
        day=day,
        stream=stream,
        requested_segment=segment,
        files=ingest_files,
    )

    if plan.status == "conflict":
        append_history_record(key_prefix, day, _sync_record_for_plan(plan))
        logger.info(
            "Observer ingest outcome=conflict day=%s stream=%s requested=%s segment=%s",
            day,
            stream,
            segment,
            plan.segment,
        )
        detail = "Conflicting sidecar metadata for existing segment"
        return (
            {
                "status": "conflict",
                **_error_body(INGEST_SIDECAR_CONFLICT, detail=detail),
                "conflicting_files": plan.conflict_files,
                "existing_segment": plan.existing_segment or plan.segment,
            },
            INGEST_SIDECAR_CONFLICT.status,
        )

    if plan.status == "storage_failed":
        logger.error(
            "No available segment slot for %s/%s/%s from %s after %s attempts",
            day,
            stream,
            segment,
            observer.get("name", "unknown"),
            MAX_INGEST_SEGMENT_ATTEMPTS,
        )
        day_dir = day_path(day)
        day_dir.mkdir(parents=True, exist_ok=True)
        failed_dir = _save_to_failed(day_dir, file_data, segment)
        return (
            {
                "status": "failed",
                **_error_body(
                    INGEST_STORAGE_FAILED,
                    detail=(
                        "No available segment slot after "
                        f"{MAX_INGEST_SEGMENT_ATTEMPTS} attempts"
                    ),
                ),
                "failed_path": str(failed_dir.relative_to(day_dir.parent)),
            },
            507,
        )

    history_recorded = False
    apply_result = save_ingest_plan(plan, allow_reentry=True)
    if apply_result.reenter_resolution:
        first_result = apply_result
        plan = resolve_ingest_plan(
            day=day,
            stream=stream,
            requested_segment=segment,
            files=ingest_files,
        )
        if plan.status == "conflict":
            append_history_record(key_prefix, day, _sync_record_for_plan(plan))
            logger.info(
                "Observer ingest outcome=conflict day=%s stream=%s requested=%s segment=%s",
                day,
                stream,
                segment,
                plan.segment,
            )
            detail = "Conflicting sidecar metadata for existing segment"
            return (
                {
                    "status": "conflict",
                    **_error_body(INGEST_SIDECAR_CONFLICT, detail=detail),
                    "conflicting_files": plan.conflict_files,
                    "existing_segment": plan.existing_segment or plan.segment,
                },
                INGEST_SIDECAR_CONFLICT.status,
            )
        if plan.status == "storage_failed":
            day_dir = day_path(day)
            day_dir.mkdir(parents=True, exist_ok=True)
            failed_dir = _save_to_failed(day_dir, file_data, segment)
            return (
                {
                    "status": "failed",
                    **_error_body(
                        INGEST_STORAGE_FAILED,
                        detail=(
                            "No available segment slot after "
                            f"{MAX_INGEST_SEGMENT_ATTEMPTS} attempts"
                        ),
                    ),
                    "failed_path": str(failed_dir.relative_to(day_dir.parent)),
                },
                507,
            )
        if plan.status == "duplicate":
            records = _records_with_apply_result(plan.records, first_result)
            sync_record = _sync_record_for_plan(plan, records=records)
            append_history_record(key_prefix, day, sync_record)
            apply_result = first_result
            history_recorded = True
        else:
            second_result = save_ingest_plan(plan, allow_reentry=False)
            apply_result = _merge_apply_results(first_result, second_result)

    if plan.status == "duplicate":
        if not history_recorded:
            append_history_record(
                key_prefix,
                day,
                _sync_record_for_plan(
                    plan, records=_records_with_apply_result(plan.records, apply_result)
                ),
            )
        logger.info(
            "Observer ingest outcome=candidate_matched day=%s stream=%s requested=%s segment=%s",
            day,
            stream,
            segment,
            plan.segment,
        )

        observer["last_seen"] = now_ms()
        observer["stats"]["duplicates_rejected"] = (
            observer["stats"].get("duplicates_rejected", 0) + 1
        )
        clear_ingest_rejection(observer)
        save_observer(observer)

        return (
            {
                "status": "duplicate",
                "existing_segment": plan.existing_segment or plan.segment,
                "message": "All files already received",
            },
            200,
        )

    if not history_recorded:
        append_history_record(
            key_prefix,
            day,
            _sync_record_for_plan(
                plan, records=_records_with_apply_result(plan.records, apply_result)
            ),
        )

    receipt_now = now_ms()
    # Delivery freshness is server receipt time in int ms, using the same
    # now_ms() clock as last_seen; last_segment_day is the validated
    # client-declared day. Keep these distinct from think/surfaces
    # last_segment_at (max segment end/synthesis progress), stream
    # last_day/last_segment records, and ActivityStateMachine last_segment_day
    # in awareness/activity_state.json; those are different stores/owners.
    observer["last_seen"] = receipt_now
    observer["last_segment"] = plan.segment
    observer["last_segment_received_at"] = receipt_now
    observer["last_segment_day"] = day
    observer["stats"]["segments_received"] = (
        observer["stats"].get("segments_received", 0) + 1
    )
    observer["stats"]["bytes_received"] = (
        observer["stats"].get("bytes_received", 0) + apply_result.bytes_written
    )
    clear_ingest_rejection(observer)
    save_observer(observer)

    outcome = "minted" if plan.created_segment else "healed"
    if plan.status == "collision":
        outcome = "minted"
    elif plan.write_files and not any(file.is_media for file in plan.write_files):
        outcome = "healed"
    logger.info(
        "Observer ingest outcome=%s day=%s stream=%s requested=%s segment=%s",
        outcome,
        day,
        stream,
        segment,
        plan.segment,
    )

    body = {
        "status": "collision" if plan.status == "collision" else "ok",
        "segment": plan.segment,
        "files": apply_result.files_written,
        "bytes": apply_result.bytes_written,
        "_created_segment": plan.created_segment,
    }
    if plan.segment_original:
        body["segment_original"] = plan.segment_original
    return body, 200


def _sync_record_for_plan(
    plan, *, records: list[dict[str, Any]] | None = None
) -> dict[str, Any]:
    sync_record: dict[str, Any] = {
        "ts": now_ms(),
        "segment": plan.segment,
        "stream": plan.stream,
        "files": records if records is not None else plan.records,
    }
    if plan.segment_original:
        sync_record["segment_original"] = plan.segment_original
    return sync_record


def _records_with_apply_result(
    records: list[dict[str, Any]], apply_result
) -> list[dict[str, Any]]:
    written = set(apply_result.files_written)
    already_held = set(apply_result.files_already_held)
    adjusted = []
    for record in records:
        item = dict(record)
        if item.get("written") in written:
            item["disposition"] = "written"
        if item.get("written") in already_held:
            item["disposition"] = "already_held"
        adjusted.append(item)
    return adjusted


def _merge_apply_results(first, second):
    return type(first)(
        files_written=[*first.files_written, *second.files_written],
        files_already_held=[*first.files_already_held, *second.files_already_held],
        bytes_written=first.bytes_written + second.bytes_written,
        reenter_resolution=False,
    )


@observer_bp.route("/ingest", methods=["POST"])
def ingest_upload() -> Any:
    """Receive file uploads from observer.

    Observer ingest is the live, single capture-segment stream from one observer.

    Expects multipart form with:
    - segment: Segment key (HHMMSS_LEN)
    - day: Day string (YYYYMMDD)
    - files: One or more media files
    - host: (optional) Hostname of observer
    - platform: (optional) Platform of observer
    - meta: (optional) JSON-encoded metadata dict (facet, setting, etc.)

    Writes files to journal and emits observe.observing event.
    Host/platform are merged into meta (meta values take precedence).

    Returns status:
    - "ok": New segment accepted
    - "duplicate": All files already received (no processing triggered)
    - "collision": Conflicting content saved with an adjusted key
    - "conflict": Sidecar/metadata conflicts with an existing segment
    - "failed": Upload rejected — contract-invalid, or no available segment slot after retries
    """
    observer, key_prefix, error = resolve_ingest_identity("observer_ingest")
    if error is not None:
        return error

    # Get segment, day, and host info from form
    segment = request.form.get("segment", "").strip()
    day = request.form.get("day", "").strip()
    host = request.form.get("host", "").strip()
    platform = request.form.get("platform", "").strip()
    meta_str = request.form.get("meta", "").strip()

    # Parse meta JSON and merge host/platform (meta values take precedence)
    meta: dict = {}
    if meta_str:
        try:
            meta = json.loads(meta_str)
        except json.JSONDecodeError:
            logger.warning(f"Invalid meta JSON from observer: {meta_str[:100]}")
    if host and "host" not in meta:
        meta["host"] = host
    if platform and "platform" not in meta:
        meta["platform"] = platform

    # Warn if client hostname differs from registered observer name
    effective_host = meta.get("host", host)
    observer_name = observer.get("name", "")
    if effective_host and effective_host != observer_name:
        logger.warning(
            f"Observer '{observer_name}' ({key_prefix}) connecting from host "
            f"'{effective_host}' — hostname differs from registered name. "
            f"Use `journal observer rename` to update if the host was renamed."
        )

    if not segment:
        return error_response(MISSING_REQUIRED_FIELD, detail="Missing segment")
    if not day:
        return error_response(MISSING_REQUIRED_FIELD, detail="Missing day")

    # Validate segment format (HHMMSS_LEN)
    if not re.match(r"^\d{6}_\d+$", segment):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Invalid segment format",
        )

    # Validate day format (YYYYMMDD)
    if not re.match(r"^\d{8}$", day):
        return error_response(INVALID_DAY, detail="Invalid day format")

    # Get uploaded files
    files = request.files.getlist("files")
    if not files:
        return error_response(INGEST_NO_FILES, detail="No files uploaded")

    # Determine stream name. A registered observer carries a locked stream on its
    # record — that is authoritative and ignores any client-provided meta.stream.
    # Otherwise trust a valid client-provided meta.stream, falling back to deriving
    # from the observer name (lossy: stream_name(observer=...) strips qualifiers
    # like ".tmux", which is why registered observers lock the stream up front).
    locked_stream = observer.get("stream")
    if locked_stream:
        stream = locked_stream
    else:
        client_stream = meta.get("stream", "").strip()
        observer_name = observer.get("name", "unknown")
        if client_stream and re.match(r"^[a-z0-9][a-z0-9._-]*$", client_stream):
            stream = client_stream
        else:
            stream = stream_name(observer=observer_name)

    bundle = current_app.config["JOURNAL_CONTRACT_BUNDLE"]
    body, status = _process_ingest_files(
        observer,
        key_prefix,
        segment,
        day,
        stream,
        files,
        bundle=bundle,
        meta=meta,
    )
    if status != 200 or body.get("status") == "duplicate":
        return jsonify(body), status

    created_segment = bool(body.pop("_created_segment", False))
    segment = body["segment"]
    saved_files = body["files"]
    segment_dir = segment_path(day, segment, stream)

    # Write stream identity only when ingest minted a new segment directory.
    if created_segment:
        try:
            result = update_stream(stream, day, segment, type="observer")
            write_segment_stream(
                segment_dir,
                stream,
                result["prev_day"],
                result["prev_segment"],
                result["seq"],
            )
        except Exception as e:
            logger.warning(f"Failed to write stream identity: {e}")

    # Add stream to meta for downstream handlers
    meta["stream"] = stream

    # Emit observe.observing event to local Callosum
    # Include meta dict with host/platform and any client-provided metadata
    event_fields: dict[str, Any] = {
        "segment": segment,
        "day": day,
        "files": saved_files,
        "observer": observer_name,
        "stream": stream,
    }
    if meta:
        event_fields["meta"] = meta
    emit("observe", "observing", **event_fields)

    logger.debug(
        f"Received {len(saved_files)} files for {day}/{segment} from {observer.get('name')}"
    )
    return jsonify(body), status


@observer_bp.route("/ingest/manifest", methods=["GET"])
def ingest_manifest() -> Any:
    """List available manifest days for an observer."""
    _observer, key_prefix, error = resolve_observer_identity()
    if error is not None:
        return error

    hist_dir = get_hist_dir(key_prefix, ensure_exists=False)
    if not hist_dir.exists():
        return jsonify({"days": {}})

    days: dict[str, dict[str, int]] = {}
    for hist_path in sorted(hist_dir.glob("*.jsonl")):
        records = load_history(key_prefix, hist_path.stem)
        pruned = pruned_segments(records)
        segments = {
            record.get("segment", "")
            for record in records
            if not record.get("type") and record.get("segment")
        }
        segments.difference_update(pruned)
        days[hist_path.stem] = {"segments": len(segments)}

    return jsonify({"days": days})


@observer_bp.route("/ingest/manifest/<day>", methods=["GET"])
def ingest_manifest_day(day: str) -> Any:
    """Return a transfer manifest for all segments on a given day."""
    _observer, _key_prefix, error = resolve_observer_identity()
    if error is not None:
        return error

    if not re.match(r"^\d{8}$", day):
        return error_response(INVALID_DAY, detail="Invalid day format")

    manifest = {
        "version": 1,
        "day": day,
        "created_at": now_ms(),
        "host": platform.node() or "unknown",
        "segments": {},
    }

    for stream, seg_key, seg_path in iter_segments(day):
        arc_key = f"{stream}/{seg_key}"
        files = []
        for file_path in sorted(seg_path.iterdir()):
            if file_path.is_file() and file_path.name not in RESERVED_SEGMENT_FILENAMES:
                files.append(
                    {
                        "name": file_path.name,
                        "sha256": compute_file_sha256(file_path),
                        "size": file_path.stat().st_size,
                    }
                )
        manifest["segments"][arc_key] = {"files": files}

    return jsonify(manifest)


@observer_bp.route("/ingest/event", methods=["POST"])
def ingest_event() -> Any:
    """Receive events from observer and relay to local Callosum.

    Expects JSON body with:
    - tract: Event tract
    - event: Event name
    - ...additional fields
    """
    observer, _key_prefix, error = resolve_ingest_identity("observer_ingest_event")
    if error is not None:
        return error

    # Parse event
    data = request.get_json(force=True) if request.is_json else {}

    tract = data.get("tract")
    event = data.get("event")

    if not tract or not event:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="Missing tract or event",
        )

    # Add observer identifier
    data["observer"] = observer.get("name", "unknown")

    # Relay to local Callosum
    emit(tract, event, **{k: v for k, v in data.items() if k not in ("tract", "event")})

    # Update last_seen on status events
    if tract == "observe" and event == "status":
        record_status_beacon(observer, data)
        save_observer(observer)

    return jsonify({"status": "ok"})


@observer_bp.route("/health", methods=["POST"])
def ingest_health() -> Any:
    """Record a diagnostics-only observer health beacon."""
    observer, _key_prefix, error = resolve_observer_identity()
    if error is not None:
        return error

    data = request.get_json(force=True) if request.is_json else {}
    record_status_beacon(observer, data)
    save_observer(observer)
    return jsonify({"status": "ok"})


def _requested_protocol_version() -> int:
    """Observer ingest protocol version the requesting peer advertises.

    Returns 1 (legacy/unversioned) when the header is absent or unparsable.
    """
    raw = request.headers.get(protocol.OBSERVER_PROTOCOL_VERSION_HEADER)
    try:
        return int(raw)
    except (TypeError, ValueError):
        return 1


def _respond_observer_segments(items: list[dict], *, client_pv: int) -> Any:
    """Finalize the observer-segments 200 response.

    Canonical shape is the {items, total, protocol_version} collection envelope.
    A peer advertising protocol < v2 (a prior release) instead receives a bare
    top-level JSON array so its list-iterating consumer keeps syncing. This
    legacy downgrade is the SINGLE place a bare array reaches the wire - delete
    this branch once no prior-release peers remain (no retirement schedule yet).
    """
    if client_pv >= protocol.OBSERVER_PROTOCOL_VERSION:
        response, status = respond_collection(items)
        body = response.get_json()
        body["protocol_version"] = protocol.OBSERVER_PROTOCOL_VERSION
        return jsonify(body), status
    return jsonify(items)


@observer_bp.route("/ingest/segments/<day>")
def ingest_segments(day: str) -> Any:
    """List uploaded segments for a day with file verification.

    Returns JSON array of segments with file status:
    - present: File exists at recorded path
    - processed: Raw media absent with terminal same-stem sidecar proof
    - missing: File not found

    Args:
        day: Day string (YYYYMMDD)
    """
    observer, key_prefix, error = resolve_observer_identity()
    if error is not None:
        return error

    # Validate day format (YYYYMMDD)
    if not re.match(r"^\d{8}$", day):
        return error_response(INVALID_DAY, detail="Invalid day format")

    client_pv = _requested_protocol_version()

    # Load sync history for this observer/day
    records = load_history(key_prefix, day)
    pruned = pruned_segments(records)

    if not records:
        return _respond_observer_segments([], client_pv=client_pv)

    # Get day directory for file verification
    day_dir = day_path(day, create=False)

    # Determine fallback stream for records that don't carry their own. A registered
    # observer's locked stream is authoritative (ignores the ?stream= query param);
    # otherwise trust a valid client-provided stream, then derive from observer name.
    locked_stream = observer.get("stream")
    if locked_stream:
        fallback_stream = locked_stream
    else:
        client_stream = request.args.get("stream", "").strip()
        observer_name = observer.get("name", "unknown")
        if client_stream and re.match(r"^[a-z0-9][a-z0-9._-]*$", client_stream):
            fallback_stream = client_stream
        else:
            fallback_stream = stream_name(observer=observer_name)

    # Build response grouped by segment, deduplicating by (written, sha256):
    # same name+sha collapses, identical bytes under different names enumerate
    # separately for AC-13 corroboration, and same-name different-sha records stay
    # distinct so received-not-written rows can be filtered out before statting.
    segments: dict[str, dict] = {}
    observed_segments: set[str] = set()  # Track which segments have been observed

    for record in records:
        # Handle "observed" record type (from event handler)
        record_type = record.get("type", "upload")
        if record_type == "observed":
            observed_segments.add(record.get("segment", ""))
            continue

        segment = record.get("segment", "")
        if segment in pruned:
            continue
        stream = record.get("stream", fallback_stream)
        segment_original = record.get("segment_original")

        if segment not in segments:
            segments[segment] = {
                "key": segment,
                "files_by_identity": {},  # Keyed by (name, sha256) for deduplication
            }
            if segment_original:
                segments[segment]["original_key"] = segment_original

        # Check each file's status
        for file_rec in record.get("files", []):
            # Old history rows predate dispositions and remain enumerable. New
            # received-not-written rows are audit-only and must not corroborate
            # held bytes for clients.
            if file_rec.get("disposition") == DISPOSITION_RECEIVED_NOT_WRITTEN:
                continue
            written = file_rec.get("written", "")
            submitted = file_rec.get("submitted", "")
            size = file_rec.get("size", 0)
            sha256 = file_rec.get("sha256", "")

            file_info = {
                "name": written,
                "size": size,
                "sha256": sha256,
            }

            # Include submitted_name only if different
            if submitted != written:
                file_info["submitted_name"] = submitted

            file_info["status"] = resolve_file_status(
                day_dir, stream, segment, written, size
            )

            # Deduplicate exact same name+sha records; identical bytes under
            # different names must remain separately corroboratable.
            segments[segment]["files_by_identity"][(written, sha256)] = file_info

    # Convert files_by_identity dicts to lists and sort by segment key
    result = []
    for segment_data in sorted(segments.values(), key=lambda s: s["key"]):
        if not segment_data["files_by_identity"]:
            continue
        segment_key = segment_data["key"]
        entry = {
            "key": segment_key,
            "observed": segment_key in observed_segments,
            "files": list(segment_data["files_by_identity"].values()),
        }
        if "original_key" in segment_data:
            entry["original_key"] = segment_data["original_key"]
        result.append(entry)
    return _respond_observer_segments(result, client_pv=client_pv)
