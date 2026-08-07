# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import re
from datetime import datetime
from pathlib import Path
from typing import Any

from flask import Blueprint, current_app, g, jsonify, request
from werkzeug.utils import secure_filename

from solstone.apps.utils import log_app_action
from solstone.convey import state
from solstone.convey.icons import lucide_svg
from solstone.convey.reasons import (
    FILE_NOT_FOUND,
    IMPORT_CLIENT_ID_CONFLICT,
    IMPORT_CONFLICT,
    IMPORT_METADATA_FAILED,
    IMPORT_NOT_FOUND,
    IMPORT_QUEUE_UNREACHABLE,
    INGEST_NO_FILES,
    INVALID_OPERATION_FOR_STATE,
    INVALID_REQUEST_VALUE,
    JOURNAL_SOURCE_PROBLEM,
    MISSING_REQUIRED_FIELD,
)
from solstone.convey.utils import (
    error_response,
    load_json,
    respond_collection,
    success_response,
)
from solstone.think.callosum import callosum_send
from solstone.think.detect_created import detect_created, resolve_created_deterministic
from solstone.think.importers.shared import find_manifest_by_hash, hash_source
from solstone.think.importers.utils import (
    build_import_info,
    find_staged_by_client_item_id,
    find_staged_by_source_hash,
    generate_content_manifest,
    get_import_details,
    list_import_timestamps,
    move_import,
    read_import_metadata,
    read_import_status_info,
    resolve_import_status,
    save_import_file,
    save_import_text,
    update_import_metadata_fields,
    write_import_metadata,
)
from solstone.think.media import (
    MEDIA_EXTENSIONS,
    canonical_source,
    canonical_source_signal,
)
from solstone.think.utils import day_path, now_ms

from .journal_sources import (
    STATE_AREAS,
    create_state_directory,
    find_journal_source_by_name,
    generate_key,
    get_state_directory,
    is_valid_journal_source_name,
    journal_source_state_prefix,
    list_journal_sources,
    require_journal_source,
    save_journal_source,
)
from .resolve import (
    ResolveInvalid,
    ResolveNotFound,
    resolve_config,
    resolve_config_all,
    resolve_entity,
    resolve_staged_facet,
)

import_bp = Blueprint(
    "app:import",
    __name__,
    url_prefix="/app/import",
    static_folder="static",
    static_url_path="/static",
)

SOURCE_METADATA = [
    {
        "name": "ics",
        "display_name": "calendar",
        "icon": "calendar",
        "description": "import events from Google Calendar, Apple Calendar, or Outlook",
        "input_type": "file",
        "upload_prompt": "upload your .ics file or .zip export",
        "has_guide": True,
        "accept": ".ics,.zip",
    },
    {
        "name": "chatgpt",
        "display_name": "ChatGPT",
        "icon": "message-square",
        "description": "import your conversation history from ChatGPT",
        "input_type": "file",
        "upload_prompt": "upload your ChatGPT export .zip file",
        "has_guide": True,
        "accept": ".zip",
    },
    {
        "name": "claude",
        "display_name": "Claude",
        "icon": "message-circle",
        "description": "import your conversation history from Claude",
        "input_type": "file",
        "upload_prompt": "upload your Claude export .zip file",
        "has_guide": True,
        "accept": ".zip",
    },
    {
        "name": "gemini",
        "display_name": "Gemini",
        "icon": "sparkles",
        "description": "import your activity history from Google Gemini",
        "input_type": "file",
        "upload_prompt": "upload your Google Takeout .zip file",
        "has_guide": True,
        "accept": ".zip,.json",
    },
    {
        "name": "obsidian",
        "display_name": "notes",
        "icon": "file-text",
        "description": "import notes from Obsidian, Logseq, or any markdown vault",
        "input_type": "path_input",
        "upload_prompt": "paste the full path to your vault folder",
        "has_guide": True,
        "accept": "",
    },
    {
        "name": "kindle",
        "display_name": "Kindle",
        "icon": "book-open",
        "description": "import highlights and clippings from your Kindle",
        "input_type": "file",
        "upload_prompt": "upload your My Clippings.txt file",
        "has_guide": True,
        "accept": ".txt",
    },
    {
        "name": "journal_archive",
        "display_name": "journal",
        "icon": "book",
        "description": "import a full journal export from another journal",
        "input_type": "file",
        "upload_prompt": "upload your journal export .zip file",
        "has_guide": True,
        "accept": ".zip",
    },
    {
        "name": "recording",
        "display_name": "meeting audio",
        "icon": "mic",
        "description": "import audio from meetings or conversations",
        "input_type": "file",
        "upload_prompt": "upload an audio file (.m4a, .mp3, .wav)",
        "has_guide": False,
        "accept": ",".join(sorted(MEDIA_EXTENSIONS)),
    },
    {
        "name": "document",
        "display_name": "document",
        "icon": "file",
        "description": "import a PDF document",
        "input_type": "file",
        "upload_prompt": "upload a PDF file",
        "has_guide": False,
        "accept": ".pdf",
    },
    {
        "name": "image",
        "display_name": "image",
        "icon": "image",
        "description": "add a photo or screenshot and let sol describe what's in it",
        "input_type": "file",
        "upload_prompt": "upload an image (PNG, JPEG, WebP, GIF, TIFF)",
        "has_guide": False,
        "accept": ".png,.jpg,.jpeg,.webp,.gif,.tiff",
    },
    {
        "name": "quick",
        "display_name": "quick import",
        "icon": "zap",
        "description": "paste text or drop any file for quick import",
        "input_type": "text",
        "upload_prompt": "paste text or drag and drop a file",
        "has_guide": False,
        "accept": "",
    },
]


def _source_display_name(source_type: str | None) -> str | None:
    """Return the current display name for a known source type, else None."""
    if not source_type:
        return None
    entry = next((s for s in SOURCE_METADATA if s["name"] == source_type), None)
    return entry["display_name"] if entry else None


