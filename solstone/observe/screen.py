#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""
Screen analysis formatter for indexing and clustering.

Provides format_screen() and format_screen_text() functions for converting
screen.jsonl frame analyses to markdown format.
"""

from __future__ import annotations

import json
import logging
from collections import Counter
from datetime import datetime
from importlib import import_module
from pathlib import Path
from typing import Any, Callable

from solstone.observe.category_registry import CATEGORIES
from solstone.observe.detect import qualified_objects
from solstone.observe.utils import load_analysis_frames, parse_screen_filename

logger = logging.getLogger(__name__)

# Re-export CATEGORIES for consumers that import from screen
__all__ = ["CATEGORIES", "format_screen", "format_screen_text"]

# Cache for discovered category formatters
_formatter_cache: dict[str, Callable | None] = {}


def _load_category_formatter(category: str) -> Callable | None:
    """Load formatter for a category from solstone.observe.categories.<category>.

    Args:
        category: Category name (e.g., "meeting", "messaging")

    Returns:
        The format function or None if not found
    """
    if category in _formatter_cache:
        return _formatter_cache[category]

    try:
        module = import_module(f"solstone.observe.categories.{category}")
        formatter = getattr(module, "format", None)
        _formatter_cache[category] = formatter
        return formatter
    except (ImportError, AttributeError) as e:
        logger.debug(f"No formatter for category {category}: {e}")
        _formatter_cache[category] = None
        return None


def _format_category_content(category: str, content: Any, context: dict) -> str:
    """Format category-specific content to markdown.

    Tries discovered formatter first, falls back to default rendering.

    Args:
        category: Category name
        content: Category content (str for markdown, dict for JSON)
        context: Dict with frame, file_path, timestamp_str

    Returns:
        Formatted markdown string
    """
    # Try discovered formatter
    formatter = _load_category_formatter(category)
    if formatter:
        result = formatter(content, context)
        if result:
            return result

    # Default formatting
    if isinstance(content, str):
        return f"**{category.title()}:**\n\n{content.strip()}\n"
    elif isinstance(content, dict):
        return f"**{category.title()}:**\n\n```json\n{json.dumps(content, indent=2)}\n```\n"
    return ""


def format_screen(
    entries: list[dict],
    context: dict | None = None,
) -> tuple[list[dict], dict]:
    """Format screen.jsonl entries to markdown chunks.

    This is the formatter function used by the formatters registry.

    Args:
        entries: Raw JSONL entries (first line is metadata, rest are frames)
        context: Optional context with:
            - file_path: Path to JSONL file (for extracting base timestamp)
            - entity_names: Comma-separated entity names for context
            - include_entity_context: Whether to include entity header

    Returns:
        Tuple of (chunks, meta) where:
            - chunks: List of dicts with keys:
                - timestamp: int (unix ms)
                - markdown: str
                - source: dict (original frame entry)
            - meta: Dict with optional "header" and "error" keys
    """
    ctx = context or {}
    file_path = ctx.get("file_path")
    entity_names = ctx.get("entity_names", "")
    include_entity_context = ctx.get("include_entity_context", True)

    # Separate metadata from frame entries
    # Only first entry can be metadata (has "raw" key but no "timestamp" key)
    frame_entries = []
    skipped_count = 0
    for i, entry in enumerate(entries):
        if i == 0 and "timestamp" not in entry and "raw" in entry:
            pass  # Skip metadata entry
        elif "timestamp" in entry:
            frame_entries.append(entry)
        else:
            skipped_count += 1

    # Build meta dict with optional error
    meta: dict[str, Any] = {}
    if skipped_count > 0:
        error_msg = f"Skipped {skipped_count} entries missing 'timestamp' field"
        if file_path:
            error_msg += f" in {file_path}"
        meta["error"] = error_msg
        logger.info(error_msg)

    chunks: list[dict[str, Any]] = []

    # Extract position/connector from filename for header
    # e.g., "center_DP-3_screen.jsonl" -> position="center", connector="DP-3"
    position, connector = "unknown", "unknown"
    if file_path:
        file_path = Path(file_path)
        position, connector = parse_screen_filename(file_path.stem)

    # Build header with entity context if requested
    header_lines = []
    if include_entity_context and entity_names:
        header_lines = [
            "# Entity Context",
            "",
            f"Frequently used names that may appear: {entity_names}",
            "",
            "---",
            "",
        ]

    # Add frame analyses header with monitor info if available
    if position != "unknown" and connector != "unknown":
        header_lines.append(f"# Frame Analyses ({position} - {connector})")
    else:
        header_lines.append("# Frame Analyses")

    meta["header"] = "\n".join(header_lines)

    # Extract base timestamp from segment directory (HHMMSS_LEN)
    # Expected structure: YYYYMMDD/<stream>/HHMMSS_LEN/screen.jsonl
    base_hour = base_minute = base_second = 0
    base_timestamp_ms = 0  # Unix timestamp in milliseconds for segment start
    if file_path:
        try:
            from solstone.think.utils import day_from_path, segment_parse

            # Get segment start time from parent directory
            file_path = Path(file_path)
            start_time, _ = segment_parse(file_path.parent.name)
            if start_time:
                base_hour = start_time.hour
                base_minute = start_time.minute
                base_second = start_time.second

                day_dir = day_from_path(file_path)
                if day_dir:
                    day_date = datetime.strptime(day_dir, "%Y%m%d").date()
                    dt = datetime.combine(day_date, start_time)
                    base_timestamp_ms = int(dt.timestamp() * 1000)
        except (ValueError, AttributeError):
            pass

    # Sort all frames chronologically
    sorted_frames = sorted(frame_entries, key=lambda f: f.get("timestamp", 0))

    for frame in sorted_frames:
        lines = []

        # Calculate absolute time
        frame_offset = frame.get("timestamp", 0)
        total_seconds = (
            base_hour * 3600 + base_minute * 60 + base_second + int(frame_offset)
        )
        abs_hour = (total_seconds // 3600) % 24
        abs_minute = (total_seconds // 60) % 60
        abs_second = total_seconds % 60

        # Build frame header with timestamp
        frame_header = f"### {abs_hour:02d}:{abs_minute:02d}:{abs_second:02d}"

        lines.append(frame_header)
        lines.append("")

        # Add analysis if present
        analysis = frame.get("analysis", {})
        if analysis:
            category = analysis.get("primary", "unknown")
            description = analysis.get("visual_description", "")

            lines.append(f"**Category:** {category}")
            lines.append("")
            if description:
                lines.append(description)
                lines.append("")

        # Detected-object tags — read-side qualified-objects policy is the
        # single source of truth (detect.py); stored rows are raw/unfiltered.
        detections = frame.get("detections")
        if detections:
            qualified = qualified_objects(detections)
            if qualified:
                counts = Counter(obj["class_name"] for obj in qualified)
                ordered = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))
                tags = ", ".join(
                    name if count == 1 else f"{name} ×{count}"
                    for name, count in ordered
                )
                lines.append(f"**Tags:** {tags}")
                lines.append("")

        # Build context for category formatters
        timestamp_str = f"{abs_hour:02d}:{abs_minute:02d}:{abs_second:02d}"
        format_context = {
            "frame": frame,
            "file_path": file_path,
            "timestamp_str": timestamp_str,
        }

        # Add category-specific content from content dict
        frame_content = frame.get("content", {})
        for cat, cat_data in frame_content.items():
            if cat_data:
                formatted = _format_category_content(cat, cat_data, format_context)
                if formatted:
                    lines.append(formatted)

        # Calculate absolute unix timestamp in milliseconds
        frame_timestamp_ms = base_timestamp_ms + int(frame_offset * 1000)

        chunks.append(
            {
                "timestamp": frame_timestamp_ms,
                "markdown": "\n".join(lines),
                "source": frame,
            }
        )

    # Indexer metadata - agent is always "screen" for screen analysis
    meta["indexer"] = {"agent": "screen"}

    return chunks, meta


def format_screen_text(jsonl_path: Path) -> str:
    """Load and format screen.jsonl to markdown text.

    Convenience function for cluster.py that loads frames and formats to text.

    Args:
        jsonl_path: Path to screen.jsonl file

    Returns:
        Formatted markdown string
    """
    frames = load_analysis_frames(jsonl_path)
    if not frames:
        return ""

    context = {"file_path": jsonl_path, "include_entity_context": False}
    chunks, meta = format_screen(frames, context)

    parts = []
    if meta.get("header"):
        parts.append(meta["header"])
    parts.extend(chunk["markdown"] for chunk in chunks)
    return "\n".join(parts)
