# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Observer app - manage observer connections.

Provides endpoints for:
- Managing observer registrations (UI)
- Receiving file uploads from observers (ingest)
- Receiving transferred segments from other instances (transfer ingest)
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
from pathlib import Path
from typing import Any

from flask import Blueprint, Response, jsonify, request, stream_with_context
from werkzeug.utils import secure_filename

import solstone.convey.bridge as convey_bridge
from solstone.apps.utils import log_app_action
from solstone.convey import emit
from solstone.convey.bridge import _SSE_HEARTBEAT_SECONDS
from solstone.convey.copy import OBSERVER_CALLOSUM_LIVE_LABEL
from solstone.convey.reasons import (
    AUTH_REQUIRED,
    FEATURE_UNAVAILABLE,
    FILE_READ_FAILED,
    INGEST_NO_FILES,
    INGEST_STORAGE_FAILED,
    INVALID_DAY,
    INVALID_SEGMENT_OR_STREAM,
    MISSING_REQUIRED_FIELD,
    PAIRED_DEVICE_NOT_FOUND,
    PL_REVOKED,
    SETTINGS_OPERATION_FAILED,
    Reason,
)
from solstone.convey.utils import error_response
from solstone.observe.utils import (
    MAX_SEGMENT_ATTEMPTS,
    compute_bytes_sha256,
    compute_file_sha256,
    find_available_segment,
)
from solstone.think.streams import stream_name, update_stream, write_segment_stream
from solstone.think.utils import day_path, iter_segments, now_ms, segment_path

from .utils import (
    ObserverRegistry,
    append_history_record,
    find_segment_by_sha256,
    get_hist_dir,
    get_observers_dir,
    list_observers,
    load_history,
    load_observer,
    observer_filename_prefix,
    resolve_observer_identity,
    save_observer,
)

logger = logging.getLogger(__name__)

observer_bp = Blueprint(
    "app:observer",
    __name__,
    url_prefix="/app/observer",
)
OBSERVER_CALLOSUM_SSE_ROUTE = "/app/observer/<key>/callosum"
_OBSERVER_CALLOSUM_SSE_RULE = OBSERVER_CALLOSUM_SSE_ROUTE.removeprefix(
    observer_bp.url_prefix or ""
)

# Key length in bytes (256 bits = 32 bytes)
KEY_BYTES = 32
ACTIVE_THRESHOLD_MS = 30_000
STALE_THRESHOLD_MS = 120_000
FUTURE_CLOCK_DRIFT_TOLERANCE_MS = 5 * 60 * 1000

OBSERVER_STATE_LABELS = {
    "connected": "Connected",
    "stale": "Stale",
    "disconnected": "Disconnected",
    "revoked": "Revoked",
}


def _error_body(reason: Reason, *, detail: str | None = None) -> dict[str, str]:
    return {
        "error": reason.message,
        "reason_code": reason.code,
        "detail": detail or "",
    }


def _sse_error_event(reason: Reason, *, detail: str) -> str:
    return f"event: error\ndata: {json.dumps(_error_body(reason, detail=detail))}\n\n"


def _generate_key() -> str:
    """Generate a URL-safe key for observer authentication."""
    return base64.urlsafe_b64encode(secrets.token_bytes(KEY_BYTES)).decode().rstrip("=")


def _classify_observer_freshness(
    last_seen_ms: int | None,
    revoked: bool,
    now_ms: int,
) -> dict[str, object]:
    """Classify a registered observer's freshness.

    Returns keys: state, group, elapsed_ms, clock_skew.
    """
    if revoked:
        return {
            "state": "revoked",
            "group": "inactive",
            "elapsed_ms": None,
            "clock_skew": False,
        }
    if last_seen_ms is None:
        return {
            "state": "disconnected",
            "group": "inactive",
            "elapsed_ms": None,
            "clock_skew": False,
        }
    elapsed = now_ms - last_seen_ms
    if elapsed < -FUTURE_CLOCK_DRIFT_TOLERANCE_MS:
        return {
            "state": "disconnected",
            "group": "inactive",
            "elapsed_ms": elapsed,
            "clock_skew": True,
        }
    if elapsed < 0:
        return {
            "state": "connected",
            "group": "active",
            "elapsed_ms": 0,
            "clock_skew": False,
        }
    if elapsed < ACTIVE_THRESHOLD_MS:
        return {
            "state": "connected",
            "group": "active",
            "elapsed_ms": elapsed,
            "clock_skew": False,
        }
    if elapsed < STALE_THRESHOLD_MS:
        return {
            "state": "stale",
            "group": "stale",
            "elapsed_ms": elapsed,
            "clock_skew": False,
        }
    return {
        "state": "disconnected",
        "group": "inactive",
        "elapsed_ms": elapsed,
        "clock_skew": False,
    }


