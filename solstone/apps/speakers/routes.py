# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Speaker voiceprint management app - sentence-based embeddings.

Voiceprints are stored at the journal level (not per-facet) since a person's
voice is the same regardless of which facet they appear in.
"""

from __future__ import annotations

import json
import logging
import re
from dataclasses import dataclass
from datetime import date
from pathlib import Path
from typing import TYPE_CHECKING, Any

from flask import (
    Blueprint,
    current_app,
    jsonify,
    request,
    send_file,
)

from solstone.apps.speakers import native as native_speakers
from solstone.apps.speakers.attribution import (
    _speaker_encoder_identity,
    accumulate_voiceprints,
    append_speaker_correction,
    apply_label_patches,
    attribute_segment,
    backfill_last_seen,
    backfill_segments,
    propagate_speaker_correction,
    save_speaker_labels,
)
from solstone.apps.speakers.audio import audio_serve_url, resolve_audio_file
from solstone.apps.speakers.bootstrap import (
    bootstrap_voiceprints,
    link_import,
    merge_names,
    resolve_name_variants,
    seed_from_imports,
)
from solstone.apps.speakers.copy import (
    OWNER_DETECT_CANDIDATE_GUIDANCE,
    OWNER_REJECTION_COOLDOWN_GUIDANCE,
    SPK_OVERVIEW_KNOWN_VOICES_SORTS,
    TR_NOT_IN_NEW_VOICES,
    speaker_copy_payload,
)
from solstone.apps.speakers.discovery import (
    SpeakerDiscoveryKernelError,
    discover_unknown_speakers,
    discovery_kernel_failure_http_result,
    get_cluster_presence,
    identify_cluster,
    load_discovery_cache,
    read_discovery_cache_snapshot,
    resolve_statement_cluster,
    undo_identify_operation,
)
from solstone.apps.speakers.eligibility import (
    current_principal_id,
    is_speaker_attach_candidate,
)
from solstone.apps.speakers.encoder_config import (
    OWNER_BOOTSTRAP_MIN_STMTS,
    OWNER_THRESHOLD,
)
from solstone.apps.speakers.owner import (
    bootstrap_owner_from_manual_tags,
    classify_sentences,
    confirm_owner_candidate,
    detect_owner_candidate,
    ensure_principal_entity,
    load_manual_tag_stats,
    load_owner_bootstrap_diagnostics,
    load_owner_centroid,
    load_owner_manual_bootstrap_guidance,
    load_owner_provisional_centroid,
    owner_detection_ready,
    owner_rejection_cooldown_payload,
    principal_identity_or_none,
    rebuild_owner_centroid,
    reject_owner_candidate,
)
from solstone.apps.speakers.quality import get_speaker_quality_status
from solstone.apps.speakers.status import get_speakers_status
from solstone.apps.speakers.suggest import suggest_opportunities
from solstone.apps.utils import log_app_action
from solstone.convey.date_nav import build_date_nav_index
from solstone.convey.day_grid import build_day_grid_payload
from solstone.convey.reasons import (
    ENTITY_BLOCKED,
    ENTITY_NOT_FOUND,
    FILE_NOT_FOUND,
    FILE_READ_FAILED,
    INVALID_DAY,
    INVALID_ENTITY_TYPE,
    INVALID_MONTH,
    INVALID_REQUEST_VALUE,
    INVALID_SEGMENT_OR_STREAM,
    MISSING_REQUEST_BODY,
    MISSING_REQUIRED_FIELD,
    SPEAKER_ATTRIBUTION_STATE_INVALID,
    SPEAKER_COMMAND_FAILED,
    SPEAKER_DISCOVERY_FAILED,
    SPEAKER_IDENTIFY_CONFLICT,
    SPEAKER_IDENTIFY_OPERATION_NOT_FOUND,
    SPEAKER_IDENTIFY_RECOVERABLE,
    SPEAKER_IDENTIFY_REPAIR_REQUIRED,
    SPEAKER_LABELS_BUSY,
    SPEAKER_NOT_FOUND,
    SPEAKER_OWNER_CENTROID_REQUIRED,
    SPEAKER_OWNER_IDENTITY_REQUIRED,
    SPEAKER_OWNER_VOICE_TOO_CLOSE,
    SPEAKER_REVIEW_UNAVAILABLE,
    SPEAKER_SENTENCE_MISSING,
    SPEAKER_VOICEPRINT_BUSY,
)
from solstone.convey.utils import (
    DATE_RE,
    error_response,
    safe_day_path,
    success_response,
)
from solstone.think.awareness import get_current
from solstone.think.entities import find_matching_entity
from solstone.think.entities.history import trust_operation_lock
from solstone.think.entities.journal import (
    ensure_journal_entity_memory,
    get_journal_principal,
    journal_entity_memory_path,
    load_all_journal_entities,
    load_journal_entity,
)
from solstone.think.journal_io.errors import LockTimeout
from solstone.think.journal_io.npz import load_npz
from solstone.think.media import MIME_TYPES
from solstone.think.speaker_cluster_dismissals import (
    list_dismissals,
    record_cluster_dismissal,
)
from solstone.think.speaker_identify_operations import (
    fold_all_operations,
    fold_operation,
)
from solstone.think.speaker_keep_separate import list_assertions
from solstone.think.utils import (
    STREAM_RE,
    day_dirs,
    day_path,
    get_journal,
    iter_segments,
    now_ms,
    segment_parse,
    segment_start_ts_ms,
)
from solstone.think.utils import segment_key as validate_segment_key
from solstone.think.utils import segment_path as get_segment_path

if TYPE_CHECKING:
    import numpy as np

    from solstone.think.entities.core import EntityDict

logger = logging.getLogger(__name__)
SEGMENT_KEY_RE = re.compile(r"\d{6}_\d+")
OWNER_STATUS_CANDIDATE = "candidate"
OWNER_STATUS_CONFIRMED = "confirmed"
OWNER_STATUS_ROUTING_TOKENS = {
    "candidate": OWNER_STATUS_CANDIDATE,
    "confirmed": OWNER_STATUS_CONFIRMED,
}
PROPAGATION_CLI_VERB = "speakers propagate-correction"
PEOPLE_SEARCH_LIMIT = 8


@dataclass(frozen=True)
class VoiceprintRemovalResult:
    outcome: str
    entity_id: str
    keys_removed: list[str]
    file_deleted: bool
    voiceprints_path: Path | None


speakers_bp = Blueprint(
    "app:speakers",
    __name__,
    url_prefix="/app/speakers",
    static_folder="static",
    static_url_path="/static",
)


def _normalize_embedding(emb: np.ndarray) -> np.ndarray | None:
    from solstone.think.entities import normalize_embedding

    return normalize_embedding(emb)


def _parse_time_to_seconds(time_str: str) -> int:
    """Parse HH:MM:SS time string to seconds."""
    parts = time_str.split(":")
    return int(parts[0]) * 3600 + int(parts[1]) * 60 + int(parts[2])


def _time_to_seconds(t) -> int:
    """Convert datetime.time to seconds since midnight."""
    return t.hour * 3600 + t.minute * 60 + t.second


def _load_embeddings_file(
    npz_path: Path,
) -> tuple[np.ndarray, np.ndarray, np.ndarray | None] | None:
    """Load embeddings, statement_ids, and optional durations from NPZ file.

    Returns tuple of (embeddings, statement_ids, durations_s) or None if file is invalid.
    """
    if not npz_path.exists():
        return None

    try:
        data = load_npz(npz_path)
        if data is None:
            return None
        embeddings = data.get("embeddings")
        statement_ids = data.get("statement_ids")
        durations_s = data.get("durations_s")

        if embeddings is None or statement_ids is None:
            return None

        return embeddings, statement_ids, durations_s
    except Exception as e:
        logger.warning("Failed to load embeddings %s: %s", npz_path, e)
        return None


def _load_segment_speakers(segment_dir: Path) -> list[str]:
    """Load speaker names from segment's speakers.json.

    Args:
        segment_dir: Path to segment directory

    Returns:
        List of speaker name strings, or empty list if not found/invalid.
    """
    speakers_path = segment_dir / "talents" / "speakers.json"
    if not speakers_path.exists():
        return []

    try:
        with open(speakers_path, "r", encoding="utf-8") as f:
            data = json.load(f)

        # Must be a list of strings
        if not isinstance(data, list):
            return []

        # Filter to only strings
        return [name for name in data if isinstance(name, str) and name.strip()]
    except (json.JSONDecodeError, OSError) as e:
        logger.warning("Failed to load speakers.json from %s: %s", segment_dir, e)
        return []


def _load_entity_voiceprints_file(
    entity_id: str,
) -> tuple[np.ndarray, list[dict]] | None:
    from solstone.think.entities import load_entity_voiceprints_file

    return load_entity_voiceprints_file(entity_id)


def _save_voiceprint(
    entity_id: str,
    embedding: np.ndarray,
    day: str,
    segment_key: str,
    source: str,
    sentence_id: int,
    stream: str | None = None,
) -> Path:
    """Save a voiceprint to the entity's journal-level voiceprints.npz.

    Voiceprints are stored at entities/<id>/voiceprints.npz since a person's
    voice is the same across all facets.

    Args:
        entity_id: Entity ID (slug)
        embedding: Normalized embedding vector (256-dim)
        day: Day string (YYYYMMDD)
        segment_key: Segment directory name
        source: Audio source stem
        sentence_id: Sentence ID within transcript

    Returns:
        Path to the voiceprints.npz file
    """
    import numpy as np

    folder = ensure_journal_entity_memory(entity_id)
    npz_path = folder / "voiceprints.npz"

    metadata = {
        "day": day,
        "segment_key": segment_key,
        "source": source,
        "sentence_id": sentence_id,
        "added_at": now_ms(),
        "last_seen_ts": segment_start_ts_ms(day, segment_key),
    }
    if stream:
        metadata["stream"] = stream
    native_speakers.write_voiceprint(
        get_journal(),
        entity_id=entity_id,
        embedding=embedding.astype(np.float32).reshape(-1).tolist(),
        metadata=metadata,
        encoder=_speaker_encoder_identity(),
    )
    return npz_path


def _remove_voiceprint(
    entity_id: str,
    day: str,
    segment_key: str,
    source: str,
    sentence_id: int,
) -> VoiceprintRemovalResult:
    """Remove a specific voiceprint entry from an entity's voiceprints.npz.

    Matches by (day, segment_key, source, sentence_id) metadata key.
    """
    rendered_key = _render_voiceprint_key(day, segment_key, source, sentence_id)
    try:
        folder = journal_entity_memory_path(entity_id)
    except (RuntimeError, ValueError):
        return VoiceprintRemovalResult(
            outcome="not_found",
            entity_id=entity_id,
            keys_removed=[],
            file_deleted=False,
            voiceprints_path=None,
        )

    npz_path = folder / "voiceprints.npz"
    if not npz_path.exists():
        return VoiceprintRemovalResult(
            outcome="not_found",
            entity_id=entity_id,
            keys_removed=[],
            file_deleted=False,
            voiceprints_path=npz_path,
        )

    result = native_speakers.remove_voiceprint(
        get_journal(),
        entity_id=entity_id,
        key={
            "day": day,
            "segment_key": segment_key,
            "source": source,
            "sentence_id": sentence_id,
        },
        encoder=_speaker_encoder_identity(),
    )
    outcome = str(result.get("outcome", "not_found"))
    return VoiceprintRemovalResult(
        outcome=outcome,
        entity_id=entity_id,
        keys_removed=[rendered_key] if outcome != "not_found" else [],
        file_deleted=outcome == "unlinked",
        voiceprints_path=npz_path,
    )


def _render_voiceprint_key(
    day: str,
    segment_key: str,
    source: str,
    sentence_id: int,
) -> str:
    return f"{day}/{segment_key}/{source}#{sentence_id}"


def _voiceprint_removal_payload(result: VoiceprintRemovalResult) -> dict[str, Any]:
    rel_path = None
    if result.voiceprints_path is not None:
        journal_root = Path(get_journal())
        try:
            rel_path = str(result.voiceprints_path.relative_to(journal_root))
        except ValueError:
            rel_path = str(result.voiceprints_path)
    return {
        "outcome": result.outcome,
        "entity_id": result.entity_id,
        "keys_removed": result.keys_removed,
        "file_deleted": result.file_deleted,
        "path": rel_path,
    }


def _propagation_reversal_payload(old_speaker: str, new_speaker: str) -> dict[str, str]:
    return {
        "verb": PROPAGATION_CLI_VERB,
        "old_speaker": new_speaker,
        "new_speaker": old_speaker,
        "bounded_to": "segments where these two appear",
    }


def _propagation_response_payload(result: dict[str, Any]) -> dict[str, Any]:
    payload = dict(result)
    payload["reversal"] = _propagation_reversal_payload(
        str(result["old_speaker"]),
        str(result["new_speaker"]),
    )
    return payload


def _propagation_offer(old_speaker: str | None, new_speaker: str) -> dict[str, Any]:
    if not old_speaker:
        return {
            "available": False,
            "reason": "no_old_speaker",
            "statement_count": 0,
            "segment_count": 0,
        }
    try:
        result = propagate_speaker_correction(old_speaker, new_speaker, commit=False)
    except Exception:
        logger.exception("Failed to preview speaker correction propagation")
        return {
            "available": False,
            "reason": "preview_failed",
            "statement_count": 0,
            "segment_count": 0,
        }

    statement_count = int(result.get("statement_count") or 0)
    segment_count = int(result.get("segment_count") or 0)
    if statement_count == 0:
        return {
            "available": False,
            "reason": "no_changes",
            "statement_count": 0,
            "segment_count": 0,
        }

    return {
        "available": True,
        "statement_count": statement_count,
        "segment_count": segment_count,
        "route": "/app/speakers/api/propagate-correction",
        "request": {
            "old_speaker": old_speaker,
            "new_speaker": new_speaker,
            "commit": False,
        },
    }


def _load_speaker_labels(segment_dir: Path) -> dict | None:
    """Load speaker_labels.json from a segment's talents/ directory.

    Returns the parsed JSON dict, or None if not found/invalid.
    """
    labels_path = segment_dir / "talents" / "speaker_labels.json"
    if not labels_path.is_file():
        return None
    try:
        with open(labels_path) as f:
            return json.load(f)
    except (json.JSONDecodeError, OSError):
        return None


def _audio_embedding_sources(segment_path: Path) -> list[str]:
    """Return audio embedding source stems from a segment directory."""
    return sorted(
        path.stem
        for path in segment_path.glob("*.npz")
        if path.stem.endswith("_audio") or path.stem == "audio"
    )


def _speaker_sentence_needs_review(
    label: dict | None, labels_data: dict | None
) -> bool:
    """Return the shared web/CLI review flag for one sentence."""
    if label:
        return label.get("confidence") == "medium" or not label.get("speaker")
    return True if labels_data else False


def _segment_has_speaker_review(labels_data: dict | None) -> bool:
    if not labels_data:
        return False
    return any(
        _speaker_sentence_needs_review(label, labels_data)
        for label in labels_data.get("labels", [])
    )


def _load_speaker_corrections(segment_dir: Path) -> list[dict]:
    """Load speaker_corrections.json from a segment's talents/ directory.

    Returns list of correction entries, or empty list if not found.
    """
    corr_path = segment_dir / "talents" / "speaker_corrections.json"
    if not corr_path.is_file():
        return []
    try:
        with open(corr_path) as f:
            data = json.load(f)
        return data.get("corrections", [])
    except (json.JSONDecodeError, OSError):
        return []


def _check_owner_contamination(embedding: np.ndarray) -> bool:
    """Check if an embedding is too close to the owner centroid.

    Returns True if the embedding is contaminated (should NOT be saved
    to a non-owner entity's voiceprints).
    """
    import numpy as np

    from solstone.apps.speakers.owner import load_owner_centroid

    centroid_data = load_owner_centroid()
    if centroid_data is not None:
        owner_centroid = centroid_data.centroid
        owner_threshold = centroid_data.threshold
    else:
        principal_id = _principal_id_or_none()
        if principal_id is None:
            return False
        owner_centroid = load_owner_provisional_centroid(principal_id)
        if owner_centroid is None:
            return False
        owner_threshold = OWNER_THRESHOLD
    score = float(np.dot(embedding, owner_centroid))
    return score >= owner_threshold


def _principal_id_or_none() -> str | None:
    """Return the current journal principal id if one exists."""
    principal = get_journal_principal()
    if principal is None:
        return None
    return str(principal["id"])


def _ensure_attribution_target(entity_id: str) -> EntityDict | None:
    """Resolve an attribution target, creating the principal on first owner tag."""
    entity = load_journal_entity(entity_id)
    if entity is not None:
        return entity
    if get_journal_principal() is not None:
        return None
    identity = principal_identity_or_none()
    if identity is None or identity[0] != entity_id:
        return None
    return ensure_principal_entity()


def _person_search_strings(entity: EntityDict) -> list[str]:
    """Return owner-entered strings that participate in people search."""
    values = [str(entity.get("name") or "")]
    aka = entity.get("aka")
    if isinstance(aka, list):
        values.extend(str(value) for value in aka if value)
    return values


def _entity_voiceprints_exist(entity_id: str) -> bool:
    """Return whether the entity has any stored voiceprint file."""
    if not entity_id:
        return False
    return (journal_entity_memory_path(entity_id) / "voiceprints.npz").exists()


def search_people_for_speakers(query: str) -> dict[str, Any]:
    """Return read-only people search results for the who-is-this sheet."""
    q = query.strip()
    if not q:
        return {"query": "", "people": []}

    folded_query = q.casefold()
    principal_id = current_principal_id()
    people: list[dict[str, Any]] = []
    for entity in load_all_journal_entities().values():
        entity_id = str(entity.get("id") or "")
        name = str(entity.get("name") or "")
        if not is_speaker_attach_candidate(entity, principal_id=principal_id):
            continue
        if not any(
            folded_query in value.casefold() for value in _person_search_strings(entity)
        ):
            continue
        people.append(
            {
                "entity_id": entity_id,
                "name": name,
                "has_voice": _entity_voiceprints_exist(entity_id),
            }
        )

    people.sort(key=lambda item: (item["name"].casefold(), item["entity_id"]))
    return {"query": q, "people": people[:PEOPLE_SEARCH_LIMIT]}


def _assign_attribution_impl(
    day: Any,
    stream: Any,
    segment_key: Any,
    source: Any,
    sentence_id: Any,
    speaker: Any,
) -> Any:
    """Assign a speaker to a sentence, inserting a label row when a stub omitted it."""
    if not all([day, stream, segment_key, source, sentence_id is not None, speaker]):
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="Missing required fields",
        )
    if not isinstance(day, str) or not DATE_RE.fullmatch(day):
        return error_response(
            INVALID_DAY,
            detail="Use a valid day, stream, and segment, then pick a sentence.",
        )
    if not isinstance(segment_key, str) or not SEGMENT_KEY_RE.fullmatch(segment_key):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Use a valid day, stream, and segment, then pick a sentence.",
        )
    if not isinstance(stream, str) or not STREAM_RE.fullmatch(stream):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Use a valid day, stream, and segment, then pick a sentence.",
        )
    try:
        sentence_id_int = int(sentence_id)
    except (TypeError, ValueError):
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="Use a numeric sentence id.",
        )
    speaker_id = str(speaker)

    principal_id = _principal_id_or_none()
    try:
        with trust_operation_lock():
            segment_dir = get_segment_path(day, segment_key, stream)
            labels_data = _load_speaker_labels(segment_dir)
            if not labels_data:
                return error_response(
                    SPEAKER_REVIEW_UNAVAILABLE,
                    detail="No speaker labels found",
                )

            label = None
            for item in labels_data.get("labels", []):
                if item.get("sentence_id") == sentence_id_int:
                    label = item
                    break

            existing_speaker = label.get("speaker") if label else None
            if (
                existing_speaker == speaker_id
                and label.get("method") == "user_assigned"
            ):
                response = {"status": "already_assigned"}
                if speaker_id == principal_id:
                    response["owner_bootstrap_outcome"] = "not_attempted"
                return success_response(response)
            if existing_speaker:
                return error_response(
                    SPEAKER_ATTRIBUTION_STATE_INVALID,
                    detail="Pick a sentence without a speaker.",
                )

            sentences, _ = _load_sentences(day, segment_key, source, stream=stream)
            if not any(sentence.get("id") == sentence_id_int for sentence in sentences):
                return error_response(
                    SPEAKER_SENTENCE_MISSING,
                    detail="Pick a different sentence with an embedding.",
                )

            emb = _get_sentence_embedding(
                day, segment_key, source, sentence_id_int, stream=stream
            )
            if emb is None:
                return error_response(
                    SPEAKER_SENTENCE_MISSING,
                    detail="Pick a different sentence with an embedding.",
                )

            target_entity = _ensure_attribution_target(speaker_id)
            if not target_entity:
                return error_response(
                    SPEAKER_NOT_FOUND,
                    detail=f"Entity '{speaker_id}' not found",
                )
            if target_entity.get("blocked"):
                return error_response(
                    ENTITY_BLOCKED,
                    detail="Choose an unblocked speaker.",
                )

            if speaker_id != principal_id and _check_owner_contamination(emb):
                return error_response(
                    SPEAKER_OWNER_VOICE_TOO_CLOSE,
                    detail="Embedding too similar to owner voice; cannot save",
                )

            try:
                _save_voiceprint(
                    speaker_id,
                    emb,
                    day,
                    segment_key,
                    source,
                    sentence_id_int,
                    stream=stream,
                )
            except LockTimeout as exc:
                return _voiceprint_busy_response(exc)

            old_method = label.get("method") if label else None
            try:
                apply_label_patches(
                    segment_dir,
                    {
                        sentence_id_int: {
                            "speaker": speaker_id,
                            "confidence": "high",
                            "method": "user_assigned",
                        }
                    },
                    allow_insert=True,
                )
            except LockTimeout as exc:
                return _labels_busy_response(exc)

            try:
                append_speaker_correction(
                    segment_dir,
                    {
                        "sentence_id": sentence_id_int,
                        "original_speaker": None,
                        "corrected_speaker": speaker_id,
                        "original_method": old_method,
                        "timestamp": now_ms(),
                    },
                )
            except LockTimeout as exc:
                return _labels_busy_response(exc)
    except LockTimeout as exc:
        return _labels_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)

    log_app_action(
        app="speakers",
        facet=None,
        action="attribution_assign",
        params={
            "day": day,
            "stream": stream,
            "segment_key": segment_key,
            "source": source,
            "sentence_id": sentence_id_int,
            "speaker": speaker_id,
        },
    )
    bootstrap_result = _maybe_bootstrap_owner_from_attestation(principal_id, speaker_id)

    response = {"status": "assigned", "speaker": speaker_id}
    if speaker_id == principal_id:
        response.update(_owner_bootstrap_response_fields(bootstrap_result))
    return success_response(response)


def _busy_location(exc: LockTimeout | native_speakers.NativeSpeakerResolveError) -> str:
    if isinstance(exc, LockTimeout):
        return str(exc.path)
    return exc.detail or exc.message


def _voiceprint_busy_response(
    exc: LockTimeout | native_speakers.NativeSpeakerResolveError,
) -> Any:
    logger.warning("voiceprint storage busy for %s", _busy_location(exc))
    return error_response(
        SPEAKER_VOICEPRINT_BUSY,
        detail="voiceprint storage is busy; try again",
    )


def _labels_busy_response(
    exc: LockTimeout | native_speakers.NativeSpeakerResolveError,
) -> Any:
    logger.warning("speaker labels busy for %s", _busy_location(exc))
    return error_response(
        SPEAKER_LABELS_BUSY,
        detail="speaker labels are busy; try again",
    )


def _native_storage_busy_response(
    exc: native_speakers.NativeSpeakerResolveError,
) -> Any:
    detail = exc.detail or ""
    if exc.reason != "tempfail" or not (
        "could not acquire lock" in detail or "storage is busy" in detail
    ):
        raise exc
    if "speaker_labels.json" in detail or "speaker_corrections.json" in detail:
        return _labels_busy_response(exc)
    return _voiceprint_busy_response(exc)


def _owner_bootstrap_status_fields() -> dict[str, Any]:
    """Return shared owner bootstrap diagnostics for status surfaces."""
    diagnostics = load_owner_bootstrap_diagnostics(_principal_id_or_none())
    return {
        **diagnostics,
        "segments_with_embeddings": diagnostics["segments_available"],
    }


def _maybe_bootstrap_owner_from_attestation(
    principal_id: str | None, speaker_id: str | None
) -> dict[str, Any] | None:
    """Refresh manual owner bootstrap state after a principal attestation."""
    if principal_id is None or speaker_id != principal_id:
        return None
    try:
        result = bootstrap_owner_from_manual_tags()
        if "error" in result:
            logger.warning(
                "owner manual bootstrap failed after attestation: %s",
                result["error"],
            )
        return result
    except Exception:
        logger.exception("owner manual bootstrap failed after attestation")
        return {"error": "owner manual bootstrap failed"}


def _owner_bootstrap_response_fields(result: dict[str, Any] | None) -> dict[str, Any]:
    """Map manual owner bootstrap internals to the browser assign contract."""
    if result is None:
        return {"owner_bootstrap_outcome": "failed"}

    if result.get("status") == "confirmed" and set(result) == {
        "status",
        "principal_id",
        "cluster_size",
        "evidence_tier",
    }:
        return {"owner_bootstrap_outcome": "built"}
    if (
        result.get("status") == "confirmed"
        and result.get("next_step") == "rebuild_owner"
    ):
        return {"owner_bootstrap_outcome": "already_built"}
    if result.get("status") == "low_quality":
        response = {"owner_bootstrap_outcome": "refused"}
        guidance = result.get("guidance")
        if isinstance(guidance, str):
            response["owner_bootstrap_guidance"] = guidance
        return response
    if result.get("error_kind") == "voiceprint_busy":
        return {"owner_bootstrap_outcome": "busy"}
    return {"owner_bootstrap_outcome": "failed"}


def _resolve_entity_display(
    entity_id: str,
    entity_cache: dict,
    principal_id: str | None,
) -> dict:
    """Resolve an entity ID to display info."""
    if entity_id not in entity_cache:
        entity_cache[entity_id] = load_journal_entity(entity_id)
    entity = entity_cache[entity_id]
    name = entity["name"] if entity else entity_id
    return {
        "name": name,
        "entity_id": entity_id,
        "is_owner": entity_id == principal_id,
    }


def _scan_segment_embeddings(day: str) -> list[dict]:
    """Scan a day for segments with audio embeddings.

    Only includes segments that have audio embedding NPZ files.
    Segments with a speakers.json file will include speaker names;
    segments without speakers.json will have an empty speakers list.

    Returns list of segment info dicts with keys:
        - key: segment directory name (HHMMSS_LEN)
        - start: formatted start time (HH:MM)
        - end: formatted end time (HH:MM)
        - duration: duration in seconds
        - sources: list of audio sources (e.g., ["mic_audio", "sys_audio"])
        - speakers: list of speaker names from speakers.json
        - speaker_count: number of speakers
    """
    segments = []
    for s_stream, s_key, s_path in iter_segments(day):
        # Validate segment key format
        parsed = segment_parse(s_key)
        if parsed[0] is None:
            continue

        start_time, end_time = parsed

        sources = _audio_embedding_sources(s_path)
        if not sources:
            continue

        # Load speakers.json (may be empty if not yet processed)
        speakers = _load_segment_speakers(s_path)

        # Calculate duration from start and end times
        duration = _time_to_seconds(end_time) - _time_to_seconds(start_time)

        segments.append(
            {
                "key": s_key,
                "stream": s_stream,
                "start": f"{start_time.hour:02d}:{start_time.minute:02d}",
                "end": f"{end_time.hour:02d}:{end_time.minute:02d}",
                "duration": duration,
                "sources": sources,
                "speakers": speakers,
                "speaker_count": len(speakers),
            }
        )

    return segments


def _load_sentences(
    day: str, segment_key: str, source: str, stream: str | None = None
) -> tuple[list[dict], tuple[np.ndarray, np.ndarray, np.ndarray | None] | None]:
    """Load transcript sentences and their embeddings for an audio source.

    Args:
        day: Day string (YYYYMMDD)
        segment_key: Segment directory name (HHMMSS_LEN)
        source: Audio source stem (e.g., "mic_audio")
        stream: Stream name for path resolution

    Returns:
        Tuple of (sentences, emb_data):
        - sentences: List of dicts with id, offset, text, has_embedding
        - emb_data: Tuple of (embeddings, statement_ids, durations_s) or None if no embeddings
    """
    if stream:
        segment_dir = get_segment_path(day, segment_key, stream, create=False)
    else:
        segment_dir = day_path(day) / segment_key

    # Load JSONL transcript
    jsonl_path = segment_dir / f"{source}.jsonl"
    if not jsonl_path.exists():
        return [], None

    sentences = []
    with open(jsonl_path) as f:
        lines = f.readlines()

    if not lines:
        return [], None

    # Get segment start time to compute relative offsets
    # JSONL contains absolute wall-clock times (e.g., "14:30:22")
    # Audio files start at time 0, so we need relative offset
    parsed = segment_parse(segment_key)
    segment_start_seconds = _time_to_seconds(parsed[0]) if parsed[0] else 0

    # First line is metadata, skip it
    # Remaining lines are sentences indexed by line number (1-based segment ID)
    for i, line in enumerate(lines[1:], start=1):
        try:
            entry = json.loads(line)
            abs_seconds = _parse_time_to_seconds(entry.get("start", "00:00:00"))
            # Convert absolute time to relative offset from segment start
            offset = abs_seconds - segment_start_seconds
            sentences.append(
                {
                    "id": i,
                    "offset": offset,
                    "text": entry.get("text", ""),
                }
            )
        except (json.JSONDecodeError, ValueError, IndexError):
            continue

    # Load embeddings
    npz_path = segment_dir / f"{source}.npz"
    emb_data = _load_embeddings_file(npz_path)

    if emb_data is not None:
        embeddings, statement_ids, _ = emb_data
        emb_map = {int(sid): True for sid in statement_ids}

        # Mark which sentences have embeddings
        for sentence in sentences:
            sentence["has_embedding"] = sentence["id"] in emb_map

    return sentences, emb_data


def _get_sentence_embedding(
    day: str, segment_key: str, source: str, sentence_id: int, stream: str | None = None
) -> np.ndarray | None:
    """Get a specific sentence's embedding, normalized."""
    if stream:
        segment_dir = get_segment_path(day, segment_key, stream, create=False)
    else:
        segment_dir = day_path(day) / segment_key
    npz_path = segment_dir / f"{source}.npz"

    emb_data = _load_embeddings_file(npz_path)
    if emb_data is None:
        return None

    embeddings, statement_ids, _ = emb_data

    # Find the embedding for this sentence
    for i, sid in enumerate(statement_ids):
        if int(sid) == sentence_id:
            return _normalize_embedding(embeddings[i])

    return None


@speakers_bp.route("/")
def index() -> Any:
    """Serve the speakers SPA shell."""
    return current_app.send_static_file("shell.html")


@speakers_bp.route("/<day>")
def speakers_day(day: str) -> Any:
    """Serve the speakers SPA shell for a specific day."""
    if not DATE_RE.fullmatch(day):
        return "", 404

    return current_app.send_static_file("shell.html")


@speakers_bp.route("/api/state")
def api_state() -> Any:
    """Return initial speakers workspace state."""
    try:
        speaker_filter = (request.args.get("speaker") or "").strip()
        speaker_filter_name = None
        if speaker_filter:
            entity = load_journal_entity(speaker_filter)
            if entity:
                speaker_filter_name = str(entity.get("name") or speaker_filter)
        return jsonify(
            {
                "today": date.today().strftime("%Y%m%d"),
                "owner_min_statements": OWNER_BOOTSTRAP_MIN_STMTS,
                "owner_status_routing_tokens": OWNER_STATUS_ROUTING_TOKENS,
                "not_in_new_voices_copy": TR_NOT_IN_NEW_VOICES,
                "speaker_copy": speaker_copy_payload(),
                "speaker_filter_name": speaker_filter_name,
            }
        )
    except Exception:
        logger.exception("error loading speakers state")
        return error_response(
            FILE_READ_FAILED,
            detail="Failed to load speaker state.",
        )


@speakers_bp.route("/api/people/search", methods=["GET"])
def api_people_search() -> Any:
    """Return read-only journal people matches for speaker discovery."""
    try:
        return jsonify(search_people_for_speakers(request.args.get("q") or ""))
    except Exception:
        logger.exception("error searching speakers people")
        return error_response(
            SPEAKER_COMMAND_FAILED,
            detail="Failed to search people.",
            status=500,
        )


def _speaker_segment_counts(month: str | None = None) -> dict[str, int]:
    stats: dict[str, int] = {}

    for day_name in day_dirs().keys():
        if month is not None and not day_name.startswith(month):
            continue

        segments = _scan_segment_embeddings(day_name)
        if segments:
            stats[day_name] = len(segments)

    return stats


def _coverage_from_counts(counts: dict[str, int]) -> dict[str, str] | None:
    first_day: str | None = None
    last_day: str | None = None
    for day, count in counts.items():
        if count <= 0:
            continue
        if first_day is None or day < first_day:
            first_day = day
        if last_day is None or day > last_day:
            last_day = day
    if first_day is None or last_day is None:
        return None
    return {"start": first_day, "end": last_day}


def _speaker_grid_counts() -> tuple[dict[str, int], dict[str, int]]:
    days: dict[str, int] = {}
    activity: dict[str, int] = {}

    for day_name in day_dirs().keys():
        activity_count = 0
        needs_review_count = 0

        for _stream, segment_key, segment_dir in iter_segments(day_name):
            parsed = segment_parse(segment_key)
            if parsed[0] is None:
                continue
            if not _audio_embedding_sources(segment_dir):
                continue

            activity_count += 1
            labels_data = _load_speaker_labels(segment_dir)
            if _segment_has_speaker_review(labels_data):
                needs_review_count += 1

        if activity_count > 0:
            activity[day_name] = activity_count
        if needs_review_count > 0:
            days[day_name] = needs_review_count

    return days, activity


@speakers_bp.route("/api/index")
def api_index() -> Any:
    """Return read-only whole-journal date navigation coverage."""
    return jsonify(build_date_nav_index(_speaker_segment_counts()))


@speakers_bp.route("/api/grid")
def api_grid() -> Any:
    """Return day-grid data for speaker review progress."""
    days, activity = _speaker_grid_counts()
    return jsonify(
        build_day_grid_payload(
            days,
            max(days, default=None),
            coverage=_coverage_from_counts(activity),
            activity=activity,
        )
    )


@speakers_bp.route("/api/quality")
def api_quality() -> Any:
    """Return bounded local speaker-quality counters for the overview."""
    return jsonify(get_speaker_quality_status())


@speakers_bp.route("/api/stats/<month>")
def api_stats(month: str) -> Any:
    """Return segment counts for each day in a month.

    Used by calendar heatmap to show days with embedding segments.
    """
    if not re.fullmatch(r"\d{6}", month):
        return error_response(
            INVALID_MONTH,
            detail="Invalid month format, expected YYYYMM",
        )

    stats = _speaker_segment_counts(month)
    return jsonify(stats)


@speakers_bp.route("/api/segments/<day>")
def api_segments(day: str) -> Any:
    """Return segments with audio embeddings for a day."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, detail="Invalid day format")

    try:
        limit = max(0, int(request.args.get("limit", 20)))
        offset = max(0, int(request.args.get("offset", 0)))
    except (ValueError, TypeError):
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="Invalid limit/offset parameter",
        )

    speaker_filter = request.args.get("speaker")
    if speaker_filter is not None:
        speaker_filter = speaker_filter.strip()
        if not speaker_filter:
            return error_response(
                INVALID_REQUEST_VALUE,
                detail="Invalid speaker parameter",
            )

    segments = _scan_segment_embeddings(day)
    segments.sort(key=lambda s: s["key"])
    if speaker_filter:
        segments = [
            seg
            for seg in segments
            if _segment_has_speaker(day, seg["stream"], seg["key"], speaker_filter)
        ]
    total = len(segments)
    segments = segments[offset : offset + limit]

    principal = get_journal_principal()
    principal_id = principal["id"] if principal else None
    for seg in segments:
        seg_dir = get_segment_path(day, seg["key"], seg["stream"], create=False)
        labels_data = _load_speaker_labels(seg_dir)
        if labels_data:
            labels = labels_data.get("labels", [])
            seg["attribution_total"] = len(labels)
            seg["attribution_needs_review"] = sum(
                1
                for label in labels
                if _speaker_sentence_needs_review(label, labels_data)
            )
            seg["attribution_null"] = sum(
                1 for label in labels if not label.get("speaker")
            )
            owner_count = sum(
                1
                for label in labels
                if label.get("speaker") and label.get("speaker") == principal_id
            )
            seg["attribution_non_owner_total"] = len(labels) - owner_count
        else:
            seg["attribution_total"] = 0
            seg["attribution_needs_review"] = 0
            seg["attribution_null"] = 0
            seg["attribution_non_owner_total"] = 0

    return jsonify({"segments": segments, "total": total})


@speakers_bp.route("/api/segments-cli/<day>")
def api_cli_segments(day: str) -> Any:
    """Return a bounded day segment list for CLI callers."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, detail="Invalid day format")

    try:
        limit = int(request.args.get("limit", 20))
    except (ValueError, TypeError):
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="Invalid limit parameter",
        )
    if limit < 1:
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="Limit must be at least 1",
        )

    segments = _scan_segment_embeddings(day)
    segments.sort(key=lambda s: s["key"])
    total = len(segments)
    return success_response(
        {
            "day": day,
            "segments": segments[:limit],
            "returned": min(limit, total),
            "limit": limit,
            "total": total,
        }
    )