def _source_metadata_payload(source: dict[str, Any]) -> dict[str, Any]:
    payload = dict(source)
    payload["icon_svg"] = lucide_svg(source["icon"])
    return payload


def _link_id_from_identity() -> str | None:
    return (
        g.identity.fingerprint
        if g.identity.mode in {"pl-direct", "pl-via-spl"}
        else None
    )


def _form_bool(value: str | None) -> bool:
    return value.strip().lower() in {"true", "1", "yes"} if value else False


CANONICAL_IMPORT_SOURCES = {"audio", "image", "document", "text"}


def _clean_optional(value: Any) -> str | None:
    if value is None:
        return None
    if isinstance(value, str):
        cleaned = value.strip()
    else:
        cleaned = str(value).strip()
    return cleaned or None


def _client_bag(value: Any) -> dict:
    if isinstance(value, dict):
        return value
    if isinstance(value, str) and value.strip():
        try:
            parsed = json.loads(value)
        except json.JSONDecodeError:
            return {}
        return parsed if isinstance(parsed, dict) else {}
    return {}


def _build_save_summary(
    metadata: dict,
    *,
    status: str,
    replay: bool,
    duplicate: dict | None,
    recommended_action: str | None = None,
    in_progress: bool = False,
) -> dict:
    """Build the versioned import staging summary response."""
    client = metadata.get("client")
    if not isinstance(client, dict):
        client = {}
    action = recommended_action
    if action is None:
        action = "do_not_start" if status == "duplicate" else "start"

    summary: dict[str, Any] = {
        "schema_version": 1,
        "status": status,
        "replay": replay,
        "path": str(metadata.get("file_path", "")),
        "timestamp": str(metadata.get("user_timestamp", "")),
        "client_item_id": str(metadata.get("client_item_id", "")),
        "source": metadata.get("source", "text"),
        "facet": metadata.get("facet"),
        "setting": metadata.get("setting"),
        "recommended_action": action,
        "metadata": {
            "original_filename": metadata.get("original_filename"),
            "mime_type": metadata.get("mime_type"),
            "imported_via": metadata.get("imported_via"),
            "observer_handle": metadata.get("observer_handle"),
            "source_hint": metadata.get("source_hint"),
            "client": client,
        },
        "diagnostics": {
            "timestamp_detection_method": metadata.get(
                "timestamp_detection_method", "duplicate"
            ),
            "timestamp_detection_model_called": metadata.get(
                "timestamp_detection_model_called", False
            ),
            "timestamp_detection_no_match_reason": metadata.get(
                "timestamp_detection_no_match_reason"
            ),
            "source_inference": metadata.get("source_inference", "default"),
        },
    }
    if duplicate is not None:
        summary["duplicate"] = {
            "import_id": duplicate.get("import_id"),
            "imported_at": duplicate.get("imported_at"),
            "entry_count": duplicate.get("entry_count"),
            "state": duplicate.get("state"),
        }
    if in_progress:
        summary["in_progress"] = True
    return summary


def _load_import_metadata_or_none(journal_root: Path, timestamp: str) -> dict | None:
    try:
        metadata = read_import_metadata(journal_root, timestamp)
    except (FileNotFoundError, json.JSONDecodeError, OSError):
        return None
    return metadata if isinstance(metadata, dict) else None


def _duplicate_summary_metadata(
    journal_root: Path,
    *,
    client_item_id: str,
    source: str,
    source_inference: str,
    duplicate: dict,
    existing_metadata: dict | None = None,
) -> dict:
    import_id = str(duplicate.get("import_id") or "")
    metadata = dict(existing_metadata or {})
    if not metadata and import_id:
        metadata = dict(_load_import_metadata_or_none(journal_root, import_id) or {})

    metadata.setdefault("file_path", str(journal_root / "imports" / import_id))
    metadata.setdefault("user_timestamp", import_id)
    metadata.setdefault("original_filename", None)
    metadata.setdefault("mime_type", None)
    metadata.setdefault("imported_via", duplicate.get("imported_via"))
    metadata.setdefault("observer_handle", duplicate.get("observer_handle"))
    metadata.setdefault("source_hint", None)
    metadata.setdefault("client", {})
    metadata.setdefault("facet", None)
    metadata.setdefault("setting", None)
    metadata.setdefault("timestamp_detection_method", "duplicate")
    metadata.setdefault("timestamp_detection_model_called", False)
    metadata.setdefault("timestamp_detection_no_match_reason", None)
    metadata["client_item_id"] = client_item_id
    metadata["source_inference"] = metadata.get("source_inference") or source_inference
    if metadata.get("source") not in CANONICAL_IMPORT_SOURCES:
        metadata["source"] = source
    return metadata


