# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Local frame embedding via MobileViT-small ONNX.

Produces 256-dimensional embeddings for visual clustering and representative
frame selection, cutting Phase 1 LLM input size before categorisation.

Public API:
    from solstone.observe.embed import reduce_frames, embed_frames, cluster_and_select

    # Top-level: embed + cluster in one call, graceful fallback when model absent
    reduced = reduce_frames(qualified_frames)

    # Lower-level, if you need the embeddings separately
    embeddings = embed_frames(qualified_frames)
    reduced    = cluster_and_select(qualified_frames, embeddings)

Model setup:
    Export MobileViTModel from HuggingFace and place the ONNX at:
        solstone/observe/assets/mobilevit-small.onnx

    One-time export (requires transformers + torch, not a runtime dep):
        python -c "
        from transformers import MobileViTModel
        import torch
        model = MobileViTModel.from_pretrained('apple/mobilevit-small')
        model.eval()
        dummy = torch.zeros(1, 3, 256, 256)
        torch.onnx.export(
            model, dummy,
            'solstone/observe/assets/mobilevit-small.onnx',
            input_names=['pixel_values'],
            output_names=['last_hidden_state', 'pooler_output'],
            dynamic_axes={'pixel_values': {0: 'batch'}},
            opset_version=14,
        )
        "

    When the model file is absent, reduce_frames() returns all frames unchanged.
    No other functionality is affected.