def _segment_has_speaker(
    day: str, stream: str, segment_key: str, entity_id: str
) -> bool:
    """Return whether a segment has any label attributed to entity_id."""
    seg_dir = get_segment_path(day, segment_key, stream, create=False)
    labels_data = _load_speaker_labels(seg_dir)
    if not labels_data:
        return False
    return any(
        label.get("speaker") == entity_id for label in labels_data.get("labels", [])
    )


@speakers_bp.route("/api/speakers/known")
def api_speakers_known() -> Any:
    """Return known voice cards for the speakers overview."""
    sort = request.args.get("sort") or SPK_OVERVIEW_KNOWN_VOICES_SORTS[0]
    sort = sort.replace("_", " ")
    if sort not in SPK_OVERVIEW_KNOWN_VOICES_SORTS:
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="Invalid sort parameter",
        )

    speakers = list(get_speakers_status(section="speakers"))
    if sort == SPK_OVERVIEW_KNOWN_VOICES_SORTS[1]:
        speakers.sort(
            key=lambda item: (
                -int(item.get("embedding_count") or 0),
                str(item.get("name") or item.get("entity_id") or "").lower(),
                str(item.get("entity_id") or ""),
            )
        )
    elif sort == SPK_OVERVIEW_KNOWN_VOICES_SORTS[2]:
        speakers.sort(
            key=lambda item: (
                str(item.get("name") or item.get("entity_id") or "").lower(),
                str(item.get("entity_id") or ""),
            )
        )
    else:
        speakers.sort(
            key=lambda item: (
                item.get("last_seen_ts") is None,
                -(int(item.get("last_seen_ts") or 0)),
                str(item.get("name") or item.get("entity_id") or "").lower(),
                str(item.get("entity_id") or ""),
            )
        )

    return jsonify({"speakers": speakers, "total": len(speakers), "sort": sort})