def _duplicate_or_replay_response(
    journal_root: Path,
    *,
    client_item_id: str,
    source_hash: str,
    source: str,
    source_inference: str,
) -> Any | None:
    existing = find_staged_by_client_item_id(journal_root, client_item_id)
    if existing:
        if existing.get("source_hash") == source_hash:
            resolution = resolve_import_status(existing)
            is_terminal = resolution.status in {"success", "running"}
            return jsonify(
                _build_save_summary(
                    existing,
                    status="staged",
                    replay=True,
                    duplicate=None,
                    recommended_action="do_not_start" if is_terminal else "start",
                    in_progress=resolution.status == "running",
                )
            )
        return error_response(
            IMPORT_CLIENT_ID_CONFLICT,
            detail=(
                "client_item_id already staged for different content; use a new "
                "client_item_id or re-fetch the existing item"
            ),
        )

    imported = find_manifest_by_hash(journal_root, source_hash)
    if imported:
        duplicate = {
            "import_id": imported.get("import_id"),
            "imported_at": imported.get("imported_at"),
            "entry_count": imported.get("entry_count"),
            "state": "imported",
            "imported_via": imported.get("imported_via"),
            "observer_handle": imported.get("observer_handle"),
        }
        metadata = _duplicate_summary_metadata(
            journal_root,
            client_item_id=client_item_id,
            source=source,
            source_inference=source_inference,
            duplicate=duplicate,
        )
        return jsonify(
            _build_save_summary(
                metadata,
                status="duplicate",
                replay=False,
                duplicate=duplicate,
            )
        )

    staged_duplicate = find_staged_by_source_hash(journal_root, source_hash)
    if staged_duplicate:
        resolution = resolve_import_status(staged_duplicate)
        duplicate = {
            "import_id": staged_duplicate.get("timestamp"),
            "imported_at": None,
            "entry_count": None,
            "state": "staged",
        }
        metadata = _duplicate_summary_metadata(
            journal_root,
            client_item_id=client_item_id,
            source=source,
            source_inference=source_inference,
            duplicate=duplicate,
            existing_metadata=staged_duplicate,
        )
        if resolution.status not in {"success", "running"}:
            return jsonify(
                _build_save_summary(
                    metadata,
                    status="staged",
                    replay=False,
                    duplicate=None,
                    recommended_action="start",
                )
            )
        return jsonify(
            _build_save_summary(
                metadata,
                status="duplicate",
                replay=False,
                duplicate=duplicate,
                in_progress=resolution.status == "running",
            )
        )

    return None


@import_bp.route("/api/save", methods=["POST"])
def import_save() -> Any:
    upload = request.files.get("file")
    text = request.form.get("text", "").strip()
    client_item_id = request.form.get("client_item_id", "").strip()
    facet = request.form.get("facet", "").strip() or None
    setting = request.form.get("setting", "").strip() or None
    source_hint = request.form.get("source_hint", "").strip() or None
    imported_via = request.form.get("imported_via", "").strip() or "web_dashboard"
    observer_handle = request.form.get("observer_handle", "").strip() or None
    deterministic_only = _form_bool(request.form.get("deterministic_only"))
    client = _client_bag(request.form.get("client"))

    if not client_item_id:
        return error_response(MISSING_REQUIRED_FIELD, detail="Missing client_item_id")

    # Generate timestamp for folder name
    timestamp_ms = now_ms()

    # Determine filename
    if upload and upload.filename:
        filename = secure_filename(upload.filename)
    elif text:
        filename = "paste.txt"
    else:
        return error_response(INGEST_NO_FILES, detail="No input")

    original_filename = upload.filename if upload else "paste.txt"
    mime_type = upload.content_type if upload else "text/plain"
    source = canonical_source(filename=original_filename, content_type=mime_type)
    source_inference = canonical_source_signal(
        filename=original_filename,
        content_type=mime_type,
    )
    journal_root = Path(state.journal_root)

    # Detect timestamp from content first (need temporary save for detection)
    ts = None
    detection_result = None
    timestamp_detection_method = "upload_fallback"
    timestamp_detection_model_called = False
    timestamp_detection_no_match_reason = None

    # Create temporary file for detection if needed
    if upload:
        import tempfile

        # Preserve original filename structure in temp file name for timestamp detection
        # Use prefix to include original filename (minus extension)
        original_stem = Path(filename).stem
        suffix = Path(filename).suffix
        with tempfile.NamedTemporaryFile(
            delete=False, prefix=f"{original_stem}_", suffix=suffix
        ) as tmp:
            upload.save(tmp.name)
            temp_path = tmp.name
            upload.seek(0)  # Reset file pointer for later save
    else:
        import tempfile

        with tempfile.NamedTemporaryFile(mode="w", delete=False, suffix=".txt") as tmp:
            tmp.write(text)
            temp_path = tmp.name

    try:
        temp_source = Path(temp_path)
        source_hash = hash_source(temp_source)
        duplicate_or_replay = _duplicate_or_replay_response(
            journal_root,
            client_item_id=client_item_id,
            source_hash=source_hash,
            source=source,
            source_inference=source_inference,
        )
        if duplicate_or_replay is not None:
            return duplicate_or_replay

        try:
            original_name = upload.filename if upload else None
            detection_result = resolve_created_deterministic(
                temp_path,
                original_filename=original_name,
            )
            if (
                detection_result
                and detection_result.get("day")
                and detection_result.get("time")
            ):
                ts = f"{detection_result['day']}_{detection_result['time']}"
                timestamp_detection_method = "deterministic"
        except Exception:
            detection_result = None

        if not ts:
            if deterministic_only:
                timestamp_detection_no_match_reason = "no_deterministic_match"
            else:
                try:
                    # Pass original filename for better timestamp detection
                    original_name = upload.filename if upload else None
                    detection_result = detect_created(
                        temp_path,
                        original_filename=original_name,
                    )
                    timestamp_detection_model_called = True
                    if (
                        detection_result
                        and detection_result.get("day")
                        and detection_result.get("time")
                    ):
                        ts = f"{detection_result['day']}_{detection_result['time']}"
                        timestamp_detection_method = "model"
                    else:
                        timestamp_detection_no_match_reason = "model_no_match"
                except Exception:
                    detection_result = None
                    timestamp_detection_model_called = True
                    timestamp_detection_no_match_reason = "model_no_match"

        # Use detected timestamp or fall back to upload timestamp
        folder_timestamp = (
            ts
            if ts
            else datetime.fromtimestamp(timestamp_ms / 1000).strftime("%Y%m%d_%H%M%S")
        )

        # Save the actual file using utility function
        if upload:
            file_path = save_import_file(
                journal_root=journal_root,
                timestamp=folder_timestamp,
                source_path=temp_source,
                filename=filename,
            )
        else:
            file_path = save_import_text(
                journal_root=journal_root,
                timestamp=folder_timestamp,
                content=text,
                filename=filename,
            )

        # Build metadata dict
        metadata = {
            "original_filename": original_filename,
            "upload_timestamp": timestamp_ms,
            "upload_datetime": datetime.fromtimestamp(timestamp_ms / 1000).isoformat(),
            "detection_result": detection_result,
            "detected_timestamp": ts,
            "user_timestamp": folder_timestamp,
            "timestamp_detection_method": timestamp_detection_method,
            "timestamp_detection_model_called": timestamp_detection_model_called,
            "timestamp_detection_no_match_reason": timestamp_detection_no_match_reason,
            "source_inference": source_inference,
            "file_size": file_path.stat().st_size if file_path.exists() else 0,
            "mime_type": mime_type,
            "facet": facet,
            "setting": setting,
            "file_path": str(file_path),
            "imported_via": imported_via,
            "link_id": _link_id_from_identity(),
            "observer_handle": observer_handle,
            "client_item_id": client_item_id,
            "source_hash": source_hash,
            "source": source,
            "source_hint": source_hint,
            "client": client,
        }

        # Write metadata using utility function
        write_import_metadata(
            journal_root=journal_root,
            timestamp=folder_timestamp,
            metadata=metadata,
        )

        return jsonify(
            _build_save_summary(
                metadata,
                status="staged",
                replay=False,
                duplicate=None,
            )
        )
    finally:
        # Clean up temporary file
        Path(temp_path).unlink(missing_ok=True)


