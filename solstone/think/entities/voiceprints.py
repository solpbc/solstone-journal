# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared voiceprint helpers for entity-aware speaker workflows."""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Any, Callable

from solstone.think.entities.journal import (
    ensure_journal_entity_memory,
    get_journal_principal,
    journal_entity_memory_path,
)
from solstone.think.journal_io.errors import MalformedDataError
from solstone.think.journal_io.npz import load_npz, update_npz

if TYPE_CHECKING:
    import numpy as np

logger = logging.getLogger(__name__)
VOICEPRINT_KEYS = ("embeddings", "metadata")


@dataclass(frozen=True)
class OwnerCentroid:
    """Confirmed owner centroid plus browser-facing metadata."""

    centroid: np.ndarray
    threshold: float
    cluster_size: int
    last_refreshed_at: str
    intra_cosine_p25: float | None
    streams: list[str]
    created_at: str | None = None
    evidence_hash: str | None = None
    evidence_intra_cosine_p25: float | None = None
    margin: float | None = None
    evidence_tier: str = "standard"


def normalize_embedding(emb: np.ndarray) -> np.ndarray | None:
    """L2-normalize an embedding vector. Returns None if norm is zero."""
    import numpy as np

    emb = emb.astype(np.float32)
    norm = np.linalg.norm(emb)
    if norm > 0:
        return emb / norm
    return None


def _pairwise_cosines(embeddings: np.ndarray) -> np.ndarray:
    """Return pairwise cosine similarities for a cluster of embeddings."""
    import numpy as np

    n = embeddings.shape[0]
    if n < 2:
        return np.empty(0, dtype=np.float32)
    norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
    norms = np.where(norms < 1e-9, 1.0, norms)
    e_norm = embeddings / norms
    if n > 5000:
        rng = np.random.default_rng(seed=0)
        i = rng.integers(0, n, size=1000)
        j = rng.integers(0, n, size=1000)
        mask = i != j
        i = i[mask]
        j = j[mask]
        return np.einsum("ij,ij->i", e_norm[i], e_norm[j]).astype(
            np.float32, copy=False
        )
    sim = e_norm @ e_norm.T
    iu = np.triu_indices(n, k=1)
    return sim[iu].astype(np.float32, copy=False)


def compute_intra_cosine_p25(embeddings: np.ndarray) -> float | None:
    """Return p25 pairwise cosine for embeddings, or None when unavailable."""
    import numpy as np

    cosines = _pairwise_cosines(np.asarray(embeddings, dtype=np.float32))
    if cosines.size == 0:
        return None
    return float(np.percentile(cosines, 25))


def load_embeddings_file(
    npz_path: Path,
) -> tuple[np.ndarray, np.ndarray, np.ndarray | None] | None:
    """Load embeddings, statement ids, and optional durations from an NPZ file."""
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
    except Exception as exc:
        logger.warning("Failed to load embeddings %s: %s", npz_path, exc)
        return None


def load_entity_voiceprints_file(
    entity_id: str,
) -> tuple[np.ndarray, list[dict]] | None:
    """Load an entity's voiceprints.npz, returning embeddings and parsed metadata."""
    try:
        folder = journal_entity_memory_path(entity_id)
    except (RuntimeError, ValueError):
        return None

    npz_path = folder / "voiceprints.npz"
    if not npz_path.exists():
        return None

    try:
        data = load_npz(npz_path)
        if data is None:
            return None
        embeddings = data.get("embeddings")
        metadata_arr = data.get("metadata")
        if embeddings is None or metadata_arr is None:
            return None
        metadata_list = [json.loads(str(m)) for m in metadata_arr]
        return embeddings, metadata_list
    except Exception as exc:
        logger.warning("Failed to load voiceprints for entity %s: %s", entity_id, exc)
        return None


def _array_string(data: Any) -> str | None:
    """Return a scalar NPZ string value, or None when absent."""
    if data is None:
        return None
    import numpy as np

    value = np.asarray(data).item()
    if value is None:
        return None
    return str(value)


def _array_float(data: Any) -> float | None:
    """Return a scalar NPZ float value, or None when absent."""
    if data is None:
        return None
    import numpy as np

    value = np.asarray(data).item()
    if value is None:
        return None
    return float(value)


