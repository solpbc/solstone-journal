# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Speaker subsystem status aggregation."""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any

from solstone.think.awareness import get_current
from solstone.think.utils import day_dirs, get_journal

logger = logging.getLogger(__name__)

SECTIONS = ("embeddings", "owner", "speakers", "clusters", "imports", "attribution")


def get_speakers_status(section: str | None = None) -> Any:
    """Aggregate speaker subsystem status.

    Args:
        section: Optional section name to return. If None, returns all sections.

    Returns:
        Dict with all sections, or a single section's value if section is specified.
    """
    builders = {
        "embeddings": _embeddings_section,
        "owner": _owner_section,
        "speakers": _speakers_section,
        "clusters": _clusters_section,
        "imports": _imports_section,
        "attribution": _attribution_section,
    }

    if section:
        builder = builders.get(section)
        if builder is None:
            return {
                "error": f"Unknown section '{section}'. Valid: {', '.join(SECTIONS)}"
            }
        return builder()

    return {name: builder() for name, builder in builders.items()}


def _embeddings_section() -> dict[str, Any]:
    from solstone.apps.speakers.routes import _scan_segment_embeddings

    segments = 0
    streams: dict[str, int] = {}
    days_seen: set[str] = set()

    for day in day_dirs().keys():
        day_segments = _scan_segment_embeddings(day)
        if day_segments:
            days_seen.add(day)
        for seg in day_segments:
            segments += 1
            stream = seg["stream"]
            streams[stream] = streams.get(stream, 0) + 1

    sorted_days = sorted(days_seen) if days_seen else []
    return {
        "segments": segments,
        "streams": streams,
        "days": len(sorted_days),
        "date_range": [sorted_days[0], sorted_days[-1]] if sorted_days else None,
    }


def _owner_section() -> dict[str, Any]:
    from solstone.apps.speakers.owner import (
        load_owner_bootstrap_diagnostics,
        load_owner_centroid,
    )
    from solstone.think.entities.journal import get_journal_principal

    voiceprint = get_current().get("voiceprint", {})
    status = voiceprint.get("status", "none")
    result: dict[str, Any] = {"status": status}
    principal = get_journal_principal()
    principal_id = str(principal["id"]) if principal else None
    diagnostics = load_owner_bootstrap_diagnostics(principal_id)

    if status == "candidate":
        result["cluster_size"] = voiceprint.get("cluster_size")
        result["detected_at"] = voiceprint.get("detected_at")
        result["streams_represented"] = voiceprint.get("streams_represented")
        result["recommendation"] = voiceprint.get("recommendation")
    elif status == "low_quality":
        result["source"] = voiceprint.get("source", "hdbscan")
        result["low_quality_reason"] = voiceprint.get("low_quality_reason", "")
        result["observed_value"] = voiceprint.get("observed_value", 0.0)
        result["threshold_value"] = voiceprint.get("threshold_value", 0.0)
        result["segments_checked"] = voiceprint.get("segments_checked", 0)
        result["attempted_at"] = voiceprint.get("attempted_at", "")
        result.update(diagnostics)
    elif status == "no_cluster":
        result["segments_checked"] = voiceprint.get("segments_checked")
        result["attempted_at"] = voiceprint.get("attempted_at")
    elif status in {"none", "rejected"}:
        result.update(diagnostics)

    centroid = load_owner_centroid()
    result["centroid_saved"] = centroid is not None
    if status == "confirmed" and centroid is not None:
        result["centroid_metadata"] = {
            "cluster_size": centroid.cluster_size,
            "streams": centroid.streams,
            "last_refreshed_at": centroid.last_refreshed_at,
            "intra_cosine_p25": centroid.intra_cosine_p25,
        }
    return result


def _speakers_section() -> list[dict[str, Any]]:
    from solstone.apps.speakers.owner import compute_intra_cosine_p25
    from solstone.think.entities.journal import (
        load_journal_entity,
        scan_journal_entities,
    )
    from solstone.think.entities.voiceprints import load_entity_voiceprints_file

    speakers = []
    for entity_id in scan_journal_entities():
        result = load_entity_voiceprints_file(entity_id)
        if result is None:
            continue

        embeddings, metadata_list = result
        streams: set[str] = set()
        segments: set[tuple[str, str]] = set()
        last_seen_values: list[int] = []
        for metadata in metadata_list:
            stream = metadata.get("stream")
            if isinstance(stream, str) and stream:
                streams.add(stream)
            segments.add((metadata.get("day", ""), metadata.get("segment_key", "")))
            last_seen_ts = metadata.get("last_seen_ts")
            if isinstance(last_seen_ts, int):
                last_seen_values.append(last_seen_ts)

        entity = load_journal_entity(entity_id) or {}
        speakers.append(
            {
                "entity_id": entity_id,
                "name": entity.get("name", entity_id),
                "embedding_count": len(embeddings),
                "segment_count": len(segments),
                "streams": sorted(streams),
                "last_seen_ts": max(last_seen_values) if last_seen_values else None,
                "intra_cosine_p25": compute_intra_cosine_p25(embeddings),
            }
        )

    return speakers


def _clusters_section() -> dict[str, Any] | None:
    cache_path = Path(get_journal()) / "awareness" / "discovery_clusters.json"
    if not cache_path.exists():
        return None
    try:
        data = json.loads(cache_path.read_text())
        clusters = data.get("clusters", [])
        return {
            "cached_at": data.get("version"),
            "count": len(clusters),
            "clusters": clusters,
        }
    except Exception:
        logger.warning("Failed to read discovery cache", exc_info=True)
        return None


def _imports_section() -> dict[str, Any]:
    meetings = 0
    screens = 0

    for _day, day_abs in day_dirs().items():
        day_dir = Path(day_abs)
        if not day_dir.is_dir():
            continue
        for stream_dir in sorted(day_dir.iterdir()):
            if not stream_dir.is_dir():
                continue
            for seg_dir in sorted(stream_dir.iterdir()):
                if not seg_dir.is_dir():
                    continue
                if (seg_dir / "meetings.md").exists():
                    meetings += 1
                if (seg_dir / "screen.md").exists():
                    screens += 1

    return {"meetings_files": meetings, "screen_files": screens}


def _attribution_section() -> dict[str, Any]:
    total_files = 0
    total_labels = 0
    by_confidence: dict[str, int] = {}
    by_method: dict[str, int] = {}

    for _day, day_abs in day_dirs().items():
        day_dir = Path(day_abs)
        if not day_dir.is_dir():
            continue
        for stream_dir in sorted(day_dir.iterdir()):
            if not stream_dir.is_dir():
                continue
            for seg_dir in sorted(stream_dir.iterdir()):
                if not seg_dir.is_dir():
                    continue
                labels_file = seg_dir / "talents" / "speaker_labels.json"
                if not labels_file.exists():
                    continue
                try:
                    data = json.loads(labels_file.read_text())
                except Exception:
                    continue
                total_files += 1
                for label in data.get("labels", []):
                    total_labels += 1
                    confidence = label.get("confidence", "unknown")
                    method = label.get("method", "unknown")
                    by_confidence[confidence] = by_confidence.get(confidence, 0) + 1
                    by_method[method] = by_method.get(method, 0) + 1

    return {
        "files": total_files,
        "labels": total_labels,
        "by_confidence": by_confidence,
        "by_method": by_method,
    }