@import_bp.route("/api/save-path", methods=["POST"])
def import_save_path() -> Any:
    """Register a local filesystem path for import (e.g. Obsidian vault)."""
    data = request.get_json(force=True)
    client_item_id = data.get("client_item_id", "").strip()
    local_path = data.get("path", "").strip()
    facet = data.get("facet", "").strip() or None
    setting = data.get("setting", "").strip() or None
    source_hint = data.get("source_hint", "").strip() or None
    imported_via = data.get("imported_via", "").strip() or "web_dashboard"
    observer_handle = data.get("observer_handle", "").strip() or None
    client = _client_bag(data.get("client"))

    if not client_item_id:
        return error_response(MISSING_REQUIRED_FIELD, detail="Missing client_item_id")

    if not local_path:
        return error_response(MISSING_REQUIRED_FIELD, detail="Missing path")

    local = Path(local_path)
    if not local.exists():
        return error_response(FILE_NOT_FOUND, detail=f"Path not found: {local_path}")

    timestamp_ms = now_ms()
    folder_timestamp = (
        f"{datetime.fromtimestamp(timestamp_ms / 1000).strftime('%Y%m%d_%H%M%S')}"
    )

    journal_root = Path(state.journal_root)
    source_hash = hash_source(local)
    source = canonical_source(filename=local.name)
    source_inference = canonical_source_signal(filename=local.name)
    duplicate_or_replay = _duplicate_or_replay_response(
        journal_root,
        client_item_id=client_item_id,
        source_hash=source_hash,
        source=source,
        source_inference=source_inference,
    )
    if duplicate_or_replay is not None:
        return duplicate_or_replay

    metadata = {
        "original_filename": local.name,
        "upload_timestamp": timestamp_ms,
        "upload_datetime": datetime.fromtimestamp(timestamp_ms / 1000).isoformat(),
        "user_timestamp": folder_timestamp,
        "timestamp_detection_method": "path_fallback",
        "timestamp_detection_model_called": False,
        "timestamp_detection_no_match_reason": None,
        "source_inference": source_inference,
        "file_path": local_path,
        "facet": facet,
        "setting": setting,
        "is_local_path": True,
        "mime_type": None,
        "imported_via": imported_via,
        "link_id": _link_id_from_identity(),
        "observer_handle": observer_handle,
        "client_item_id": client_item_id,
        "source_hash": source_hash,
        "source": source,
        "source_hint": source_hint,
        "client": client,
    }

    write_import_metadata(
        journal_root=journal_root,
        timestamp=folder_timestamp,
        metadata=metadata,
    )

    return jsonify(
        _build_save_summary(
            metadata,
            status="staged",
            replay=False,
            duplicate=None,
        )
    )


@import_bp.route("/api/meta", methods=["POST"])
def import_update_metadata() -> Any:
    """Update stored metadata for a saved import."""
    data = request.get_json(force=True)
    raw_path = data.get("path", "").strip()
    if not raw_path:
        return error_response(MISSING_REQUIRED_FIELD, detail="Missing import path")

    # Extract timestamp from path
    # Path format: .../imports/{timestamp}/{filename}
    file_path = Path(raw_path)
    timestamp = file_path.parent.name
    journal_root = Path(state.journal_root)

    try:
        metadata = read_import_metadata(journal_root=journal_root, timestamp=timestamp)
    except FileNotFoundError:
        return error_response(IMPORT_NOT_FOUND, detail="Import metadata not found")
    except Exception as exc:
        return error_response(
            IMPORT_METADATA_FAILED,
            detail=f"Failed to read metadata: {exc}",
        )

    status_info = read_import_status_info(journal_root, timestamp, metadata)
    resolution = resolve_import_status(status_info)
    if resolution.status in {"success", "running"}:
        return error_response(
            INVALID_OPERATION_FOR_STATE,
            detail="import already started or processed",
        )

    source_hash = metadata.get("source_hash")
    if source_hash and find_manifest_by_hash(journal_root, source_hash):
        return error_response(
            INVALID_OPERATION_FOR_STATE,
            detail="content already imported",
        )

    updates: dict[str, Any] = {}
    for key in (
        "facet",
        "setting",
        "original_filename",
        "mime_type",
        "source_hint",
        "observer_handle",
        "imported_via",
        "client",
    ):
        if key not in data:
            continue
        if key in {"facet", "setting", "source_hint", "observer_handle"}:
            updates[key] = _clean_optional(data.get(key))
        elif key == "client":
            updates[key] = _client_bag(data.get(key))
        else:
            updates[key] = data.get(key)

    changed = {
        key: value
        for key, value in updates.items()
        if key not in metadata or metadata.get(key) != value
    }

    try:
        update_import_metadata_fields(
            journal_root=journal_root,
            timestamp=timestamp,
            updates=updates,
        )
    except FileNotFoundError:
        return error_response(IMPORT_NOT_FOUND, detail="Import metadata not found")
    except Exception as exc:
        return error_response(
            IMPORT_METADATA_FAILED,
            detail=f"Failed to update metadata: {exc}",
        )

    return jsonify(
        {
            "status": "ok",
            "path": raw_path,
            "timestamp": timestamp,
            "updated": changed,
        }
    )