def _serialize_observer(observer: dict[str, Any], current_now: int) -> dict[str, Any]:
    """Serialize a registered observer for management API consumers."""
    freshness = _classify_observer_freshness(
        observer.get("last_seen"),
        observer.get("revoked", False),
        current_now,
    )
    key_prefix = observer_filename_prefix(observer)
    return {
        "key_prefix": key_prefix,
        "name": observer.get("name", ""),
        "created_at": observer.get("created_at", 0),
        "last_seen": observer.get("last_seen"),
        "last_segment": observer.get("last_segment"),
        "enabled": observer.get("enabled", True),
        "revoked": observer.get("revoked", False),
        "revoked_at": observer.get("revoked_at"),
        "stats": observer.get("stats", {}),
        "live": convey_bridge.subscription_count(key_prefix) > 0,
        "last_chat_request_at": convey_bridge.last_chat_request_at(key_prefix),
        **freshness,
        "label": OBSERVER_STATE_LABELS[str(freshness["state"])],
    }


def _revoke_observer(key: str) -> bool:
    """Revoke observer by key (soft-delete)."""
    observer = load_observer(key)
    if not observer:
        return False
    observer["revoked"] = True
    observer["revoked_at"] = now_ms()
    return save_observer(observer)


# === Management API (session-protected) ===


@observer_bp.route("/api/list")
def api_list() -> Any:
    """List all registered observers."""
    current_now = now_ms()
    observers = list_observers()
    # Sanitize output - don't expose full keys
    result = [_serialize_observer(observer, current_now) for observer in observers]

    group_order = {"active": 0, "stale": 1, "inactive": 2}
    result.sort(
        key=lambda observer: (
            group_order[observer.get("group", "inactive")],
            1 if observer.get("last_seen") is None else 0,
            -(observer.get("last_seen") or 0),
            observer.get("key_prefix", ""),
        )
    )

    return jsonify(
        {
            "thresholds": {
                "active_ms": ACTIVE_THRESHOLD_MS,
                "stale_ms": STALE_THRESHOLD_MS,
            },
            "labels": {
                "live": OBSERVER_CALLOSUM_LIVE_LABEL,
            },
            "observers": result,
        }
    )


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
def callosum_sse(key: str) -> Any:
    """Stream Callosum events to an authenticated observer process."""
    observer, key_prefix, error = resolve_observer_identity(key)
    if error is not None:
        return error
    auth_key = observer.get("key")
    fingerprint = observer.get("fingerprint")

    handle = convey_bridge.register_sse_subscriber(key_prefix)

    def current_observer() -> dict | None:
        if isinstance(fingerprint, str) and fingerprint:
            return ObserverRegistry.singleton().by_fingerprint(fingerprint)
        if isinstance(auth_key, str) and auth_key:
            return load_observer(auth_key)
        return None

    def generate():
        try:
            yield ": heartbeat\n\n"
            while True:
                if handle.dropped.is_set():
                    return
                try:
                    serialized_message = handle.queue.get(
                        timeout=_SSE_HEARTBEAT_SECONDS
                    )
                except queue.Empty:
                    observer_now = current_observer()
                    if not observer_now:
                        yield _sse_error_event(
                            AUTH_REQUIRED, detail="Authorization required"
                        )
                        return
                    if observer_now.get("revoked", False):
                        yield _sse_error_event(PL_REVOKED, detail="Observer revoked")
                        return
                    if not observer_now.get("enabled", True):
                        yield _sse_error_event(
                            FEATURE_UNAVAILABLE, detail="Observer disabled"
                        )
                        return
                    yield ": heartbeat\n\n"
                    continue

                if handle.dropped.is_set():
                    return
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