@speakers_bp.route("/api/speakers/<day>/<stream>/<segment_key>")
def api_segment_speakers(day: str, stream: str, segment_key: str) -> Any:
    """Return speaker names with entity matching for a segment.

    Matches detected speaker names against all journal entities.
    """
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, detail="Invalid day format")

    if not validate_segment_key(segment_key):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Invalid segment key",
        )

    # Load speakers from speakers.json
    segment_dir = get_segment_path(day, segment_key, stream, create=False)
    speakers = _load_segment_speakers(segment_dir)
    if not speakers:
        return jsonify({"matched": [], "unmatched": []})

    # Load all journal entities for matching
    journal_entities = load_all_journal_entities()
    entities_list = [e for e in journal_entities.values() if not e.get("blocked")]

    # Match each speaker name to an entity
    matched = []
    unmatched = []

    for speaker_name in speakers:
        entity = find_matching_entity(speaker_name, entities_list)
        if entity:
            matched.append(
                {
                    "detected_name": speaker_name,
                    "entity_name": entity.get("name"),
                    "entity_type": entity.get("type"),
                }
            )
        else:
            unmatched.append(speaker_name)

    return jsonify(
        {
            "matched": matched,
            "unmatched": unmatched,
        }
    )


@speakers_bp.route("/api/review/<day>/<stream>/<segment_key>/<source>")
def api_review(day: str, stream: str, segment_key: str, source: str) -> Any:
    """Return sentences with pre-computed speaker labels for review."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, detail="Invalid day format")
    if not validate_segment_key(segment_key):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Invalid segment key",
        )

    sentences, emb_data = _load_sentences(day, segment_key, source, stream=stream)
    if not sentences:
        return error_response(
            SPEAKER_REVIEW_UNAVAILABLE,
            detail="No transcript found",
        )

    segment_dir = get_segment_path(day, segment_key, stream, create=False)
    labels_data = _load_speaker_labels(segment_dir)
    label_map: dict[int, dict] = {}
    if labels_data:
        for label in labels_data.get("labels", []):
            sid = label.get("sentence_id")
            if sid is not None:
                label_map[int(sid)] = label

    corrections = _load_speaker_corrections(segment_dir)
    correction_map: dict[int, dict] = {}
    for correction in corrections:
        sid = correction.get("sentence_id")
        if sid is not None:
            correction_map[int(sid)] = correction

    principal = get_journal_principal()
    principal_id = principal["id"] if principal else None
    entity_cache: dict[str, dict | None] = {}

    duration_map: dict[int, float] = {}
    if emb_data is not None:
        _, statement_ids, durations_s = emb_data
        for idx, sid in enumerate(statement_ids):
            duration_map[int(sid)] = (
                float(durations_s[idx])
                if durations_s is not None and idx < len(durations_s)
                else 0.0
            )

    review_sentences = [s for s in sentences if s.get("has_embedding")]
    for sentence in review_sentences:
        sentence["duration_s"] = duration_map.get(sentence["id"], 0.0)
    needs_review_count = 0
    corrections_count = 0

    for sentence in review_sentences:
        sid = sentence["id"]
        label = label_map.get(sid)
        if label:
            entity_id = label.get("speaker")
            confidence = label.get("confidence")
            method = label.get("method")
            if entity_id:
                info = _resolve_entity_display(entity_id, entity_cache, principal_id)
                sentence["speaker_entity_id"] = entity_id
                sentence["speaker_name"] = info["name"]
                sentence["is_owner"] = info["is_owner"]
            else:
                sentence["speaker_entity_id"] = None
                sentence["speaker_name"] = None
                sentence["is_owner"] = False

            sentence["confidence"] = confidence
            sentence["method"] = method
            sentence["needs_review"] = _speaker_sentence_needs_review(
                label, labels_data
            )
        else:
            sentence["speaker_entity_id"] = None
            sentence["speaker_name"] = None
            sentence["confidence"] = None
            sentence["method"] = None
            sentence["is_owner"] = False
            sentence["needs_review"] = _speaker_sentence_needs_review(None, labels_data)

        correction = correction_map.get(sid)
        sentence["is_correction"] = sentence.get("method") in {
            "user_corrected",
            "user_assigned",
        }
        if correction and sentence["is_correction"]:
            orig_speaker = correction.get("original_speaker")
            if orig_speaker:
                orig_info = _resolve_entity_display(
                    orig_speaker,
                    entity_cache,
                    principal_id,
                )
                sentence["original_speaker_entity_id"] = orig_speaker
                sentence["original_speaker_name"] = orig_info["name"]
            else:
                sentence["original_speaker_entity_id"] = None
                sentence["original_speaker_name"] = None
            corrections_count += 1
        else:
            sentence["original_speaker_entity_id"] = None
            sentence["original_speaker_name"] = None

        if sentence.get("needs_review"):
            needs_review_count += 1

    journal_entities = load_all_journal_entities()
    all_entities = []
    for eid, entity in journal_entities.items():
        if entity.get("blocked"):
            continue
        all_entities.append(
            {
                "entity_id": eid,
                "name": entity.get("name", eid),
                "is_principal": bool(entity.get("is_principal")),
            }
        )
    if not any(e.get("is_principal") for e in journal_entities.values()):
        identity = principal_identity_or_none()
        if identity is not None and identity[0] not in journal_entities:
            all_entities.append(
                {"entity_id": identity[0], "name": identity[1], "is_principal": True}
            )
    all_entities.sort(key=lambda x: (not x["is_principal"], x["name"].lower()))

    audio_file = None
    audio_mimetype = None
    audio_path = resolve_audio_file(segment_dir, source)
    if audio_path is not None:
        audio_file = audio_serve_url(day, stream, segment_key, audio_path.name)
        audio_mimetype = MIME_TYPES[audio_path.suffix]

    parsed = segment_parse(segment_key)
    start_time, end_time = parsed if parsed[0] else (None, None)

    return jsonify(
        {
            "segment": {
                "key": segment_key,
                "start": (
                    f"{start_time.hour:02d}:{start_time.minute:02d}"
                    if start_time
                    else ""
                ),
                "end": (
                    f"{end_time.hour:02d}:{end_time.minute:02d}" if end_time else ""
                ),
            },
            "source": source,
            "sentences": review_sentences,
            "all_entities": all_entities,
            "audio_file": audio_file,
            "audio_mimetype": audio_mimetype,
            "has_labels": labels_data is not None,
            "summary": {
                "total": len(review_sentences),
                "needs_review": needs_review_count,
                "corrections": corrections_count,
            },
        }
    )


@speakers_bp.route("/api/review-cli/<day>/<stream>/<segment_key>/<source>")
def api_cli_review(day: str, stream: str, segment_key: str, source: str) -> Any:
    """Return sentence rows for CLI callers without browser-only review payload."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, detail="Invalid day format")
    if not validate_segment_key(segment_key):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Invalid segment key",
        )
    if not STREAM_RE.fullmatch(stream):
        return error_response(INVALID_SEGMENT_OR_STREAM, detail="Invalid stream")

    sentences, _ = _load_sentences(day, segment_key, source, stream=stream)
    if not sentences:
        return error_response(
            SPEAKER_REVIEW_UNAVAILABLE,
            detail="No transcript found",
        )

    segment_dir = get_segment_path(day, segment_key, stream, create=False)
    labels_data = _load_speaker_labels(segment_dir)
    label_map: dict[int, dict] = {}
    if labels_data:
        for label in labels_data.get("labels", []):
            sid = label.get("sentence_id")
            if sid is not None:
                label_map[int(sid)] = label

    rows = []
    for sentence in sentences:
        sentence_id = int(sentence["id"])
        label = label_map.get(sentence_id)
        rows.append(
            {
                "sentence_id": sentence_id,
                "text": sentence.get("text", ""),
                "has_embedding": bool(sentence.get("has_embedding")),
                "speaker": label.get("speaker") if label else None,
                "confidence": label.get("confidence") if label else None,
                "method": label.get("method") if label else None,
                "needs_review": _speaker_sentence_needs_review(label, labels_data),
            }
        )

    return success_response(
        {
            "day": day,
            "stream": stream,
            "segment_key": segment_key,
            "source": source,
            "sentences": rows,
        }
    )


