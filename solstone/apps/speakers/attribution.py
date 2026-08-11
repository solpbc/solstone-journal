# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Speaker attribution engine — 4-layer per-segment pipeline.

Runs per-segment after transcription and embedding.  Operates in layers
from cheapest to most expensive:

Layer 1: Owner separation (cosine similarity to owner centroid passes Layer 1)
Layer 2: Structural heuristics (speaker count, setting field, screen.json,
         meetings.md) — no LLM
Layer 3: Acoustic matching (voiceprint cosine similarity, same-stream
         preference) — no LLM
Layer 4: Contextual identification (LLM) — handled externally via talent hook

High-confidence attributions from Layers 2-3 automatically accumulate
into entity voiceprints, creating a learning flywheel.
"""

from __future__ import annotations

import json
import logging
import math
import re
import time
from collections import defaultdict
from collections.abc import Callable
from pathlib import Path
from typing import TYPE_CHECKING, Any

from solstone.apps.speakers import speaker_resolve_transport as native_speakers
from solstone.apps.speakers.evidence import _read_segment_overlap_fraction
from solstone.apps.speakers.encoder_config import (
    ACOUSTIC_HIGH,
    ACOUSTIC_MARGIN_MIN,
    ACOUSTIC_MEDIUM,
    CC_CONFIDENCE_GATE,
    CC_COVERAGE_GATE,
    ENCODER_ID,
    NOISY_FLYWHEEL_OVERLAP_MAX,
    VP_DECAY_LAMBDA,
    VP_OUTLIER_MIN_SAMPLES,
    VP_OUTLIER_MIN_SIMILARITY,
    WESPEAKER_EMBEDDING_WIDTH,
    WESPEAKER_MODEL_SHA256,
)
from solstone.think.entities import (
    EntityResolutionError,
    EntityResolutionOutcome,
    ResolutionOrigin,
    ResolutionScope,
    get_identity_names,
    load_entity_voiceprints_file,
    normalize_embedding,
    record_entity_resolution,
)
from solstone.think.entities.voiceprints import (
    load_embeddings_file,
    load_owner_centroid,
)
from solstone.think.entities.journal import (
    get_journal_principal,
    load_all_journal_entities,
)
from solstone.think.journal_io import (
    LockTimeout,
    MalformedPolicy,
    atomic_replace,
    contained_path,
    hold_lock,
    read_json,
    write_json,
)
from solstone.think.utils import (
    day_path,
    get_journal,
    now_ms,
    segment_path,
    segment_start_ts_ms,
)

if TYPE_CHECKING:
    import numpy as np

logger = logging.getLogger(__name__)
CHANNEL_ORDER = ("screen", "meeting_day", "setting", "speakers")
VOICEPRINT_ACCUMULATION_METHODS = {
    "structural_single_speaker",
    "structural_setting",
    "acoustic",
    "acoustic_cluster",
}


def _speaker_encoder_identity() -> dict[str, Any]:
    """Return the configured identity for speaker embedding artifacts."""
    return {
        "id": ENCODER_ID,
        "sha256": WESPEAKER_MODEL_SHA256,
        "width": WESPEAKER_EMBEDDING_WIDTH,
    }


def _decay_weighted_centroid(
    embeddings: np.ndarray,
    metas: list[dict],
    stream: str,
    now_ms_value: int,
    normalize_embedding: Callable[[np.ndarray], np.ndarray | None],
) -> np.ndarray | None:
    """Build a same-stream-preferred, decay-weighted centroid."""
    import numpy as np

    normalized_rows: list[tuple[np.ndarray, dict]] = []
    for emb, meta in zip(embeddings, metas):
        normalized = normalize_embedding(emb)
        if normalized is None:
            continue
        normalized_rows.append((normalized, meta if isinstance(meta, dict) else {}))

    same_stream = [
        (emb, meta) for emb, meta in normalized_rows if meta.get("stream") == stream
    ]
    basis = same_stream if len(same_stream) >= 5 else normalized_rows
    if not basis:
        return None

    weighted_sum = np.zeros_like(basis[0][0], dtype=np.float32)
    total_weight = 0.0
    for emb, meta in basis:
        try:
            added_at_ms = float(meta.get("added_at"))
        except (TypeError, ValueError):
            added_at_ms = float(now_ms_value)
        age_days = max(0.0, (float(now_ms_value) - added_at_ms) / 86_400_000.0)
        weight = math.exp(-VP_DECAY_LAMBDA * age_days)
        weighted_sum += emb * weight
        total_weight += weight

    if total_weight <= 0:
        return None
    return normalize_embedding(weighted_sum / total_weight)


def _passes_acoustic_margin(matched_score: float, other_scores: list[float]) -> bool:
    runner_up = max(0.0, max(other_scores, default=0.0))
    return matched_score - runner_up >= ACOUSTIC_MARGIN_MIN


def _load_integer_speaker_labels(seg_dir: Path, source: str) -> dict[int, int]:
    """Load integer per-sentence speaker labels from the segment JSONL."""
    jsonl_path = seg_dir / f"{source}.jsonl"
    if not jsonl_path.exists():
        return {}

    labels: dict[int, int] = {}
    try:
        with open(jsonl_path, encoding="utf-8") as f:
            lines = f.readlines()
    except Exception:
        return labels

    for sid, line in enumerate(lines[1:], start=1):
        try:
            speaker = json.loads(line).get("speaker")
        except json.JSONDecodeError:
            continue
        if isinstance(speaker, int):
            labels[sid] = speaker
    return labels


# ---------------------------------------------------------------------------
# Layer 2 helpers: structural signal parsing
# ---------------------------------------------------------------------------


def _derive_owner_name_variants(identity_names: list[str]) -> set[str]:
    """Return lowercase owner name variants from configured identity names."""
    variants: set[str] = set()
    for name in identity_names:
        lowered = name.strip().lower()
        if not lowered:
            continue
        variants.add(lowered)
        variants.update(part for part in lowered.split() if part)
    return variants


def _parse_setting_names(setting: str) -> list[str]:
    """Parse participant names from an import setting field.

    Examples:
        With identity "Avery Stone":
        "Avery and Jordan at coffee" -> ["Jordan"]
        "Meeting with Priya and Mateo" -> ["Priya", "Mateo"]
        "Lunch with Jordan Lee" -> ["Jordan Lee"]
    """
    if not setting:
        return []
    # Strip leading context words
    text = re.sub(
        r"^(meeting|call|lunch|coffee|dinner|chat|conversation|zoom|hangout)"
        r"\s+(with\s+)?",
        "",
        setting,
        flags=re.IGNORECASE,
    )
    # Strip trailing location/topic clauses
    text = re.sub(
        r"\s+(at|in|about|re|regarding|on|over)\s+.*$",
        "",
        text,
        flags=re.IGNORECASE,
    )
    # Split by connectors (comma, ampersand, and/or the word "and")
    parts = re.split(r",\s*(?:and\s+)?|\s+and\s+|&\s*", text)
    # Filter owner name variants and noise
    owner_names = _derive_owner_name_variants(get_identity_names())
    names: list[str] = []
    for part in parts:
        part = part.strip()
        if part and len(part) > 1 and part.lower() not in owner_names:
            names.append(part)
    return names


def _load_setting_field_with_gaps(
    seg_dir: Path,
) -> tuple[str | None, list[dict[str, str]]]:
    """Read the setting field and report present-but-unreadable gaps."""
    jsonl_path = seg_dir / "imported_audio.jsonl"
    if not jsonl_path.exists():
        return None, []
    try:
        with open(jsonl_path, encoding="utf-8") as f:
            first_line = f.readline().strip()
    except OSError:
        return None, [{"source": "setting", "reason": "unreadable"}]
    except UnicodeDecodeError:
        return None, [{"source": "setting", "reason": "malformed_json"}]

    if not first_line:
        return None, []
    try:
        data = json.loads(first_line)
    except json.JSONDecodeError:
        return None, [{"source": "setting", "reason": "malformed_json"}]
    if not isinstance(data, dict):
        return None, [{"source": "setting", "reason": "wrong_shape"}]
    setting = data.get("setting")
    if setting is None:
        return None, []
    if not isinstance(setting, str):
        return None, [{"source": "setting", "reason": "wrong_shape"}]
    return setting, []


def _load_setting_field(seg_dir: Path) -> str | None:
    """Read the setting field from the first line of imported_audio.jsonl."""
    return _load_setting_field_with_gaps(seg_dir)[0]


def _load_segment_speakers_with_gaps(
    seg_dir: Path,
) -> tuple[list[str], list[dict[str, str]]]:
    """Load speaker names and report present-but-malformed speakers output."""
    speakers_path = seg_dir / "talents" / "speakers.json"
    if not speakers_path.exists():
        return [], []
    try:
        data = json.loads(speakers_path.read_text(encoding="utf-8"))
    except OSError:
        logger.warning(
            "speaker attribution: failed to read segment speakers from %s",
            speakers_path,
            exc_info=True,
        )
        return [], [{"source": "speakers", "reason": "unreadable"}]
    except (UnicodeDecodeError, json.JSONDecodeError):
        logger.warning(
            "speaker attribution: failed to read segment speakers from %s",
            speakers_path,
            exc_info=True,
        )
        return [], [{"source": "speakers", "reason": "malformed_json"}]
    if not isinstance(data, list):
        logger.warning(
            "speaker attribution: malformed segment speakers in %s",
            speakers_path,
        )
        return [], [{"source": "speakers", "reason": "wrong_shape"}]

    names = [name for name in data if isinstance(name, str) and name.strip()]
    if any(not isinstance(name, str) for name in data):
        logger.warning(
            "speaker attribution: malformed segment speakers in %s",
            speakers_path,
        )
        return names, [{"source": "speakers", "reason": "wrong_shape"}]
    return names, []


def _extract_screen_participants_with_gaps(
    seg_dir: Path,
) -> tuple[list[str], list[dict[str, str]]]:
    """Extract attendee names and report present-but-malformed screen output."""
    screen_path = seg_dir / "talents" / "screen.json"
    if not screen_path.exists():
        return [], []
    try:
        data = json.loads(screen_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        logger.warning(
            "speaker attribution: failed to read screen participants from %s",
            screen_path,
            exc_info=True,
        )
        return [], [{"source": "screen", "reason": "malformed_json"}]
    if not isinstance(data, dict) or not isinstance(data.get("entities"), list):
        logger.warning(
            "speaker attribution: malformed screen participants in %s",
            screen_path,
        )
        return [], [{"source": "screen", "reason": "wrong_shape"}]
    names = [
        entity["name"].strip()
        for entity in data["entities"]
        if isinstance(entity, dict)
        and entity.get("type") == "Person"
        and entity.get("role") == "attendee"
        and isinstance(entity.get("name"), str)
        and entity["name"].strip()
    ]
    return names, []


def _extract_screen_participants(seg_dir: Path) -> list[str]:
    """Extract attendee names from structured screen.json agent output."""
    return _extract_screen_participants_with_gaps(seg_dir)[0]


def _extract_meeting_participants_with_gaps(
    day: str, segment_key: str
) -> tuple[list[str], list[dict[str, str]]]:
    """Extract participant names and report unreadable daily meetings output."""
    meetings_path = day_path(day, create=False) / "talents" / "meetings.md"
    if not meetings_path.exists():
        return [], []
    try:
        content = meetings_path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return [], [{"source": "meeting_day", "reason": "unreadable"}]

    names: list[str] = []
    part_re = re.compile(
        r"\*\*Participants?\s*[:–—\-]\*\*\s*(.*)"
        r"|\*\*Participants?\*\*\s*[:–—\-]\s*(.*)",
        re.IGNORECASE,
    )
    for line in content.splitlines():
        m = part_re.search(line)
        if m:
            participant_text = m.group(1) or m.group(2) or ""
            for name in re.split(r"[,;]", participant_text):
                name = name.strip().strip("*").strip()
                if name and len(name) > 1:
                    names.append(name)
    return names, []


def _assemble_candidate_evidence(
    name_channels: dict[str, set[str]],
    name_entity_ids: dict[str, str],
) -> list[dict]:
    """Return deterministic per-entity evidence from resolved names."""
    entity_sources: dict[str, set[str]] = defaultdict(set)
    for name, entity_id in name_entity_ids.items():
        entity_sources[entity_id].update(name_channels.get(name, set()))

    channel_rank = {channel: index for index, channel in enumerate(CHANNEL_ORDER)}
    return [
        {
            "entity_id": entity_id,
            "sources": sorted(
                sources,
                key=lambda source: channel_rank[source],
            ),
        }
        for entity_id, sources in sorted(entity_sources.items())
        if sources
    ]


def _candidate_name_channels(
    speakers: list[str],
    setting_names: list[str],
    screen_names: list[str],
    meeting_names: list[str],
) -> dict[str, set[str]]:
    name_channels: dict[str, set[str]] = defaultdict(set)
    for channel, names in (
        ("screen", screen_names),
        ("meeting_day", meeting_names),
        ("setting", setting_names),
        ("speakers", speakers),
    ):
        for name in names:
            name_channels[name].add(channel)
    return name_channels


def compute_segment_candidate_evidence_readonly(
    day: str,
    stream: str,
    segment_key: str,
) -> tuple[list[dict], list[dict]]:
    """Compute per-segment candidate evidence without writing journal state."""
    seg_dir = segment_path(day, segment_key, stream, create=False)
    if not seg_dir.is_dir():
        return [], []

    evidence_gaps: list[dict[str, str]] = []
    speakers, speakers_gaps = _load_segment_speakers_with_gaps(seg_dir)
    evidence_gaps.extend(speakers_gaps)
    setting, setting_gaps = _load_setting_field_with_gaps(seg_dir)
    evidence_gaps.extend(setting_gaps)
    setting_names = _parse_setting_names(setting) if setting else []
    screen_names, screen_gaps = _extract_screen_participants_with_gaps(seg_dir)
    evidence_gaps.extend(screen_gaps)
    meeting_names, meeting_gaps = _extract_meeting_participants_with_gaps(
        day, segment_key
    )
    evidence_gaps.extend(meeting_gaps)

    name_channels = _candidate_name_channels(
        speakers,
        setting_names,
        screen_names,
        meeting_names,
    )
    candidate_names: list[str] = list(
        dict.fromkeys(speakers + setting_names + screen_names + meeting_names)
    )

    entities_list = [
        e for e in load_all_journal_entities().values() if not e.get("blocked")
    ]
    resolution_scope = ResolutionScope.journal()
    resolution_origin = ResolutionOrigin(
        lane="apps.speakers.aggregation",
        day=day,
        segment_id=segment_key,
        field="candidate_name",
    )
    name_entity_ids: dict[str, str] = {}
    for name in candidate_names:
        try:
            resolution = record_entity_resolution(
                name,
                entities_list,
                scope=resolution_scope,
                origin=resolution_origin,
                read_only=True,
            )
        except EntityResolutionError:
            evidence_gaps.append({"source": "resolution", "reason": "stale_resolution"})
            continue
        if resolution.outcome == EntityResolutionOutcome.RESOLVED and resolution.entity:
            name_entity_ids[name] = resolution.entity["id"]

    return _assemble_candidate_evidence(name_channels, name_entity_ids), evidence_gaps


# ---------------------------------------------------------------------------
# Core attribution pipeline
# ---------------------------------------------------------------------------


def attribute_segment(
    day: str,
    stream: str,
    segment_key: str,
    *,
    read_only: bool = False,
) -> dict[str, Any]:
    """Run Layers 1-3 of speaker attribution for a segment.

    Returns a result dict containing:
        labels           - list of per-sentence label dicts
        unmatched        - sentence IDs still needing Layer 4
        unmatched_texts  - {sentence_id: text} for LLM context
        source           - audio source stem processed
        candidates       - list of candidate speaker names
        metadata         - owner centroid refresh timestamp + voiceprint counts
    """
    import numpy as np

    seg_dir = segment_path(day, segment_key, stream, create=not read_only)
    if read_only and not seg_dir.is_dir():
        return {
            "status": "skipped",
            "skip_reason": "segment_missing",
            "labels": [],
            "unmatched": [],
            "source": None,
            "metadata": {},
        }

    # --- prerequisite: owner centroid ---
    centroid_data = load_owner_centroid()
    if centroid_data is None:
        return {"error": "no_owner_centroid", "labels": [], "unmatched": []}

    owner_centroid = centroid_data.centroid
    owner_threshold = centroid_data.threshold
    owner_margin = centroid_data.margin

    # --- prerequisite: embeddings ---
    npz_files = sorted(
        [
            p
            for p in seg_dir.glob("*.npz")
            if p.stem.endswith("_audio") or p.stem == "audio"
        ],
        key=lambda p: p.name,
    )
    if not npz_files:
        return {"labels": [], "unmatched": [], "source": None, "metadata": {}}

    source_path = npz_files[0]
    source = source_path.stem

    emb_data = load_embeddings_file(source_path)
    if emb_data is None:
        return {"labels": [], "unmatched": [], "source": source, "metadata": {}}

    embeddings, statement_ids, _ = emb_data
    if len(embeddings) == 0:
        return {"labels": [], "unmatched": [], "source": source, "metadata": {}}

    # --- entity setup ---
    journal_entities = load_all_journal_entities()
    entities_list = [e for e in journal_entities.values() if not e.get("blocked")]

    principal = get_journal_principal()
    owner_entity_id = principal["id"] if principal else None

    # --- initialise labels ---
    labels: dict[int, dict] = {}
    for sid in statement_ids:
        sid_int = int(sid)
        labels[sid_int] = {
            "sentence_id": sid_int,
            "speaker": None,
            "confidence": None,
            "method": None,
        }

    margin_non_principal_entity_ids = sorted(
        {
            str(e["id"])
            for e in entities_list
            if e.get("id") != owner_entity_id and not e.get("is_principal")
        }
    )
    voiceprint_centroid_cache: dict[str, dict[str, Any]] = {}
    centroid_now_ms: int | None = None

    def _voiceprint_centroid_entry(eid: str) -> dict[str, Any]:
        nonlocal centroid_now_ms

        cached = voiceprint_centroid_cache.get(eid)
        if cached is not None:
            return cached

        entry: dict[str, Any] = {
            "centroid": None,
            "embedding_count": 0,
            "usable": False,
        }
        result = load_entity_voiceprints_file(eid)
        if result is not None:
            vp_embs, vp_meta = result
            entry["embedding_count"] = len(vp_embs)
            if len(vp_embs) > 0:
                if centroid_now_ms is None:
                    centroid_now_ms = int(time.time() * 1000)
                centroid = _decay_weighted_centroid(
                    vp_embs,
                    vp_meta,
                    stream,
                    centroid_now_ms,
                    normalize_embedding,
                )
                if centroid is not None:
                    entry["centroid"] = centroid
                    entry["usable"] = True

        voiceprint_centroid_cache[eid] = entry
        return entry

    # ==========================
    # LAYER 1: Owner separation
    # ==========================
    non_owner_sids: list[int] = []
    margin_declined_sids: set[int] = set()

    def _replacement_label(
        sid: int,
        speaker: str,
        confidence: str,
        method: str,
        *,
        acoustic_margin_declined: bool = False,
    ) -> dict[str, Any]:
        label: dict[str, Any] = {
            "sentence_id": sid,
            "speaker": speaker,
            "confidence": confidence,
            "method": method,
        }
        if sid in margin_declined_sids:
            label["owner_margin_declined"] = True
        if acoustic_margin_declined:
            label["acoustic_margin_declined"] = True
        return label

    for emb, sid in zip(embeddings, statement_ids):
        sid_int = int(sid)
        normalized = normalize_embedding(emb)
        if normalized is None:
            continue
        score = float(np.dot(normalized, owner_centroid))
        owner_claimed = score >= owner_threshold
        if owner_claimed and owner_margin is not None:
            best_non_owner_cos = float("-inf")
            for eid in margin_non_principal_entity_ids:
                entry = _voiceprint_centroid_entry(eid)
                if not entry["usable"]:
                    continue
                best_non_owner_cos = max(
                    best_non_owner_cos,
                    float(np.dot(normalized, entry["centroid"])),
                )
            owner_claimed = (score - best_non_owner_cos) >= owner_margin

        if owner_claimed:
            labels[sid_int] = {
                "sentence_id": sid_int,
                "speaker": owner_entity_id,
                "confidence": "high",
                "method": "owner_centroid",
            }
        else:
            if score >= owner_threshold and owner_margin is not None:
                labels[sid_int]["owner_margin_declined"] = True
                margin_declined_sids.add(sid_int)
            non_owner_sids.append(sid_int)

    # ================================
    # LAYER 2: Structural heuristics
    # ================================
    evidence_gaps: list[dict[str, str]] = []
    speakers, speakers_gaps = _load_segment_speakers_with_gaps(seg_dir)
    evidence_gaps.extend(speakers_gaps)
    setting, setting_gaps = _load_setting_field_with_gaps(seg_dir)
    evidence_gaps.extend(setting_gaps)
    setting_names = _parse_setting_names(setting) if setting else []
    screen_names, screen_gaps = _extract_screen_participants_with_gaps(seg_dir)
    evidence_gaps.extend(screen_gaps)
    meeting_names, meeting_gaps = _extract_meeting_participants_with_gaps(
        day, segment_key
    )
    evidence_gaps.extend(meeting_gaps)

    name_channels = _candidate_name_channels(
        speakers,
        setting_names,
        screen_names,
        meeting_names,
    )

    # Deduplicate, preserve order
    candidate_names: list[str] = list(
        dict.fromkeys(speakers + setting_names + screen_names + meeting_names)
    )

    # Resolve candidates to entities
    candidate_entities: dict[str, dict] = {}
    name_entity_ids: dict[str, str] = {}
    resolution_scope = ResolutionScope.journal()
    resolution_origin = ResolutionOrigin(
        lane="apps.speakers.attribution",
        day=day,
        segment_id=segment_key,
        field="candidate_name",
    )
    for name in candidate_names:
        resolution = record_entity_resolution(
            name,
            entities_list,
            scope=resolution_scope,
            origin=resolution_origin,
            read_only=read_only,
        )
        if resolution.outcome == EntityResolutionOutcome.RESOLVED and resolution.entity:
            candidate_entities[resolution.entity["id"]] = resolution.entity
            name_entity_ids[name] = resolution.entity["id"]

    candidate_evidence = _assemble_candidate_evidence(name_channels, name_entity_ids)

    # 2a: single-listed-speaker — all non-owner sentences belong to them
    if len(speakers) == 1:
        resolution = record_entity_resolution(
            speakers[0],
            entities_list,
            scope=resolution_scope,
            origin=ResolutionOrigin(
                lane="apps.speakers.attribution",
                day=day,
                segment_id=segment_key,
                field="structural_single_speaker",
            ),
            read_only=read_only,
        )
        if resolution.outcome == EntityResolutionOutcome.RESOLVED and resolution.entity:
            for sid in non_owner_sids:
                if labels[sid]["speaker"] is None:
                    confidence = "medium" if sid in margin_declined_sids else "high"
                    labels[sid] = _replacement_label(
                        sid,
                        resolution.entity["id"],
                        confidence,
                        "structural_single_speaker",
                    )

    # 2b: single setting-field participant (import segments without speakers.json)
    elif not speakers and len(setting_names) == 1:
        resolution = record_entity_resolution(
            setting_names[0],
            entities_list,
            scope=resolution_scope,
            origin=ResolutionOrigin(
                lane="apps.speakers.attribution",
                day=day,
                segment_id=segment_key,
                field="structural_setting",
            ),
            read_only=read_only,
        )
        if resolution.outcome == EntityResolutionOutcome.RESOLVED and resolution.entity:
            for sid in non_owner_sids:
                if labels[sid]["speaker"] is None:
                    confidence = "medium" if sid in margin_declined_sids else "high"
                    labels[sid] = _replacement_label(
                        sid,
                        resolution.entity["id"],
                        confidence,
                        "structural_setting",
                    )

    # ============================
    # LAYER 3: Acoustic matching
    # ============================
    unresolved = [sid for sid in non_owner_sids if labels[sid]["speaker"] is None]
    voiceprint_versions: dict[str, int] = {}

    if unresolved:
        # Determine which entities to match against
        if candidate_entities:
            vp_entity_ids = set(candidate_entities.keys())
        else:
            vp_entity_ids = {
                e["id"] for e in entities_list if not e.get("is_principal")
            }

        # Load centroids with same-stream preference
        if centroid_now_ms is None:
            centroid_now_ms = int(time.time() * 1000)
        voiceprint_centroids: dict[str, np.ndarray] = {}
        for eid in vp_entity_ids:
            entry = _voiceprint_centroid_entry(eid)
            if entry["embedding_count"] == 0:
                continue
            voiceprint_versions[eid] = entry["embedding_count"]

            if entry["usable"]:
                voiceprint_centroids[eid] = entry["centroid"]

        # Build sentence-to-embedding index
        sid_to_idx = {int(s): i for i, s in enumerate(statement_ids)}

        # Hybrid Layer 3: map per-segment integer clusters to entity centroids.
        integer_speakers = _load_integer_speaker_labels(seg_dir, source)
        labeled = [
            sid for sid in unresolved if isinstance(integer_speakers.get(sid), int)
        ]
        coverage = len(labeled) / len(unresolved) if unresolved else 0.0
        if labeled and coverage >= CC_COVERAGE_GATE and voiceprint_centroids:
            cluster_members: dict[int, list[int]] = defaultdict(list)
            cluster_embeddings: dict[int, list[np.ndarray]] = defaultdict(list)
            for sid in labeled:
                idx = sid_to_idx.get(sid)
                if idx is None:
                    continue
                normalized = normalize_embedding(embeddings[idx])
                if normalized is None:
                    continue
                cluster_id = integer_speakers[sid]
                cluster_members[cluster_id].append(sid)
                cluster_embeddings[cluster_id].append(normalized)

            cluster_centroids: dict[int, np.ndarray] = {}
            for cluster_id, member_embeddings in cluster_embeddings.items():
                centroid = normalize_embedding(np.mean(member_embeddings, axis=0))
                if centroid is not None:
                    cluster_centroids[cluster_id] = centroid

            pairs: list[tuple[float, int, str]] = []
            for cluster_id, cluster_centroid in cluster_centroids.items():
                for eid, entity_centroid in voiceprint_centroids.items():
                    score = float(np.dot(cluster_centroid, entity_centroid))
                    pairs.append((score, cluster_id, eid))

            assigned: dict[int, tuple[str, float]] = {}
            used_clusters: set[int] = set()
            used_entities: set[str] = set()
            for score, cluster_id, eid in sorted(pairs, reverse=True):
                if cluster_id in used_clusters or eid in used_entities:
                    continue
                assigned[cluster_id] = (eid, score)
                used_clusters.add(cluster_id)
                used_entities.add(eid)

            if assigned:
                mean_confidence = sum(score for _, score in assigned.values()) / len(
                    assigned
                )
                if mean_confidence >= CC_CONFIDENCE_GATE:
                    for cluster_id, (eid, score) in assigned.items():
                        if score < ACOUSTIC_MEDIUM:
                            continue
                        confidence = "high" if score >= ACOUSTIC_HIGH else "medium"
                        acoustic_margin_declined = False
                        if confidence == "high":
                            other_scores = [
                                pair_score
                                for pair_score, pair_cluster_id, pair_eid in pairs
                                if pair_cluster_id == cluster_id and pair_eid != eid
                            ]
                            acoustic_margin_declined = not _passes_acoustic_margin(
                                score,
                                other_scores,
                            )
                            if acoustic_margin_declined:
                                confidence = "medium"
                        for sid in cluster_members[cluster_id]:
                            label_confidence = confidence
                            if (
                                sid in margin_declined_sids
                                and label_confidence == "high"
                            ):
                                label_confidence = "medium"
                            labels[sid] = _replacement_label(
                                sid,
                                eid,
                                label_confidence,
                                "acoustic_cluster",
                                acoustic_margin_declined=acoustic_margin_declined,
                            )

        for sid in unresolved:
            if labels[sid]["speaker"] is not None:
                continue
            idx = sid_to_idx.get(sid)
            if idx is None:
                continue
            normalized = normalize_embedding(embeddings[idx])
            if normalized is None:
                continue

            best_eid: str | None = None
            best_score = 0.0
            runner_up_scores: list[float] = []
            for eid, centroid in voiceprint_centroids.items():
                score = float(np.dot(normalized, centroid))
                if score > best_score:
                    if best_eid is not None:
                        runner_up_scores.append(best_score)
                    best_score = score
                    best_eid = eid
                else:
                    runner_up_scores.append(score)

            if best_eid is not None:
                if best_score >= ACOUSTIC_HIGH:
                    acoustic_margin_declined = not _passes_acoustic_margin(
                        best_score,
                        runner_up_scores,
                    )
                    confidence = (
                        "medium"
                        if sid in margin_declined_sids or acoustic_margin_declined
                        else "high"
                    )
                    labels[sid] = _replacement_label(
                        sid,
                        best_eid,
                        confidence,
                        "acoustic",
                        acoustic_margin_declined=acoustic_margin_declined,
                    )
                elif best_score >= ACOUSTIC_MEDIUM:
                    labels[sid] = _replacement_label(
                        sid,
                        best_eid,
                        "medium",
                        "acoustic",
                    )

    # --- collect final unmatched for Layer 4 ---
    final_unmatched = [
        int(sid) for sid in statement_ids if labels[int(sid)]["speaker"] is None
    ]

    # --- load transcript text for LLM context ---
    unmatched_texts: dict[int, str] = {}
    if final_unmatched:
        jsonl_path = seg_dir / f"{source}.jsonl"
        if jsonl_path.exists():
            try:
                with open(jsonl_path, encoding="utf-8") as f:
                    lines = f.readlines()
                for i, line in enumerate(lines[1:], start=1):
                    if i in final_unmatched:
                        try:
                            entry = json.loads(line)
                            unmatched_texts[i] = entry.get("text", "")
                        except json.JSONDecodeError:
                            pass
            except Exception:
                pass

    # --- owner centroid refresh timestamp ---
    owner_refreshed_at = centroid_data.last_refreshed_at or None

    return {
        "labels": [labels[int(sid)] for sid in statement_ids],
        "unmatched": final_unmatched,
        "unmatched_texts": unmatched_texts,
        "source": source,
        "candidates": candidate_names,
        "metadata": {
            "owner_centroid_last_refreshed_at": owner_refreshed_at,
            "voiceprint_versions": voiceprint_versions,
            "candidate_evidence": candidate_evidence,
            "candidate_evidence_gaps": evidence_gaps,
        },
    }


# ---------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------


def update_speaker_labels(
    seg_dir: Path,
    transform: Callable[[dict | None], dict | None],
) -> None:
    """Apply a locked read-modify-write transform to speaker_labels.json."""
    path = seg_dir / "talents" / "speaker_labels.json"
    with hold_lock(path):
        current = read_json(
            path,
            on_error=MalformedPolicy.WARN_AND_SKIP,
            default=None,
        )
        new = transform(current)
        if new is None:
            return
        write_json(path, new)


def append_speaker_correction(seg_dir: Path, correction: dict) -> None:
    """Append a correction through the native speaker-identity owner."""
    native_speakers.append_correction(get_journal(), seg_dir, correction)


def remap_speaker_corrections_for_entity_merge(
    seg_dir: Path,
    source_id: str,
    target_id: str,
) -> int:
    """Merge-only locked replace-all remap of source_id→target_id across all corrections.

    Distinct from append_speaker_correction (the UI append path). Reads fresh
    under lock so a concurrent append is preserved.
    """
    path = seg_dir / "talents" / "speaker_corrections.json"
    with hold_lock(path):
        current = read_json(
            path,
            on_error=MalformedPolicy.WARN_AND_SKIP,
            default=None,
        )
        if not current:
            return 0

        corrections = (
            current.get("corrections", []) if isinstance(current, dict) else []
        )
        changed_entries = 0
        for correction in corrections:
            if not isinstance(correction, dict):
                continue
            changed = False
            for field in ("original_speaker", "corrected_speaker"):
                if correction.get(field) == source_id:
                    correction[field] = target_id
                    changed = True
            if changed:
                changed_entries += 1

        if changed_entries == 0:
            return 0

        atomic_replace(path, json.dumps({"corrections": corrections}, indent=2))
        return changed_entries


def apply_entity_merge_segment_inverse(
    entries: list[dict[str, Any]],
    *,
    source_id: str,
    target_id: str,
) -> dict[str, int]:
    """Undo recorded speaker label/correction rewrites under owner locks."""

    counts = {"labels_rewritten": 0, "corrections_rewritten": 0, "files_rewritten": 0}
    by_path: dict[str, list[dict[str, Any]]] = {}
    for entry in entries:
        by_path.setdefault(str(entry["path"]), []).append(entry)

    journal = Path(get_journal())
    for path_rel, path_entries in by_path.items():
        path = contained_path(journal, path_rel)
        if path.name == "speaker_labels.json":
            if _apply_speaker_label_inverse_file(
                path, path_entries, source_id=source_id, target_id=target_id
            ):
                counts["labels_rewritten"] += 1
                counts["files_rewritten"] += 1
        elif path.name == "speaker_corrections.json":
            if _apply_speaker_correction_inverse_file(
                path, path_entries, source_id=source_id, target_id=target_id
            ):
                counts["corrections_rewritten"] += 1
                counts["files_rewritten"] += 1
        else:
            raise ValueError(f"unsupported speaker inverse path: {path_rel}")
    return counts


def _apply_speaker_label_inverse_file(
    path: Path,
    entries: list[dict[str, Any]],
    *,
    source_id: str,
    target_id: str,
) -> bool:
    with hold_lock(path):
        current = read_json(path, on_error=MalformedPolicy.RAISE, default=None)
        if not isinstance(current, dict):
            raise ValueError(f"speaker label inverse path is not an object: {path}")
        labels = current.get("labels")
        if not isinstance(labels, list):
            raise ValueError(f"speaker label inverse path has no labels list: {path}")
        changed = False
        for entry in entries:
            row = _match_speaker_inverse_row(
                labels,
                entry,
                fields=("speaker",),
                source_id=source_id,
                target_id=target_id,
            )
            row["speaker"] = source_id
            changed = True
        if changed:
            write_json(path, current)
        return changed


def _apply_speaker_correction_inverse_file(
    path: Path,
    entries: list[dict[str, Any]],
    *,
    source_id: str,
    target_id: str,
) -> bool:
    with hold_lock(path):
        current = read_json(path, on_error=MalformedPolicy.RAISE, default=None)
        if not isinstance(current, dict):
            raise ValueError(
                f"speaker correction inverse path is not an object: {path}"
            )
        corrections = current.get("corrections")
        if not isinstance(corrections, list):
            raise ValueError(
                f"speaker correction inverse path has no corrections list: {path}"
            )
        changed = False
        for group in _group_speaker_inverse_entries(entries):
            row = _match_speaker_inverse_row(
                corrections,
                group,
                fields=tuple(group["fields"]),
                source_id=source_id,
                target_id=target_id,
            )
            for field in group["fields"]:
                row[field] = source_id
                changed = True
        if changed:
            atomic_replace(path, json.dumps({"corrections": corrections}, indent=2))
        return changed


def _group_speaker_inverse_entries(
    entries: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    grouped: dict[str, dict[str, Any]] = {}
    for entry in entries:
        preimage = entry.get("row_preimage")
        key = json.dumps(preimage, sort_keys=True, ensure_ascii=False)
        group = grouped.setdefault(
            key,
            {
                "row_preimage": preimage,
                "row_key": entry.get("row_key"),
                "fields": [],
            },
        )
        group["fields"].append(entry["field"])
    return list(grouped.values())


def _match_speaker_inverse_row(
    rows: list[Any],
    entry: dict[str, Any],
    *,
    fields: tuple[str, ...],
    source_id: str,
    target_id: str,
) -> dict[str, Any]:
    preimage = entry.get("row_preimage")
    if not isinstance(preimage, dict):
        raise ValueError("speaker inverse entry is missing row_preimage")
    expected = dict(preimage)
    for field in fields:
        if expected.get(field) != source_id:
            raise ValueError("speaker inverse preimage does not contain source id")
        expected[field] = target_id
    matches = [row for row in rows if isinstance(row, dict) and row == expected]
    if len(matches) != 1:
        raise ValueError("speaker inverse locator did not match exactly one row")
    return matches[0]


def _label_sentence_id(label: dict) -> int | None:
    sid = label.get("sentence_id")
    if sid is None:
        return None
    try:
        return int(sid)
    except (TypeError, ValueError):
        return None


def _is_user_label(label: dict) -> bool:
    method = label.get("method")
    return isinstance(method, str) and method.startswith("user_")


def _load_corrections_by_sentence(seg_dir: Path) -> dict[int, dict]:
    corr_path = seg_dir / "talents" / "speaker_corrections.json"
    corrected: dict[int, dict] = {}
    if not corr_path.is_file():
        return corrected
    try:
        with open(corr_path, encoding="utf-8") as f:
            corr_data = json.load(f)
        for entry in corr_data.get("corrections", []):
            sid = entry.get("sentence_id")
            if sid is not None:
                corrected[int(sid)] = entry
    except (json.JSONDecodeError, OSError):
        pass
    return corrected


def _apply_correction_overlay(label: dict, correction: dict) -> bool:
    speaker = correction.get("corrected_speaker")
    if speaker is None:
        if correction.get("correction_kind") != "identify_undo":
            return False
        label["speaker"] = None
        label["confidence"] = None
        label["method"] = None
        return True

    label["speaker"] = speaker
    label["confidence"] = "high"
    if correction.get("original_speaker") == speaker:
        label["method"] = "user_confirmed"
    elif correction.get("original_speaker") is None:
        label["method"] = "user_assigned"
    else:
        label["method"] = "user_corrected"
    return True


def _speaker_labels_payload(
    seg_dir: Path,
    labels: list[dict],
    metadata: dict[str, Any],
    current: dict | None,
) -> dict:
    result = dict(current) if isinstance(current, dict) else {}
    result.pop("skipped", None)
    result.pop("reason", None)
    prepared_labels = labels

    corrected = _load_corrections_by_sentence(seg_dir)
    if corrected:
        for label in prepared_labels:
            sid = label.get("sentence_id")
            if sid is not None and int(sid) in corrected:
                corr = corrected[int(sid)]
                _apply_correction_overlay(label, corr)
        logger.info(
            "Preserved %d user corrections in %s",
            len(corrected),
            seg_dir,
        )

    user_by_sid: dict[int, dict] = {}
    if isinstance(current, dict):
        current_labels = current.get("labels", [])
        if isinstance(current_labels, list):
            for current_label in current_labels:
                if not isinstance(current_label, dict):
                    continue
                sid = _label_sentence_id(current_label)
                if sid is not None and _is_user_label(current_label):
                    user_by_sid[sid] = current_label

    merged_labels: list[dict] = []
    fresh_sids: set[int] = set()
    for label in prepared_labels:
        sid = _label_sentence_id(label)
        if sid is None:
            merged_labels.append(label)
            continue
        fresh_sids.add(sid)
        merged_labels.append(user_by_sid.get(sid, label))

    user_only = [
        label for sid, label in sorted(user_by_sid.items()) if sid not in fresh_sids
    ]
    merged_labels.extend(user_only)

    result["labels"] = merged_labels
    shaped_metadata = _speaker_labels_metadata(metadata)
    result.update(shaped_metadata)
    if "candidate_evidence_gaps" not in shaped_metadata:
        result.pop("candidate_evidence_gaps", None)
    return result


def _speaker_labels_metadata(metadata: dict[str, Any]) -> dict[str, Any]:
    result = {
        "owner_centroid_last_refreshed_at": metadata.get(
            "owner_centroid_last_refreshed_at"
        ),
        "voiceprint_versions": metadata.get("voiceprint_versions", {}),
        "candidate_evidence": metadata.get("candidate_evidence") or [],
    }
    gaps = metadata.get("candidate_evidence_gaps") or []
    if gaps:
        result["candidate_evidence_gaps"] = gaps
    return result


def save_speaker_labels(
    seg_dir: Path,
    labels: list[dict],
    metadata: dict[str, Any],
) -> Path:
    """Write speaker labels through the native speaker-identity owner."""
    native_speakers.write_full_labels(
        get_journal(), seg_dir, labels, _speaker_labels_metadata(metadata)
    )

    out_path = seg_dir / "talents" / "speaker_labels.json"
    logger.info("Wrote %d labels to %s", len(labels), out_path)
    return out_path


def save_speaker_labels_stub(seg_dir: Path, reason: str) -> None:
    """Write an attribution stub through the native speaker-identity owner."""
    native_speakers.write_stub_labels(get_journal(), seg_dir, reason)


def _read_speaker_labels(seg_dir: Path) -> dict | None:
    path = seg_dir / "talents" / "speaker_labels.json"
    return read_json(
        path,
        on_error=MalformedPolicy.WARN_AND_SKIP,
        default=None,
    )


def _labels_by_sentence(payload: dict | None) -> dict[int, dict]:
    if not isinstance(payload, dict):
        return {}
    labels = payload.get("labels", [])
    if not isinstance(labels, list):
        return {}
    by_sid: dict[int, dict] = {}
    for label in labels:
        if not isinstance(label, dict):
            continue
        sid = _label_sentence_id(label)
        if sid is not None:
            by_sid[sid] = label
    return by_sid


def _speaker_label_changes(
    current: dict | None,
    updated: dict,
    *,
    day: str,
    stream: str,
    segment_key: str,
    source: str | None,
) -> list[dict[str, Any]]:
    current_by_sid = _labels_by_sentence(current)
    updated_by_sid = _labels_by_sentence(updated)
    changes: list[dict[str, Any]] = []
    for sid in sorted(set(current_by_sid) | set(updated_by_sid)):
        before = current_by_sid.get(sid, {})
        after = updated_by_sid.get(sid, {})
        before_fields = (
            before.get("speaker"),
            before.get("method"),
            before.get("confidence"),
        )
        after_fields = (
            after.get("speaker"),
            after.get("method"),
            after.get("confidence"),
        )
        if before_fields == after_fields:
            continue
        changes.append(
            {
                "day": day,
                "stream": stream,
                "segment_key": segment_key,
                "source": source,
                "sentence_id": sid,
                "from_speaker": before.get("speaker"),
                "to_speaker": after.get("speaker"),
                "from_method": before.get("method"),
                "to_method": after.get("method"),
                "from_confidence": before.get("confidence"),
                "to_confidence": after.get("confidence"),
            }
        )
    return changes


def _speaker_evidence_changed(current: dict | None, updated: dict) -> bool:
    if not isinstance(current, dict):
        return True
    return current.get("candidate_evidence") != updated.get(
        "candidate_evidence"
    ) or current.get("candidate_evidence_gaps", []) != updated.get(
        "candidate_evidence_gaps", []
    )


def process_attributed_segment(
    day: str,
    stream: str,
    segment_key: str,
    *,
    commit: bool,
    read_only: bool,
) -> dict[str, Any]:
    result = attribute_segment(
        day,
        stream,
        segment_key,
        read_only=read_only,
    )
    if result.get("status") == "skipped":
        return {
            "status": "skipped",
            "day": day,
            "stream": stream,
            "segment_key": segment_key,
            "source": result.get("source"),
            "changes": [],
            "changed_count": 0,
            "accumulated": {},
            "error": None,
            "skip_reason": result.get("skip_reason"),
        }
    if result.get("error"):
        return {
            "status": "error",
            "day": day,
            "stream": stream,
            "segment_key": segment_key,
            "source": result.get("source"),
            "changes": [],
            "changed_count": 0,
            "accumulated": {},
            "error": result["error"],
        }

    labels = result.get("labels", [])
    metadata = result.get("metadata", {})
    source = result.get("source")
    seg_dir = segment_path(day, segment_key, stream, create=False)
    current = _read_speaker_labels(seg_dir)
    updated = _speaker_labels_payload(seg_dir, labels, metadata, current)
    changes = _speaker_label_changes(
        current,
        updated,
        day=day,
        stream=stream,
        segment_key=segment_key,
        source=source,
    )
    evidence_changed = _speaker_evidence_changed(current, updated)

    should_write = commit and (current is None or bool(changes) or evidence_changed)
    accumulated: dict[str, int] = {}
    if should_write:
        save_speaker_labels(seg_dir, labels, metadata)
        if source:
            accumulated = accumulate_voiceprints(
                day, stream, segment_key, labels, source
            )

    return {
        "status": "changed" if changes else "unchanged",
        "day": day,
        "stream": stream,
        "segment_key": segment_key,
        "source": source,
        "changes": changes,
        "changed_count": len(changes),
        "accumulated": accumulated,
        "error": None,
    }


def find_labeled_segments_for_speakers(
    entity_ids: set[str],
) -> list[tuple[str, str, str, Path]]:
    """Return existing labeled segments whose current labels reference any entity."""
    from solstone.think.utils import day_dirs, iter_segments

    if not entity_ids:
        return []

    entity_id_bytes = [entity_id.encode("utf-8") for entity_id in sorted(entity_ids)]
    found: list[tuple[str, str, str, Path]] = []
    for day_name in sorted(day_dirs().keys()):
        for stream_name, seg_key, seg_path in iter_segments(day_name):
            labels_path = seg_path / "talents" / "speaker_labels.json"
            if not labels_path.is_file():
                continue
            try:
                raw = labels_path.read_bytes()
            except OSError:
                logger.warning("Failed to read speaker labels %s", labels_path)
                continue
            if not any(entity_id in raw for entity_id in entity_id_bytes):
                continue
            try:
                data = json.loads(raw.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError):
                logger.warning("Failed to parse speaker labels %s", labels_path)
                continue
            labels = data.get("labels", []) if isinstance(data, dict) else []
            if any(
                isinstance(label, dict) and label.get("speaker") in entity_ids
                for label in labels
            ):
                found.append((day_name, stream_name, seg_key, seg_path))
    return found


def propagate_speaker_correction(
    old_speaker: str,
    new_speaker: str,
    *,
    commit: bool = False,
) -> dict[str, Any]:
    """Re-attribute labeled segments mentioning either side of a correction."""
    segments = find_labeled_segments_for_speakers({old_speaker, new_speaker})
    segment_results: list[dict[str, Any]] = []
    changes: list[dict[str, Any]] = []
    errors: list[str] = []

    for day_name, stream_name, seg_key, _seg_path in segments:
        try:
            segment_result = process_attributed_segment(
                day_name,
                stream_name,
                seg_key,
                commit=commit,
                read_only=not commit,
            )
        except LockTimeout:
            raise
        except Exception as exc:
            errors.append(f"{day_name}/{stream_name}/{seg_key}: {exc}")
            continue
        segment_results.append(segment_result)
        if segment_result.get("error"):
            errors.append(
                f"{day_name}/{stream_name}/{seg_key}: {segment_result['error']}"
            )
            continue
        changes.extend(segment_result.get("changes", []))

    changed_segments = {
        (change["day"], change["stream"], change["segment_key"]) for change in changes
    }
    return {
        "status": "applied" if commit else "preview",
        "commit": commit,
        "old_speaker": old_speaker,
        "new_speaker": new_speaker,
        "segments_scanned": len(segments),
        "segments_considered": len(segment_results),
        "segment_count": len(changed_segments),
        "statement_count": len(changes),
        "changes": changes,
        "segments": segment_results,
        "errors": errors,
    }


def apply_label_patches(
    seg_dir: Path,
    patches: dict[int, dict],
    *,
    allow_insert: bool,
) -> None:
    """Apply per-sentence patches through the native speaker-identity owner."""
    native_speakers.patch_labels(
        get_journal(),
        seg_dir,
        patches,
        allow_insert=allow_insert,
    )


def restore_label_rows(
    seg_dir: Path,
    restorations: list[dict[str, Any]],
) -> dict[str, Any]:
    """Compare-restore rows through the native speaker-identity owner."""
    return native_speakers.restore_label_rows(get_journal(), seg_dir, restorations)


# ---------------------------------------------------------------------------
# Voiceprint accumulation
# ---------------------------------------------------------------------------


def accumulate_voiceprints(
    day: str,
    stream: str,
    segment_key: str,
    labels: list[dict],
    source: str,
) -> dict[str, int]:
    """Save high-confidence embeddings to entity voiceprints.

    Eligibility:
    - Layer 2 structural attributions (high confidence)
    - Layer 3 acoustic attributions with confidence "high"

    Guards:
    - Owner contamination: never save embeddings with owner similarity
      above the owner threshold to non-owner voiceprints
    - Idempotent: checks existing voiceprint keys before saving

    Returns dict mapping entity_id -> number of new embeddings saved.
    """
    import numpy as np

    centroid_data = load_owner_centroid()
    if centroid_data is None:
        return {}
    owner_centroid = centroid_data.centroid
    owner_threshold = centroid_data.threshold

    seg_dir = segment_path(day, segment_key, stream, create=False)
    jsonl_path = seg_dir / f"{source}.jsonl"
    overlap_fraction = _read_segment_overlap_fraction(jsonl_path)
    if overlap_fraction > NOISY_FLYWHEEL_OVERLAP_MAX:
        logger.info(
            "flywheel skip: overlap=%.3f exceeds %.2f at %s/%s/%s",
            overlap_fraction,
            NOISY_FLYWHEEL_OVERLAP_MAX,
            day,
            segment_key,
            source,
        )
        return {}

    emb_data = load_embeddings_file(seg_dir / f"{source}.npz")
    if emb_data is None:
        return {}

    embeddings, statement_ids, _ = emb_data
    sid_to_idx = {int(s): i for i, s in enumerate(statement_ids)}
    principal = get_journal_principal()
    owner_entity_id = principal["id"] if principal else None

    # Collect per-entity
    entity_new: dict[str, list[tuple[np.ndarray, dict]]] = defaultdict(list)
    entity_existing: dict[str, set] = {}
    entity_existing_centroids: dict[str, tuple[int, np.ndarray | None]] = {}

    for label in labels:
        if label.get("confidence") != "high":
            continue
        if label.get("method") not in VOICEPRINT_ACCUMULATION_METHODS:
            continue
        speaker = label.get("speaker")
        if not speaker or speaker == owner_entity_id:
            continue

        sid = label["sentence_id"]
        idx = sid_to_idx.get(sid)
        if idx is None:
            continue

        normalized = normalize_embedding(embeddings[idx])
        if normalized is None:
            continue

        # Contamination guard — owner voice must never leak into non-owner voiceprints
        owner_score = float(np.dot(normalized, owner_centroid))
        if owner_score >= owner_threshold:
            continue

        # Idempotency check
        if speaker not in entity_existing:
            result = load_entity_voiceprints_file(speaker)
            if result is None:
                entity_existing[speaker] = set()
                entity_existing_centroids[speaker] = (0, None)
            else:
                existing_embs, existing_meta = result
                entity_existing[speaker] = {
                    (
                        meta.get("day"),
                        meta.get("segment_key"),
                        meta.get("source"),
                        meta.get("sentence_id"),
                    )
                    for meta in existing_meta
                }
                existing_norms = [
                    existing
                    for emb in existing_embs
                    if (existing := normalize_embedding(emb)) is not None
                ]
                existing_centroid = (
                    normalize_embedding(np.mean(existing_norms, axis=0))
                    if existing_norms
                    else None
                )
                entity_existing_centroids[speaker] = (
                    len(existing_embs),
                    existing_centroid,
                )
        vp_key = (day, segment_key, source, sid)
        if vp_key in entity_existing[speaker]:
            continue

        existing_count, existing_centroid = entity_existing_centroids[speaker]
        if (
            existing_count >= VP_OUTLIER_MIN_SAMPLES
            and existing_centroid is not None
            and float(np.dot(normalized, existing_centroid)) < VP_OUTLIER_MIN_SIMILARITY
        ):
            continue

        metadata = {"sentence_id": sid, "method": label["method"]}
        entity_new[speaker].append((normalized, metadata))
        entity_existing[speaker].add(vp_key)

    if not entity_new:
        return {}

    prepared_labels: list[dict[str, Any]] = []
    prepared_embeddings: list[dict[str, Any]] = []
    for entity_id, items in entity_new.items():
        for embedding, metadata in items:
            sentence_id = int(metadata["sentence_id"])
            prepared_labels.append(
                {
                    "sentence_id": sentence_id,
                    "speaker": entity_id,
                    "confidence": "high",
                    "method": metadata["method"],
                }
            )
            prepared_embeddings.append(
                {
                    "sentence_id": sentence_id,
                    "values": embedding.astype(np.float32).tolist(),
                }
            )

    response = native_speakers.accumulate_voiceprints(
        get_journal(),
        day=day,
        stream=stream,
        segment_key=segment_key,
        source=source,
        now_ms=now_ms(),
        encoder=_speaker_encoder_identity(),
        labels=prepared_labels,
        embeddings=prepared_embeddings,
        entity_ids=sorted(entity_new),
    )
    outcome = next(
        (
            value
            for key, value in response.items()
            if key in {"Completed", "NothingEligible", "NoOwnerCentroid"}
        ),
        {},
    )
    reports = outcome.get("entity_reports", {}) if isinstance(outcome, dict) else {}
    return {
        entity_id: int(report.get("written_rows", 0))
        for entity_id, report in reports.items()
        if isinstance(report, dict) and int(report.get("written_rows", 0)) > 0
    }


# ---------------------------------------------------------------------------
# Backfill
# ---------------------------------------------------------------------------


def _has_audio_embeddings(seg_dir: Path) -> bool:
    """Return True if the segment has audio embedding NPZ files."""
    for p in seg_dir.glob("*.npz"):
        if p.stem.endswith("_audio") or p.stem == "audio":
            return True
    return False


def _has_speaker_labels(seg_dir: Path) -> bool:
    """Check if the segment already has speaker_labels.json."""
    return (seg_dir / "talents" / "speaker_labels.json").exists()


def backfill_segments(
    *,
    dry_run: bool = False,
    progress_callback: Any | None = None,
    reattribute: bool = False,
) -> dict[str, Any]:
    """Run attribution across all segments with embeddings.

    Processes chronologically (oldest first) so voiceprint accumulation
    builds progressively.  Skips segments that already have
    speaker_labels.json (resumable, respects user corrections).

    Parameters
    ----------
    dry_run : bool
        If True, enumerate and report but don't write labels or accumulate.
    progress_callback : callable, optional
        Called with (processed, total, day, stream, segment_key) after
        each segment.
    reattribute : bool
        If True, reprocess segments that already have speaker_labels.json.

    Returns
    -------
    dict with keys:
        total_segments   - all segments scanned
        total_eligible   - segments with embeddings
        already_labeled  - skipped (speaker_labels.json exists)
        processed        - segments attributed this run
        skipped_no_embed - segments without embeddings (pre-Jan)
        errors           - list of error strings
        speakers_seen    - dict of entity_id -> attribution count
    """
    from solstone.think.utils import day_dirs, iter_segments

    days = day_dirs()
    sorted_days = sorted(days.keys())

    # Phase 1: enumerate all eligible segments
    eligible: list[tuple[str, str, str, Path]] = []  # (day, stream, key, path)
    total_segments = 0
    no_embed_count = 0

    for day_name in sorted_days:
        segments = iter_segments(day_name)
        for stream_name, seg_key, seg_path in segments:
            total_segments += 1
            if not _has_audio_embeddings(seg_path):
                no_embed_count += 1
                continue
            eligible.append((day_name, stream_name, seg_key, seg_path))

    # Phase 2: filter already-labeled
    to_process: list[tuple[str, str, str, Path]] = []
    already_labeled = 0
    for day_name, stream_name, seg_key, seg_path in eligible:
        if not reattribute and _has_speaker_labels(seg_path):
            already_labeled += 1
        else:
            to_process.append((day_name, stream_name, seg_key, seg_path))

    stats: dict[str, Any] = {
        "total_segments": total_segments,
        "total_eligible": len(eligible),
        "already_labeled": already_labeled,
        "processed": 0,
        "skipped_no_embed": no_embed_count,
        "errors": [],
        "speakers_seen": {},
    }

    if dry_run:
        return stats

    # Phase 3: attribute each segment chronologically
    total_to_do = len(to_process)
    speakers_seen: dict[str, int] = {}

    for i, (day_name, stream_name, seg_key, _seg_path) in enumerate(to_process, 1):
        try:
            result = process_attributed_segment(
                day_name,
                stream_name,
                seg_key,
                commit=True,
                read_only=False,
            )

            if result.get("error"):
                stats["errors"].append(
                    f"{day_name}/{stream_name}/{seg_key}: {result['error']}"
                )
                if progress_callback:
                    progress_callback(i, total_to_do, day_name, stream_name, seg_key)
                continue

            # Track speakers
            for change in result.get("changes", []):
                speaker = change.get("to_speaker")
                if speaker:
                    speakers_seen[speaker] = speakers_seen.get(speaker, 0) + 1

            stats["processed"] += 1

        except Exception as exc:
            stats["errors"].append(f"{day_name}/{stream_name}/{seg_key}: {exc}")

        if progress_callback:
            progress_callback(i, total_to_do, day_name, stream_name, seg_key)

    stats["speakers_seen"] = speakers_seen
    return stats


def _load_attributed_speakers(labels_path: Path) -> set[str]:
    """Return entity ids attributed in one speaker_labels.json file."""
    try:
        data = json.loads(labels_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return set()
    return {
        str(label["speaker"])
        for label in data.get("labels", [])
        if isinstance(label.get("speaker"), str) and label.get("speaker")
    }


def backfill_last_seen(*, dry_run: bool = True) -> dict[str, Any]:
    """Backfill last_seen_ts on existing voiceprint metadata rows."""
    from solstone.think.entities.voiceprints import load_entity_voiceprints_file
    from solstone.think.utils import day_dirs, iter_segments

    entity_max_ts: dict[str, int] = {}
    labels_read = 0
    errors: list[str] = []

    for day_name in sorted(day_dirs().keys()):
        for _stream_name, seg_key, seg_path in iter_segments(day_name):
            labels_path = seg_path / "talents" / "speaker_labels.json"
            if not labels_path.exists():
                continue
            labels_read += 1
            try:
                segment_ts = segment_start_ts_ms(day_name, seg_key)
            except ValueError as exc:
                errors.append(f"{day_name}/{seg_key}: {exc}")
                continue
            for entity_id in _load_attributed_speakers(labels_path):
                entity_max_ts[entity_id] = max(
                    entity_max_ts.get(entity_id, 0),
                    segment_ts,
                )

    pending: dict[str, dict[str, int]] = {}
    rows_scanned = 0
    rows_pending = 0
    rows_written = 0

    def needs_update(metadata: dict, max_ts: int) -> bool:
        current = metadata.get("last_seen_ts")
        return not isinstance(current, int) or current < max_ts

    for entity_id, max_ts in sorted(entity_max_ts.items()):
        voiceprints = load_entity_voiceprints_file(entity_id)
        if voiceprints is None:
            continue

        _embeddings, metadata_rows = voiceprints
        rows_scanned += len(metadata_rows)
        update_count = sum(1 for row in metadata_rows if needs_update(row, max_ts))
        if update_count <= 0:
            continue

        pending[entity_id] = {
            "rows": update_count,
            "last_seen_ts": max_ts,
        }
        rows_pending += update_count
        if dry_run:
            continue

        result = native_speakers.backfill_voiceprint_last_seen(
            get_journal(),
            entity_id=entity_id,
            last_seen_ts=max_ts,
            encoder=_speaker_encoder_identity(),
        )
        rows_written += int(result.get("rows_written", 0))

    return {
        "dry_run": dry_run,
        "labels_read": labels_read,
        "entities_seen": len(entity_max_ts),
        "entities_pending": len(pending),
        "rows_scanned": rows_scanned,
        "rows_pending": rows_pending,
        "rows_written": rows_written,
        "pending": pending,
        "errors": errors,
    }