@observer_bp.route("/api/create", methods=["POST"])
def api_create() -> Any:
    """Create a new observer registration."""
    data = request.get_json(force=True) if request.is_json else {}
    name = data.get("name", "").strip()
    if not name:
        return error_response(MISSING_REQUIRED_FIELD, detail="Name is required")

    # Generate key
    key = _generate_key()

    # Create observer record
    observer_data = {
        "key": key,
        "name": name,
        "created_at": now_ms(),
        "last_seen": None,
        "last_segment": None,
        "enabled": True,
        "stats": {
            "segments_received": 0,
            "bytes_received": 0,
        },
    }

    if not save_observer(observer_data):
        return error_response(
            SETTINGS_OPERATION_FAILED,
            detail="Failed to save observer",
        )

    # Log observer creation (journal-level, no facet)
    log_app_action(
        app="observer",
        facet=None,
        action="observer_create",
        params={"name": name, "key_prefix": key[:8]},
    )

    # Build ingest URL
    ingest_url = f"/app/observer/ingest/{key}"

    return jsonify(
        {
            "key": key,
            "key_prefix": key[:8],
            "name": name,
            "ingest_url": ingest_url,
        }
    )


@observer_bp.route("/api/<key_prefix>", methods=["DELETE"])
def api_delete(key_prefix: str) -> Any:
    """Revoke an observer by key prefix (soft-delete)."""
    # Find observer by prefix
    observers_dir = get_observers_dir()
    observer_path = observers_dir / f"{key_prefix}.json"
    if not observer_path.exists():
        return error_response(PAIRED_DEVICE_NOT_FOUND, detail="Observer not found")

    try:
        with open(observer_path) as f:
            data = json.load(f)
        key = data.get("key", "")
        name = data.get("name", "")
    except (json.JSONDecodeError, OSError):
        return error_response(FILE_READ_FAILED, detail="Failed to read observer")

    if not _revoke_observer(key):
        return error_response(
            SETTINGS_OPERATION_FAILED,
            detail="Failed to revoke observer",
        )

    # Log observer revocation (journal-level, no facet)
    log_app_action(
        app="observer",
        facet=None,
        action="observer_revoke",
        params={"name": name, "key_prefix": key_prefix},
    )

    return jsonify({"status": "ok"})


@observer_bp.route("/api/<key_prefix>/key")
def api_get_key(key_prefix: str) -> Any:
    """Get full key and ingest URL for an observer."""
    # Find observer by prefix
    observers_dir = get_observers_dir()
    observer_path = observers_dir / f"{key_prefix}.json"
    if not observer_path.exists():
        return error_response(PAIRED_DEVICE_NOT_FOUND, detail="Observer not found")

    try:
        with open(observer_path) as f:
            data = json.load(f)
    except (json.JSONDecodeError, OSError):
        return error_response(FILE_READ_FAILED, detail="Failed to read observer")

    if data.get("revoked", False):
        return error_response(
            PL_REVOKED,
            detail="key unavailable — observer revoked",
        )

    log_app_action(
        app="observer",
        facet=None,
        action="observer_key_view",
        params={"name": data.get("name", ""), "key_prefix": key_prefix},
    )

    key = data.get("key", "")
    return jsonify(
        {
            "key": key,
            "name": data.get("name", ""),
            "ingest_url": f"/app/observer/ingest/{key}",
        }
    )


# === Sync history helpers ===


def _find_by_inode(day_dir: Path, inode: int) -> Path | None:
    """Find a file by inode in the day directory.

    Searches recursively for a file with the given inode.

    Args:
        day_dir: Path to day directory
        inode: Inode number to search for

    Returns:
        Path to file if found, None otherwise
    """
    try:
        for path in day_dir.rglob("*"):
            if path.is_file():
                try:
                    if path.stat().st_ino == inode:
                        return path
                except OSError:
                    continue
    except OSError:
        pass
    return None


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