@import_bp.route("/api/list")
def import_list() -> Any:
    """Get list of all imports with their metadata."""
    source_filter = request.args.get("source", "").strip() or None
    # Get all import timestamps using utility function
    timestamps = list_import_timestamps(journal_root=Path(state.journal_root))

    # Build info for each import using utility function
    imports = []
    for timestamp in timestamps:
        import_data = build_import_info(
            journal_root=Path(state.journal_root),
            timestamp=timestamp,
        )
        display_name = _source_display_name(import_data.get("source_type"))
        if display_name:
            import_data["source_display"] = display_name

        resolution = resolve_import_status(import_data)
        import_data["status"] = resolution.status
        import_data["error"] = resolution.error
        import_data["error_stage"] = resolution.error_stage

        if source_filter is None or import_data.get("source_type") == source_filter:
            imports.append(import_data)

    # Sort by imported_at (newest first)
    imports.sort(key=lambda x: x.get("imported_at", 0), reverse=True)

    try:
        page = max(1, int(request.args.get("page", 1)))
    except ValueError:
        page = 1
    try:
        per_page = min(100, max(1, int(request.args.get("per_page", 25))))
    except ValueError:
        per_page = 25

    total = len(imports)
    total_entries_written = sum(imp.get("entries_written") or 0 for imp in imports)
    total_entities_seeded = sum(imp.get("entities_seeded") or 0 for imp in imports)

    start = (page - 1) * per_page
    page_imports = imports[start : start + per_page]

    return jsonify(
        {
            "imports": page_imports,
            "total": total,
            "page": page,
            "per_page": per_page,
            "pages": (total + per_page - 1) // per_page if total > 0 else 0,
            "total_entries_written": total_entries_written,
            "total_entities_seeded": total_entities_seeded,
        }
    )


@import_bp.route("/api/sources")
def import_sources() -> Any:
    """Return available import source metadata."""
    return respond_collection(
        [_source_metadata_payload(source) for source in SOURCE_METADATA]
    )


@import_bp.route("/api/guide/<source>")
def import_guide(source: str) -> Any:
    """Return export guide markdown for a given source."""
    if not re.fullmatch(r"[a-z_]+", source):
        return error_response(INVALID_REQUEST_VALUE, detail="Invalid source name")
    guide_path = Path(__file__).parent / "guides" / f"{source}.md"
    if not guide_path.is_file():
        return error_response(
            FILE_NOT_FOUND, detail=f"No guide available for '{source}'"
        )
    return (
        guide_path.read_text(encoding="utf-8"),
        200,
        {"Content-Type": "text/markdown; charset=utf-8"},
    )


@import_bp.route("/<timestamp>")
def import_detail(timestamp: str) -> Any:
    """Serve the import SPA shell for a specific timestamp."""
    return current_app.send_static_file("shell.html")


@import_bp.route("/api/<timestamp>")
def import_detail_api(timestamp: str) -> Any:
    """Get detailed data for a specific import."""
    try:
        # Use utility function to get all details
        result = get_import_details(
            journal_root=Path(state.journal_root),
            timestamp=timestamp,
        )
        # Resolve status the same way the history list does, so an in-progress
        # import (task_id present, no imported.json yet) reads as "running"
        # rather than falling through to "failed". Both filesystem reads stay
        # inside the guard: an import removed between them is not found, not a 500.
        import_data = build_import_info(
            journal_root=Path(state.journal_root),
            timestamp=timestamp,
        )
    except FileNotFoundError:
        return error_response(IMPORT_NOT_FOUND, detail="Import not found")

    resolution = resolve_import_status(import_data)
    result["status"] = resolution.status
    result["error"] = resolution.error
    result["error_stage"] = resolution.error_stage

    return jsonify(result)