"""

from __future__ import annotations

import logging
from pathlib import Path

import numpy as np

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Paths and constants
# ---------------------------------------------------------------------------

ASSETS_DIR = Path(__file__).parent / "assets"
MOBILEVIT_MODEL_PATH = ASSETS_DIR / "mobilevit-small.onnx"

INPUT_SIZE = (256, 256)  # (width, height) — MobileViT-small default

# ImageNet normalisation applied after [0,1] scaling
IMAGENET_MEAN = np.array([0.485, 0.456, 0.406], dtype=np.float32)
IMAGENET_STD = np.array([0.229, 0.224, 0.225], dtype=np.float32)

EMBEDDING_DIM = (
    640  # MobileViT-small pooler_output width (verified from exported model)
)

# Cosine distance below which a frame joins an existing cluster (default).
# Tune down for finer-grained representation, up for more aggressive reduction.
DEFAULT_DISTANCE_THRESHOLD = 0.20

# Minimum representative frames to keep per call to cluster_and_select(), even
# if clustering would collapse the whole segment to fewer. Filled in by
# max-temporal-spread to avoid losing scene context.
DEFAULT_MIN_FRAMES = 5

# ---------------------------------------------------------------------------
# Session cache
# ---------------------------------------------------------------------------

_session = None


def _get_session():
    global _session
    if _session is None:
        import onnxruntime as ort

        if not MOBILEVIT_MODEL_PATH.is_file():
            raise FileNotFoundError(
                f"MobileViT-small model not found at {MOBILEVIT_MODEL_PATH}. "
                "See the module docstring for the one-time export command."
            )
        providers = (
            ["CUDAExecutionProvider", "CPUExecutionProvider"]
            if "CUDAExecutionProvider" in ort.get_available_providers()
            else ["CPUExecutionProvider"]
        )
        _session = ort.InferenceSession(str(MOBILEVIT_MODEL_PATH), providers=providers)
        logger.debug(
            "MobileViT-small session loaded (providers=%s)",
            [p for p in _session.get_providers()],
        )
    return _session


# ---------------------------------------------------------------------------
# Public helpers
# ---------------------------------------------------------------------------


def is_available() -> bool:
    """Return True if the MobileViT model file is present on disk."""
    return MOBILEVIT_MODEL_PATH.is_file()


# ---------------------------------------------------------------------------
# Preprocessing
# ---------------------------------------------------------------------------


def _preprocess(frame_bytes: bytes) -> np.ndarray:
    """Decode PNG frame bytes → normalised NCHW float32 tensor (1, 3, H, W)."""
    import io

    from PIL import Image

    img = Image.open(io.BytesIO(frame_bytes)).convert("RGB")
    img = img.resize(INPUT_SIZE, Image.BILINEAR)
    arr = np.array(img, dtype=np.float32) / 255.0
    arr = (arr - IMAGENET_MEAN) / IMAGENET_STD
    # HWC → CHW → NCHW
    return arr.transpose(2, 0, 1)[np.newaxis, :, :, :]


# ---------------------------------------------------------------------------
# Embedding
# ---------------------------------------------------------------------------


def embed_frames(frames: list[dict]) -> np.ndarray:
    """
    Embed a list of frame dicts → float32 array (N, EMBEDDING_DIM).

    Each dict must carry a 'frame_bytes' key (PNG bytes from VideoProcessor).
    Raises FileNotFoundError if the model file is absent.
    """
    if not frames:
        return np.empty((0, EMBEDDING_DIM), dtype=np.float32)

    session = _get_session()
    input_name = session.get_inputs()[0].name

    # Prefer an output named 'pooler_output'; fall back to the last output.
    outputs = session.get_outputs()
    output_name = next(
        (o.name for o in outputs if "pooler" in o.name.lower()),
        outputs[-1].name,
    )

    embeddings: list[np.ndarray] = []
    for frame in frames:
        tensor = _preprocess(frame["frame_bytes"])
        raw = session.run([output_name], {input_name: tensor})[0]
        # Flatten and truncate/pad to EMBEDDING_DIM for safety
        vec = raw.reshape(-1).astype(np.float32)
        if vec.shape[0] != EMBEDDING_DIM:
            padded = np.zeros(EMBEDDING_DIM, dtype=np.float32)
            copy_len = min(vec.shape[0], EMBEDDING_DIM)
            padded[:copy_len] = vec[:copy_len]
            vec = padded
        embeddings.append(vec)

    return np.stack(embeddings)


# ---------------------------------------------------------------------------
# Clustering
# ---------------------------------------------------------------------------


def cluster_and_select(
    frames: list[dict],
    embeddings: np.ndarray,
    distance_threshold: float = DEFAULT_DISTANCE_THRESHOLD,
    min_frames: int = DEFAULT_MIN_FRAMES,
) -> list[dict]:
    """
    Greedy cosine-distance clustering — one representative per cluster.

    Iterates frames in order. A frame starts a new cluster when its cosine
    distance from every existing cluster representative exceeds
    `distance_threshold`. Scene-cut frames (frame["scene_cut"] is True) always
    start a new cluster regardless of distance, because they carry definitionally
    new information.

    After clustering, if fewer than `min_frames` frames were selected, the
    shortfall is filled by max-temporal-spread: repeatedly picking the frame
    whose minimum time-gap from any already-selected frame is largest. This
    prevents very uniform footage from collapsing to a single representative.

    Args:
        frames: Frame dicts from VideoProcessor.process().
        embeddings: float32 array (N, D) from embed_frames(), same order.
        distance_threshold: Cosine distance in [0, 1]. Lower → more frames
            kept. Higher → more aggressive reduction.
        min_frames: Minimum number of frames to return. Clipped to len(frames).

    Returns:
        Subset of `frames` in original order, one per cluster (≥ min_frames).
    """
    if not frames:
        return []
    if len(frames) != len(embeddings):
        raise ValueError(
            f"frames ({len(frames)}) and embeddings ({len(embeddings)}) must be same length"
        )

    # L2-normalise for cosine similarity via dot product
    norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
    norms = np.maximum(norms, 1e-8)
    normed = embeddings / norms  # (N, D)

    selected: list[int] = [0]  # first frame always seeds the first cluster

    for i in range(1, len(frames)):
        if frames[i].get("scene_cut"):
            # Scene cuts always open a new cluster
            selected.append(i)
            continue

        rep_embs = normed[selected]  # (K, D)
        cosine_sims = normed[i] @ rep_embs.T  # (K,)
        min_dist = float(1.0 - cosine_sims.max())

        if min_dist >= distance_threshold:
            selected.append(i)

    # Minimum frames floor: if clustering was too aggressive, top up with
    # temporal spread so we never drop below min_frames per segment.
    target = min(min_frames, len(frames))
    if len(selected) < target:
        selected_set = set(selected)
        selected_ts = [frames[i]["timestamp"] for i in selected]

        while len(selected) < target:
            best_i, best_gap = None, -1.0
            for i in range(len(frames)):
                if i in selected_set:
                    continue
                gap = min(abs(frames[i]["timestamp"] - t) for t in selected_ts)
                if gap > best_gap:
                    best_gap, best_i = gap, i
            if best_i is None:
                break
            selected.append(best_i)
            selected_set.add(best_i)
            selected_ts.append(frames[best_i]["timestamp"])

        selected.sort()  # restore temporal order

    return [frames[i] for i in selected]


# ---------------------------------------------------------------------------
# Top-level entry point
# ---------------------------------------------------------------------------


def reduce_frames(
    frames: list[dict],
    distance_threshold: float = DEFAULT_DISTANCE_THRESHOLD,
    min_frames: int = DEFAULT_MIN_FRAMES,
) -> list[dict]:
    """
    Embed and cluster frames, returning the minimal representative set.

    Wraps embed_frames + cluster_and_select with a graceful fallback: if the
    MobileViT model is absent or inference fails, all frames are returned
    unchanged so the rest of the pipeline continues unaffected.

    Args:
        frames: Qualified frames from VideoProcessor.process().
        distance_threshold: Passed through to cluster_and_select().
        min_frames: Minimum frames to keep; passed through to cluster_and_select().

    Returns:
        Reduced list — always includes the first frame and all scene cuts,
        and at least min_frames entries (or all frames if fewer exist).
    """
    if not frames:
        return []

    if not is_available():
        logger.debug("MobileViT model not present; skipping embedding reduction")
        return frames

    try:
        embeddings = embed_frames(frames)
        reduced = cluster_and_select(frames, embeddings, distance_threshold, min_frames)
        logger.debug(
            "Embedding reduction: %d → %d frames (threshold=%.2f, min=%d)",
            len(frames),
            len(reduced),
            distance_threshold,
            min_frames,
        )
        return reduced
    except Exception:
        logger.exception("Embedding reduction failed; returning all frames unchanged")
        return frames