def _process_ingest_files(
    observer: dict,
    key_prefix: str,
    segment: str,
    day: str,
    stream: str,
    uploaded_files,
    *,
    source: str | None = None,
) -> tuple[dict, int]:
    """Shared ingest pipeline: read/hash files, dedup, deconflict, save, record history, update stats.

    Parameters
    ----------
    observer : dict
        Observer metadata dict (must include 'stats', 'name', 'last_seen', etc.)
    key_prefix : str
        First 8 chars of observer key.
    segment : str
        Requested segment key (HHMMSS_LEN format).
    day : str
        Day string (YYYYMMDD format).
    stream : str
        Stream name (already resolved by caller).
    uploaded_files : list
        List of Flask FileStorage objects from request.files.getlist("files").
    source : str or None
        If provided, added as "source" field to history record (e.g., "transfer").

    Returns
    -------
    tuple of (dict, int)
        Response body dict and HTTP status code.
    """
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

    # Check for duplicate submission by SHA256
    incoming_sha256s = {fd[3] for fd in file_data}
    existing_segment, matched_sha256s = find_segment_by_sha256(
        key_prefix, day, incoming_sha256s
    )

    if existing_segment:
        logger.info(
            f"Duplicate segment rejected: {day}/{segment} from {observer.get('name')} "
            f"(matches existing {existing_segment})"
        )

        observer["last_seen"] = now_ms()
        observer["stats"]["duplicates_rejected"] = (
            observer["stats"].get("duplicates_rejected", 0) + 1
        )
        save_observer(observer)

        return (
            {
                "status": "duplicate",
                "existing_segment": existing_segment,
                "message": "All files already received",
            },
            200,
        )

    partial_match = bool(matched_sha256s)

    # Ensure day directory exists
    day_dir = day_path(day)
    day_dir.mkdir(parents=True, exist_ok=True)

    # Find available segment key within the stream directory
    stream_dir = day_dir / stream
    stream_dir.mkdir(parents=True, exist_ok=True)

    original_segment = segment
    available_segment = find_available_segment(stream_dir, segment)

    if available_segment is None:
        logger.error(
            f"No available segment slot for {day}/{stream}/{segment} from "
            f"{observer.get('name', 'unknown')} after {MAX_SEGMENT_ATTEMPTS} attempts"
        )
        failed_dir = _save_to_failed(day_dir, file_data, segment)
        return (
            {
                "status": "failed",
                **_error_body(
                    INGEST_STORAGE_FAILED,
                    detail=(
                        "No available segment slot after "
                        f"{MAX_SEGMENT_ATTEMPTS} attempts"
                    ),
                ),
                "failed_path": str(failed_dir.relative_to(day_dir.parent)),
            },
            507,
        )

    segment = available_segment
    if segment != original_segment:
        logger.info(
            f"Segment collision resolved: {original_segment} -> {segment} "
            f"for observer {observer.get('name', 'unknown')}"
        )

    # Create segment directory for files (under stream)
    segment_dir = segment_path(day, segment, stream)
    segment_dir.mkdir(parents=True, exist_ok=True)

    # Save files from memory to disk
    saved_files = []
    file_records = []
    total_bytes = 0

    for submitted_filename, simple_filename, content, sha256 in file_data:
        target_path = segment_dir / simple_filename

        try:
            target_path.write_bytes(content)
            stat = target_path.stat()
            file_size = stat.st_size
            file_inode = stat.st_ino

            saved_files.append(simple_filename)
            total_bytes += file_size

            file_records.append(
                {
                    "submitted": submitted_filename,
                    "written": simple_filename,
                    "inode": file_inode,
                    "size": file_size,
                    "sha256": sha256,
                }
            )

            logger.info(f"Saved {simple_filename} to {segment_dir}")
        except OSError as e:
            logger.error(f"Failed to save {simple_filename}: {e}")
            return (
                _error_body(
                    INGEST_STORAGE_FAILED,
                    detail=f"Failed to save {simple_filename}",
                ),
                500,
            )

    if not saved_files:
        return _error_body(INGEST_NO_FILES, detail="No valid files saved"), 400

    sync_record = {
        "ts": now_ms(),
        "segment": segment,
        "stream": stream,
        "files": file_records,
    }
    if segment != original_segment:
        sync_record["segment_original"] = original_segment
    if partial_match:
        sync_record["partial_match_sha256s"] = list(matched_sha256s)
    if source:
        sync_record["source"] = source
    append_history_record(key_prefix, day, sync_record)

    observer["last_seen"] = now_ms()
    observer["last_segment"] = segment
    observer["stats"]["segments_received"] = (
        observer["stats"].get("segments_received", 0) + 1
    )
    observer["stats"]["bytes_received"] = (
        observer["stats"].get("bytes_received", 0) + total_bytes
    )
    save_observer(observer)

    status = "collision" if segment != original_segment else "ok"
    return {
        "status": status,
        "segment": segment,
        "files": saved_files,
        "bytes": total_bytes,
    }, 200


