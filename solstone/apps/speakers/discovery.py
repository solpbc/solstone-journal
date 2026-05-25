# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unknown speaker discovery - cluster unmatched embeddings to find recurring voices."""

from __future__ import annotations

import json
import logging
from collections import defaultdict
from datetime import datetime
from pathlib import Path
from typing import Any

import numpy as np
from sklearn.cluster import HDBSCAN

from solstone.think.entities.core import atomic_write
from solstone.think.utils import day_dirs, day_path, get_journal, now_ms, segment_path

logger = logging.getLogger(__name__)

MIN_CLUSTER_SIZE = 5
MIN_SAMPLES = 3
MIN_SEGMENT_DIVERSITY = 3
MAX_UNMATCHED_EMBEDDINGS = 10000


def _routes_helpers():
    """Load speakers route helpers lazily to avoid import cycles."""
    from solstone.apps.speakers.routes import (
        _append_speaker_correction,
        _check_owner_contamination,
        _load_embeddings_file,
        _load_speaker_labels,
        _normalize_embedding,
        _save_speaker_labels,
        _scan_segment_embeddings,
    )

    return (
        _load_embeddings_file,
        _load_speaker_labels,
        _normalize_embedding,
        _save_speaker_labels,
        _scan_segment_embeddings,
        _append_speaker_correction,
        _check_owner_contamination,
    )


def _owner_helpers():
    """Load owner helpers lazily to avoid import cycles."""
    from solstone.apps.speakers.owner import load_owner_centroid

    return load_owner_centroid


def _audio_url(day: str, stream: str, segment_key: str, source: str) -> str:
    """Build the existing speakers audio-serving URL for a sample."""
    return f"/app/speakers/api/serve_audio/{day}/{stream}/{segment_key}/{source}.flac"


def _discovery_cache_path() -> Path:
    """Return the temporary cache path for discovery cluster assignments."""
    awareness_dir = Path(get_journal()) / "awareness"
    awareness_dir.mkdir(parents=True, exist_ok=True)
    return awareness_dir / "discovery_clusters.json"


def _discovery_resolved_path() -> Path:
    """Return the idempotency sentinel path for resolved discovery clusters."""
    return _discovery_cache_path().with_suffix(".resolved.json")


def load_resolved_cluster(cluster_id: int) -> dict[str, Any] | None:
    """Return cached identify result metadata for a resolved cluster."""
    path = _discovery_resolved_path()
    if not path.exists():
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError):
        return None
    entry = data.get(str(cluster_id))
    return entry if isinstance(entry, dict) else None


def _write_resolved_cluster(cluster_id: int, entity_id: str, label: str) -> None:
    path = _discovery_resolved_path()
    try:
        data = json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}
    except (json.JSONDecodeError, OSError):
        data = {}
    data[str(cluster_id)] = {
        "entity_id": entity_id,
        "label": label,
        "ts": datetime.utcnow().isoformat(timespec="seconds") + "Z",
    }
    atomic_write(path, json.dumps(data, indent=2, sort_keys=True), prefix=".discovery_")


def _get_sentence_text(segment_dir: Path, source: str, sentence_id: int) -> str | None:
    """Return transcript text for a sentence ID from the source transcript."""
    jsonl_path = segment_dir / f"{source}.jsonl"
    if not jsonl_path.exists():
        return None
    try:
        lines = jsonl_path.read_text(encoding="utf-8").splitlines()
        if sentence_id < 1 or sentence_id >= len(lines):
            return None
        entry = json.loads(lines[sentence_id])
        return entry.get("text")
    except (json.JSONDecodeError, OSError, IndexError):
        return None


def _clear_discovery_cache() -> None:
    """Remove the cached discovery assignment file if present."""
    _discovery_cache_path().unlink(missing_ok=True)
    _discovery_resolved_path().unlink(missing_ok=True)


