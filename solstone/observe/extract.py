# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Frame extraction selection for vision analysis pipeline.

Determines which categorized frames should receive detailed content extraction.
Provides AI-based selection with a deterministic max-temporal-spread fallback.

The first qualified frame is always included regardless of selection results.
"""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import TYPE_CHECKING

from solstone.observe.category_registry import DEFAULT_MAX_EXTRACTIONS

if TYPE_CHECKING:
    from typing import Any

logger = logging.getLogger(__name__)

_SCHEMA = json.loads(
    (Path(__file__).parent / "extract.schema.json").read_text(encoding="utf-8")
)

def select_frames_for_extraction(
    categorized_frames: list[dict[str, Any]],
    max_extractions: int = DEFAULT_MAX_EXTRACTIONS,
    categories: dict[str, dict] | None = None,
) -> list[int]:
    """Select which frames should receive detailed content extraction.

    The first qualified frame is always included, even if it exceeds max_extractions
    by one. This ensures we always have context from the start of the recording.

    Category importance settings from config are a hard filter: ``ignore`` drops
    the category entirely; ``low`` keeps at most 2 frames per category;
    ``normal``/``high`` are uncapped. The first frame is always re-added even
    if its category is capped.

    Parameters
    ----------
    categorized_frames : list[dict]
        List of categorized frame data, each containing:
        - frame_id: int
        - timestamp: float
        - analysis: dict with primary, secondary, overlap, visual_description
    max_extractions : int
        Maximum number of frames to extract (default: 20).
        First frame may cause this to be exceeded by 1.
    categories : dict, optional
        Category metadata dict (from solstone.observe.category_registry.CATEGORIES).
        If provided, enables AI-based selection. If None, uses fallback.

    Returns
    -------
    list[int]
        Frame IDs to extract, sorted in frame order.
    """
    if not categorized_frames:
        return []

    # Load config overrides; importance is a hard per-category filter (see _apply_category_caps).
    config_overrides = _get_category_config()

    # Try AI selection if categories provided
    if categories is not None:
        try:
            selected = _ai_select_frames(
                categorized_frames, max_extractions, categories, config_overrides
            )
        except Exception as e:
            logger.warning(f"AI frame selection failed, using fallback: {e}")
            selected = _fallback_select_frames(categorized_frames, max_extractions)
    else:
        selected = _fallback_select_frames(categorized_frames, max_extractions)

    # Enforce hard per-category caps before guaranteeing the first frame,
    # so the first frame is re-added even if its category is capped.
    selected = _apply_category_caps(selected, categorized_frames, config_overrides)

    # Ensure first frame is always included
    first_frame_id = categorized_frames[0]["frame_id"]
    if first_frame_id not in selected:
        selected = [first_frame_id] + selected

    # Return sorted by frame_id for deterministic output order
    return sorted(selected)


def _get_category_config() -> dict[str, dict]:
    """Load category overrides from journal config.

    Returns
    -------
    dict[str, dict]
        Category overrides with importance and extraction fields.
    """
    from solstone.think.utils import get_config

    config = get_config()
    return config.get("describe", {}).get("categories", {})


def _apply_category_caps(
    selected_ids: list[int],
    categorized_frames: list[dict[str, Any]],
    config_overrides: dict[str, dict],
) -> list[int]:
    """Enforce hard per-category caps on a selection.

    importance -> cap: ``ignore`` = 0 (dropped), ``low`` = 2 per category,
    ``normal``/``high`` = uncapped. Frames are considered lowest-frame_id first,
    so the surviving low-importance frames are deterministic.
    """
    id_to_category = {
        f["frame_id"]: f.get("analysis", {}).get("primary") for f in categorized_frames
    }
    caps = {"ignore": 0, "low": 2}
    counts: dict[str, int] = {}
    kept: list[int] = []
    for frame_id in sorted(selected_ids):
        category = id_to_category.get(frame_id)
        importance = config_overrides.get(category, {}).get("importance", "normal")
        cap = caps.get(importance)
        if cap == 0:
            continue
        if cap is not None:
            if counts.get(category, 0) >= cap:
                continue
            counts[category] = counts.get(category, 0) + 1
        kept.append(frame_id)
    return kept


def _build_extraction_guidance(
    categories: dict[str, dict],
    config_overrides: dict[str, dict] | None = None,
) -> str:
    """Build extraction guidance from category configs with config overrides.

    Groups categories by importance level (high, normal, low, ignore).
    Categories with 'ignore' importance only show the name, no extraction guidance.

    Parameters
    ----------
    categories : dict
        Category metadata dict mapping category name to config.
        Each config may have an 'extraction' field with guidance text.
    config_overrides : dict, optional
        User overrides from journal config. May contain 'extraction' and
        'importance' per category.

    Returns
    -------
    str
        Formatted extraction guidance for the prompt, grouped by importance.
    """
    config_overrides = config_overrides or {}

    # Group categories by importance
    groups: dict[str, list[str]] = {"high": [], "normal": [], "low": [], "ignore": []}

    for name in sorted(categories.keys()):
        meta = categories[name]
        override = config_overrides.get(name, {})

        # Get importance (default: normal)
        importance = override.get("importance", "normal")

        # Get extraction guidance (config override takes precedence)
        extraction = override.get("extraction") or meta.get("extraction")

        # For ignore, just list the category name
        if importance == "ignore":
            groups["ignore"].append(f"- {name}")
        elif extraction:
            groups[importance].append(f"- {name}: {extraction}")

    # Check if all categories are normal (no grouping needed)
    has_non_normal = groups["high"] or groups["low"] or groups["ignore"]

    if not has_non_normal:
        # Simple flat list when all normal
        return (
            "\n".join(groups["normal"])
            if groups["normal"]
            else "No category-specific rules."
        )

    # Build grouped output
    sections = []

    if groups["high"]:
        sections.append("**Prioritize:**\n" + "\n".join(groups["high"]))

    if groups["normal"]:
        sections.append("**Normal:**\n" + "\n".join(groups["normal"]))

    if groups["low"]:
        sections.append("**Low priority:**\n" + "\n".join(groups["low"]))

    if groups["ignore"]:
        sections.append("**Skip unless notable:**\n" + "\n".join(groups["ignore"]))

    return "\n\n".join(sections) if sections else "No category-specific rules."


def _ai_select_frames(
    categorized_frames: list[dict[str, Any]],
    max_extractions: int,
    categories: dict[str, dict],
    config_overrides: dict[str, dict] | None = None,
) -> list[int]:
    """AI-based frame selection.

    Analyzes all categorization results to determine which frames contain
    the most valuable content for detailed extraction.

    Parameters
    ----------
    categorized_frames : list[dict]
        List of categorized frame data.
    max_extractions : int
        Maximum number of frames to select.
    categories : dict
        Category metadata dict for building extraction guidance.
    config_overrides : dict, optional
        User overrides from journal config for extraction/importance.

    Returns
    -------
    list[int]
        Selected frame IDs.

    Raises
    ------
    Exception
        If AI selection fails (will trigger fallback in caller).
    """
    from solstone.think.models import generate
    from solstone.think.prompts import load_prompt

    # Build extraction guidance with config overrides
    extraction_guidance = _build_extraction_guidance(categories, config_overrides)

    # Load prompt with template substitution
    prompt_content = load_prompt(
        "extract",
        base_dir=Path(__file__).parent,
        context={
            "extraction_guidance": extraction_guidance,
            "max_extractions": str(max_extractions),
        },
    )

    # Build compact frame summaries for the model
    frame_summaries = []
    valid_frame_ids = set()
    for f in categorized_frames:
        frame_id = f["frame_id"]
        valid_frame_ids.add(frame_id)
        analysis = f.get("analysis", {})
        frame_summaries.append(
            {
                "frame_id": frame_id,
                "timestamp": f["timestamp"],
                "primary": analysis.get("primary", "?"),
                "secondary": analysis.get("secondary", "none"),
                "overlap": analysis.get("overlap", True),
                "visual_description": analysis.get("visual_description", ""),
            }
        )

    # Call LLM for frame selection
    response = generate(
        contents=json.dumps(frame_summaries),
        context="observe.extract.selection",
        system_instruction=prompt_content.text,
        json_output=True,
        json_schema=_SCHEMA,
        thinking_budget=4096,
        max_output_tokens=1024,
        temperature=0.3,
    )

    # Parse response - expecting wrapped frame_ids or a bare frame_id list
    selected_ids = json.loads(response)
    # Permanent shape-compat: schema emits {"frame_ids": [...]}; tolerate pre-reshape bare-list outputs defensively.
    if isinstance(selected_ids, dict):
        selected_ids = selected_ids.get("frame_ids", [])

    if not isinstance(selected_ids, list):
        raise ValueError(f"Expected list of frame_ids, got {type(selected_ids)}")

    # Filter to valid frame IDs only
    raw_count = len(selected_ids)
    selected_ids = [fid for fid in selected_ids if fid in valid_frame_ids]
    filtered_count = raw_count - len(selected_ids)
    if filtered_count > 0:
        logger.info(f"AI returned {filtered_count} invalid frame ID(s), filtered out")

    # Hard cap at 2x max_extractions (max is a soft target)
    hard_limit = 2 * max_extractions
    if len(selected_ids) > hard_limit:
        logger.info(f"AI selected {len(selected_ids)} frames, capping to {hard_limit}")
        selected_ids = selected_ids[:hard_limit]

    logger.info(f"AI frame selection: {len(selected_ids)} frames selected")
    return selected_ids


def _fallback_select_frames(
    categorized_frames: list[dict[str, Any]],
    max_extractions: int,
) -> list[int]:
    """Fallback frame selection when AI selection is unavailable.

    If total frames <= max_extractions: returns all frame IDs (input order).
    Otherwise: deterministically selects max_extractions frames spread across the
    segment's timeline via greedy farthest-point sampling on the timestamp axis,
    seeded with the lowest-frame_id frame; ties are broken by lowest frame_id.
    """
    if not categorized_frames:
        return []

    all_ids = [f["frame_id"] for f in categorized_frames]
    if len(all_ids) <= max_extractions:
        return all_ids

    frames = [(f["frame_id"], float(f["timestamp"])) for f in categorized_frames]
    seed = min(frames, key=lambda p: p[0])
    selected = [seed]
    selected_ts = [seed[1]]
    remaining = [p for p in frames if p != seed]

    while len(selected) < max_extractions and remaining:
        best = None
        best_key: tuple[float, int] | None = None
        for frame_id, timestamp in remaining:
            min_dist = min(abs(timestamp - ts) for ts in selected_ts)
            # Maximize distance to the nearest selected frame; tie -> lowest frame_id.
            key = (min_dist, -frame_id)
            if best_key is None or key > best_key:
                best_key = key
                best = (frame_id, timestamp)
        selected.append(best)
        selected_ts.append(best[1])
        remaining.remove(best)

    return [frame_id for frame_id, _ in selected]


__all__ = [
    "DEFAULT_MAX_EXTRACTIONS",
    "select_frames_for_extraction",
]