@observer_bp.route("/ingest", methods=["POST"])
@observer_bp.route("/ingest/<key>", methods=["POST"])
def ingest_upload(key: str | None = None) -> Any:
    """Receive file uploads from observer.

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
    - "collision": New segment saved with adjusted key (directory conflict)
    """
    observer, key_prefix, error = resolve_observer_identity(key)
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
            f"Use `sol observer rename` to update if the host was renamed."
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

    # Determine stream name: trust client-provided stream in meta if valid,
    # otherwise derive from observer registration name.
    # Deriving from observer name via stream_name(observer=...) calls _strip_hostname,
    # which strips qualifiers like ".tmux" — so "fedora.tmux" becomes "fedora",
    # colliding both observers into one stream.
    client_stream = meta.get("stream", "").strip()
    observer_name = observer.get("name", "unknown")
    if client_stream and re.match(r"^[a-z0-9][a-z0-9._-]*$", client_stream):
        stream = client_stream
    else:
        stream = stream_name(observer=observer_name)

    body, status = _process_ingest_files(
        observer, key_prefix, segment, day, stream, files
    )
    if status != 200 or body.get("status") == "duplicate":
        return jsonify(body), status

    segment = body["segment"]
    saved_files = body["files"]
    segment_dir = segment_path(day, segment, stream)

    # Write stream identity for this segment
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

    logger.info(
        f"Received {len(saved_files)} files for {day}/{segment} from {observer.get('name')}"
    )
    return jsonify(body), status


@observer_bp.route("/ingest/<key>/transfer", methods=["POST"])
def ingest_transfer(key: str) -> Any:
    """Receive transferred file uploads from another solstone instance."""
    observer, key_prefix, error = resolve_observer_identity(key)
    if error is not None:
        return error

    segment = request.form.get("segment", "").strip()
    day = request.form.get("day", "").strip()
    stream = request.form.get("stream", "").strip()
    host = request.form.get("host", "").strip()
    platform_name = request.form.get("platform", "").strip()
    meta_str = request.form.get("meta", "").strip()

    meta: dict = {}
    if meta_str:
        try:
            meta = json.loads(meta_str)
        except json.JSONDecodeError:
            logger.warning(f"Invalid meta JSON from observer: {meta_str[:100]}")
    if host and "host" not in meta:
        meta["host"] = host
    if platform_name and "platform" not in meta:
        meta["platform"] = platform_name

    if not segment:
        return error_response(MISSING_REQUIRED_FIELD, detail="Missing segment")
    if not day:
        return error_response(MISSING_REQUIRED_FIELD, detail="Missing day")
    if not stream:
        return error_response(MISSING_REQUIRED_FIELD, detail="Missing stream")
    if not re.match(r"^\d{6}_\d+$", segment):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Invalid segment format",
        )
    if not re.match(r"^\d{8}$", day):
        return error_response(INVALID_DAY, detail="Invalid day format")
    if not re.match(r"^[a-z0-9][a-z0-9._-]*$", stream):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Invalid stream format",
        )

    files = request.files.getlist("files")
    if not files:
        return error_response(INGEST_NO_FILES, detail="No files uploaded")

    body, status = _process_ingest_files(
        observer,
        key_prefix,
        segment,
        day,
        stream,
        files,
        source="transfer",
    )
    if status != 200 or body.get("status") == "duplicate":
        return jsonify(body), status

    observer_name = observer.get("name", "")
    event_fields: dict[str, Any] = {
        "segment": body["segment"],
        "day": day,
        "files": body["files"],
        "observer": observer_name,
        "stream": stream,
    }
    if meta:
        event_fields["meta"] = meta
    emit("observe", "transferred", **event_fields)

    return jsonify(body), status


@observer_bp.route("/ingest/<key>/manifest", methods=["GET"])
def ingest_manifest(key: str) -> Any:
    """List available manifest days for an observer."""
    _observer, key_prefix, error = resolve_observer_identity(key)
    if error is not None:
        return error

    hist_dir = get_hist_dir(key_prefix, ensure_exists=False)
    if not hist_dir.exists():
        return jsonify({"days": {}})

    days: dict[str, dict[str, int]] = {}
    for hist_path in sorted(hist_dir.glob("*.jsonl")):
        records = load_history(key_prefix, hist_path.stem)
        segments = {
            record.get("segment", "")
            for record in records
            if not record.get("type") and record.get("segment")
        }
        days[hist_path.stem] = {"segments": len(segments)}

    return jsonify({"days": days})