def discover_unknown_speakers() -> dict[str, Any]:
    """Scan journal for recurring unknown speaker clusters."""
    load_owner_centroid = _owner_helpers()
    (
        load_embeddings_file,
        load_speaker_labels,
        normalize_embedding,
        _,
        scan_segment_embeddings,
        _,
        _,
    ) = _routes_helpers()

    centroid_data = load_owner_centroid()
    if centroid_data is None:
        _clear_discovery_cache()
        return {"clusters": []}

    owner_centroid = centroid_data.centroid
    owner_threshold = centroid_data.threshold
    embedding_chunks: list[np.ndarray] = []
    provenance: list[dict[str, Any]] = []

    for day in sorted(day_dirs().keys()):
        for segment in scan_segment_embeddings(day):
            stream = segment["stream"]
            seg_key = segment["key"]
            seg_dir = segment_path(day, seg_key, stream, create=False)

            labels_data = load_speaker_labels(seg_dir)
            attributed_sids: set[int] = set()
            if labels_data:
                for label in labels_data.get("labels", []):
                    sentence_id = label.get("sentence_id")
                    if label.get("speaker") is not None and sentence_id is not None:
                        attributed_sids.add(int(sentence_id))

            for source in segment["sources"]:
                emb_data = load_embeddings_file(seg_dir / f"{source}.npz")
                if emb_data is None:
                    continue

                embeddings, statement_ids, _ = emb_data
                if len(embeddings) == 0:
                    continue

                for emb, sid in zip(embeddings, statement_ids):
                    sid_int = int(sid)
                    if sid_int in attributed_sids:
                        continue

                    normalized = normalize_embedding(emb)
                    if normalized is None:
                        continue

                    score = float(np.dot(normalized, owner_centroid))
                    if score >= owner_threshold:
                        continue

                    embedding_chunks.append(normalized.reshape(1, -1))
                    provenance.append(
                        {
                            "day": day,
                            "stream": stream,
                            "segment_key": seg_key,
                            "source": source,
                            "sentence_id": sid_int,
                        }
                    )

    if not embedding_chunks:
        _clear_discovery_cache()
        return {"clusters": []}

    embeddings_matrix = np.vstack(embedding_chunks)
    if len(embeddings_matrix) > MAX_UNMATCHED_EMBEDDINGS:
        rng = np.random.default_rng(42)
        indices = rng.choice(
            len(embeddings_matrix),
            MAX_UNMATCHED_EMBEDDINGS,
            replace=False,
        )
        indices.sort()
        embeddings_matrix = embeddings_matrix[indices]
        provenance = [provenance[int(i)] for i in indices]

    if len(embeddings_matrix) < MIN_CLUSTER_SIZE:
        _clear_discovery_cache()
        return {"clusters": []}

    clusterer = HDBSCAN(
        min_cluster_size=MIN_CLUSTER_SIZE,
        min_samples=MIN_SAMPLES,
        metric="euclidean",
    )
    clusterer.fit(embeddings_matrix)
    labels = clusterer.labels_
    if np.all(labels == -1):
        _clear_discovery_cache()
        return {"clusters": []}

    result_clusters: list[dict[str, Any]] = []
    cache_clusters: dict[str, list[dict[str, Any]]] = {}

    for cid in sorted(set(labels[labels != -1])):
        cluster_indices = np.flatnonzero(labels == int(cid))
        segment_set = {
            (
                provenance[int(idx)]["day"],
                provenance[int(idx)]["stream"],
                provenance[int(idx)]["segment_key"],
            )
            for idx in cluster_indices
        }
        if len(segment_set) < MIN_SEGMENT_DIVERSITY:
            continue

        cluster_embeddings = embeddings_matrix[cluster_indices]
        centroid = normalize_embedding(np.mean(cluster_embeddings, axis=0))
        if centroid is None:
            continue

        similarities = np.dot(cluster_embeddings, centroid)
        sorted_positions = np.argsort(similarities)[::-1]

        samples: list[dict[str, Any]] = []
        seen_segments: set[tuple[str, str, str]] = set()

        for pos in sorted_positions:
            record = provenance[int(cluster_indices[int(pos)])]
            seg_triplet = (record["day"], record["stream"], record["segment_key"])
            if seg_triplet in seen_segments:
                continue
            seen_segments.add(seg_triplet)
            seg_dir = segment_path(
                record["day"], record["segment_key"], record["stream"], create=False
            )
            samples.append(
                {
                    **record,
                    "audio_url": _audio_url(
                        record["day"],
                        record["stream"],
                        record["segment_key"],
                        record["source"],
                    ),
                    "text": _get_sentence_text(
                        seg_dir,
                        record["source"],
                        record["sentence_id"],
                    )
                    or "",
                }
            )
            if len(samples) == 3:
                break

        if len(samples) < 3:
            for pos in sorted_positions:
                record = provenance[int(cluster_indices[int(pos)])]
                seg_dir = segment_path(
                    record["day"], record["segment_key"], record["stream"], create=False
                )
                sample = {
                    **record,
                    "audio_url": _audio_url(
                        record["day"],
                        record["stream"],
                        record["segment_key"],
                        record["source"],
                    ),
                    "text": _get_sentence_text(
                        seg_dir,
                        record["source"],
                        record["sentence_id"],
                    )
                    or "",
                }
                if sample in samples:
                    continue
                samples.append(sample)
                if len(samples) == 3:
                    break

        result_clusters.append(
            {
                "cluster_id": int(cid),
                "size": int(len(cluster_indices)),
                "segment_count": len(segment_set),
                "samples": samples,
            }
        )
        cache_clusters[str(int(cid))] = [provenance[int(i)] for i in cluster_indices]

    if not result_clusters:
        _clear_discovery_cache()
        return {"clusters": []}

    cache_path = _discovery_cache_path()
    tmp_path = cache_path.with_suffix(".tmp")
    with open(tmp_path, "w", encoding="utf-8") as f:
        json.dump(
            {
                "version": datetime.now().isoformat(),
                "clusters": cache_clusters,
            },
            f,
            indent=2,
        )
    tmp_path.rename(cache_path)

    result_clusters.sort(key=lambda cluster: cluster["size"], reverse=True)
    return {"clusters": result_clusters}


