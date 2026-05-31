# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Ingest endpoints for journal source imports."""

from __future__ import annotations

import hashlib
import json
import logging
import os
import re
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from flask import abort, g, jsonify, request
from werkzeug.utils import secure_filename

from solstone.convey import emit, state
from solstone.convey.reasons import (
    INGEST_NO_FILES,
    INVALID_JSON_REQUEST,
    INVALID_REQUEST_VALUE,
    MISSING_REQUIRED_FIELD,
)
from solstone.convey.utils import error_response
from solstone.observe.utils import (
    compute_bytes_sha256,
    compute_file_sha256,
    find_available_segment,
)
from solstone.think.entities.core import EntityDict, entity_slug
from solstone.think.entities.journal import (
    has_journal_principal,
    load_all_journal_entities,
    save_journal_entity,
)
from solstone.think.entities.matching import find_matching_entity
from solstone.think.utils import DEFAULT_STREAM, STREAM_RE, day_path

from .journal_sources import (
    get_state_directory,
    journal_source_state_prefix,
    require_journal_source,
    save_journal_source,
)

logger = logging.getLogger(__name__)

_DAY_RE = re.compile(r"^\d{8}$")
_SEGMENT_RE = re.compile(r"^\d{6}_\d+$")
_FACET_NAME_RE = re.compile(r"^[a-z0-9][a-z0-9_-]*$")
_IMPORT_ID_RE = re.compile(r"^\d{8}_\d{6}$")

_NEVER_TRANSFER_PATHS = frozenset({"convey.password_hash"})
_IDENTITY_PATHS = frozenset(
    {
        "identity.name",
        "identity.preferred",
        "identity.bio",
        "identity.pronouns",
        "identity.aliases",
        "identity.email_addresses",
        "identity.timezone",
    }
)


def _append_decision(log_path: Path, entry: dict) -> None:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with open(log_path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(entry, ensure_ascii=False) + "\n")