@speakers_bp.route("/api/confirm-attribution", methods=["POST"])
def api_confirm_attribution() -> Any:
    """Confirm a medium-confidence speaker attribution."""
    data = request.get_json()
    if not data:
        return error_response(MISSING_REQUEST_BODY, detail="No data provided")

    day = data.get("day")
    stream = data.get("stream")
    segment_key = data.get("segment_key")
    source = data.get("source")
    sentence_id = data.get("sentence_id")

    if not all([day, stream, segment_key, source, sentence_id is not None]):
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="Missing required fields",
        )
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, detail="Invalid day format")
    if not SEGMENT_KEY_RE.fullmatch(segment_key):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Invalid segment key",
        )
    if not STREAM_RE.fullmatch(stream):
        return error_response(INVALID_SEGMENT_OR_STREAM, detail="Invalid stream")

    principal_id = _principal_id_or_none()
    try:
        with trust_operation_lock():
            segment_dir = get_segment_path(day, segment_key, stream)
            labels_data = _load_speaker_labels(segment_dir)
            if not labels_data:
                return error_response(
                    SPEAKER_REVIEW_UNAVAILABLE,
                    detail="No speaker labels found",
                )

            label = None
            for item in labels_data.get("labels", []):
                if item.get("sentence_id") == sentence_id:
                    label = item
                    break

            if label is None:
                return error_response(
                    SPEAKER_SENTENCE_MISSING,
                    detail="Sentence not found in labels",
                )

            speaker = label.get("speaker")
            if not speaker:
                return error_response(
                    SPEAKER_ATTRIBUTION_STATE_INVALID,
                    detail="sentence has no speaker assignment yet",
                )

            target_entity = _ensure_attribution_target(str(speaker))
            if not target_entity:
                return error_response(
                    SPEAKER_NOT_FOUND,
                    detail=f"Entity '{speaker}' not found",
                )

            confidence = label.get("confidence")
            if confidence == "high" and label.get("method") == "user_confirmed":
                return success_response({"status": "already_confirmed"})
            if confidence != "medium":
                return error_response(
                    SPEAKER_ATTRIBUTION_STATE_INVALID,
                    detail="attribution is not medium confidence",
                )

            emb = _get_sentence_embedding(
                day,
                segment_key,
                source,
                sentence_id,
                stream=stream,
            )
            if emb is None:
                return error_response(
                    SPEAKER_SENTENCE_MISSING,
                    detail="Sentence embedding not found",
                )

            if speaker != principal_id and _check_owner_contamination(emb):
                return error_response(
                    SPEAKER_OWNER_VOICE_TOO_CLOSE,
                    detail="Embedding too similar to owner voice — cannot save",
                )

            try:
                _save_voiceprint(
                    speaker,
                    emb,
                    day,
                    segment_key,
                    source,
                    sentence_id,
                    stream=stream,
                )
            except LockTimeout as exc:
                return _voiceprint_busy_response(exc)

            old_method = label.get("method")
            try:
                apply_label_patches(
                    segment_dir,
                    {sentence_id: {"confidence": "high", "method": "user_confirmed"}},
                    allow_insert=False,
                )
            except LockTimeout as exc:
                return _labels_busy_response(exc)

            try:
                append_speaker_correction(
                    segment_dir,
                    {
                        "sentence_id": sentence_id,
                        "original_speaker": speaker,
                        "corrected_speaker": speaker,
                        "original_method": old_method,
                        "timestamp": now_ms(),
                    },
                )
            except LockTimeout as exc:
                return _labels_busy_response(exc)
    except LockTimeout as exc:
        return _labels_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)

    log_app_action(
        app="speakers",
        facet=None,
        action="attribution_confirm",
        params={
            "day": day,
            "stream": stream,
            "segment_key": segment_key,
            "source": source,
            "sentence_id": sentence_id,
            "speaker": speaker,
        },
    )
    _maybe_bootstrap_owner_from_attestation(principal_id, speaker)

    return success_response({"status": "confirmed", "speaker": speaker})


