# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared voiceprint helpers for entity-aware speaker workflows."""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import TYPE_CHECKING, Any, Callable

from solstone.think.entities.journal import (
    ensure_journal_entity_memory,
    journal_entity_memory_path,
)
from solstone.think.journal_io.errors import MalformedDataError
from solstone.think.journal_io.npz import load_npz, update_npz

if TYPE_CHECKING:
    import numpy as np

logger = logging.getLogger(__name__)
VOICEPRINT_KEYS = ("embeddings", "metadata")


def normalize_embedding(emb: np.ndarray) -> np.ndarray | None:
    """L2-normalize an embedding vector. Returns None if norm is zero."""
    import numpy as np

    emb = emb.astype(np.float32)
    norm = np.linalg.norm(emb)
    if norm > 0:
        return emb / norm
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