def _load_owner_voiceprint_summary(
    principal_id: str,
) -> tuple[float | None, list[str]]:
    """Compute owner cohesion and stream list from the principal voiceprints."""
    voiceprints = load_entity_voiceprints_file(principal_id)
    if voiceprints is None:
        return None, []

    embeddings, metadata = voiceprints
    streams = sorted(
        {
            str(item["stream"])
            for item in metadata
            if isinstance(item.get("stream"), str) and item.get("stream")
        }
    )
    return compute_intra_cosine_p25(embeddings), streams


def load_owner_centroid() -> OwnerCentroid | None:
    """Load the confirmed owner centroid and metadata for the principal entity."""
    import numpy as np

    principal = get_journal_principal()
    if not principal:
        return None

    principal_id = str(principal["id"])
    centroid_path = journal_entity_memory_path(principal_id) / "owner_centroid.npz"
    if not centroid_path.exists():
        return None

    try:
        data = load_npz(centroid_path)
        if data is None:
            return None
        centroid = data.get("centroid")
        threshold = data.get("threshold")
        cluster_size = data.get("cluster_size")
        last_refreshed_at = data.get("last_refreshed_at")
        created_at = data.get("created_at")
        evidence_hash = data.get("evidence_hash")
        evidence_intra_p25 = data.get("evidence_intra_cosine_p25")
        margin = data.get("margin")
        evidence_tier_data = data.get("evidence_tier")
        if centroid is None or threshold is None or cluster_size is None:
            return None

        normalized = centroid.astype(np.float32).reshape(-1)
        norm = np.linalg.norm(normalized)
        if norm == 0:
            return None
        normalized = normalized / norm
        refreshed = _array_string(last_refreshed_at) or ""
        size = int(np.asarray(cluster_size).item())
        thresh = float(np.asarray(threshold).item())
        # Centroids written before evidence tiering are standard-tier evidence.
        evidence_tier = _array_string(evidence_tier_data) or "standard"

        intra_p25, streams = _load_owner_voiceprint_summary(principal_id)
        return OwnerCentroid(
            centroid=normalized,
            threshold=thresh,
            cluster_size=size,
            last_refreshed_at=refreshed,
            intra_cosine_p25=intra_p25,
            streams=streams,
            created_at=_array_string(created_at),
            evidence_hash=_array_string(evidence_hash),
            evidence_intra_cosine_p25=_array_float(evidence_intra_p25),
            margin=_array_float(margin),
            evidence_tier=evidence_tier,
        )
    except Exception as exc:
        logger.warning("Failed to load owner centroid %s: %s", centroid_path, exc)
        return None

def load_existing_voiceprint_keys(entity_id: str) -> set[tuple]:
    """Return saved voiceprint identity keys for idempotency checks."""
    result = load_entity_voiceprints_file(entity_id)
    if result is None:
        return set()

    _, metadata_list = result
    return {
        (m.get("day"), m.get("segment_key"), m.get("source"), m.get("sentence_id"))
        for m in metadata_list
    }


def save_voiceprints_batch(
    entity_id: str,
    new_items: list[tuple[np.ndarray, dict]],
) -> int:
    """Append a batch of normalized voiceprints to an entity in one write."""
    import numpy as np

    if not new_items:
        return 0

    folder = ensure_journal_entity_memory(entity_id)
    npz_path = folder / "voiceprints.npz"

    new_emb_list = [emb.reshape(1, -1).astype(np.float32) for emb, _ in new_items]
    new_meta_dicts = [meta_dict for _, meta_dict in new_items]
    new_emb_np = np.vstack(new_emb_list)

    def transform(current: dict[str, np.ndarray]) -> dict[str, np.ndarray]:
        if current:
            try:
                existing_emb = current["embeddings"]
                existing_meta_strings = current["metadata"]
            except KeyError as exc:
                raise MalformedDataError(npz_path) from exc
            existing_meta_dicts = [json.loads(str(m)) for m in existing_meta_strings]
        else:
            existing_emb = np.empty((0, 256), dtype=np.float32)
            existing_meta_dicts = []

        combined_emb = (
            np.vstack([existing_emb, new_emb_np])
            if len(existing_emb) > 0
            else new_emb_np
        ).astype(np.float32, copy=False)
        combined_meta_dicts = existing_meta_dicts + new_meta_dicts
        return {
            "embeddings": combined_emb,
            "metadata": np.asarray(
                [json.dumps(item) for item in combined_meta_dicts],
                dtype=str,
            ),
        }

    update_npz(npz_path, transform, expected_keys=VOICEPRINT_KEYS)
    return len(new_items)