@speakers_bp.route("/api/correct-attribution", methods=["POST"])
def api_correct_attribution() -> Any:
    """Correct a speaker attribution to a different entity."""
    data = request.get_json()
    if not data:
        return error_response(MISSING_REQUEST_BODY, detail="No data provided")

    day = data.get("day")
    stream = data.get("stream")
    segment_key = data.get("segment_key")
    source = data.get("source")
    sentence_id = data.get("sentence_id")
    new_speaker = data.get("new_speaker")

    if not all(
        [day, stream, segment_key, source, sentence_id is not None, new_speaker]
    ):
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="Missing required fields",
        )
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, detail="Invalid day format")
    if not SEGMENT_KEY_RE.fullmatch(segment_key):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Invalid segment key",
        )
    if not STREAM_RE.fullmatch(stream):
        return error_response(INVALID_SEGMENT_OR_STREAM, detail="Invalid stream")

    principal_id = _principal_id_or_none()
    voiceprint_removal = VoiceprintRemovalResult(
        outcome="not_found",
        entity_id="",
        keys_removed=[],
        file_deleted=False,
        voiceprints_path=None,
    )
    try:
        with trust_operation_lock():
            target_entity = _ensure_attribution_target(new_speaker)
            if not target_entity:
                return error_response(
                    SPEAKER_NOT_FOUND,
                    detail=f"Entity '{new_speaker}' not found",
                )
            if target_entity.get("blocked"):
                return error_response(
                    ENTITY_BLOCKED,
                    detail=f"Entity '{new_speaker}' is blocked",
                )

            segment_dir = get_segment_path(day, segment_key, stream)
            labels_data = _load_speaker_labels(segment_dir)
            if not labels_data:
                return error_response(
                    SPEAKER_REVIEW_UNAVAILABLE,
                    detail="No speaker labels found",
                )

            label = None
            for item in labels_data.get("labels", []):
                if item.get("sentence_id") == sentence_id:
                    label = item
                    break

            if label is None:
                return error_response(
                    SPEAKER_SENTENCE_MISSING,
                    detail="Sentence not found in labels",
                )

            old_speaker = label.get("speaker")
            old_method = label.get("method")
            if old_speaker == new_speaker:
                return success_response({"status": "already_correct"})

            emb = _get_sentence_embedding(
                day,
                segment_key,
                source,
                sentence_id,
                stream=stream,
            )
            if emb is None:
                return error_response(
                    SPEAKER_SENTENCE_MISSING,
                    detail="Sentence embedding not found",
                )

            if new_speaker != principal_id and _check_owner_contamination(emb):
                return error_response(
                    SPEAKER_OWNER_VOICE_TOO_CLOSE,
                    detail="Embedding too similar to owner voice — cannot save",
                )

            try:
                if old_speaker:
                    voiceprint_removal = _remove_voiceprint(
                        old_speaker,
                        day,
                        segment_key,
                        source,
                        sentence_id,
                    )

                _save_voiceprint(
                    new_speaker,
                    emb,
                    day,
                    segment_key,
                    source,
                    sentence_id,
                    stream=stream,
                )
            except LockTimeout as exc:
                return _voiceprint_busy_response(exc)

            try:
                apply_label_patches(
                    segment_dir,
                    {
                        sentence_id: {
                            "speaker": new_speaker,
                            "confidence": "high",
                            "method": "user_corrected",
                        }
                    },
                    allow_insert=False,
                )
            except LockTimeout as exc:
                return _labels_busy_response(exc)

            try:
                append_speaker_correction(
                    segment_dir,
                    {
                        "sentence_id": sentence_id,
                        "original_speaker": old_speaker,
                        "corrected_speaker": new_speaker,
                        "original_method": old_method,
                        "timestamp": now_ms(),
                    },
                )
            except LockTimeout as exc:
                return _labels_busy_response(exc)
            if old_speaker is None:
                voiceprint_removal = VoiceprintRemovalResult(
                    outcome="not_found",
                    entity_id="",
                    keys_removed=[],
                    file_deleted=False,
                    voiceprints_path=None,
                )
            else:
                voiceprint_removal = VoiceprintRemovalResult(
                    outcome=voiceprint_removal.outcome,
                    entity_id=str(old_speaker),
                    keys_removed=voiceprint_removal.keys_removed,
                    file_deleted=voiceprint_removal.file_deleted,
                    voiceprints_path=voiceprint_removal.voiceprints_path,
                )
    except LockTimeout as exc:
        return _labels_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)

    log_app_action(
        app="speakers",
        facet=None,
        action="attribution_correct",
        params={
            "day": day,
            "stream": stream,
            "segment_key": segment_key,
            "source": source,
            "sentence_id": sentence_id,
            "old_speaker": old_speaker,
            "new_speaker": new_speaker,
            "voiceprint_removal": _voiceprint_removal_payload(voiceprint_removal),
        },
    )
    _maybe_bootstrap_owner_from_attestation(principal_id, new_speaker)
    propagation_offer = _propagation_offer(old_speaker, new_speaker)

    return success_response(
        {
            "status": "corrected",
            "old_speaker": old_speaker,
            "new_speaker": new_speaker,
            "voiceprint_removal": _voiceprint_removal_payload(voiceprint_removal),
            "propagation_offer": propagation_offer,
        }
    )