@import_bp.route("/api/<timestamp>/content")
def import_content_list(timestamp: str) -> Any:
    """Get paginated content items for an import."""
    journal_root = Path(state.journal_root)
    import_dir = journal_root / "imports" / timestamp
    if not import_dir.exists():
        return error_response(IMPORT_NOT_FOUND, detail="Import not found")

    manifest_path = import_dir / "content_manifest.jsonl"
    if (
        not manifest_path.exists()
        and generate_content_manifest(journal_root, timestamp) is None
    ):
        return error_response(IMPORT_NOT_FOUND, detail="No content available")

    items: list[dict] = []
    try:
        with open(manifest_path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    items.append(json.loads(line))
                except json.JSONDecodeError:
                    continue
    except OSError:
        return error_response(IMPORT_METADATA_FAILED, detail="Failed to read manifest")

    source_type = ""
    persisted_source_display = ""
    imported_path = import_dir / "imported.json"
    if imported_path.exists():
        try:
            imported = json.loads(imported_path.read_text(encoding="utf-8"))
            source_type = imported.get("source_type", "")
            persisted_source_display = imported.get("source_display", "")
        except (OSError, json.JSONDecodeError):
            pass

    source_meta = next((s for s in SOURCE_METADATA if s["name"] == source_type), None)
    source_display = (
        source_meta["display_name"]
        if source_meta
        else (persisted_source_display or source_type)
    )

    month_counts: dict[str, int] = {}
    for item in items:
        date = item.get("date", "")
        if len(date) >= 6:
            month = date[:6]
            month_counts[month] = month_counts.get(month, 0) + 1

    q = request.args.get("q", "").strip().lower()
    month = request.args.get("month", "").strip()

    filtered = items
    if month:
        filtered = [item for item in filtered if item.get("date", "").startswith(month)]
    if q:
        filtered = [
            item
            for item in filtered
            if q in item.get("title", "").lower()
            or q in item.get("preview", "").lower()
        ]

    try:
        page = max(1, int(request.args.get("page", 1)))
    except ValueError:
        page = 1
    try:
        per_page = min(100, max(1, int(request.args.get("per_page", 50))))
    except ValueError:
        per_page = 50
    total = len(filtered)
    start = (page - 1) * per_page
    page_items = filtered[start : start + per_page]

    return jsonify(
        {
            "items": page_items,
            "total": total,
            "page": page,
            "per_page": per_page,
            "pages": (total + per_page - 1) // per_page if total > 0 else 0,
            "months": dict(sorted(month_counts.items())),
            "source_type": source_type,
            "source_display": source_display,
            "source_icon_svg": lucide_svg(source_meta["icon"]) if source_meta else None,
        }
    )


@import_bp.route("/api/<timestamp>/content/<item_id>")
def import_content_detail(timestamp: str, item_id: str) -> Any:
    """Get full content for a specific imported item."""
    journal_root = Path(state.journal_root)
    import_dir = journal_root / "imports" / timestamp
    if not import_dir.exists():
        return error_response(IMPORT_NOT_FOUND, detail="Import not found")

    manifest_path = import_dir / "content_manifest.jsonl"
    if not manifest_path.exists():
        generate_content_manifest(journal_root, timestamp)
    if not manifest_path.exists():
        return error_response(IMPORT_NOT_FOUND, detail="No content available")

    item = None
    try:
        with open(manifest_path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    entry = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if entry.get("id") == item_id:
                    item = entry
                    break
    except OSError:
        return error_response(IMPORT_METADATA_FAILED, detail="Failed to read manifest")

    if item is None:
        return error_response(IMPORT_NOT_FOUND, detail="Item not found")

    source_type = ""
    imported_path = import_dir / "imported.json"
    if imported_path.exists():
        try:
            imported = json.loads(imported_path.read_text(encoding="utf-8"))
            source_type = imported.get("source_type", "")
        except (OSError, json.JSONDecodeError):
            pass

    content_parts: list[dict] = []
    for seg in item.get("segments", []):
        day = seg.get("day", "")
        key = seg.get("key", "")
        if not day or not key:
            continue
        seg_dir = day_path(day, create=False) / f"import.{source_type}" / key
        if not seg_dir.exists():
            continue

        jsonl_path = seg_dir / "conversation_transcript.jsonl"
        if jsonl_path.exists():
            try:
                lines = jsonl_path.read_text(encoding="utf-8").strip().split("\n")
            except OSError:
                continue
            for line in lines[1:]:
                try:
                    content_parts.append(json.loads(line))
                except json.JSONDecodeError:
                    continue
            continue

        for md_file in seg_dir.glob("*_transcript.md"):
            try:
                md_content = md_file.read_text(encoding="utf-8")
            except OSError:
                continue
            title = item.get("title", "")
            if title:
                sections = re.split(r"(?m)^## ", md_content)
                for section in sections:
                    stripped = section.strip()
                    if stripped.startswith(title):
                        content_parts.append(
                            {"type": "markdown", "content": "## " + stripped}
                        )
                        break
                else:
                    content_parts.append(
                        {"type": "markdown", "content": md_content.strip()}
                    )
            else:
                content_parts.append(
                    {"type": "markdown", "content": md_content.strip()}
                )

    return jsonify({"item": item, "content": content_parts})


@import_bp.route("/api/start", methods=["POST"])
def import_start() -> Any:
    data = request.get_json(force=True)
    path = data.get("path")
    ts = data.get("timestamp")
    force = data.get("force", False)
    if not path or not ts:
        return error_response(MISSING_REQUIRED_FIELD, detail="missing params")

    # Extract original timestamp from path and handle timestamp changes
    file_path = Path(path)
    journal_root = Path(state.journal_root)
    imports_dir = journal_root / "imports"
    is_local_path = not str(file_path).startswith(str(imports_dir))
    original_timestamp = file_path.parent.name if not is_local_path else ts

    # Read import metadata before any move. Saved metadata is the authority for
    # facet, setting, and source routing.
    try:
        metadata = read_import_metadata(
            journal_root=journal_root,
            timestamp=original_timestamp,
        )
    except FileNotFoundError:
        return error_response(
            IMPORT_NOT_FOUND,
            detail=f"Import metadata not found for {original_timestamp}",
        )
    except Exception as e:
        return error_response(
            IMPORT_METADATA_FAILED,
            detail=f"Failed to read metadata: {str(e)}",
        )

    source_hash = metadata.get("source_hash")
    if source_hash and find_manifest_by_hash(journal_root, source_hash):
        return error_response(
            INVALID_OPERATION_FOR_STATE,
            detail="content already imported; will not start",
        )

    # Generate task ID
    task_id = str(now_ms())

    # If timestamp changed, move the import directory through the imports/ owner
    if not is_local_path and original_timestamp != ts:
        try:
            new_import_dir = move_import(
                journal_root=journal_root,
                old_timestamp=original_timestamp,
                new_timestamp=ts,
            )
        except FileNotFoundError:
            return error_response(
                IMPORT_NOT_FOUND,
                detail=f"Import directory not found for {original_timestamp}",
            )
        except FileExistsError:
            return error_response(
                IMPORT_CONFLICT,
                detail=f"Import already exists for timestamp {ts}",
            )
        except Exception as e:
            return error_response(
                IMPORT_METADATA_FAILED,
                detail=f"Failed to rename import directory: {str(e)}",
            )

        # Update path to point to new location
        path = str(new_import_dir / file_path.name)

        # Update file_path in metadata (need to update after reading)
        # We'll handle this after reading the metadata below

    # Update file_path in metadata if timestamp changed
    if not is_local_path and original_timestamp != ts:
        try:
            update_import_metadata_fields(
                journal_root=journal_root,
                timestamp=ts,
                updates={"file_path": path},
            )
            metadata["file_path"] = path
        except Exception as e:
            return error_response(
                IMPORT_METADATA_FAILED,
                detail=f"Failed to update file path in metadata: {str(e)}",
            )

    facet = metadata.get("facet")
    setting = metadata.get("setting")
    source_hint = _clean_optional(metadata.get("source_hint"))

    # Build command
    cmd = ["journal", "importer", path, ts]
    if facet:
        cmd.extend(["--facet", facet])
    if setting:
        cmd.extend(["--setting", setting])
    if source_hint:
        cmd.extend(["--source", source_hint])
    if force:
        cmd.append("--force")

    # A successful send proves this request reached the bus socket. This closes
    # a silent-drop class; the measured field defect reached supervisor and is
    # fixed by queue_if_active_cmd_differs plus terminal-status gates.
    ok = callosum_send(
        "supervisor",
        "request",
        ref=task_id,
        cmd=cmd,
        queue_if_active_cmd_differs=True,
    )
    if not ok:
        return error_response(
            IMPORT_QUEUE_UNREACHABLE,
            detail=(
                "your journal's background service isn't running. "
                "start it, then try again."
            ),
        )

    # Store task_id and source_hint after the accepted send.
    try:
        update_import_metadata_fields(
            journal_root=journal_root,
            timestamp=ts,
            updates={"task_id": task_id, "source_hint": source_hint},
        )
    except Exception as e:
        return error_response(
            IMPORT_METADATA_FAILED,
            detail=(
                f"Supervisor accepted task {task_id}, but metadata could not be "
                f"updated: {str(e)}"
            ),
            extra={"task_id": task_id},
        )

    return jsonify({"status": "ok", "task_id": task_id})


@import_bp.route("/api/journal-sources/create", methods=["POST"])
def api_journal_source_create() -> Any:
    data = request.get_json(force=True) if request.is_json else {}
    name = data.get("name", "").strip()
    if not name:
        return error_response(MISSING_REQUIRED_FIELD, detail="Name is required")
    if not is_valid_journal_source_name(name):
        return error_response(
            JOURNAL_SOURCE_PROBLEM,
            detail="Invalid journal source name",
        )
    if find_journal_source_by_name(name):
        return error_response(
            JOURNAL_SOURCE_PROBLEM,
            status=409,
            detail=f"Journal source '{name}' already exists",
        )
    key = generate_key()
    source_data = {
        "key": key,
        "name": name,
        "created_at": now_ms(),
        "enabled": True,
        "revoked": False,
        "revoked_at": None,
        "stats": {
            "segments_received": 0,
            "entities_received": 0,
            "facets_received": 0,
            "imports_received": 0,
            "config_received": 0,
        },
    }
    if not save_journal_source(source_data):
        return error_response(
            JOURNAL_SOURCE_PROBLEM,
            status=500,
            detail="Failed to save journal source",
        )
    create_state_directory(Path(state.journal_root), key[:8])
    log_app_action(
        app="import",
        facet=None,
        action="journal_source_create",
        params={"name": name, "key_prefix": key[:8]},
    )
    return jsonify({"key": key, "key_prefix": key[:8], "name": name})


@import_bp.route("/api/journal-sources/list")
def api_journal_source_list() -> Any:
    sources = list_journal_sources()
    result = []
    for s in sources:
        if s.get("pair_mode") == "pl":
            continue
        result.append(
            {
                "name": s.get("name", ""),
                "prefix": journal_source_state_prefix(s),
                "status": "revoked" if s.get("revoked") else "active",
                "created_at": s.get("created_at"),
            }
        )
    return respond_collection(result)


@import_bp.route("/api/journal-sources/<name>/revoke", methods=["POST"])
def api_journal_source_revoke(name: str) -> Any:
    source = find_journal_source_by_name(name)
    if not source:
        return error_response(
            JOURNAL_SOURCE_PROBLEM,
            status=404,
            detail=f"Journal source '{name}' not found",
        )
    if source.get("revoked"):
        return error_response(
            JOURNAL_SOURCE_PROBLEM,
            status=409,
            detail=f"Journal source '{name}' is already revoked",
        )
    source["revoked"] = True
    source["revoked_at"] = now_ms()
    if not save_journal_source(source):
        return error_response(
            JOURNAL_SOURCE_PROBLEM,
            status=500,
            detail="Failed to save journal source",
        )
    log_app_action(
        app="import",
        facet=None,
        action="journal_source_revoke",
        params={"name": name, "key_prefix": journal_source_state_prefix(source)},
    )
    return jsonify(
        {"name": name, "prefix": journal_source_state_prefix(source), "revoked": True}
    )


@import_bp.route("/api/journal-sources/<name>/status")
def api_journal_source_status(name: str) -> Any:
    source = find_journal_source_by_name(name)
    if not source:
        return error_response(
            JOURNAL_SOURCE_PROBLEM,
            status=404,
            detail=f"Journal source '{name}' not found",
        )
    return jsonify(
        {
            "name": source.get("name", ""),
            "prefix": journal_source_state_prefix(source),
            "status": "revoked" if source.get("revoked") else "active",
            "created_at": source.get("created_at"),
            "revoked": source.get("revoked", False),
            "revoked_at": source.get("revoked_at"),
            "stats": source.get("stats", {}),
        }
    )


@import_bp.route("/api/journal-sources/<name>/staged")
def api_journal_source_staged(name: str) -> Any:
    source = find_journal_source_by_name(name)
    if not source:
        return error_response(
            JOURNAL_SOURCE_PROBLEM,
            status=404,
            detail=f"Journal source '{name}' not found",
        )
    area = request.args.get("area")
    if area is not None and area not in {"entities", "facets", "config"}:
        return error_response(
            INVALID_REQUEST_VALUE,
            status=400,
            detail="Area must be one of: entities, facets, config",
        )
    # Mirrors api_journal_source_status: a registry-returned record always has a
    # valid prefix, so call journal_source_state_prefix directly (no try/except).
    state_dir = get_state_directory(journal_source_state_prefix(source))
    items: list[dict[str, Any]] = []

    if area in {None, "entities"}:
        staged_dir = state_dir / "entities" / "staged"
        for staged_path in sorted(staged_dir.glob("*.json")):
            payload = load_json(staged_path)
            if not isinstance(payload, dict):
                continue
            items.append(
                {
                    "area": "entities",
                    "source_id": staged_path.stem,
                    "reason": payload.get("reason"),
                    "source_entity": payload.get("source_entity"),
                    "match_candidates": payload.get("match_candidates"),
                    "staged_at": payload.get("staged_at"),
                }
            )

    if area in {None, "facets"}:
        staged_dir = state_dir / "facets" / "staged"
        for staged_path in sorted(staged_dir.glob("**/*.staged.json")):
            payload = load_json(staged_path)
            if not isinstance(payload, dict):
                continue
            relative_path = staged_path.relative_to(staged_dir)
            parts = relative_path.parts
            if len(parts) < 3:
                continue
            line = {
                "area": "facets",
                "staged_file": relative_path.as_posix(),
                "facet": parts[0],
                "file_type": parts[1],
            }
            line.update(payload)
            items.append(line)

    if area in {None, "config"}:
        diff = load_json(state_dir / "config" / "diff.json")
        # Config-parity decision: include the config item iff diff.json exists
        # AND loads as a dict. A missing / unreadable / non-dict diff is omitted
        # (still HTTP 200) — never a 500, never an empty-diff placeholder.
        if isinstance(diff, dict):
            items.append({"area": "config", "diff": diff})

    return respond_collection(items)


@import_bp.route("/api/journal-sources/<name>/resolve-entity", methods=["POST"])
def api_journal_source_resolve_entity(name: str) -> Any:
    source = find_journal_source_by_name(name)
    if not source:
        return error_response(
            JOURNAL_SOURCE_PROBLEM,
            status=404,
            detail=f"Journal source '{name}' not found",
        )
    state_dir = get_state_directory(journal_source_state_prefix(source))
    data = request.get_json(force=True)

    try:
        result = resolve_entity(
            state_dir,
            data["source_id"],
            data["action"],
            data.get("target"),
        )
    except ResolveNotFound as exc:
        return error_response(IMPORT_NOT_FOUND, detail=str(exc))
    except ResolveInvalid as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    return jsonify(result)


@import_bp.route("/api/journal-sources/<name>/resolve-facet", methods=["POST"])
def api_journal_source_resolve_facet(name: str) -> Any:
    source = find_journal_source_by_name(name)
    if not source:
        return error_response(
            JOURNAL_SOURCE_PROBLEM,
            status=404,
            detail=f"Journal source '{name}' not found",
        )
    state_dir = get_state_directory(journal_source_state_prefix(source))
    data = request.get_json(force=True)

    try:
        resolve_staged_facet(state_dir, data["staged_file"], data["mode"])
    except ResolveNotFound as exc:
        return error_response(IMPORT_NOT_FOUND, detail=str(exc))
    except (ResolveInvalid, ValueError) as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    return success_response()


@import_bp.route("/api/journal-sources/<name>/resolve-config", methods=["POST"])
def api_journal_source_resolve_config(name: str) -> Any:
    source = find_journal_source_by_name(name)
    if not source:
        return error_response(
            JOURNAL_SOURCE_PROBLEM,
            status=404,
            detail=f"Journal source '{name}' not found",
        )
    state_dir = get_state_directory(journal_source_state_prefix(source))
    data = request.get_json(force=True)

    try:
        resolve_config(state_dir, data["field"], data["action"])
    except ResolveNotFound as exc:
        return error_response(IMPORT_NOT_FOUND, detail=str(exc))
    except ResolveInvalid as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    return success_response()


@import_bp.route("/api/journal-sources/<name>/resolve-config-all", methods=["POST"])
def api_journal_source_resolve_config_all(name: str) -> Any:
    source = find_journal_source_by_name(name)
    if not source:
        return error_response(
            JOURNAL_SOURCE_PROBLEM,
            status=404,
            detail=f"Journal source '{name}' not found",
        )
    state_dir = get_state_directory(journal_source_state_prefix(source))
    data = request.get_json(force=True)

    try:
        count = resolve_config_all(state_dir, data["category"])
    except ResolveNotFound as exc:
        return error_response(IMPORT_NOT_FOUND, detail=str(exc))
    except ResolveInvalid as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    return jsonify({"count": count})


@import_bp.route("/journal/<key_prefix>/manifest/<area>")
@require_journal_source
def journal_source_manifest(key_prefix: str, area: str) -> Any:
    if area not in STATE_AREAS:
        # PROTOCOL-ONLY: journal-source manifest area from non-owner clients.
        return error_response(
            INVALID_REQUEST_VALUE, status=404, detail="Unknown manifest area"
        )
    state_path = get_state_directory(g.derived_prefix) / area / "state.json"
    try:
        data = json.loads(state_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        data = {}
    return jsonify(data)


# Segment ingest routes (separate module to keep routes.py manageable)
from .ingest import register_ingest_routes

register_ingest_routes(import_bp)