def rewrite_voiceprint_metadata(
    entity_id: str,
    mutator: Callable[[list[dict]], int],
) -> int:
    """Rewrite an entity's voiceprint metadata in place when mutator changes rows."""
    try:
        folder = journal_entity_memory_path(entity_id)
    except (RuntimeError, ValueError):
        return 0

    npz_path = folder / "voiceprints.npz"
    if not npz_path.exists():
        return 0

    updates = 0

    def transform(current: dict[str, np.ndarray]) -> dict[str, np.ndarray] | None:
        import numpy as np

        nonlocal updates
        embeddings = current.get("embeddings")
        metadata_arr = current.get("metadata")
        if embeddings is None or metadata_arr is None:
            return None
        metadata = [json.loads(str(item)) for item in metadata_arr]

        updates = mutator(metadata)
        if updates <= 0:
            return None
        return {
            "embeddings": embeddings,
            "metadata": np.asarray([json.dumps(item) for item in metadata], dtype=str),
        }

    update_npz(npz_path, transform, expected_keys=VOICEPRINT_KEYS)
    return updates


def apply_entity_merge_voiceprint_inverse(
    entity_id: str,
    support: list[dict[str, Any]],
    active_support: list[list[dict[str, Any]]],
) -> int:
    """Remove merge-added voiceprints under the voiceprint npz owner lock."""

    active_keys = {
        tuple(entry.get("key", [])) for section in active_support for entry in section
    }
    removable = [
        entry
        for entry in support
        if entry.get("delta_applied")
        and not entry.get("target_preexisting")
        and tuple(entry.get("key", [])) not in active_keys
    ]
    if not removable:
        return 0

    try:
        folder = journal_entity_memory_path(entity_id)
    except (RuntimeError, ValueError) as exc:
        raise ValueError(f"voiceprint inverse target not found: {entity_id}") from exc
    npz_path = folder / "voiceprints.npz"
    if not npz_path.exists():
        raise ValueError(f"voiceprint inverse target file not found: {npz_path}")

    removed = 0

    def transform(current: dict[str, np.ndarray]) -> dict[str, np.ndarray] | None:
        import numpy as np

        nonlocal removed
        embeddings = current.get("embeddings")
        metadata_arr = current.get("metadata")
        if embeddings is None or metadata_arr is None:
            raise MalformedDataError(npz_path)
        metadata = [json.loads(str(item)) for item in metadata_arr]
        remove_indexes: set[int] = set()
        for entry in removable:
            key = tuple(entry.get("key", []))
            expected_meta = entry.get("metadata")
            matches = [
                index
                for index, meta in enumerate(metadata)
                if _voiceprint_key(meta) == key and meta == expected_meta
            ]
            if len(matches) != 1:
                raise ValueError(
                    "voiceprint inverse locator did not match exactly one row"
                )
            remove_indexes.add(matches[0])
        keep_indexes = [
            index for index in range(len(metadata)) if index not in remove_indexes
        ]
        removed = len(remove_indexes)
        if not keep_indexes:
            return {}
        return {
            "embeddings": embeddings[keep_indexes],
            "metadata": np.asarray(
                [json.dumps(metadata[index]) for index in keep_indexes],
                dtype=str,
            ),
        }

    update_npz(npz_path, transform, expected_keys=VOICEPRINT_KEYS)
    return removed


def _voiceprint_key(meta: dict[str, Any]) -> tuple[Any, Any, Any, Any]:
    return (
        meta.get("day"),
        meta.get("segment_key"),
        meta.get("source"),
        meta.get("sentence_id"),
    )


def voiceprint_file_path(entity_id: str) -> Path:
    """Return the canonical voiceprints.npz path for an entity."""
    return ensure_journal_entity_memory(entity_id) / "voiceprints.npz"