@speakers_bp.route("/api/propagate-correction", methods=["POST"])
def api_propagate_correction() -> Any:
    """Preview or apply scoped re-attribution after a correction."""
    data = request.get_json(silent=True) or {}
    old_speaker = data.get("old_speaker")
    new_speaker = data.get("new_speaker")
    commit = bool(data.get("commit", False))

    if not old_speaker or not new_speaker:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="Missing required fields",
        )
    old_speaker = str(old_speaker)
    new_speaker = str(new_speaker)
    if old_speaker == new_speaker:
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="Choose two different speakers.",
        )

    old_entity = _ensure_attribution_target(old_speaker)
    if not old_entity:
        return error_response(
            SPEAKER_NOT_FOUND,
            detail=f"Entity '{old_speaker}' not found",
        )
    if old_entity.get("blocked"):
        return error_response(
            ENTITY_BLOCKED,
            detail=f"Entity '{old_speaker}' is blocked",
        )

    new_entity = _ensure_attribution_target(new_speaker)
    if not new_entity:
        return error_response(
            SPEAKER_NOT_FOUND,
            detail=f"Entity '{new_speaker}' not found",
        )
    if new_entity.get("blocked"):
        return error_response(
            ENTITY_BLOCKED,
            detail=f"Entity '{new_speaker}' is blocked",
        )

    try:
        result = propagate_speaker_correction(
            old_speaker,
            new_speaker,
            commit=commit,
        )
    except LockTimeout as exc:
        if exc.path.name in ("speaker_labels.json", "speaker_corrections.json"):
            return _labels_busy_response(exc)
        return _voiceprint_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)

    if commit and result.get("statement_count"):
        log_app_action(
            app="speakers",
            facet=None,
            action="attribution_propagate_correction",
            params={
                "old_speaker": old_speaker,
                "new_speaker": new_speaker,
                "statement_count": result["statement_count"],
                "segment_count": result["segment_count"],
            },
        )

    return jsonify(_propagation_response_payload(result))


@speakers_bp.route("/api/assign-attribution", methods=["POST"])
def api_assign_attribution() -> Any:
    """Assign a speaker to an unattributed sentence."""
    data = request.get_json()
    if not data:
        return error_response(MISSING_REQUEST_BODY, detail="No data provided")

    day = data.get("day")
    stream = data.get("stream")
    segment_key = data.get("segment_key")
    source = data.get("source")
    sentence_id = data.get("sentence_id")
    speaker = data.get("speaker")
    return _assign_attribution_impl(
        day, stream, segment_key, source, sentence_id, speaker
    )


@speakers_bp.route("/api/owner/status")
def api_owner_status() -> Any:
    """Return the current owner voiceprint confirmation state."""
    voiceprint = get_current().get("voiceprint", {})
    status = voiceprint.get("status", "none")
    diagnostics = _owner_bootstrap_status_fields()

    if status == "confirmed":
        centroid = load_owner_centroid()
        manual_tag_stats = load_manual_tag_stats(_principal_id_or_none())
        metadata = {
            "cluster_size": centroid.cluster_size if centroid is not None else 0,
            "streams": centroid.streams if centroid is not None else [],
            "created_at": centroid.created_at if centroid is not None else None,
            "last_refreshed_at": (
                centroid.last_refreshed_at if centroid is not None else ""
            ),
            "threshold": centroid.threshold if centroid is not None else None,
            "margin": centroid.margin if centroid is not None else None,
            "intra_cosine_p25": (
                centroid.intra_cosine_p25 if centroid is not None else None
            ),
            "evidence_hash": centroid.evidence_hash if centroid is not None else None,
            "evidence_intra_cosine_p25": (
                centroid.evidence_intra_cosine_p25 if centroid is not None else None
            ),
            "evidence_tier": centroid.evidence_tier if centroid is not None else None,
        }
        return jsonify(
            {
                "status": OWNER_STATUS_CONFIRMED,
                "centroid_metadata": metadata,
                "manual_tags_count": manual_tag_stats["manual_tags_count"],
            }
        )

    if status == "candidate":
        return jsonify(
            {
                "status": OWNER_STATUS_CANDIDATE,
                "cluster_size": voiceprint.get("cluster_size"),
                "samples": voiceprint.get("samples", []),
                "evidence_tier": voiceprint.get("evidence_tier"),
            }
        )

    if status == "low_quality":
        guidance = load_owner_manual_bootstrap_guidance(_principal_id_or_none())
        return jsonify(
            {
                "status": "low_quality",
                "source": voiceprint.get("source", "candidate_pool"),
                "low_quality_reason": voiceprint.get("low_quality_reason", ""),
                "observed_value": voiceprint.get("observed_value", 0.0),
                "threshold_value": voiceprint.get("threshold_value", 0.0),
                "evidence_tier": voiceprint.get("evidence_tier"),
                "intra_cosine_p25_bound": voiceprint.get("intra_cosine_p25_bound"),
                **diagnostics,
                "next_step": guidance["next_step"],
                "guidance": guidance["guidance"],
            }
        )

    if status == "no_cluster":
        guidance = load_owner_manual_bootstrap_guidance(_principal_id_or_none())
        return jsonify(
            {
                "status": "no_cluster",
                **diagnostics,
                "next_step": guidance["next_step"],
                "guidance": guidance["guidance"],
            }
        )

    if status in {"none", "rejected"}:
        cooldown = owner_rejection_cooldown_payload(voiceprint)
        if cooldown is not None:
            return jsonify(
                {
                    "status": "none",
                    **diagnostics,
                    **cooldown,
                    "next_step": "wait_for_cooldown",
                    "guidance": OWNER_REJECTION_COOLDOWN_GUIDANCE,
                }
            )
        if diagnostics["segments_available"] > 0:
            return jsonify(
                {
                    "status": "needs_detection",
                    **diagnostics,
                    "next_step": "detect_candidate",
                    "guidance": OWNER_DETECT_CANDIDATE_GUIDANCE,
                }
            )
        guidance = load_owner_manual_bootstrap_guidance(_principal_id_or_none())
        return jsonify(
            {
                "status": "none",
                **diagnostics,
                "next_step": guidance["next_step"],
                "guidance": guidance["guidance"],
            }
        )

    guidance = load_owner_manual_bootstrap_guidance(_principal_id_or_none())
    return jsonify(
        {
            "status": "none",
            **diagnostics,
            "next_step": guidance["next_step"],
            "guidance": guidance["guidance"],
        }
    )


@speakers_bp.route("/api/owner/detect", methods=["POST"])
def api_owner_detect() -> Any:
    """Run owner voice candidate detection."""
    body = request.get_json(silent=True) or {}
    force = bool(body.get("force") is True) if isinstance(body, dict) else False
    try:
        result = detect_owner_candidate(force=force)
    except LockTimeout as exc:
        return _voiceprint_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)
    if result.get("error_kind") == "voiceprint_busy":
        return error_response(SPEAKER_VOICEPRINT_BUSY, detail=result["error"])
    return jsonify(result)


@speakers_bp.route("/api/owner/build-from-tags", methods=["POST"])
def api_owner_build_from_tags() -> Any:
    """Build a confirmed owner centroid directly from validated manual tags."""
    try:
        result = bootstrap_owner_from_manual_tags()
    except LockTimeout as exc:
        return _voiceprint_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)
    if result.get("error_kind") == "voiceprint_busy":
        return error_response(SPEAKER_VOICEPRINT_BUSY, detail=result["error"])
    if "error" in result:
        return error_response(ENTITY_NOT_FOUND, detail=result["error"], status=400)
    if result.get("status") == "confirmed":
        log_app_action(
            app="speakers",
            facet=None,
            action="owner_voiceprint_build_from_tags",
            params={
                "principal_id": result["principal_id"],
                "cluster_size": result.get("cluster_size"),
            },
        )
    return jsonify(result)


@speakers_bp.route("/api/owner/rebuild", methods=["POST"])
def api_owner_rebuild() -> Any:
    """Rebuild the confirmed owner centroid from current manual-tag evidence."""
    body = request.get_json(silent=True) or {}
    override = bool(body.get("override") is True) if isinstance(body, dict) else False
    try:
        result = rebuild_owner_centroid(override=override)
    except LockTimeout as exc:
        return _voiceprint_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)
    if result.get("error_kind") == "voiceprint_busy":
        return error_response(SPEAKER_VOICEPRINT_BUSY, detail=result["error"])
    if result.get("status") == "rebuilt":
        log_app_action(
            app="speakers",
            facet=None,
            action="owner_voiceprint_rebuild",
            params={
                "principal_id": result["principal_id"],
                "cluster_size": result.get("cluster_size"),
                "override": bool(result.get("override_applied")),
            },
        )
    return jsonify(result)


@speakers_bp.route("/api/owner/tag-cli", methods=["POST"])
def api_cli_owner_tag() -> Any:
    """Tag one sentence as the configured owner voice through the shared assign path."""
    data = request.get_json(silent=True)
    if not data:
        return error_response(MISSING_REQUEST_BODY, detail="No data provided")

    principal_id = _principal_id_or_none()
    if principal_id is None:
        identity = principal_identity_or_none()
        if identity is None:
            return error_response(
                SPEAKER_OWNER_IDENTITY_REQUIRED,
                detail="Set your journal identity before tagging your voice.",
            )
        principal_id = identity[0]

    return _assign_attribution_impl(
        data.get("day"),
        data.get("stream"),
        data.get("segment_key"),
        data.get("source"),
        data.get("sentence_id"),
        principal_id,
    )


@speakers_bp.route("/api/owner/confirm", methods=["POST"])
def api_owner_confirm() -> Any:
    """Confirm the current owner voice candidate and persist the centroid."""
    try:
        result = confirm_owner_candidate()
    except LockTimeout as exc:
        return _voiceprint_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)
    if result.get("error_kind") == "voiceprint_busy":
        return error_response(SPEAKER_VOICEPRINT_BUSY, detail=result["error"])
    if "error" in result:
        code = 404 if "No candidate" in result["error"] else 400
        reason = SPEAKER_REVIEW_UNAVAILABLE if code == 404 else ENTITY_NOT_FOUND
        return error_response(reason, detail=result["error"], status=code)

    log_app_action(
        app="speakers",
        facet=None,
        action="owner_voiceprint_confirm",
        params={
            "principal_id": result["principal_id"],
            "cluster_size": result["cluster_size"],
        },
    )

    return jsonify({"status": "confirmed", "principal_id": result["principal_id"]})


@speakers_bp.route("/api/owner/reject", methods=["POST"])
def api_owner_reject() -> Any:
    """Reject the current owner voice candidate."""
    try:
        reject_owner_candidate()
    except LockTimeout as exc:
        return _voiceprint_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)
    return jsonify({"status": "needs_detection"})