def _write_state_atomic(state_path: Path, state_data: dict) -> None:
    state_path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_path = tempfile.mkstemp(dir=state_path.parent, suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(state_data, handle, indent=2)
        Path(tmp_path).rename(state_path)
    except Exception:
        Path(tmp_path).unlink(missing_ok=True)
        raise


def _flatten_config(cfg: dict, prefix: str = "") -> dict[str, Any]:
    """Flatten a nested config dict to dot-separated paths."""
    result: dict[str, Any] = {}
    for key, value in cfg.items():
        path = f"{prefix}{key}" if prefix else key
        if isinstance(value, dict):
            result.update(_flatten_config(value, f"{path}."))
        else:
            result[path] = value
    return result


def _is_never_transfer(path: str) -> bool:
    return path in _NEVER_TRANSFER_PATHS


def _categorize_field(path: str) -> str:
    """Return category for a config field path."""
    if path in _IDENTITY_PATHS:
        return "transferable"
    return "preference"


from .facet_ingest import process_facet


def register_ingest_routes(bp) -> None:
    @bp.route("/journal/<key_prefix>/ingest/segments", methods=["POST"])
    @require_journal_source
    def ingest_segments(key_prefix: str):
        if journal_source_state_prefix(g.journal_source) != key_prefix:
            abort(403, description="Key prefix mismatch")

        metadata_raw = request.form.get("metadata")
        if not metadata_raw:
            return error_response(MISSING_REQUIRED_FIELD, detail="Missing metadata")

        try:
            metadata = json.loads(metadata_raw)
        except json.JSONDecodeError:
            return error_response(
                INVALID_JSON_REQUEST,
                detail="Invalid metadata JSON",
            )

        if not isinstance(metadata, dict):
            return error_response(
                INVALID_JSON_REQUEST,
                detail="Invalid metadata JSON",
            )

        segments = metadata.get("segments")
        if not isinstance(segments, list):
            return error_response(
                MISSING_REQUIRED_FIELD, detail="Missing segments array"
            )

        log_path = get_state_directory(key_prefix) / "segments" / "log.jsonl"
        pair_mode = g.journal_source.get("pair_mode")
        sender_fingerprint = (
            g.journal_source["fingerprint"] if pair_mode == "pl" else None
        )
        sender_instance_id = (
            g.journal_source.get("peer_instance_id") if pair_mode == "pl" else None
        )

        copied = 0
        skipped = 0
        deconflicted = 0
        errors: list[dict[str, str]] = []
        new_state = {}

        for idx, segment in enumerate(segments):
            day = ""
            segment_key = ""
            try:
                if not isinstance(segment, dict):
                    raise ValueError("Segment metadata must be an object")

                day = str(segment.get("day", "")).strip()
                stream = str(segment.get("stream", "")).strip()
                segment_key = str(segment.get("segment_key", "")).strip()
                files = segment.get("files")

                if not _DAY_RE.match(day):
                    raise ValueError("Invalid day format")
                if stream != DEFAULT_STREAM and not STREAM_RE.fullmatch(stream):
                    raise ValueError("Invalid stream format")
                if not _SEGMENT_RE.match(segment_key):
                    raise ValueError("Invalid segment_key format")
                if not isinstance(files, list) or not files:
                    raise ValueError("Segment must list at least one file")

                expected_names = []
                for raw_name in files:
                    name = secure_filename(str(raw_name))
                    if not name:
                        raise ValueError("Invalid filename in metadata")
                    expected_names.append(name)

                if len(set(expected_names)) != len(expected_names):
                    raise ValueError("Duplicate filenames in metadata")

                uploaded_files = request.files.getlist(f"files_{idx}")
                file_infos: dict[str, dict[str, str | int | bytes]] = {}
                for upload in uploaded_files:
                    if not upload.filename:
                        continue
                    filename = secure_filename(upload.filename)
                    if not filename:
                        continue
                    content = upload.read()
                    if len(content) == 0:
                        continue
                    if filename in file_infos:
                        raise ValueError(f"Duplicate uploaded filename: {filename}")
                    file_infos[filename] = {
                        "name": filename,
                        "content": content,
                        "sha256": compute_bytes_sha256(content),
                        "size": len(content),
                    }

                expected_set = set(expected_names)
                uploaded_set = set(file_infos.keys())
                if expected_set != uploaded_set:
                    missing = sorted(expected_set - uploaded_set)
                    unexpected = sorted(uploaded_set - expected_set)
                    parts = []
                    if missing:
                        parts.append(f"Missing uploaded files: {', '.join(missing)}")
                    if unexpected:
                        parts.append(
                            f"Unexpected uploaded files: {', '.join(unexpected)}"
                        )
                    raise ValueError("; ".join(parts))

                original_segment_key = segment_key
                arc_key = f"{stream}/{segment_key}"
                day_dir = day_path(day)
                stream_dir = day_dir / stream
                segment_dir = stream_dir / segment_key
                action = "copied"
                reason = "new segment"

                if segment_dir.exists():
                    exact_match = True
                    for name in expected_names:
                        file_path = segment_dir / name
                        if not file_path.is_file():
                            exact_match = False
                            break
                        if compute_file_sha256(file_path) != file_infos[name]["sha256"]:
                            exact_match = False
                            break

                    if exact_match:
                        action = "skipped"
                        reason = "exact match"
                    else:
                        new_key = find_available_segment(stream_dir, segment_key)
                        if new_key is None:
                            raise ValueError("No available segment slot")
                        segment_key = new_key
                        arc_key = f"{stream}/{segment_key}"
                        segment_dir = stream_dir / segment_key
                        action = "deconflicted"
                        reason = "segment key conflict"

                if action in {"copied", "deconflicted"}:
                    segment_dir.mkdir(parents=True, exist_ok=True)
                    for name in expected_names:
                        (segment_dir / name).write_bytes(file_infos[name]["content"])

                file_records = [
                    {
                        "name": name,
                        "sha256": str(file_infos[name]["sha256"]),
                        "size": int(file_infos[name]["size"]),
                    }
                    for name in expected_names
                ]
                state_record: dict[str, Any] = {"files": file_records}
                if sender_fingerprint is not None:
                    state_record["sender_fingerprint"] = sender_fingerprint
                if sender_instance_id is not None:
                    state_record["sender_instance_id"] = sender_instance_id
                new_state.setdefault(day, {})[arc_key] = state_record
                if action == "deconflicted":
                    original_arc_key = f"{stream}/{original_segment_key}"
                    new_state.setdefault(day, {})[original_arc_key] = dict(state_record)

                entry = {
                    "ts": datetime.now(timezone.utc).isoformat(),
                    "action": action,
                    "item_type": "segment",
                    "item_id": f"{day}/{arc_key}",
                    "reason": reason,
                    "files": expected_names,
                }
                if action == "deconflicted":
                    entry["original_key"] = original_segment_key
                if sender_fingerprint is not None:
                    entry["sender_fingerprint"] = sender_fingerprint
                if sender_instance_id is not None:
                    entry["sender_instance_id"] = sender_instance_id
                _append_decision(log_path, entry)

                if action == "copied":
                    copied += 1
                elif action == "skipped":
                    skipped += 1
                else:
                    deconflicted += 1
            except Exception as exc:
                errors.append(
                    {
                        "segment_key": segment_key,
                        "day": day,
                        "error": str(exc),
                    }
                )

        if new_state:
            state_path = get_state_directory(key_prefix) / "segments" / "state.json"
            state_path.parent.mkdir(parents=True, exist_ok=True)
            try:
                existing = json.loads(state_path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                existing = {}

            for day, segments_for_day in new_state.items():
                existing.setdefault(day, {}).update(segments_for_day)

            _write_state_atomic(state_path, existing)

        written = copied + deconflicted
        if written > 0:
            source = g.journal_source
            source.setdefault("stats", {})
            source["stats"]["segments_received"] = (
                source["stats"].get("segments_received", 0) + written
            )
            save_journal_source(source)

            try:
                emit("supervisor", "request", cmd=["journal", "indexer", "--rescan"])
            except Exception:
                logger.warning("Failed to trigger indexer rescan via Callosum")

        return jsonify(
            {
                "segments_received": written,
                "segments_skipped": skipped,
                "segments_deconflicted": deconflicted,
                "errors": errors,
            }
        )

    @bp.route("/journal/<key_prefix>/ingest/entities", methods=["POST"])
    @require_journal_source
    def ingest_entities(key_prefix: str):
        if journal_source_state_prefix(g.journal_source) != key_prefix:
            abort(403, description="Key prefix mismatch")

        payload = request.get_json(silent=True)
        if not isinstance(payload, dict):
            return error_response(INVALID_JSON_REQUEST, detail="Invalid JSON body")

        entities = payload.get("entities")
        if not isinstance(entities, list):
            return error_response(
                MISSING_REQUIRED_FIELD, detail="Missing entities array"
            )

        state_dir = get_state_directory(key_prefix)
        log_path = state_dir / "entities" / "log.jsonl"
        state_path = state_dir / "entities" / "state.json"
        staged_dir = state_dir / "entities" / "staged"

        try:
            entity_state = json.loads(state_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            entity_state = {}
        if not isinstance(entity_state, dict):
            entity_state = {}
        id_map = entity_state.get("id_map")
        received = entity_state.get("received")
        if not isinstance(id_map, dict) or not isinstance(received, dict):
            entity_state = {"id_map": {}, "received": {}}
        else:
            entity_state = {"id_map": dict(id_map), "received": dict(received)}

        target_entities = load_all_journal_entities()
        target_has_principal = has_journal_principal()

        auto_merged = 0
        created = 0
        staged = 0
        skipped = 0
        errors: list[dict[str, str]] = []

        for entity_data in entities:
            try:
                if not isinstance(entity_data, dict):
                    raise ValueError("Entity data must be an object")

                name = str(entity_data.get("name", "")).strip()
                if not name:
                    raise ValueError("Entity name is required")

                source_id = str(entity_data.get("id") or entity_slug(name))
                if not source_id:
                    raise ValueError("Entity id is required")
                entity_data["id"] = source_id

                content_hash = hashlib.sha256(
                    json.dumps(entity_data, sort_keys=True, ensure_ascii=False).encode()
                ).hexdigest()

                existing_hash = entity_state["received"].get(source_id)
                if existing_hash == content_hash:
                    skipped += 1
                    entity_state["received"][source_id] = content_hash
                    _append_decision(
                        log_path,
                        {
                            "ts": datetime.now(timezone.utc).isoformat(),
                            "action": "skipped",
                            "item_type": "entity",
                            "item_id": source_id,
                            "match_tier": None,
                            "reason": "idempotent",
                            "source": entity_data,
                            "target": None,
                            "fields_changed": [],
                        },
                    )
                    continue

                match = find_matching_entity(
                    entity_data["name"], list(target_entities.values())
                )

                if match is not None and match.is_high_confidence:
                    target_id = str(match["id"])
                    target_entity: EntityDict = dict(target_entities[target_id])
                    pre_merge_snapshot = dict(target_entity)

                    aka_by_lower: dict[str, str] = {}
                    for values in (
                        target_entity.get("aka", []),
                        entity_data.get("aka", []),
                    ):
                        if not isinstance(values, list):
                            continue
                        for value in values:
                            if not value:
                                continue
                            key = str(value).lower()
                            if key not in aka_by_lower:
                                aka_by_lower[key] = str(value)
                    if aka_by_lower:
                        target_entity["aka"] = sorted(
                            aka_by_lower.values(), key=str.lower
                        )

                    merged_emails: list[str] = []
                    seen_emails: set[str] = set()
                    for values in (
                        target_entity.get("emails", []),
                        entity_data.get("emails", []),
                    ):
                        if not isinstance(values, list):
                            continue
                        for value in values:
                            if not value:
                                continue
                            email = str(value)
                            key = email.lower()
                            if key in seen_emails:
                                continue
                            seen_emails.add(key)
                            merged_emails.append(email)
                    if merged_emails:
                        target_entity["emails"] = merged_emails

                    source_created = entity_data.get("created_at")
                    target_created = target_entity.get("created_at")
                    if source_created is not None and target_created is not None:
                        target_entity["created_at"] = min(
                            source_created, target_created
                        )
                    elif source_created is not None:
                        target_entity["created_at"] = source_created

                    save_journal_entity(target_entity)
                    target_entities[target_id] = target_entity
                    fields_changed = sorted(
                        key
                        for key in set(pre_merge_snapshot) | set(target_entity)
                        if pre_merge_snapshot.get(key) != target_entity.get(key)
                    )
                    entity_state["id_map"][source_id] = target_id
                    auto_merged += 1
                    _append_decision(
                        log_path,
                        {
                            "ts": datetime.now(timezone.utc).isoformat(),
                            "action": "auto_merged",
                            "item_type": "entity",
                            "item_id": source_id,
                            "match_tier": int(match.tier),
                            "reason": "high_confidence_match",
                            "source": entity_data,
                            "target": target_entity,
                            "fields_changed": fields_changed,
                        },
                    )
                elif match is not None and not match.is_high_confidence:
                    staged_dir.mkdir(parents=True, exist_ok=True)
                    staged_payload = {
                        "source_entity": entity_data,
                        "match_candidates": [
                            {
                                "id": match["id"],
                                "name": match["name"],
                                "tier": int(match.tier),
                            }
                        ],
                        "reason": "low_confidence_match",
                        "staged_at": datetime.now(timezone.utc).isoformat(),
                    }
                    (staged_dir / f"{source_id}.json").write_text(
                        json.dumps(staged_payload, indent=2, ensure_ascii=False) + "\n",
                        encoding="utf-8",
                    )
                    staged += 1
                    _append_decision(
                        log_path,
                        {
                            "ts": datetime.now(timezone.utc).isoformat(),
                            "action": "staged",
                            "item_type": "entity",
                            "item_id": source_id,
                            "match_tier": int(match.tier),
                            "reason": "low_confidence_match",
                            "source": entity_data,
                            "target": None,
                            "fields_changed": [],
                        },
                    )
                else:
                    source_slug = entity_data["id"]
                    if source_slug in target_entities:
                        staged_dir.mkdir(parents=True, exist_ok=True)
                        staged_payload = {
                            "source_entity": entity_data,
                            "match_candidates": [
                                {
                                    "id": source_slug,
                                    "name": target_entities[source_slug]["name"],
                                    "tier": None,
                                }
                            ],
                            "reason": "id_collision",
                            "staged_at": datetime.now(timezone.utc).isoformat(),
                        }
                        (staged_dir / f"{source_id}.json").write_text(
                            json.dumps(staged_payload, indent=2, ensure_ascii=False)
                            + "\n",
                            encoding="utf-8",
                        )
                        staged += 1
                        _append_decision(
                            log_path,
                            {
                                "ts": datetime.now(timezone.utc).isoformat(),
                                "action": "staged",
                                "item_type": "entity",
                                "item_id": source_id,
                                "match_tier": None,
                                "reason": "id_collision",
                                "source": entity_data,
                                "target": None,
                                "fields_changed": [],
                            },
                        )
                    elif entity_data.get("is_principal") and target_has_principal:
                        staged_dir.mkdir(parents=True, exist_ok=True)
                        staged_payload = {
                            "source_entity": entity_data,
                            "match_candidates": [],
                            "reason": "principal_conflict",
                            "staged_at": datetime.now(timezone.utc).isoformat(),
                        }
                        (staged_dir / f"{source_id}.json").write_text(
                            json.dumps(staged_payload, indent=2, ensure_ascii=False)
                            + "\n",
                            encoding="utf-8",
                        )
                        staged += 1
                        _append_decision(
                            log_path,
                            {
                                "ts": datetime.now(timezone.utc).isoformat(),
                                "action": "staged",
                                "item_type": "entity",
                                "item_id": source_id,
                                "match_tier": None,
                                "reason": "principal_conflict",
                                "source": entity_data,
                                "target": None,
                                "fields_changed": [],
                            },
                        )
                    else:
                        save_journal_entity(entity_data)
                        target_entities[source_slug] = entity_data
                        if entity_data.get("is_principal"):
                            target_has_principal = True
                        entity_state["id_map"][source_id] = source_id
                        created += 1
                        _append_decision(
                            log_path,
                            {
                                "ts": datetime.now(timezone.utc).isoformat(),
                                "action": "created",
                                "item_type": "entity",
                                "item_id": source_id,
                                "match_tier": None,
                                "reason": "no_match",
                                "source": entity_data,
                                "target": None,
                                "fields_changed": [],
                            },
                        )

                entity_state["received"][source_id] = content_hash
            except Exception as exc:
                entity_id = (
                    entity_data.get("id", "") if isinstance(entity_data, dict) else ""
                )
                errors.append({"entity_id": entity_id, "error": str(exc)})

        _write_state_atomic(state_path, entity_state)

        written = auto_merged + created
        if written > 0:
            source = g.journal_source
            source.setdefault("stats", {})
            source["stats"]["entities_received"] = (
                source["stats"].get("entities_received", 0) + written
            )
            save_journal_source(source)

        return jsonify(
            {
                "auto_merged": auto_merged,
                "created": created,
                "staged": staged,
                "skipped": skipped,
                "errors": errors,
            }
        )

    @bp.route("/journal/<key_prefix>/ingest/facets", methods=["POST"])
    @require_journal_source
    def ingest_facets(key_prefix: str):
        if journal_source_state_prefix(g.journal_source) != key_prefix:
            abort(403, description="Key prefix mismatch")

        metadata_raw = request.form.get("metadata")
        if not metadata_raw:
            return error_response(MISSING_REQUIRED_FIELD, detail="Missing metadata")

        try:
            metadata = json.loads(metadata_raw)
        except json.JSONDecodeError:
            return error_response(
                INVALID_JSON_REQUEST,
                detail="Invalid metadata JSON",
            )

        if not isinstance(metadata, dict):
            return error_response(
                INVALID_JSON_REQUEST,
                detail="Invalid metadata JSON",
            )

        facets = metadata.get("facets")
        if not isinstance(facets, list):
            return error_response(MISSING_REQUIRED_FIELD, detail="Missing facets array")

        state_dir = get_state_directory(key_prefix)
        entities_state_path = state_dir / "entities" / "state.json"
        facets_state_path = state_dir / "facets" / "state.json"
        log_path = state_dir / "facets" / "log.jsonl"
        staged_dir = state_dir / "facets" / "staged"

        try:
            entities_state = json.loads(entities_state_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            entities_state = {}
        if not isinstance(entities_state, dict):
            entities_state = {}
        id_map = entities_state.get("id_map")
        if not isinstance(id_map, dict):
            id_map = {}

        try:
            facets_state = json.loads(facets_state_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            facets_state = {}
        if not isinstance(facets_state, dict):
            facets_state = {}
        received = facets_state.get("received")
        if not isinstance(received, dict):
            received = {}
        facets_state = {"received": dict(received)}

        created = 0
        merged = 0
        skipped = 0
        staged = 0
        errors: list[dict[str, str]] = []
        written_facets: set[str] = set()

        for facet_idx, facet in enumerate(facets):
            if not isinstance(facet, dict):
                return error_response(
                    INVALID_REQUEST_VALUE,
                    detail="Facet metadata must be an object",
                )

            facet_name = str(facet.get("name", "")).strip()
            files = facet.get("files")
            if not facet_name:
                return error_response(
                    MISSING_REQUIRED_FIELD,
                    detail="Facet name is required",
                )
            if not _FACET_NAME_RE.match(facet_name):
                return error_response(
                    INVALID_REQUEST_VALUE,
                    detail="Invalid facet name",
                )
            if not isinstance(files, list):
                return error_response(
                    MISSING_REQUIRED_FIELD,
                    detail="Facet files must be an array",
                )

            file_bytes: list[bytes] = []
            normalized_files: list[dict[str, str]] = []
            for file_idx, file_meta in enumerate(files):
                if not isinstance(file_meta, dict):
                    return error_response(
                        INVALID_REQUEST_VALUE,
                        detail="Facet file metadata must be an object",
                    )

                path_value = file_meta.get("path")
                type_value = file_meta.get("type")
                if not isinstance(path_value, str) or not isinstance(type_value, str):
                    return error_response(
                        MISSING_REQUIRED_FIELD,
                        detail="Facet file metadata must include path and type",
                    )

                upload = request.files.get(f"files_{facet_idx}_{file_idx}")
                if upload is None:
                    return error_response(
                        INGEST_NO_FILES,
                        detail=(
                            f"Missing uploaded file for facet {facet_idx} file {file_idx}"
                        ),
                    )

                file_bytes.append(upload.read())
                normalized_files.append({"path": path_value, "type": type_value})

            facet_result = process_facet(
                facet_name=facet_name,
                files=normalized_files,
                file_data=file_bytes,
                journal_root=Path(state.journal_root),
                id_map=id_map,
                log_path=log_path,
                staged_dir=staged_dir,
                received=facets_state["received"],
            )
            created += facet_result["created"]
            merged += facet_result["merged"]
            skipped += facet_result["skipped"]
            staged += facet_result["staged"]
            errors.extend(facet_result["errors"])
            if facet_result["wrote_files"]:
                written_facets.add(facet_name)

        _write_state_atomic(facets_state_path, facets_state)

        if written_facets:
            source = g.journal_source
            source.setdefault("stats", {})
            source["stats"]["facets_received"] = source["stats"].get(
                "facets_received", 0
            ) + len(written_facets)
            save_journal_source(source)

        return jsonify(
            {
                "created": created,
                "merged": merged,
                "skipped": skipped,
                "staged": staged,
                "errors": errors,
            }
        )

    @bp.route("/journal/<key_prefix>/ingest/imports", methods=["POST"])
    @require_journal_source
    def ingest_imports(key_prefix: str):
        if journal_source_state_prefix(g.journal_source) != key_prefix:
            abort(403, description="Key prefix mismatch")

        payload = request.get_json(silent=True)
        if not isinstance(payload, dict):
            return error_response(INVALID_JSON_REQUEST, detail="Invalid JSON body")

        imports = payload.get("imports")
        if not isinstance(imports, list):
            return error_response(
                MISSING_REQUIRED_FIELD, detail="Missing imports array"
            )

        state_dir = get_state_directory(key_prefix)
        log_path = state_dir / "imports" / "log.jsonl"
        state_path = state_dir / "imports" / "state.json"
        staged_dir = state_dir / "imports" / "staged"

        try:
            imports_state = json.loads(state_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            imports_state = {}
        if not isinstance(imports_state, dict):
            imports_state = {}
        received = imports_state.get("received")
        if not isinstance(received, dict):
            received = {}
        imports_state = {"received": dict(received)}

        journal_root = Path(state.journal_root)

        copied = 0
        skipped = 0
        staged = 0
        errors: list[dict[str, str]] = []

        for item in imports:
            try:
                if not isinstance(item, dict):
                    raise ValueError("Import item must be an object")

                import_id = str(item.get("id", "")).strip()
                if not import_id or not _IMPORT_ID_RE.match(import_id):
                    raise ValueError(f"Invalid import id: {import_id!r}")

                import_json = item.get("import_json")
                imported_json = item.get("imported_json")
                content_manifest = item.get("content_manifest")

                if not isinstance(import_json, dict):
                    raise ValueError("import_json must be an object")
                if not isinstance(imported_json, dict):
                    raise ValueError("imported_json must be an object")
                if not isinstance(content_manifest, list):
                    raise ValueError("content_manifest must be an array")

                hash_input = json.dumps(
                    {
                        "import_json": import_json,
                        "imported_json": imported_json,
                        "content_manifest": content_manifest,
                    },
                    sort_keys=True,
                    ensure_ascii=False,
                ).encode()
                content_hash = hashlib.sha256(hash_input).hexdigest()

                if imports_state["received"].get(import_id) == content_hash:
                    skipped += 1
                    _append_decision(
                        log_path,
                        {
                            "ts": datetime.now(timezone.utc).isoformat(),
                            "action": "skipped",
                            "item_type": "import",
                            "item_id": import_id,
                            "reason": "idempotent",
                        },
                    )
                    continue

                target_dir = journal_root / "imports" / import_id
                if target_dir.is_dir():
                    staged_dir.mkdir(parents=True, exist_ok=True)
                    staged_payload = {
                        "import_id": import_id,
                        "import_json": import_json,
                        "imported_json": imported_json,
                        "content_manifest": content_manifest,
                        "reason": "id_collision",
                        "staged_at": datetime.now(timezone.utc).isoformat(),
                    }
                    (staged_dir / f"{import_id}.json").write_text(
                        json.dumps(staged_payload, indent=2, ensure_ascii=False) + "\n",
                        encoding="utf-8",
                    )
                    staged += 1
                    _append_decision(
                        log_path,
                        {
                            "ts": datetime.now(timezone.utc).isoformat(),
                            "action": "staged",
                            "item_type": "import",
                            "item_id": import_id,
                            "reason": "id_collision",
                        },
                    )
                else:
                    target_dir.mkdir(parents=True, exist_ok=True)
                    (target_dir / "import.json").write_text(
                        json.dumps(import_json, indent=2, ensure_ascii=False) + "\n",
                        encoding="utf-8",
                    )
                    (target_dir / "imported.json").write_text(
                        json.dumps(imported_json, indent=2, ensure_ascii=False) + "\n",
                        encoding="utf-8",
                    )
                    lines = [
                        json.dumps(entry, ensure_ascii=False)
                        for entry in content_manifest
                    ]
                    (target_dir / "content_manifest.jsonl").write_text(
                        "\n".join(lines) + "\n" if lines else "",
                        encoding="utf-8",
                    )
                    copied += 1
                    _append_decision(
                        log_path,
                        {
                            "ts": datetime.now(timezone.utc).isoformat(),
                            "action": "copied",
                            "item_type": "import",
                            "item_id": import_id,
                            "reason": "new",
                        },
                    )

                imports_state["received"][import_id] = content_hash
            except Exception as exc:
                import_id_str = item.get("id", "") if isinstance(item, dict) else ""
                errors.append({"import_id": str(import_id_str), "error": str(exc)})

        _write_state_atomic(state_path, imports_state)

        if copied > 0:
            source = g.journal_source
            source.setdefault("stats", {})
            source["stats"]["imports_received"] = (
                source["stats"].get("imports_received", 0) + copied
            )
            save_journal_source(source)

        return jsonify(
            {
                "copied": copied,
                "skipped": skipped,
                "staged": staged,
                "errors": errors,
            }
        )

    @bp.route("/journal/<key_prefix>/ingest/config", methods=["POST"])
    @require_journal_source
    def ingest_config(key_prefix: str):
        if journal_source_state_prefix(g.journal_source) != key_prefix:
            abort(403, description="Key prefix mismatch")

        payload = request.get_json(silent=True)
        if not isinstance(payload, dict):
            return error_response(INVALID_JSON_REQUEST, detail="Invalid JSON body")

        source_config = payload.get("config")
        if not isinstance(source_config, dict):
            return error_response(
                MISSING_REQUIRED_FIELD, detail="Missing config object"
            )

        state_dir = get_state_directory(key_prefix)
        log_path = state_dir / "config" / "log.jsonl"
        state_path = state_dir / "config" / "state.json"
        config_dir = state_dir / "config"

        try:
            config_state = json.loads(state_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            config_state = {}
        if not isinstance(config_state, dict):
            config_state = {}

        content_hash = hashlib.sha256(
            json.dumps(source_config, sort_keys=True, ensure_ascii=False).encode()
        ).hexdigest()

        if config_state.get("last_hash") == content_hash:
            _append_decision(
                log_path,
                {
                    "ts": datetime.now(timezone.utc).isoformat(),
                    "action": "skipped",
                    "item_type": "config",
                    "item_id": "journal.json",
                    "reason": "idempotent",
                },
            )
            return jsonify({"staged": False, "skipped": True, "reason": "idempotent"})

        from solstone.think.utils import get_config

        target_config = get_config()
        source_flat = _flatten_config(source_config)
        target_flat = _flatten_config(target_config)

        all_keys = sorted(set(source_flat) | set(target_flat))
        diff = {}
        for key in all_keys:
            if _is_never_transfer(key):
                continue
            source_val = source_flat.get(key)
            target_val = target_flat.get(key)
            if source_val != target_val:
                diff[key] = {
                    "source": source_val,
                    "target": target_val,
                    "category": _categorize_field(key),
                }

        config_dir.mkdir(parents=True, exist_ok=True)
        (config_dir / "source_config.json").write_text(
            json.dumps(source_config, indent=2, ensure_ascii=False) + "\n",
            encoding="utf-8",
        )
        (config_dir / "diff.json").write_text(
            json.dumps(diff, indent=2, ensure_ascii=False) + "\n",
            encoding="utf-8",
        )

        config_state["last_hash"] = content_hash
        _write_state_atomic(state_path, config_state)

        _append_decision(
            log_path,
            {
                "ts": datetime.now(timezone.utc).isoformat(),
                "action": "staged",
                "item_type": "config",
                "item_id": "journal.json",
                "reason": "config_received",
            },
        )

        source = g.journal_source
        source.setdefault("stats", {})
        source["stats"]["config_received"] = (
            source["stats"].get("config_received", 0) + 1
        )
        save_journal_source(source)

        return jsonify(
            {
                "staged": True,
                "skipped": False,
                "diff_fields": len(diff),
            }
        )