@observer_bp.route("/ingest/<key>/manifest/<day>", methods=["GET"])
def ingest_manifest_day(key: str, day: str) -> Any:
    """Return a transfer manifest for all segments on a given day."""
    _observer, _key_prefix, error = resolve_observer_identity(key)
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
            if file_path.is_file():
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
@observer_bp.route("/ingest/<key>/event", methods=["POST"])
def ingest_event(key: str | None = None) -> Any:
    """Receive events from observer and relay to local Callosum.

    Expects JSON body with:
    - tract: Event tract
    - event: Event name
    - ...additional fields
    """
    observer, _key_prefix, error = resolve_observer_identity(key)
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
        observer["last_seen"] = now_ms()
        save_observer(observer)

    return jsonify({"status": "ok"})


@observer_bp.route("/ingest/segments/<day>")
@observer_bp.route("/ingest/<key>/segments/<day>")
def ingest_segments(day: str, key: str | None = None) -> Any:
    """List uploaded segments for a day with file verification.

    Returns JSON array of segments with file status:
    - present: File exists at recorded path
    - relocated: File found at different path (by inode)
    - missing: File not found

    Args:
        day: Day string (YYYYMMDD)
        key: Observer authentication key (from URL path, legacy)
    """
    observer, key_prefix, error = resolve_observer_identity(key)
    if error is not None:
        return error

    # Validate day format (YYYYMMDD)
    if not re.match(r"^\d{8}$", day):
        return error_response(INVALID_DAY, detail="Invalid day format")

    # Load sync history for this observer/day
    records = load_history(key_prefix, day)

    if not records:
        return jsonify([])

    # Get day directory for file verification
    day_dir = day_path(day)

    # Determine stream: trust client-provided query param if valid,
    # otherwise derive from observer name (same logic as ingest_upload).
    client_stream = request.args.get("stream", "").strip()
    observer_name = observer.get("name", "unknown")
    if client_stream and re.match(r"^[a-z0-9][a-z0-9._-]*$", client_stream):
        fallback_stream = client_stream
    else:
        fallback_stream = stream_name(observer=observer_name)

    # Build response grouped by segment, deduplicating by sha256
    # Later records overwrite earlier ones (most recent upload wins)
    segments: dict[str, dict] = {}
    observed_segments: set[str] = set()  # Track which segments have been observed

    for record in records:
        # Handle "observed" record type (from event handler)
        record_type = record.get("type", "upload")
        if record_type == "observed":
            observed_segments.add(record.get("segment", ""))
            continue

        segment = record.get("segment", "")
        stream = record.get("stream", fallback_stream)
        segment_original = record.get("segment_original")

        if segment not in segments:
            segments[segment] = {
                "key": segment,
                "files_by_sha": {},  # Keyed by sha256 for deduplication
            }
            if segment_original:
                segments[segment]["original_key"] = segment_original

        # Check each file's status
        for file_rec in record.get("files", []):
            written = file_rec.get("written", "")
            submitted = file_rec.get("submitted", "")
            inode = file_rec.get("inode")
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

            # Check file status - files are in stream/segment directories
            segment_dir = day_dir / stream / segment
            recorded_path = segment_dir / written
            if recorded_path.exists():
                file_info["status"] = "present"
            elif inode and day_dir.exists():
                # Try to find by inode
                relocated = _find_by_inode(day_dir, inode)
                if relocated:
                    file_info["status"] = "relocated"
                    file_info["current_path"] = str(relocated.relative_to(day_dir))
                else:
                    file_info["status"] = "missing"
            else:
                file_info["status"] = "missing"

            # Deduplicate by sha256 - later uploads overwrite earlier
            segments[segment]["files_by_sha"][sha256] = file_info

    # Convert files_by_sha dicts to lists and sort by segment key
    result = []
    for segment_data in sorted(segments.values(), key=lambda s: s["key"]):
        segment_key = segment_data["key"]
        entry = {
            "key": segment_key,
            "observed": segment_key in observed_segments,
            "files": list(segment_data["files_by_sha"].values()),
        }
        if "original_key" in segment_data:
            entry["original_key"] = segment_data["original_key"]
        result.append(entry)
    return jsonify(result)
