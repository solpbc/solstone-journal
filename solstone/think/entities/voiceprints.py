# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared voiceprint helpers for entity-aware speaker workflows."""

from __future__ import annotations

import fcntl
import json
import logging
from pathlib import Path
from typing import Callable

import numpy as np

from solstone.think.entities.journal import (
    ensure_journal_entity_memory,
    journal_entity_memory_path,
)

logger = logging.getLogger(__name__)


def normalize_embedding(emb: np.ndarray) -> np.ndarray | None:
    """L2-normalize an embedding vector. Returns None if norm is zero."""
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
        with np.load(npz_path, allow_pickle=False) as data:
            embeddings = data.get("embeddings")
            metadata_arr = data.get("metadata")
            if embeddings is None or metadata_arr is None:
                return None
            metadata_list = [json.loads(m) for m in metadata_arr]
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
    if not new_items:
        return 0

    folder = ensure_journal_entity_memory(entity_id)
    npz_path = folder / "voiceprints.npz"

    if npz_path.exists():
        try:
            with np.load(npz_path, allow_pickle=False) as data:
                existing_emb = data["embeddings"]
                existing_meta_strings = data["metadata"]
                existing_meta_dicts = [json.loads(m) for m in existing_meta_strings]
        except (FileNotFoundError, OSError, ValueError) as exc:
            logger.warning(
                "Failed to load existing voiceprints for %s from %s: %s. Starting fresh.",
                entity_id,
                npz_path,
                exc,
            )
            existing_emb = np.empty((0, 256), dtype=np.float32)
            existing_meta_dicts = []
        except Exception:
            logger.exception(
                "Unexpected error loading existing voiceprints for %s from %s",
                entity_id,
                npz_path,
            )
            raise
    else:
        existing_emb = np.empty((0, 256), dtype=np.float32)
        existing_meta_dicts = []

    new_emb_list = [emb.reshape(1, -1).astype(np.float32) for emb, _ in new_items]
    new_meta_dicts = [meta_dict for _, meta_dict in new_items]

    if new_emb_list:
        new_emb_np = np.vstack(new_emb_list)
        combined_emb = (
            np.vstack([existing_emb, new_emb_np])
            if len(existing_emb) > 0
            else new_emb_np
        )
        combined_meta_dicts = existing_meta_dicts + new_meta_dicts
    else:
        combined_emb = existing_emb
        combined_meta_dicts = existing_meta_dicts

    save_voiceprints_safely(
        npz_path=npz_path,
        embeddings=combined_emb,
        metadata=combined_meta_dicts,
    )
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

    with np.load(npz_path, allow_pickle=False) as data:
        embeddings = data.get("embeddings")
        metadata_arr = data.get("metadata")
        if embeddings is None or metadata_arr is None:
            return 0
        metadata = [json.loads(item) for item in metadata_arr]

    updates = mutator(metadata)
    if updates <= 0:
        return 0

    save_voiceprints_safely(npz_path=npz_path, embeddings=embeddings, metadata=metadata)
    return updates


def voiceprint_file_path(entity_id: str) -> Path:
    """Return the canonical voiceprints.npz path for an entity."""
    return ensure_journal_entity_memory(entity_id) / "voiceprints.npz"


def save_voiceprints_safely(
    npz_path: Path,
    embeddings: np.ndarray,
    metadata: list[dict],
) -> None:
    """Safely save a voiceprint NPZ with file locking and integrity check."""
    lock_path = npz_path.with_suffix(".lock")
    tmp_path = npz_path.with_name(npz_path.stem + ".tmp.npz")
    metadata_json = np.asarray([json.dumps(item) for item in metadata], dtype=str)

    npz_path.parent.mkdir(parents=True, exist_ok=True)

    try:
        with open(lock_path, "w", encoding="utf-8") as lock_file:
            try:
                fcntl.flock(lock_file, fcntl.LOCK_EX)
                np.savez_compressed(
                    tmp_path,
                    embeddings=embeddings,
                    metadata=metadata_json,
                )
                if not tmp_path.exists():
                    raise FileNotFoundError(
                        f"Temporary voiceprint file not found: {tmp_path}"
                    )
                tmp_path.rename(npz_path)

                with np.load(npz_path, allow_pickle=False) as data:
                    if "embeddings" not in data or "metadata" not in data:
                        raise ValueError(
                            "Missing 'embeddings' or 'metadata' keys in loaded NPZ."
                        )
                logger.info(
                    "Successfully wrote and verified voiceprint file: %s",
                    npz_path,
                )
            except Exception:
                if tmp_path.exists():
                    try:
                        tmp_path.unlink()
                    except OSError:
                        logger.exception(
                            "Failed to clean up temporary voiceprint file %s",
                            tmp_path,
                        )
                raise
            finally:
                try:
                    fcntl.flock(lock_file, fcntl.LOCK_UN)
                except OSError:
                    logger.exception("Failed to release lock on %s", lock_path)
    except OSError:
        logger.exception("Failed to acquire or manage lock file %s", lock_path)
        raise