@speakers_bp.route("/api/owner/classify", methods=["POST"])
def api_owner_classify() -> Any:
    """Classify segment sentences against the confirmed owner centroid."""
    data = request.get_json()
    if not data:
        return error_response(MISSING_REQUEST_BODY, detail="No data provided")

    day = data.get("day")
    stream = data.get("stream")
    segment_key = data.get("segment_key")
    source = data.get("source")

    if not all([day, stream, segment_key, source]):
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="Missing required fields",
        )
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, detail="Invalid day format")
    if not validate_segment_key(segment_key):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Invalid segment key",
        )

    return jsonify(
        {
            "sentences": classify_sentences(day, stream, segment_key, source),
        }
    )


@speakers_bp.route("/api/discovery/scan", methods=["POST"])
def api_discovery_scan() -> Any:
    """Scan for recurring unknown speaker clusters."""
    try:
        result = discover_unknown_speakers()
    except SpeakerDiscoveryKernelError as exc:
        status, retryable = discovery_kernel_failure_http_result(exc.stage)
        return error_response(
            SPEAKER_DISCOVERY_FAILED,
            status=status,
            detail="",
            extra={"retryable": retryable},
        )
    return jsonify(result)


@speakers_bp.route("/api/discovery/cache", methods=["GET"])
def api_discovery_cache() -> Any:
    """Return visible cached discovery clusters without scanning."""
    return jsonify(read_discovery_cache_snapshot())


@speakers_bp.route("/api/discovery/cluster/<int:cluster_id>/presence", methods=["GET"])
def api_cluster_presence(cluster_id: int) -> Any:
    """Return read-only presence evidence for a discovery cluster."""
    presence = get_cluster_presence(cluster_id)
    if presence is None:
        return error_response(
            SPEAKER_REVIEW_UNAVAILABLE,
            detail=f"Cluster {cluster_id} was not found. Run a discovery scan first.",
        )
    return jsonify(presence)


@speakers_bp.route("/api/discovery/resolve-statement", methods=["GET"])
def api_resolve_statement_cluster() -> Any:
    """Resolve one statement provenance tuple to a discovery cluster."""
    required_params = (
        "voice_day",
        "voice_stream",
        "voice_segment_key",
        "voice_source",
        "voice_sentence_id",
    )
    values = {name: request.args.get(name) for name in required_params}
    missing = [name for name, value in values.items() if value in (None, "")]
    if missing:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail=f"Missing required fields: {', '.join(missing)}",
        )
    try:
        sentence_id = int(values["voice_sentence_id"] or "")
    except ValueError:
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="voice_sentence_id must be an integer",
        )

    return jsonify(
        resolve_statement_cluster(
            day=values["voice_day"] or "",
            stream=values["voice_stream"] or "",
            segment_key=values["voice_segment_key"] or "",
            source=values["voice_source"] or "",
            sentence_id=sentence_id,
        )
    )


def _operation_result_summary(result: dict[str, Any] | None) -> dict[str, Any] | None:
    if not isinstance(result, dict):
        return None
    return {
        key: result[key]
        for key in (
            "status",
            "operation_id",
            "operation_state",
            "entity_id",
            "entity_created",
            "voiceprints_saved",
            "retro_voiceprints_saved",
            "segments_updated",
            "sentences_attributed",
            "corrections_appended",
            "keep_separate_assertions_recorded",
        )
        if key in result
    }


def _undo_report_summary(report: dict[str, Any] | None) -> dict[str, Any] | None:
    if not isinstance(report, dict):
        return None
    summary = {
        "status": report.get("status"),
        "operation_id": report.get("operation_id"),
    }
    categories = report.get("undo_report")
    if isinstance(categories, dict):
        summary["undo_report"] = {
            category: data
            for category, data in categories.items()
            if isinstance(data, dict)
        }
    return summary


def _repair_summary(repair: dict[str, Any] | None) -> dict[str, Any] | None:
    if not isinstance(repair, dict):
        return None
    return {
        key: repair[key]
        for key in (
            "phase",
            "repair_code",
            "repair_categories",
            "partial_report",
            "undo_report",
        )
        if key in repair
    }


def _identify_operation_summary(state: Any) -> dict[str, Any]:
    return {
        "operation_id": state.operation_id,
        "request_id": state.request_id,
        "status": state.terminal_status,
        "target_entity_id": state.target_entity_id,
        "will_create": state.will_create,
        "entity_type": state.entity_type,
        "reviewed_near_match_entity_ids": list(state.reviewed_near_match_entity_ids),
        "cluster_member_count": len(state.cluster_member_set),
        "completed_phases": list(state.completed_phases),
        "pending_phases": list(state.pending_phases),
        "checkpoints": {
            "forward": list(state.phase_checkpoints.keys()),
            "undo": list(state.undo_phase_checkpoints.keys()),
        },
        "result": _operation_result_summary(state.result),
        "undo_report": _undo_report_summary(state.undo_report),
        "repair": _repair_summary(state.repair_required)
        or _repair_summary(state.undo_repair_required),
    }


def _operation_failure_extra(result: dict[str, Any]) -> dict[str, Any]:
    extra: dict[str, Any] = {
        "status": result.get("status"),
        "operation_id": result.get("operation_id"),
    }
    if "operation_state" in result:
        extra["operation_state"] = result.get("operation_state")
    for key in (
        "request_id",
        "completed_phases",
        "pending_phases",
        "repair_categories",
        "repair_code",
        "phase",
        "conflict_code",
        "conflicting_operation_id",
        "list_command",
        "undo_report",
    ):
        if key in result:
            extra[key] = result[key]
    return extra


def _identify_result_response(result: dict) -> Any:
    status = result.get("status")
    if status in {
        "identified",
        "resolved",
        "ambiguous",
        "no_match",
        "principal_match",
        "undone",
        "already_undone",
    }:
        return jsonify(result)
    if status in {"recoverable", "in_progress", "undoing"}:
        return error_response(
            SPEAKER_IDENTIFY_RECOVERABLE,
            detail=result.get("detail"),
            extra=_operation_failure_extra(result),
        )
    if status in {"repair_required", "undo_repair_required"}:
        return error_response(
            SPEAKER_IDENTIFY_REPAIR_REQUIRED,
            detail=result.get("repair_code") or result.get("detail"),
            extra=_operation_failure_extra(result),
        )
    if status in {"conflict", "operation_already_undone"}:
        return error_response(
            SPEAKER_IDENTIFY_CONFLICT,
            detail=result.get("conflict_code") or status,
            extra=_operation_failure_extra(result),
        )
    if status == "not_found":
        return error_response(
            SPEAKER_IDENTIFY_OPERATION_NOT_FOUND,
            detail=f"Operation {result.get('operation_id')} was not found.",
            extra=_operation_failure_extra(result),
        )
    if result.get("not_found"):
        return error_response(SPEAKER_NOT_FOUND, detail=result["error"])
    if result.get("invalid_entity_type"):
        return error_response(INVALID_ENTITY_TYPE, detail=result["error"])
    if status == "invalid_request":
        return error_response(
            INVALID_REQUEST_VALUE,
            detail=result.get("error"),
            extra={
                key: result[key]
                for key in (
                    "invalid_request_code",
                    "invalid_reviewed_near_match_entity_ids",
                )
                if key in result
            },
        )
    if "error" in result:
        return error_response(
            INVALID_REQUEST_VALUE,
            detail=result["error"],
            status=400,
        )
    return error_response(
        SPEAKER_COMMAND_FAILED,
        detail=f"Unexpected speaker identify result status: {status or 'missing'}",
        status=500,
        extra=_operation_failure_extra(result),
    )


def _optional_request_id(data: dict[str, Any]) -> tuple[str | None, Any | None]:
    request_id_value = data.get("request_id")
    if request_id_value is None:
        return None, None
    if not isinstance(request_id_value, str) or not request_id_value.strip():
        return None, error_response(
            INVALID_REQUEST_VALUE,
            detail="request_id must be a non-empty string",
        )
    return request_id_value.strip(), None


@speakers_bp.route("/api/discovery/identify", methods=["POST"])
def api_discovery_identify() -> Any:
    """Identify a discovered unknown speaker cluster by naming it."""
    data = request.get_json(silent=True) or {}
    cluster_id = data.get("cluster_id")
    name_value = data.get("name")
    entity_id_value = data.get("entity_id")
    name = str(name_value).strip() if name_value is not None else ""
    entity_id = str(entity_id_value).strip() if entity_id_value else ""
    resolve_only = bool(data.get("resolve_only", False))
    create_new = bool(data.get("create_new", False))
    entity_type = data.get("entity_type") or "Person"
    request_id, request_id_error = _optional_request_id(data)
    reviewed_near_match_entity_ids = data.get("reviewed_near_match_entity_ids")

    if cluster_id is None:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="cluster_id is required",
        )
    if not entity_id and not name:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="entity_id or name is required",
        )

    try:
        cluster_id = int(cluster_id)
    except (TypeError, ValueError):
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="cluster_id must be an integer",
        )
    if request_id_error is not None:
        return request_id_error

    try:
        result = identify_cluster(
            cluster_id,
            name=name or None,
            entity_id=entity_id or None,
            resolve_only=resolve_only,
            create_new=create_new,
            entity_type=entity_type,
            request_id=request_id,
            reviewed_near_match_entity_ids=reviewed_near_match_entity_ids,
        )
    except LockTimeout as exc:
        if exc.path.name in ("speaker_labels.json", "speaker_corrections.json"):
            return _labels_busy_response(exc)
        return _voiceprint_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)

    if result.get("status") == "identified":
        log_app_action(
            app="speakers",
            facet=None,
            action="speaker_identified",
            params={
                "entity_id": result.get("entity_id"),
                "entity_name": result.get("entity_name"),
                "cluster_id": cluster_id,
                "voiceprints_saved": result.get("voiceprints_saved"),
                "segments_updated": result.get("segments_updated"),
            },
        )

    return _identify_result_response(result)


# CLI-backing routes for sol call speakers HTTP cutover.
@speakers_bp.route("/api/status", methods=["GET"])
def api_cli_status() -> Any:
    """Return the full speakers status payload for CLI-side section selection."""
    return jsonify(get_speakers_status(None))


@speakers_bp.route("/api/bootstrap", methods=["POST"])
def api_cli_bootstrap() -> Any:
    """Bootstrap voiceprints for the CLI."""
    data = request.get_json(silent=True) or {}
    commit = bool(data.get("commit", False))
    stats = bootstrap_voiceprints(dry_run=not commit)
    if "error" in stats:
        return error_response(
            SPEAKER_OWNER_CENTROID_REQUIRED,
            detail=stats["error"],
        )
    return jsonify(stats)


@speakers_bp.route("/api/resolve-names", methods=["POST"])
def api_cli_resolve_names() -> Any:
    """Resolve speaker name variants for the CLI."""
    data = request.get_json(silent=True) or {}
    commit = bool(data.get("commit", False))
    return jsonify(resolve_name_variants(dry_run=not commit))


@speakers_bp.route("/api/seed-from-imports", methods=["POST"])
def api_cli_seed_from_imports() -> Any:
    """Seed voiceprints from imports for the CLI."""
    data = request.get_json(silent=True) or {}
    commit = bool(data.get("commit", False))
    stats = seed_from_imports(dry_run=not commit)
    if "error" in stats:
        return error_response(
            SPEAKER_OWNER_CENTROID_REQUIRED,
            detail=stats["error"],
        )
    return jsonify(stats)


@speakers_bp.route("/api/backfill-last-seen", methods=["POST"])
def api_cli_backfill_last_seen() -> Any:
    """Backfill speaker last-seen metadata for the CLI."""
    data = request.get_json(silent=True) or {}
    commit = bool(data.get("commit", False))
    return jsonify(backfill_last_seen(dry_run=not commit))


