# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Speaker attribution engine — 4-layer per-segment pipeline.

Runs per-segment after transcription and embedding.  Operates in layers
from cheapest to most expensive:

Layer 1: Owner separation (cosine similarity to owner centroid passes Layer 1)
Layer 2: Structural heuristics (speaker count, setting field, screen.md,
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
import re
from collections import defaultdict
from pathlib import Path
from typing import Any

import numpy as np

from solstone.apps.speakers._overlap import _read_segment_overlap_fraction
from solstone.apps.speakers.encoder_config import (
    ACOUSTIC_HIGH,
    ACOUSTIC_MEDIUM,
    NOISY_FLYWHEEL_OVERLAP_MAX,
)
from solstone.apps.speakers.owner import load_owner_centroid
from solstone.apps.speakers.time import segment_start_ts_ms
from solstone.think.entities import find_matching_entity
from solstone.think.entities.journal import (
    get_journal_principal,
    load_all_journal_entities,
)
from solstone.think.utils import day_path, now_ms, segment_path

logger = logging.getLogger(__name__)


def _routes_helpers():
    """Load speakers route helpers lazily to avoid import cycles."""
    from solstone.apps.speakers.routes import (
        _load_embeddings_file,
        _load_entity_voiceprints_file,
        _load_segment_speakers,
        _normalize_embedding,
    )

    return (
        _load_embeddings_file,
        _normalize_embedding,
        _load_segment_speakers,
        _load_entity_voiceprints_file,
    )


# ---------------------------------------------------------------------------
# Layer 2 helpers: structural signal parsing
# ---------------------------------------------------------------------------


def _parse_setting_names(setting: str) -> list[str]:
    """Parse participant names from an import setting field.

    Examples:
        "Jer and Jack at coffee" -> ["Jack"]
        "Meeting with Perry and Thomas" -> ["Perry", "Thomas"]
        "Lunch with John Borthwick" -> ["John Borthwick"]
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
    owner_names = {"jer", "jeremie", "jeremy", "jeremie miller"}
    names: list[str] = []
    for part in parts:
        part = part.strip()
        if part and len(part) > 1 and part.lower() not in owner_names:
            names.append(part)
    return names


def _load_setting_field(seg_dir: Path) -> str | None:
    """Read the setting field from the first line of imported_audio.jsonl."""
    jsonl_path = seg_dir / "imported_audio.jsonl"
    if not jsonl_path.exists():
        return None
    try:
        with open(jsonl_path, encoding="utf-8") as f:
            first_line = f.readline().strip()
        if first_line:
            return json.loads(first_line).get("setting")
    except Exception:
        pass
    return None


def _extract_screen_participants(seg_dir: Path) -> list[str]:
    """Extract participant names from screen.md agent output.

    screen.md captures video-call participant panels.  The content is
    free-form markdown so extraction is best-effort.
    """
    screen_path = seg_dir / "talents" / "screen.md"
    if not screen_path.exists():
        return []
    try:
        content = screen_path.read_text(encoding="utf-8")
    except Exception:
        return []

    names: list[str] = []
    kw_re = re.compile(
        r"participant|attendee|joined|present|member|panelist",
        re.IGNORECASE,
    )
    for line in content.splitlines():
        if not kw_re.search(line):
            continue
        # Strip markdown formatting
        clean = re.sub(r"[*_#\-\[\]>]", "", line)
        # Take text after the label
        after_label = re.split(r"[:–—\-]\s*", clean, maxsplit=1)
        name_text = after_label[-1] if len(after_label) > 1 else clean
        for part in re.split(r"[,;]", name_text):
            part = part.strip()
            if (
                part
                and len(part) > 2
                and not kw_re.search(part)
                and not part.lower().startswith(("the ", "a "))
            ):
                names.append(part)
    return names


def _extract_meeting_participants(day: str, segment_key: str) -> list[str]:
    """Extract participant names from daily meetings.md."""
    meetings_path = day_path(day) / "talents" / "meetings.md"
    if not meetings_path.exists():
        return []
    try:
        content = meetings_path.read_text(encoding="utf-8")
    except Exception:
        return []

    names: list[str] = []
    part_re = re.compile(r"\*\*Participants?\*\*\s*[:–—\-]\s*(.*)", re.IGNORECASE)
    for line in content.splitlines():
        m = part_re.search(line)
        if m:
            for name in re.split(r"[,;]", m.group(1)):
                name = name.strip().strip("*").strip()
                if name and len(name) > 1:
                    names.append(name)
    return names


# ---------------------------------------------------------------------------
# Core attribution pipeline
# ---------------------------------------------------------------------------


def attribute_segment(
    day: str,
    stream: str,
    segment_key: str,
) -> dict[str, Any]:
    """Run Layers 1-3 of speaker attribution for a segment.

    Returns a result dict containing:
        labels           - list of per-sentence label dicts
        unmatched        - sentence IDs still needing Layer 4
        unmatched_texts  - {sentence_id: text} for LLM context
        source           - audio source stem processed
        candidates       - list of candidate speaker names
        candidate_entity_ids - resolved entity IDs
        metadata         - owner centroid refresh timestamp + voiceprint counts
    """
    (
        load_embeddings_file,
        normalize_embedding,
        load_segment_speakers,
        load_entity_voiceprints_file,
    ) = _routes_helpers()

    seg_dir = segment_path(day, segment_key, stream)

    # --- prerequisite: owner centroid ---
    centroid_data = load_owner_centroid()
    if centroid_data is None:
        return {"error": "no_owner_centroid", "labels": [], "unmatched": []}

    owner_centroid = centroid_data.centroid
    owner_threshold = centroid_data.threshold

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

    # ==========================
    # LAYER 1: Owner separation
    # ==========================
    non_owner_sids: list[int] = []

    for emb, sid in zip(embeddings, statement_ids):
        sid_int = int(sid)
        normalized = normalize_embedding(emb)
        if normalized is None:
            continue
        score = float(np.dot(normalized, owner_centroid))
        if score >= owner_threshold:
            labels[sid_int] = {
                "sentence_id": sid_int,
                "speaker": owner_entity_id,
                "confidence": "high",
                "method": "owner_centroid",
            }
        else:
            non_owner_sids.append(sid_int)

    # ================================
    # LAYER 2: Structural heuristics
    # ================================
    speakers = load_segment_speakers(seg_dir)
    setting = _load_setting_field(seg_dir)
    setting_names = _parse_setting_names(setting) if setting else []
    screen_names = _extract_screen_participants(seg_dir)
    meeting_names = _extract_meeting_participants(day, segment_key)

    # Deduplicate, preserve order
    candidate_names: list[str] = list(
        dict.fromkeys(speakers + setting_names + screen_names + meeting_names)
    )

    # Resolve candidates to entities
    candidate_entities: dict[str, dict] = {}
    for name in candidate_names:
        entity = find_matching_entity(name, entities_list)
        if entity:
            candidate_entities[entity["id"]] = entity

    # 2a: single-listed-speaker — all non-owner sentences belong to them
    if len(speakers) == 1:
        entity = find_matching_entity(speakers[0], entities_list)
        if entity:
            for sid in non_owner_sids:
                if labels[sid]["speaker"] is None:
                    labels[sid] = {
                        "sentence_id": sid,
                        "speaker": entity["id"],
                        "confidence": "high",
                        "method": "structural_single_speaker",
                    }

    # 2b: single setting-field participant (import segments without speakers.json)
    elif not speakers and len(setting_names) == 1:
        entity = find_matching_entity(setting_names[0], entities_list)
        if entity:
            for sid in non_owner_sids:
                if labels[sid]["speaker"] is None:
                    labels[sid] = {
                        "sentence_id": sid,
                        "speaker": entity["id"],
                        "confidence": "high",
                        "method": "structural_setting",
                    }

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
        voiceprint_centroids: dict[str, np.ndarray] = {}
        for eid in vp_entity_ids:
            result = load_entity_voiceprints_file(eid)
            if result is None:
                continue
            vp_embs, vp_meta = result
            if len(vp_embs) == 0:
                continue
            voiceprint_versions[eid] = len(vp_embs)

            same_stream: list[np.ndarray] = []
            all_embs: list[np.ndarray] = []
            for ve, vm in zip(vp_embs, vp_meta):
                n = normalize_embedding(ve)
                if n is not None:
                    all_embs.append(n)
                    if vm.get("stream") == stream:
                        same_stream.append(n)

            basis = same_stream if len(same_stream) >= 5 else all_embs
            if not basis:
                continue
            centroid = normalize_embedding(np.mean(basis, axis=0))
            if centroid is not None:
                voiceprint_centroids[eid] = centroid

        # Build sentence-to-embedding index
        sid_to_idx = {int(s): i for i, s in enumerate(statement_ids)}

        for sid in unresolved:
            idx = sid_to_idx.get(sid)
            if idx is None:
                continue
            normalized = normalize_embedding(embeddings[idx])
            if normalized is None:
                continue

            best_eid: str | None = None
            best_score = 0.0
            for eid, centroid in voiceprint_centroids.items():
                score = float(np.dot(normalized, centroid))
                if score > best_score:
                    best_score = score
                    best_eid = eid

            if best_eid is not None:
                if best_score >= ACOUSTIC_HIGH:
                    labels[sid] = {
                        "sentence_id": sid,
                        "speaker": best_eid,
                        "confidence": "high",
                        "method": "acoustic",
                    }
                elif best_score >= ACOUSTIC_MEDIUM:
                    labels[sid] = {
                        "sentence_id": sid,
                        "speaker": best_eid,
                        "confidence": "medium",
                        "method": "acoustic",
                    }

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
        "candidate_entity_ids": list(candidate_entities.keys()),
        "metadata": {
            "owner_centroid_last_refreshed_at": owner_refreshed_at,
            "voiceprint_versions": voiceprint_versions,
        },
    }


# ---------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------


def save_speaker_labels(
    seg_dir: Path,
    labels: list[dict],
    metadata: dict[str, Any],
) -> Path:
    """Write speaker_labels.json to the segment's agents/ directory.

    Preserves user corrections: if speaker_corrections.json exists, any
    sentence that was corrected by the user keeps the corrected attribution
    rather than being overwritten by a fresh pipeline run.
    """
    agents_dir = seg_dir / "talents"
    agents_dir.mkdir(parents=True, exist_ok=True)

    # Load existing corrections to preserve user overrides
    corr_path = agents_dir / "speaker_corrections.json"
    corrected: dict[int, dict] = {}
    if corr_path.is_file():
        try:
            with open(corr_path, encoding="utf-8") as f:
                corr_data = json.load(f)
            for entry in corr_data.get("corrections", []):
                sid = entry.get("sentence_id")
                if sid is not None:
                    # Keep the latest correction per sentence
                    corrected[int(sid)] = entry
        except (json.JSONDecodeError, OSError):
            pass

    # Apply corrections on top of pipeline labels
    if corrected:
        for label in labels:
            sid = label.get("sentence_id")
            if sid is not None and int(sid) in corrected:
                corr = corrected[int(sid)]
                speaker = corr.get("corrected_speaker")
                if speaker is not None:
                    label["speaker"] = speaker
                    label["confidence"] = "high"
                    # Determine method from correction type
                    if corr.get("original_speaker") == speaker:
                        label["method"] = "user_confirmed"
                    elif corr.get("original_speaker") is None:
                        label["method"] = "user_assigned"
                    else:
                        label["method"] = "user_corrected"
        logger.info(
            "Preserved %d user corrections in %s",
            len(corrected),
            seg_dir,
        )

    out_path = agents_dir / "speaker_labels.json"
    data = {
        "labels": labels,
        "owner_centroid_last_refreshed_at": metadata.get(
            "owner_centroid_last_refreshed_at"
        ),
        "voiceprint_versions": metadata.get("voiceprint_versions", {}),
    }
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)

    logger.info("Wrote %d labels to %s", len(labels), out_path)
    return out_path


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
    from solstone.think.entities import (
        load_existing_voiceprint_keys,
        save_voiceprints_batch,
    )

    (
        load_embeddings_file,
        normalize_embedding,
        _,
        _,
    ) = _routes_helpers()

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
    last_seen_ts = segment_start_ts_ms(day, segment_key)

    # Eligible methods for accumulation
    eligible_methods = {
        "structural_single_speaker",
        "structural_setting",
        "acoustic",
    }

    principal = get_journal_principal()
    owner_entity_id = principal["id"] if principal else None

    # Collect per-entity
    entity_new: dict[str, list[tuple[np.ndarray, dict]]] = defaultdict(list)
    entity_existing: dict[str, set] = {}
    saved_counts: dict[str, int] = {}

    for label in labels:
        if label.get("confidence") != "high":
            continue
        if label.get("method") not in eligible_methods:
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
            entity_existing[speaker] = load_existing_voiceprint_keys(speaker)
        vp_key = (day, segment_key, source, sid)
        if vp_key in entity_existing[speaker]:
            continue

        metadata = {
            "day": day,
            "segment_key": segment_key,
            "source": source,
            "stream": stream,
            "sentence_id": sid,
            "added_at": now_ms(),
            "last_seen_ts": last_seen_ts,
        }
        entity_new[speaker].append((normalized, metadata))
        entity_existing[speaker].add(vp_key)

    for eid, items in entity_new.items():
        try:
            count = save_voiceprints_batch(eid, items)
            saved_counts[eid] = count
        except Exception as exc:
            logger.warning("Failed to accumulate voiceprints for %s: %s", eid, exc)

    return saved_counts


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
        if _has_speaker_labels(seg_path):
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

    for i, (day_name, stream_name, seg_key, seg_path) in enumerate(to_process, 1):
        try:
            result = attribute_segment(day_name, stream_name, seg_key)

            if result.get("error"):
                stats["errors"].append(
                    f"{day_name}/{stream_name}/{seg_key}: {result['error']}"
                )
                if progress_callback:
                    progress_callback(i, total_to_do, day_name, stream_name, seg_key)
                continue

            labels = result.get("labels", [])
            metadata = result.get("metadata", {})
            source = result.get("source")

            # Save labels
            save_speaker_labels(seg_path, labels, metadata)

            # Accumulate voiceprints
            if source:
                accumulate_voiceprints(day_name, stream_name, seg_key, labels, source)

            # Track speakers
            for lab in labels:
                speaker = lab.get("speaker")
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
    from solstone.think.entities.voiceprints import (
        load_entity_voiceprints_file,
        rewrite_voiceprint_metadata,
    )
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

        def mutator(rows: list[dict], *, target_ts: int = max_ts) -> int:
            changed = 0
            for row in rows:
                if needs_update(row, target_ts):
                    row["last_seen_ts"] = target_ts
                    changed += 1
            return changed

        rows_written += rewrite_voiceprint_metadata(entity_id, mutator)

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