def identify_cluster(
    cluster_id: int, name: str, entity_id: str | None = None
) -> dict[str, Any]:
    """Identify a discovered unknown speaker cluster."""
    from solstone.think.entities import (
        load_existing_voiceprint_keys,
        save_voiceprints_batch,
    )

    (
        load_embeddings_file,
        load_speaker_labels,
        normalize_embedding,
        save_speaker_labels,
        _scan,
        append_speaker_correction,
        check_owner_contamination,
    ) = _routes_helpers()

    cache_path = _discovery_cache_path()
    if not cache_path.exists():
        return {"error": "No discovery scan results. Run scan first."}

    try:
        with open(cache_path, encoding="utf-8") as f:
            cache_data = json.load(f)
    except (json.JSONDecodeError, OSError):
        return {"error": "Invalid discovery cache. Run scan again."}

    cluster_members = cache_data.get("clusters", {}).get(str(cluster_id))
    if not cluster_members:
        return {"error": f"Cluster {cluster_id} not found in scan results."}

    from solstone.apps.speakers.routes import _load_speaker_corrections
    from solstone.think.entities import entity_slug, find_matching_entity
    from solstone.think.entities.journal import (
        create_journal_entity,
        load_all_journal_entities,
        load_journal_entity,
    )

    entity_created = False

    if entity_id:
        # Direct entity ID — load it
        entity = load_journal_entity(entity_id)
        if not entity:
            return {"error": f"Entity '{entity_id}' not found."}
        entity_name = entity.get("name", name)
    else:
        journal_entities = load_all_journal_entities()
        entities_list = [
            entity for entity in journal_entities.values() if not entity.get("blocked")
        ]

        entity = find_matching_entity(name, entities_list)
        if entity:
            entity_id = entity["id"]
            entity_name = entity.get("name", name)
        else:
            entity_id = entity_slug(name)
            existing = load_journal_entity(entity_id)
            entity_created = existing is None
            entity = existing or create_journal_entity(
                entity_id=entity_id,
                name=name,
                entity_type="Person",
            )
            entity_name = entity.get("name", name)

    existing_keys = load_existing_voiceprint_keys(entity_id)
    vp_batch: list[tuple[np.ndarray, dict[str, Any]]] = []

    for member in cluster_members:
        day = member["day"]
        stream = member["stream"]
        seg_key = member["segment_key"]
        source = member["source"]
        sentence_id = int(member["sentence_id"])

        vp_key = (day, seg_key, source, sentence_id)
        if vp_key in existing_keys:
            continue

        seg_dir = segment_path(day, seg_key, stream)
        emb_data = load_embeddings_file(seg_dir / f"{source}.npz")
        if emb_data is None:
            continue

        embeddings, statement_ids, _ = emb_data
        emb_vec = None
        for emb, sid in zip(embeddings, statement_ids):
            if int(sid) == sentence_id:
                emb_vec = normalize_embedding(emb)
                break

        if emb_vec is None or check_owner_contamination(emb_vec):
            continue

        vp_batch.append(
            (
                emb_vec,
                {
                    "day": day,
                    "segment_key": seg_key,
                    "source": source,
                    "stream": stream,
                    "sentence_id": sentence_id,
                    "added_at": now_ms(),
                },
            )
        )
        existing_keys.add(vp_key)

    voiceprints_saved = save_voiceprints_batch(entity_id, vp_batch) if vp_batch else 0

    segments_map: dict[tuple[str, str, str], list[int]] = defaultdict(list)
    for member in cluster_members:
        key = (member["day"], member["stream"], member["segment_key"])
        segments_map[key].append(int(member["sentence_id"]))

    segments_updated = 0
    sentences_attributed = 0
    timestamp = now_ms()

    for (day, stream, seg_key), sentence_ids in segments_map.items():
        seg_dir_check = day_path(day, create=False) / stream / seg_key
        if not seg_dir_check.is_dir():
            continue
        seg_dir = seg_dir_check
        labels_data = load_speaker_labels(seg_dir)
        if labels_data is None:
            labels_data = {
                "labels": [],
                "owner_centroid_last_refreshed_at": None,
                "voiceprint_versions": {},
            }

        labels_by_sid: dict[int, dict[str, Any]] = {}
        for label in labels_data.get("labels", []):
            sentence_id = label.get("sentence_id")
            if sentence_id is not None:
                labels_by_sid[int(sentence_id)] = label

        existing_correction_keys = {
            (
                correction.get("sentence_id"),
                correction.get("corrected_speaker"),
            )
            for correction in _load_speaker_corrections(seg_dir)
        }

        updated = False
        for sentence_id in sorted(set(sentence_ids)):
            original = labels_by_sid.get(sentence_id, {})
            new_label = {
                "sentence_id": sentence_id,
                "speaker": entity_id,
                "confidence": "high",
                "method": "user_identified",
            }
            if original != new_label:
                updated = True
                sentences_attributed += 1
            labels_by_sid[sentence_id] = new_label

            correction_key = (sentence_id, entity_id)
            if correction_key in existing_correction_keys:
                continue

            append_speaker_correction(
                seg_dir,
                {
                    "sentence_id": sentence_id,
                    "original_speaker": original.get("speaker"),
                    "corrected_speaker": entity_id,
                    "original_method": original.get("method"),
                    "timestamp": timestamp,
                },
            )
            existing_correction_keys.add(correction_key)

        if updated:
            labels_data["labels"] = sorted(
                labels_by_sid.values(),
                key=lambda label: int(label["sentence_id"]),
            )
            save_speaker_labels(seg_dir, labels_data)
            segments_updated += 1

    _write_resolved_cluster(cluster_id, entity_id, entity_name)

    return {
        "status": "identified",
        "entity_id": entity_id,
        "entity_name": entity_name,
        "entity_created": entity_created,
        "voiceprints_saved": voiceprints_saved,
        "segments_updated": segments_updated,
        "sentences_attributed": sentences_attributed,
    }