@speakers_bp.route("/api/backfill", methods=["POST"])
def api_cli_backfill() -> Any:
    """Backfill speaker labels synchronously for the CLI."""
    data = request.get_json(silent=True) or {}
    commit = bool(data.get("commit", False))
    reattribute = bool(data.get("reattribute", False))
    kwargs: dict[str, Any] = {
        "dry_run": not commit,
        "progress_callback": None,
    }
    if reattribute:
        kwargs["reattribute"] = True
    return jsonify(backfill_segments(**kwargs))


@speakers_bp.route("/api/wipe", methods=["POST"])
def api_cli_wipe() -> Any:
    """Wipe speaker artifacts for the CLI."""
    data = request.get_json(silent=True) or {}
    commit = bool(data.get("commit", False))
    report = native_speakers.wipe_speaker_artifacts(get_journal(), dry_run=not commit)
    return jsonify(report)


@speakers_bp.route("/api/attribute-segment", methods=["POST"])
def api_cli_attribute_segment() -> Any:
    """Attribute one segment and optionally persist CLI-requested artifacts."""
    data = request.get_json(silent=True) or {}
    day = data.get("day")
    stream = data.get("stream")
    segment = data.get("segment")
    commit = bool(data.get("commit", False))
    save = bool(data.get("save", True))
    accumulate = bool(data.get("accumulate", True))

    if not all([day, stream, segment]):
        return error_response(MISSING_REQUIRED_FIELD, detail="Missing required fields")
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, detail="Invalid day format")
    if not SEGMENT_KEY_RE.fullmatch(segment):
        return error_response(
            INVALID_SEGMENT_OR_STREAM,
            detail="Invalid segment key",
        )
    if not STREAM_RE.fullmatch(stream):
        return error_response(INVALID_SEGMENT_OR_STREAM, detail="Invalid stream")

    result = attribute_segment(day, stream, segment)
    if result.get("error"):
        return error_response(
            SPEAKER_OWNER_CENTROID_REQUIRED,
            detail=result["error"],
        )

    labels = result.get("labels", [])
    metadata = result.get("metadata", {})
    source = result.get("source")
    written_path = None
    accumulated = None

    if commit and save:
        try:
            out_path = save_speaker_labels(
                get_segment_path(day, segment, stream),
                labels,
                metadata,
            )
        except LockTimeout as exc:
            return _labels_busy_response(exc)
        except native_speakers.NativeSpeakerResolveError as exc:
            return _native_storage_busy_response(exc)
        written_path = str(out_path)

    if commit and accumulate and source:
        try:
            accumulated = accumulate_voiceprints(day, stream, segment, labels, source)
        except LockTimeout as exc:
            return _voiceprint_busy_response(exc)
        except native_speakers.NativeSpeakerResolveError as exc:
            return _native_storage_busy_response(exc)

    return jsonify(
        {
            "result": result,
            "written_path": written_path,
            "accumulated": accumulated,
        }
    )


@speakers_bp.route("/api/suggest", methods=["GET"])
def api_cli_suggest() -> Any:
    """Return speaker suggestion items plus server-rendered markdown."""
    try:
        limit = int(request.args.get("limit", 5))
    except (TypeError, ValueError):
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="Invalid limit parameter",
        )
    return jsonify(suggest_opportunities(limit=limit))


@speakers_bp.route("/api/discovery/identify-cli", methods=["POST"])
def api_cli_discovery_identify() -> Any:
    """Identify a discovery cluster with CLI-compatible pass-through behavior."""
    data = request.get_json(silent=True) or {}
    cluster_id = data.get("cluster_id")
    name_value = data.get("name")
    entity_id_value = data.get("entity_id")
    name = str(name_value).strip() if name_value is not None else ""
    entity_id = str(entity_id_value).strip() if entity_id_value else ""
    resolve_only = bool(data.get("resolve_only", False))
    create_new = bool(data.get("create_new", False))
    entity_type = data.get("entity_type") or "Person"
    request_id, request_id_error = _optional_request_id(data)
    reviewed_near_match_entity_ids = data.get("reviewed_near_match_entity_ids")

    if cluster_id is None:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="cluster_id is required",
        )
    if not entity_id and not name:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="entity_id or name is required",
        )

    try:
        cluster_id = int(cluster_id)
    except (TypeError, ValueError):
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="cluster_id must be an integer",
        )
    if request_id_error is not None:
        return request_id_error

    try:
        result = identify_cluster(
            cluster_id,
            name=name or None,
            entity_id=entity_id or None,
            resolve_only=resolve_only,
            create_new=create_new,
            entity_type=entity_type,
            request_id=request_id,
            reviewed_near_match_entity_ids=reviewed_near_match_entity_ids,
        )
    except LockTimeout as exc:
        if exc.path.name in ("speaker_labels.json", "speaker_corrections.json"):
            return _labels_busy_response(exc)
        return _voiceprint_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)

    return _identify_result_response(result)


@speakers_bp.route("/api/discovery/identify/undo", methods=["POST"])
def api_discovery_identify_undo() -> Any:
    """Undo a committed discovery-cluster identify operation."""
    data = request.get_json(silent=True) or {}
    operation_id_value = data.get("operation_id")
    if not isinstance(operation_id_value, str) or not operation_id_value.strip():
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="operation_id is required",
        )

    try:
        result = undo_identify_operation(operation_id_value.strip())
    except LockTimeout as exc:
        if exc.path.name in ("speaker_labels.json", "speaker_corrections.json"):
            return _labels_busy_response(exc)
        return _voiceprint_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)
    return _identify_result_response(result)


@speakers_bp.route("/api/discovery/identify/operations", methods=["GET"])
def api_discovery_identify_operations() -> Any:
    """Return redacted identify operation summaries."""
    operations = [_identify_operation_summary(state) for state in fold_all_operations()]
    return jsonify({"operations": operations, "total": len(operations)})


@speakers_bp.route(
    "/api/discovery/identify/operations/<operation_id>",
    methods=["GET"],
)
def api_discovery_identify_operation(operation_id: str) -> Any:
    """Return one redacted identify operation summary."""
    state = fold_operation(operation_id)
    if state is None:
        return error_response(
            SPEAKER_IDENTIFY_OPERATION_NOT_FOUND,
            detail=f"Operation {operation_id} was not found.",
            extra={
                "status": "not_found",
                "operation_id": operation_id,
                "list_command": "sol call speakers identify-operations",
            },
        )
    return jsonify({"operation": _identify_operation_summary(state)})


@speakers_bp.route("/api/discovery/dismiss", methods=["POST"])
def api_discovery_dismiss() -> Any:
    """Record a read-side suppression dismissal for the current cluster members."""
    data = request.get_json(silent=True) or {}
    cluster_id = data.get("cluster_id")
    disposition = data.get("disposition")
    if cluster_id is None or not disposition:
        return error_response(
            MISSING_REQUIRED_FIELD,
            detail="cluster_id and disposition are required",
        )
    try:
        cluster_id = int(cluster_id)
    except (TypeError, ValueError):
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="cluster_id must be an integer",
        )
    cache = load_discovery_cache()
    members = cache.get("clusters", {}).get(str(cluster_id)) if cache else None
    if not members:
        return error_response(
            SPEAKER_REVIEW_UNAVAILABLE,
            detail=f"Cluster {cluster_id} was not found. Run a discovery scan first.",
        )
    try:
        event = record_cluster_dismissal(members, str(disposition))
    except ValueError as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    except LockTimeout as exc:
        return error_response(
            SPEAKER_COMMAND_FAILED,
            detail=str(exc),
            status=503,
        )
    return jsonify(
        {
            "status": "dismissed",
            "dismiss_event_id": event["dismiss_event_id"],
            "disposition": event["disposition"],
            "member_count": event["member_count"],
        }
    )


@speakers_bp.route("/api/discovery/dismissals", methods=["GET"])
def api_discovery_dismissals() -> Any:
    """Return folded cluster dismissal summaries."""
    dismissals = list_dismissals()
    return jsonify({"dismissals": dismissals, "total": len(dismissals)})


@speakers_bp.route("/api/name-variants/keep-separate", methods=["GET"])
def api_name_variants_keep_separate() -> Any:
    """Return folded keep-separate assertion summaries."""
    assertions = list_assertions()
    return jsonify({"assertions": assertions, "total": len(assertions)})


@speakers_bp.route("/api/merge-names", methods=["POST"])
def api_cli_merge_names() -> Any:
    """Merge two speaker names for the CLI."""
    data = request.get_json(silent=True) or {}
    alias = data.get("alias")
    canonical = data.get("canonical")
    result = merge_names(alias, canonical)
    if "error" in result:
        return error_response(
            SPEAKER_COMMAND_FAILED,
            detail=json.dumps(result, indent=2, default=str),
            status=400,
        )
    return jsonify(result)


@speakers_bp.route("/api/link-import", methods=["POST"])
def api_cli_link_import() -> Any:
    """Link an imported speaker name to an entity for the CLI."""
    data = request.get_json(silent=True) or {}
    name = data.get("name")
    entity_id = data.get("entity_id")
    result = link_import(name, entity_id)
    if "error" in result:
        return error_response(
            SPEAKER_COMMAND_FAILED,
            detail=json.dumps(result, indent=2, default=str),
            status=400,
        )
    return jsonify(result)


@speakers_bp.route("/api/owner/confirm-cli", methods=["POST"])
def api_cli_owner_confirm() -> Any:
    """Confirm the owner candidate with CLI-compatible full-result behavior."""
    try:
        result = confirm_owner_candidate()
    except LockTimeout as exc:
        return _voiceprint_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)
    if result.get("error_kind") == "voiceprint_busy":
        return error_response(SPEAKER_VOICEPRINT_BUSY, detail=result["error"])
    if "error" in result:
        return error_response(
            SPEAKER_COMMAND_FAILED,
            detail=json.dumps(result, indent=2, default=str),
            status=400,
        )
    return jsonify(result)


@speakers_bp.route("/api/owner/reject-cli", methods=["POST"])
def api_cli_owner_reject() -> Any:
    """Reject the owner candidate with CLI-compatible domain result behavior."""
    try:
        result = reject_owner_candidate()
    except LockTimeout as exc:
        return _voiceprint_busy_response(exc)
    except native_speakers.NativeSpeakerResolveError as exc:
        return _native_storage_busy_response(exc)
    return jsonify(result)


@speakers_bp.route("/api/owner/ready", methods=["POST"])
def api_cli_owner_ready() -> Any:
    """Return cheap owner-detection readiness without running detection."""
    return jsonify(owner_detection_ready())


@speakers_bp.route("/api/serve_audio/<day>/<path:rel_path>")
def serve_audio(day: str, rel_path: str) -> Any:
    """Serve audio files for playback."""
    if not DATE_RE.fullmatch(day):
        return error_response(INVALID_DAY, detail="Day not found", status=404)
    path, error = safe_day_path(day, rel_path)
    if error is not None:
        return error
    if not path.is_file():
        return error_response(FILE_NOT_FOUND, detail="File not found")
    mimetype = MIME_TYPES.get(path.suffix.lower())
    if mimetype is None:
        raise ValueError(f"unregistered media extension for serve_audio: {path.suffix}")
    return send_file(path, conditional=True, mimetype=mimetype)
