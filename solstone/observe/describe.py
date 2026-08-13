#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""
Describe screencast videos by detecting significant frame changes.

Processes per-monitor screencast files (.webm/.mp4/.mov), detects changes using
perceptual hashing (dHash), and sends frames for multi-stage LLM analysis:

1. Phase 1: Categorization - All frames get initial category analysis
2. Phase 2: Selection - AI/fallback selects which frames get detailed extraction
3. Phase 3: Extraction - Selected frames get category-specific content extraction

Uses Batch for async batch processing with provider routing via context.
"""

from __future__ import annotations

import argparse
import asyncio
import io
import json
import logging
import os
import sys
import tempfile
import time
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import List, Optional

from PIL import Image

from solstone.observe.category_registry import CATEGORIES, DEFAULT_MAX_EXTRACTIONS
from solstone.observe.detect import detect_objects, detections_block, screen_gate
from solstone.observe.exit_codes import EXIT_PROVIDER_BLOCKED
from solstone.observe.extract import select_frames_for_extraction
from solstone.observe.processing_record import (
    HANDLER_DESCRIBE,
    REASON_ANALYSIS_FAILED,
    REASON_CORRUPT_INPUT,
    REASON_NO_DECODABLE_FRAMES,
    REASON_OK,
    STATE_ANALYZED,
    STATE_EMPTY,
    STATE_FAILED,
    build_processing_record,
    read_processing_record_header,
    record_attempts,
    should_reenter_analysis_output,
)
from solstone.observe.utils import get_segment_key, resize_for_vlm
from solstone.think.callosum import callosum_send
from solstone.think.journal_io import install_file
from solstone.think.markdown import bound_extraction_markdown
from solstone.think.prompts import load_prompt
from solstone.think.providers import fanout_policy
from solstone.think.providers.shared import is_non_retryable_generate_reason
from solstone.think.utils import (
    day_from_path,
    get_config,
    get_journal,
    journal_relative_path,
    require_solstone,
    setup_cli,
)

logger = logging.getLogger(__name__)

# Perceptual-distance at/above which a qualified frame is a scene cut and
# bypasses the stride floor unconditionally (out of 64 dHash bits).
SCENE_CUT_THRESHOLD = 25
# Minimum wall-clock gap (seconds) between kept frames for non-scene-cut,
# dHash-qualified frames; closer arrivals are stride-dropped.
MIN_STRIDE_SECONDS = 5.0
# Frozen Fedora gate (2026-07-11): 1024 is the lowest conservative image-token
# ceiling for bundled Linux Qwen categorization. Detail extraction remains at
# current sizing because 1024 lost fine-text fidelity.
LOCAL_QWEN_CATEGORIZATION_IMAGE_TOKENS = 1024


def _categorization_image_token_budget(provider: str) -> int | None:
    """Return the proven image cap only for the bundled Linux Qwen path."""
    if provider != "local" or not sys.platform.startswith("linux"):
        return None

    from solstone.think.providers.local_endpoint import resolve_local_endpoint

    if not resolve_local_endpoint().is_bundled:
        return None
    return LOCAL_QWEN_CATEGORIZATION_IMAGE_TOKENS


def _winnow_decision(
    current_hash: int,
    current_timestamp: float,
    last_kept_hash: int,
    last_kept_timestamp: float,
    dhash_threshold: int,
    scene_cut_threshold: int,
    min_stride_seconds: float,
) -> tuple[bool, bool, str]:
    """Decide whether a non-first frame is kept, measured against the last KEPT frame.

    All three gates (dHash change, scene-cut, stride floor) compare against the
    last kept frame's hash/timestamp. The reference advances only on a kept frame,
    so a frame dropped by either the dHash gate or the stride floor leaves the
    reference untouched.

    Returns (keep, scene_cut, reason); reason is one of:
      - "below_threshold": dHash change < dhash_threshold; not qualified, not kept.
      - "scene_cut":       change >= scene_cut_threshold; kept, bypasses the stride floor.
      - "stride_dropped":  qualified, non-scene-cut, arrived < min_stride_seconds
                           after the last kept frame; not kept.
      - "kept":            qualified, non-scene-cut, past the stride floor; kept.
    """
    distance = bin(last_kept_hash ^ current_hash).count("1")
    if distance < dhash_threshold:
        return (False, False, "below_threshold")
    if distance >= scene_cut_threshold:
        return (True, True, "scene_cut")
    if (current_timestamp - last_kept_timestamp) < min_stride_seconds:
        return (False, False, "stride_dropped")
    return (True, False, "kept")


def _flattened_image_data(img: Image.Image):
    get_flattened_data = getattr(img, "get_flattened_data", None)
    if get_flattened_data is not None:
        return get_flattened_data()
    return img.getdata()


class RequestType(Enum):
    """Type of vision analysis request."""

    DESCRIBE = "describe"  # Initial categorization
    CATEGORY = "category"  # Category-specific follow-up


@dataclass
class ExistingDescribeRow:
    data: dict
    raw_line: str


@dataclass
class ExistingDescribeArtifact:
    header: dict
    record: dict
    rows: list[ExistingDescribeRow] | None


@dataclass
class IncrementalMergePlan:
    reusable_rows: dict[int, ExistingDescribeRow]
    phase1_gap_ids: set[int]
    phase3_gaps: dict[int, tuple[dict, tuple[str, ...]]]


def _read_existing_describe_artifact(path: Path) -> ExistingDescribeArtifact | None:
    try:
        lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    except (OSError, UnicodeDecodeError):
        return None
    if not lines:
        return None
    try:
        header = json.loads(lines[0])
    except json.JSONDecodeError:
        return None
    if not isinstance(header, dict):
        return None
    record = header.get("_solstone_processing")
    if not isinstance(record, dict):
        return None

    rows: list[ExistingDescribeRow] = []
    for raw_line in lines[1:]:
        try:
            row = json.loads(raw_line)
        except json.JSONDecodeError:
            return ExistingDescribeArtifact(header=header, record=record, rows=None)
        if not isinstance(row, dict):
            return ExistingDescribeArtifact(header=header, record=record, rows=None)
        rows.append(ExistingDescribeRow(data=row, raw_line=raw_line))
    return ExistingDescribeArtifact(header=header, record=record, rows=rows)


def _build_categorization_prompt() -> str:
    """
    Build the categorization prompt from template and discovered categories.

    Returns
    -------
    str
        Complete prompt with category list substituted
    """
    # Build category list (alphabetical order)
    category_lines = []
    for name in sorted(CATEGORIES.keys()):
        description = CATEGORIES[name]["description"]
        category_lines.append(f"- {name}: {description}")

    category_list = "\n".join(category_lines)

    return load_prompt(
        "describe",
        base_dir=Path(__file__).parent,
        context={"categories": category_list},
    ).text


def _build_redact_instruction(rules: List[str]) -> str:
    """Build a redaction instruction block from user-configured rules.

    Parameters
    ----------
    rules : List[str]
        Redaction rules from config, one directive per entry.

    Returns
    -------
    str
        Formatted instruction block to append to system prompts,
        or empty string if no rules.
    """
    if not rules:
        return ""

    items = "\n".join(f"- {rule}" for rule in rules)
    return (
        "\n\nRedaction rules (apply these exactly as written, do not generalize):\n"
        + items
    )


FRAME_CONTEXT = "observe.describe.frame"

# Build categorization prompt from template
CATEGORIZATION_PROMPT = _build_categorization_prompt()


def _describe_work_key(video_path: Path, day: str | None, segment: str) -> str:
    return f"{day or ''}/{segment}/{video_path.stem}"


def _emit_blocked_notification(view) -> None:
    event_fields = {
        "key": view.semantic_key,
        "work_key": view.work_key,
        "title": "Screen descriptions paused",
        "message": view.summary,
        "icon": "eye",
        "app": "sense",
        "reason_code": view.reason_code,
        "provider": view.provider,
        "model": view.model,
        "context": view.context,
    }
    if view.recovery_action:
        event_fields["action"] = view.recovery_action.target
    callosum_send(
        "notification",
        "show",
        **{key: value for key, value in event_fields.items() if value is not None},
    )


def _abort_for_blocking_request(
    req,
    *,
    output_file,
    temp_path: Path | None,
    work_key: str,
    batch=None,
) -> None:
    from solstone.convey.provider_readiness import (
        is_blocking_reason,
        present_for_reason,
    )

    reason_code = getattr(req, "reason_code", None)
    if not reason_code or not is_blocking_reason(reason_code):
        return

    if output_file and not output_file.closed:
        output_file.close()
    if temp_path:
        temp_path.unlink(missing_ok=True)

    view = present_for_reason(
        reason_code,
        provider=getattr(req, "provider", None) or "",
        model=getattr(req, "model_used", None) or getattr(req, "model", None),
        status="unhealthy",
        context=getattr(req, "context", None),
        interface="generate",
        message=getattr(req, "error", None),
        reset_at_ms=getattr(req, "reset_at_ms", None),
        work_key=work_key,
    )
    _emit_blocked_notification(view)
    raise SystemExit(EXIT_PROVIDER_BLOCKED)


# The enums in `primary` and `secondary` MUST match the filenames under observe/categories/*.md.
_SCHEMA = json.loads(
    (Path(__file__).parent / "describe.schema.json").read_text(encoding="utf-8")
)


class VideoProcessor:
    """Process per-monitor screencast videos and detect significant frame changes."""

    # Resize target for 64-bit perceptual hashing
    DHASH_SIZE = (9, 8)
    # Minimum Hamming distance for frame qualification (out of 64 bits).
    # Tuned via real-segment comparison on 094809_301/left_HDMI-2: threshold 6
    # kept +17 frames vs RMS but added no new extracted content; threshold 8
    # keeps +13 vs RMS, matches RMS's extraction count, and drops the borderline
    # near-duplicates that 6 was letting through.
    DHASH_THRESHOLD = 8
    # Skip frame if Convey UI covers more than this fraction of the frame
    MASK_SKIP_THRESHOLD = 0.8
    first_hash: Optional[int] = None
    last_hash: Optional[int] = None
    qualified_count: int = 0
    decode_failed: bool = False

    def __init__(self, video_path: Path):
        self.video_path = video_path
        self.width: Optional[int] = None
        self.height: Optional[int] = None
        # Store qualified frames as simple list
        self.qualified_frames: List[dict] = []
        describe_config = get_config().get("describe", {})
        self.scene_cut_threshold: int = describe_config.get(
            "scene_cut_threshold", SCENE_CUT_THRESHOLD
        )
        self.min_stride_seconds: float = describe_config.get(
            "min_stride_seconds", MIN_STRIDE_SECONDS
        )
        self.winnow_metrics: dict = {}

    def process(self) -> List[dict]:
        """
        Process video and return qualified frames.

        Uses dHash perceptual hashing to detect significant changes. Caches
        the dHash of the last kept frame for comparison.

        Returns:
            List of qualified frames with timestamp and frame_bytes.
        """
        # Reference = last KEPT frame; advances only on keep.
        last_kept_hash: Optional[int] = None
        last_kept_timestamp: float = 0.0
        self.first_hash = None
        self.last_hash = None
        self.qualified_count = 0
        self.decode_failed = False

        # Winnowing counters (see the metrics line after the loop for definitions).
        raw_frames = 0
        dhash_qualified = 0
        scene_cut_count = 0
        stride_dropped = 0

        # Imports deferred: av (PyAV) and cv2 (via observe.aruco) bundle
        # mismatched libavdevice majors. Keeping them out of module scope
        # avoids the macOS ObjC duplicate-class warning on every caller that
        # only needs CATEGORIES (see observe/screen.py).
        import av

        from solstone.observe.aruco import (
            detect_markers,
            mask_convey_region,
            polygon_area,
        )

        try:
            with av.open(str(self.video_path)) as container:
                if not container.streams.video:
                    logger.error(
                        f"No video stream in {self.video_path}. Skipping video."
                    )
                    self.decode_failed = True
                    return self.qualified_frames

                stream = container.streams.video[0]
                stream.thread_type = "AUTO"
                stream.codec_context.thread_count = 0
                self.width = stream.width
                self.height = stream.height

                frame_count = 0
                for frame in container.decode(video=0):
                    raw_frames += 1
                    if frame.pts is None:
                        continue

                    timestamp = frame.time if frame.time is not None else 0.0
                    frame_count += 1

                    # Convert to PIL for comparison and bytes conversion
                    arr_rgb = frame.to_ndarray(format="rgb24")
                    pil_img = Image.fromarray(arr_rgb)
                    del arr_rgb

                    # Detect ArUco markers (fiducial corner tags)
                    aruco_result = detect_markers(pil_img)
                    aruco_masked = False
                    if aruco_result is not None and aruco_result["polygon"] is not None:
                        # All 4 corner tags detected - check coverage
                        polygon = [tuple(pt) for pt in aruco_result["polygon"]]
                        mask_area = polygon_area(polygon)
                        frame_area = pil_img.width * pil_img.height
                        if mask_area / frame_area > self.MASK_SKIP_THRESHOLD:
                            # Skip frame entirely - Convey UI dominates
                            pil_img.close()
                            _extrap = (
                                " (extrapolated)"
                                if aruco_result.get("extrapolated") is not None
                                else ""
                            )
                            logger.debug(
                                f"Skipping frame at {timestamp:.2f}s "
                                f"(Convey UI covers {mask_area / frame_area:.0%}){_extrap}"
                            )
                            continue
                        # Mask the Convey region with black
                        mask_convey_region(pil_img, polygon)
                        aruco_masked = True

                    # Build frame data dict
                    frame_data: dict = {
                        "frame_id": frame_count,
                        "timestamp": timestamp,
                    }
                    # Include aruco detection result if markers were found
                    if aruco_result is not None:
                        frame_data["aruco"] = {
                            "markers": aruco_result["markers"],
                            "masked": aruco_masked,
                        }
                        if aruco_result.get("extrapolated") is not None:
                            frame_data["aruco"]["extrapolated"] = aruco_result[
                                "extrapolated"
                            ]

                    # First frame: always kept
                    if last_kept_hash is None:
                        frame_data["frame_bytes"] = self._frame_to_bytes(pil_img)
                        first_hash = self._dhash(pil_img)
                        last_kept_hash = first_hash
                        last_kept_timestamp = timestamp
                        self.first_hash = first_hash
                        self.last_hash = first_hash
                        pil_img.close()

                        self.qualified_frames.append(frame_data)
                        dhash_qualified += 1

                        logger.debug(f"First frame at {timestamp:.2f}s")
                        continue

                    # Decide against the last KEPT frame (single reference).
                    current_hash = self._dhash(pil_img)
                    keep, scene_cut, reason = _winnow_decision(
                        current_hash,
                        timestamp,
                        last_kept_hash,
                        last_kept_timestamp,
                        self.DHASH_THRESHOLD,
                        self.scene_cut_threshold,
                        self.min_stride_seconds,
                    )

                    if reason == "below_threshold":
                        pil_img.close()
                        continue

                    # Passed the dHash gate.
                    dhash_qualified += 1
                    if not keep:
                        stride_dropped += 1
                        pil_img.close()
                        continue

                    # Kept: convert full frame to bytes and advance the reference.
                    frame_data["frame_bytes"] = self._frame_to_bytes(pil_img)
                    pil_img.close()

                    self.qualified_frames.append(frame_data)
                    last_kept_hash = current_hash
                    last_kept_timestamp = timestamp
                    self.last_hash = current_hash
                    if scene_cut:
                        scene_cut_count += 1

                    logger.debug(
                        f"Qualified frame at {timestamp:.2f}s (reason: {reason})"
                    )

                self.qualified_count = len(self.qualified_frames)
                self.winnow_metrics = {
                    "raw": raw_frames,
                    "dhash_qualified": dhash_qualified,
                    "scene_cut": scene_cut_count,
                    "stride_dropped": stride_dropped,
                    "kept": self.qualified_count,
                }
                logger.info(
                    "winnowing %s raw=%d dhash_qualified=%d scene_cut=%d "
                    "stride_dropped=%d kept=%d",
                    self.video_path.name,
                    raw_frames,
                    dhash_qualified,
                    scene_cut_count,
                    stride_dropped,
                    self.qualified_count,
                )

        except av.error.FFmpegError as e:
            logger.error(
                f"Video decode error for {self.video_path}: {e}. Skipping video.",
                exc_info=True,
            )
            self.decode_failed = True
            self.qualified_count = len(self.qualified_frames)
            return self.qualified_frames
        except Exception as e:
            logger.error(
                f"Unexpected error processing video {self.video_path}: {e}",
                exc_info=True,
            )
            raise
        return self.qualified_frames

    @staticmethod
    def _format_dhash(hash_value: int | None) -> str | None:
        if hash_value is None:
            return None
        return f"{hash_value:016x}"

    def _build_metadata_header(self, fallback_observer: str | None = None) -> dict:
        # Files are in segment directories, filename is simple (e.g., center_DP-3_screen.webm)
        metadata = {"raw": self.video_path.name}

        # Add observer origin if set (from sense.py for observer uploads)
        observer = os.getenv("OBSERVER_NAME") or fallback_observer
        if observer:
            metadata["observer"] = observer

        # Add segment metadata (from sense.py via SEGMENT_META env var)
        segment_meta_str = os.getenv("SEGMENT_META")
        if segment_meta_str:
            try:
                segment_meta = json.loads(segment_meta_str)
                for key, value in segment_meta.items():
                    metadata[key] = value
            except json.JSONDecodeError:
                logger.warning(f"Invalid SEGMENT_META JSON: {segment_meta_str[:100]}")

        metadata["first_hash"] = self._format_dhash(self.first_hash)
        metadata["last_hash"] = self._format_dhash(self.last_hash)
        metadata["qualified_count"] = self.qualified_count

        # Record which brain produced the rows below this header, so a segment
        # answers "what described this?" on its own. Set by the frame pass, so
        # it names the brain that actually ran; absent when none did. This is
        # per-run, not per-call — tokens/<day>.jsonl stays the per-call truth.
        # Sibling key to _solstone_processing, never inside it — that record is
        # a consumed contract (data_state, retention, backfill).
        thinking = getattr(self, "_thinking", None)
        if thinking:
            metadata["_solstone_thinking"] = thinking
        return metadata

    def _dhash(self, img: Image.Image) -> int:
        """Compute 64-bit dHash (difference hash) for perceptual comparison."""
        small = img.resize(self.DHASH_SIZE, Image.BILINEAR).convert("L")
        pixels = list(_flattened_image_data(small))
        hash_val = 0
        for row in range(8):
            for col in range(8):
                idx = row * 9 + col
                if pixels[idx] > pixels[idx + 1]:
                    hash_val |= 1 << (row * 8 + col)
        return hash_val

    def _frame_to_bytes(self, img: Image.Image) -> bytes:
        """
        Convert full frame to PNG bytes.

        Parameters
        ----------
        img : Image.Image
            PIL Image to convert

        Returns
        -------
        bytes
            Image as PNG bytes
        """
        buf = io.BytesIO()
        img.save(buf, format="PNG", compress_level=1)
        return buf.getvalue()

    def _get_category_metadata(self, category: str) -> Optional[dict]:
        """
        Get category metadata if extraction prompt is available.

        Parameters
        ----------
        category : str
            Category from initial analysis

        Returns
        -------
        Optional[dict]
            Category metadata with 'prompt', 'output', 'context' keys,
            or None if no extraction prompt available
        """
        cat_meta = CATEGORIES.get(category)
        if cat_meta and cat_meta.get("prompt"):
            return cat_meta
        return None

    def _expected_category_names(self, analysis: dict) -> tuple[str, ...]:
        categories: list[str] = []
        primary = analysis.get("primary", "")
        secondary = analysis.get("secondary", "none")
        overlap = analysis.get("overlap", True)

        if self._get_category_metadata(primary):
            categories.append(primary)
        if (
            not overlap
            and secondary != "none"
            and self._get_category_metadata(secondary)
        ):
            categories.append(secondary)
        return tuple(categories)

    def _build_incremental_merge_plan(
        self,
        artifact: ExistingDescribeArtifact | None,
        *,
        qualified_ids: set[int],
        input_size: int,
    ) -> IncrementalMergePlan | None:
        if artifact is None or artifact.rows is None:
            return None
        if self.first_hash is None:
            return None
        if artifact.header.get("first_hash") != self._format_dhash(self.first_hash):
            return None
        if artifact.header.get("last_hash") != self._format_dhash(self.last_hash):
            return None
        if artifact.header.get("qualified_count") != self.qualified_count:
            return None
        if artifact.record.get("input_size") != input_size:
            return None

        reusable_rows: dict[int, ExistingDescribeRow] = {}
        phase1_gap_ids: set[int] = set()
        phase3_gaps: dict[int, tuple[dict, tuple[str, ...]]] = {}
        seen_ids: set[int] = set()

        for existing_row in artifact.rows:
            row = existing_row.data
            frame_id = row.get("frame_id")
            if isinstance(frame_id, bool) or not isinstance(frame_id, int):
                return None
            if frame_id not in qualified_ids or frame_id in seen_ids:
                return None
            seen_ids.add(frame_id)

            enhanced = row.get("enhanced")
            if not isinstance(enhanced, bool):
                return None

            analysis = row.get("analysis")
            if not isinstance(analysis, dict):
                phase1_gap_ids.add(frame_id)
                continue

            has_error = "error" in row
            if not has_error and enhanced is False:
                reusable_rows[frame_id] = existing_row
                continue

            expected = set(self._expected_category_names(analysis))
            if enhanced is True:
                content = row.get("content")
                if not isinstance(content, dict):
                    return None
                missing = tuple(sorted(expected - set(content)))
                if not has_error and not missing:
                    reusable_rows[frame_id] = existing_row
                    continue
                if missing:
                    requests = row.get("requests")
                    if not isinstance(requests, list):
                        return None
                    timestamp = row.get("timestamp")
                    if isinstance(timestamp, bool) or not isinstance(
                        timestamp, int | float
                    ):
                        return None
                    phase3_gaps[frame_id] = (row, missing)
                    continue

            return None

        if seen_ids != qualified_ids:
            return None

        return IncrementalMergePlan(
            reusable_rows=reusable_rows,
            phase1_gap_ids=phase1_gap_ids,
            phase3_gaps=phase3_gaps,
        )

    def _user_contents(self, prompt: str, image) -> list:
        """Build the vision request user-content list: instruction then image."""
        return [prompt, image]

    async def process_with_vision(
        self,
        max_concurrent: int,
        output_path: Optional[Path] = None,
        work_key: str | None = None,
        previous_attempts: int = 0,
        incremental_source_path: Optional[Path] = None,
    ) -> None:
        """
        Process video and write vision analysis results to file.

        Three-phase pipeline:
        1. Categorization: All frames get initial category analysis
        2. Selection: Determine which frames get detailed extraction
        3. Extraction: Selected frames get category-specific content extraction

        Parameters
        ----------
        max_concurrent : int
            Maximum number of concurrent API requests.
        output_path : Optional[Path]
            Path to write JSONL output (when None, no output file is written)
        """
        from solstone.think.batch import Batch
        from solstone.think.models import NO_BRAIN_PROVIDER, resolve_provider

        # Load config for max_extractions and redaction rules
        config = get_config()
        describe_config = config.get("describe", {})
        max_extractions = describe_config.get(
            "max_extractions", DEFAULT_MAX_EXTRACTIONS
        )
        redact_instruction = _build_redact_instruction(
            describe_config.get("redact", [])
        )
        work_key = work_key or _describe_work_key(
            self.video_path,
            day_from_path(self.video_path),
            get_segment_key(self.video_path) or "",
        )

        # Use dynamically built categorization prompt
        system_instruction = CATEGORIZATION_PROMPT + redact_instruction

        # Process video to get qualified frames (synchronous)
        qualified_frames = self.process()
        had_qualified_frames = len(qualified_frames) > 0
        current_input_size = self.video_path.stat().st_size
        qualified_by_id = {
            frame_data["frame_id"]: frame_data for frame_data in qualified_frames
        }
        qualified_ids = set(qualified_by_id)
        existing_artifact = (
            _read_existing_describe_artifact(incremental_source_path)
            if incremental_source_path is not None
            else None
        )
        carry_forward_observer = None
        if existing_artifact is not None and not os.getenv("OBSERVER_NAME"):
            observer = existing_artifact.header.get("observer")
            if isinstance(observer, str) and observer:
                carry_forward_observer = observer
        incremental_plan = self._build_incremental_merge_plan(
            existing_artifact,
            qualified_ids=qualified_ids,
            input_size=current_input_size,
        )

        # Create batch processor
        batch = Batch(max_concurrent=max_concurrent)

        # Stream output to a same-directory temp, then promote only at terminal points.
        temp_path: Optional[Path] = None
        if output_path is not None:
            temp_file = tempfile.NamedTemporaryFile(
                mode="w",
                dir=output_path.parent,
                prefix=".describe_",
                suffix=".jsonl.tmp",
                delete=False,
            )
            output_file = temp_file
            temp_path = Path(temp_file.name)
        else:
            output_file = None

        def _promote(state: str, reason_code: str) -> None:
            if output_file is not None and not output_file.closed:
                output_file.close()
            if temp_path is None or output_path is None:
                return

            final_file = tempfile.NamedTemporaryFile(
                mode="w",
                dir=output_path.parent,
                prefix=".describe_",
                suffix=".jsonl.tmp",
                delete=False,
            )
            final_path = Path(final_file.name)
            try:
                header = self._build_metadata_header(
                    fallback_observer=carry_forward_observer
                )
                attempts = previous_attempts + 1 if state == STATE_FAILED else None
                header["_solstone_processing"] = build_processing_record(
                    state=state,
                    reason_code=reason_code,
                    handler=HANDLER_DESCRIBE,
                    input_size=current_input_size,
                    attempts=attempts,
                )
                final_file.write(json.dumps(header) + "\n")
                with temp_path.open() as rows:
                    for line in rows:
                        final_file.write(line)
                final_file.close()
                install_file(final_path, output_path)
            finally:
                if not final_file.closed:
                    final_file.close()
                final_path.unlink(missing_ok=True)

        emitted_frame_ids: set[int] = set()
        emitted_row_has_error = False
        incremental_emit_lines: dict[int, str] | None = (
            {} if incremental_plan is not None else None
        )

        def _emit_row(result: dict, raw_line: str | None = None) -> None:
            nonlocal emitted_row_has_error
            result_line = (
                raw_line if raw_line is not None else json.dumps(result) + "\n"
            )
            frame_id = result["frame_id"]
            if incremental_emit_lines is not None:
                incremental_emit_lines[frame_id] = result_line
            elif output_file:
                output_file.write(result_line)
            emitted_frame_ids.add(frame_id)
            if "error" in result:
                emitted_row_has_error = True
            if logger.isEnabledFor(logging.DEBUG):
                print(result_line.rstrip("\n"), flush=True)

        try:
            if not had_qualified_frames:
                # Decoded frames are real work; do not discard them before description.
                if self.decode_failed:
                    _promote(STATE_FAILED, REASON_CORRUPT_INPUT)
                else:
                    _promote(STATE_EMPTY, REASON_NO_DECODABLE_FRAMES)
                return

            frame_provider, frame_model = resolve_provider("generate")
            if frame_provider == NO_BRAIN_PROVIDER:
                logger.info("No thinking engine selected; deferring frame description")
                return
            # Remember the brain that actually describes the rows, for the
            # header. Captured here rather than re-resolved at header-write
            # time so a mid-run deselect cannot make the header name a brain
            # that never ran, and gated on there being frames at all so an
            # empty run does not claim a description that never happened.
            if had_qualified_frames:
                self._thinking = {"provider": frame_provider, "model": frame_model}
            categorization_image_tokens = _categorization_image_token_budget(
                frame_provider
            )

            if incremental_plan is not None:
                for existing_row in incremental_plan.reusable_rows.values():
                    _emit_row(existing_row.data, raw_line=existing_row.raw_line)

            # Create vision requests for all qualified frames
            for frame_data in qualified_frames:
                frame_id = frame_data["frame_id"]
                if incremental_plan is not None and (
                    frame_id in incremental_plan.reusable_rows
                    or frame_id in incremental_plan.phase3_gaps
                ):
                    continue

                # Load frame image from bytes - keep it open until request completes
                frame_img = Image.open(io.BytesIO(frame_data["frame_bytes"]))
                frame_img = resize_for_vlm(
                    frame_img, max_image_tokens=categorization_image_tokens
                )

                req = batch.create(
                    contents=self._user_contents(
                        "Analyze this screenshot frame from a screencast recording.",
                        frame_img,
                    ),
                    context=FRAME_CONTEXT,
                    system_instruction=system_instruction,
                    json_output=True,
                    json_schema=_SCHEMA,
                    temperature=0.7,
                    max_output_tokens=512,
                    thinking_budget=1024,
                )

                # Attach metadata for tracking (store bytes, not PIL images)
                req.frame_id = frame_data["frame_id"]
                req.timestamp = frame_data["timestamp"]
                req.retry_count = 0
                req.frame_bytes = frame_data["frame_bytes"]  # Store bytes for reuse
                req.aruco = frame_data.get(
                    "aruco"
                )  # ArUco detection result (may be None)
                req.request_type = RequestType.DESCRIBE
                req.json_analysis = None  # Will store the JSON analysis result
                req.requests = []  # Track all requests for this frame
                req.initial_image = (
                    frame_img  # Keep reference to close after completion
                )

                batch.add(req)

            # Clear qualified_frames now that all requests are created
            self.qualified_frames.clear()

            # =================================================================
            # PHASE 1: Collect all categorization results
            # =================================================================
            categorized: dict = {}  # frame_id -> request
            total_frames = 0
            failed_frames = 0

            async for req in batch.drain_batch():
                total_frames += 1

                # Check for errors
                has_error = bool(req.error)
                error_msg = req.error

                # Parse JSON analysis
                if not has_error:
                    try:
                        analysis = json.loads(req.response)
                        # Unwrap single-element list (LLM sometimes wraps in [])
                        if isinstance(analysis, list) and len(analysis) == 1:
                            analysis = analysis[0]
                        if not isinstance(analysis, dict):
                            raise ValueError(
                                f"Expected dict, got {type(analysis).__name__}"
                            )
                        req.json_analysis = analysis
                    except (json.JSONDecodeError, ValueError) as e:
                        has_error = True
                        error_msg = f"Invalid JSON response: {e}"

                # Retry logic (up to 5 attempts total, so 4 retries)
                if has_error:
                    _abort_for_blocking_request(
                        req,
                        output_file=output_file,
                        temp_path=temp_path,
                        work_key=work_key,
                        batch=batch,
                    )
                if (
                    has_error
                    and req.retry_count < 4
                    and not is_non_retryable_generate_reason(
                        getattr(req, "reason_code", None)
                    )
                ):
                    req.retry_count += 1
                    total_frames -= 1  # Don't count retries
                    batch.add(req)
                    logger.info(
                        f"Retrying frame {req.frame_id} "
                        f"(attempt {req.retry_count + 1}/5): {error_msg}"
                    )
                    continue

                # Track failure after all retries exhausted
                if has_error:
                    failed_frames += 1

                # Record categorization request result
                request_record = {
                    "type": req.request_type.value,
                    "model": req.model_used,
                    "duration": req.duration,
                }
                if req.retry_count > 0:
                    request_record["retries"] = req.retry_count
                req.requests.append(request_record)

                # Store error on request for later output
                if has_error:
                    req.error_msg = error_msg

                # Close initial image - no longer needed for categorization
                if hasattr(req, "initial_image") and req.initial_image:
                    req.initial_image.close()
                    req.initial_image = None

                # Store in categorized dict (keep frame_bytes for extraction)
                categorized[req.frame_id] = req

            logger.info(
                f"Phase 1 complete: {len(categorized)} frames categorized "
                f"({failed_frames} failed)"
            )

            all_frames_failed = total_frames > 0 and failed_frames == total_frames

            # On a fresh/full run, all frames failed means there are no usable
            # rows to preserve. On an incremental run, this means only all gap
            # frames failed; reused clean rows must survive into the merge.
            if all_frames_failed and incremental_plan is None:
                error_detail = (
                    f"Error details in {output_path}"
                    if output_path
                    else "No output file"
                )
                logger.error(
                    f"All {total_frames} frame(s) failed categorization. "
                    "Promoted header-only failed output; the daily pass will "
                    f"retry until the attempt bound is reached. {error_detail}"
                )
                _promote(STATE_FAILED, REASON_ANALYSIS_FAILED)
                raise RuntimeError(
                    f"All {total_frames} frame(s) failed vision analysis after retries"
                )

            # =================================================================
            # PHASE 2: Select frames for extraction
            # =================================================================
            # Build input for selection (only successfully categorized frames)
            categorized_list = [
                {
                    "frame_id": req.frame_id,
                    "timestamp": req.timestamp,
                    "analysis": req.json_analysis,
                }
                for req in categorized.values()
                if req.json_analysis is not None
            ]
            # Sort by frame_id for consistent ordering
            categorized_list.sort(key=lambda x: x["frame_id"])

            # Run selection (pass CATEGORIES for AI-based selection)
            selected_ids = set(
                select_frames_for_extraction(
                    categorized_list, max_extractions, categories=CATEGORIES
                )
            )

            logger.info(
                f"Phase 2 complete: {len(selected_ids)} of {len(categorized_list)} "
                f"frames selected for extraction (max: {max_extractions})"
            )

            # =================================================================
            # PHASE 3: Extract content from selected frames
            # =================================================================
            # Track frames with pending extractions for merging
            frame_results: dict = {}  # frame_id -> result dict
            frame_images: dict = {}  # frame_id -> PIL Image (for cleanup)
            extraction_count = 0

            for frame_id, req in categorized.items():
                has_error = hasattr(req, "error_msg")
                error_msg = getattr(req, "error_msg", None)

                # Build base result
                result = {
                    "frame_id": req.frame_id,
                    "timestamp": req.timestamp,
                    "requests": req.requests,
                }

                if req.aruco:
                    result["aruco"] = req.aruco

                if has_error:
                    result["error"] = error_msg

                if req.json_analysis:
                    result["analysis"] = req.json_analysis
                    gate = screen_gate(req.json_analysis)
                    if gate is not None:
                        cli = await asyncio.to_thread(detect_objects, req.frame_bytes)
                        if cli is not None:
                            result["detections"] = detections_block(
                                cli, source="screen", gate=gate
                            )

                # Check if this frame is selected for extraction
                if frame_id not in selected_ids or req.json_analysis is None:
                    # Not selected or failed - output immediately with enhanced=false
                    result["enhanced"] = False

                    _emit_row(result)

                    # Release frame bytes
                    req.frame_bytes = None
                    req.json_analysis = None
                    continue

                # Frame is selected - determine extractions based on overlap logic
                primary = req.json_analysis.get("primary", "")
                secondary = req.json_analysis.get("secondary", "none")
                overlap = req.json_analysis.get("overlap", True)

                extractions = []

                # Check primary category
                primary_meta = self._get_category_metadata(primary)
                if primary_meta:
                    extractions.append((primary, primary_meta))
                else:
                    logger.warning(
                        f"Frame {frame_id}: category '{primary}' has no extraction prompt"
                    )

                # Check secondary category if no overlap
                if not overlap and secondary != "none":
                    secondary_meta = self._get_category_metadata(secondary)
                    if secondary_meta:
                        extractions.append((secondary, secondary_meta))

                # If no extractions possible, output without enhancement
                if not extractions:
                    result["enhanced"] = False

                    _emit_row(result)

                    req.frame_bytes = None
                    req.json_analysis = None
                    continue

                # Queue extraction request(s)
                full_img = Image.open(io.BytesIO(req.frame_bytes))
                full_img = resize_for_vlm(full_img)
                frame_images[frame_id] = full_img

                # Store result for merging when extractions complete
                result["enhanced"] = True
                result["pending"] = len(extractions)
                result["content"] = {}
                frame_results[frame_id] = result

                for i, (category, cat_meta) in enumerate(extractions):
                    extraction_count += 1

                    if i == 0:
                        extract_req = req
                        extract_req.category_results = {}
                    else:
                        # Create new request for secondary extraction
                        extract_req = batch.create(
                            contents=[],
                            context=cat_meta["context"],
                            json_schema=cat_meta.get("json_schema"),
                        )
                        extract_req.frame_id = req.frame_id
                        extract_req.timestamp = req.timestamp
                        extract_req.aruco = req.aruco
                        extract_req.json_analysis = req.json_analysis
                        extract_req.category_results = {}
                        extract_req.requests = result["requests"]  # Share list

                    extract_req.extraction_category = category
                    extract_req.retry_count = 0
                    extract_req.request_type = RequestType.CATEGORY

                    # Determine output format from metadata
                    is_json = cat_meta.get("output") == "json"

                    cat_provider, _ = resolve_provider("generate")
                    if cat_provider == NO_BRAIN_PROVIDER:
                        logger.info(
                            "No thinking engine selected; deferring %s extraction",
                            category,
                        )
                        continue

                    batch.update(
                        extract_req,
                        contents=self._user_contents(
                            f"Analyze this {category} screenshot.",
                            full_img,
                        ),
                        system_instruction=cat_meta["prompt"] + redact_instruction,
                        json_output=is_json,
                        json_schema=cat_meta.get("json_schema"),
                        max_output_tokens=cat_meta["max_output_tokens"],
                        thinking_budget=6144 if is_json else 4096,
                        context=cat_meta["context"],
                    )

                logger.info(
                    f"Frame {frame_id}: {len(extractions)} extraction(s) - "
                    f"{', '.join(cat for cat, _ in extractions)}"
                )

            if incremental_plan is not None:
                for frame_id, (
                    existing_row,
                    missing_categories,
                ) in incremental_plan.phase3_gaps.items():
                    frame_data = qualified_by_id[frame_id]
                    analysis = existing_row["analysis"]
                    result = dict(existing_row)
                    result["requests"] = list(existing_row["requests"])
                    result["content"] = dict(existing_row.get("content", {}))
                    result["enhanced"] = True
                    result["pending"] = len(missing_categories)
                    result.pop("error", None)
                    frame_results[frame_id] = result

                    full_img = Image.open(io.BytesIO(frame_data["frame_bytes"]))
                    full_img = resize_for_vlm(full_img)
                    frame_images[frame_id] = full_img

                    for category in missing_categories:
                        cat_meta = self._get_category_metadata(category)
                        if cat_meta is None:
                            continue
                        extraction_count += 1
                        extract_req = batch.create(
                            contents=[],
                            context=cat_meta["context"],
                            json_schema=cat_meta.get("json_schema"),
                        )
                        extract_req.frame_id = frame_id
                        extract_req.timestamp = result["timestamp"]
                        extract_req.aruco = result.get("aruco")
                        extract_req.json_analysis = analysis
                        extract_req.category_results = {}
                        extract_req.requests = result["requests"]
                        extract_req.extraction_category = category
                        extract_req.retry_count = 0
                        extract_req.request_type = RequestType.CATEGORY

                        is_json = cat_meta.get("output") == "json"
                        cat_provider, _ = resolve_provider("generate")
                        if cat_provider == NO_BRAIN_PROVIDER:
                            logger.info(
                                "No thinking engine selected; deferring %s extraction",
                                category,
                            )
                            continue

                        batch.update(
                            extract_req,
                            contents=self._user_contents(
                                f"Analyze this {category} screenshot.",
                                full_img,
                            ),
                            system_instruction=cat_meta["prompt"] + redact_instruction,
                            json_output=is_json,
                            json_schema=cat_meta.get("json_schema"),
                            max_output_tokens=cat_meta["max_output_tokens"],
                            thinking_budget=6144 if is_json else 4096,
                            context=cat_meta["context"],
                        )

            logger.info(f"Phase 3: {extraction_count} extraction request(s) queued")

            # Drain extraction results
            async for req in batch.drain_batch():
                has_error = bool(req.error)
                error_msg = req.error

                # Parse extraction result
                if not has_error:
                    category = req.extraction_category
                    cat_meta = self._get_category_metadata(category)
                    if cat_meta and cat_meta.get("output") == "json":
                        try:
                            result_data = json.loads(req.response)
                            req.category_results[category] = result_data
                        except json.JSONDecodeError as e:
                            has_error = True
                            error_msg = f"Invalid JSON response for {category}: {e}"
                    else:
                        # Markdown output - bound before journaling
                        req.category_results[category] = bound_extraction_markdown(
                            req.response
                        )

                # Retry logic
                if has_error:
                    _abort_for_blocking_request(
                        req,
                        output_file=output_file,
                        temp_path=temp_path,
                        work_key=work_key,
                        batch=batch,
                    )
                if (
                    has_error
                    and req.retry_count < 4
                    and not is_non_retryable_generate_reason(
                        getattr(req, "reason_code", None)
                    )
                ):
                    req.retry_count += 1
                    batch.add(req)
                    logger.info(
                        f"Retrying extraction {req.frame_id}/{req.extraction_category} "
                        f"(attempt {req.retry_count + 1}/5): {error_msg}"
                    )
                    continue

                # Record extraction request result
                request_record = {
                    "type": req.request_type.value,
                    "model": req.model_used,
                    "duration": req.duration,
                    "category": req.extraction_category,
                }
                if req.retry_count > 0:
                    request_record["retries"] = req.retry_count
                req.requests.append(request_record)

                # Get the frame result we're merging into
                result = frame_results.get(req.frame_id)
                if result is None:
                    logger.error(f"Extraction result for unknown frame {req.frame_id}")
                    continue

                # Merge extraction result
                if has_error:
                    if "error" not in result:
                        result["error"] = error_msg
                else:
                    for category, cat_result in req.category_results.items():
                        result["content"][category] = cat_result

                # Decrement pending count
                result["pending"] -= 1

                # If all extractions complete, output the result
                if result["pending"] <= 0:
                    del result["pending"]

                    _emit_row(result)

                    # Clean up
                    del frame_results[req.frame_id]
                    if req.frame_id in frame_images:
                        frame_images[req.frame_id].close()
                        del frame_images[req.frame_id]

            for frame_id, result in list(frame_results.items()):
                result["error"] = "Extraction never completed"
                result.pop("pending", None)
                _emit_row(result)
                del frame_results[frame_id]
                if frame_id in frame_images:
                    frame_images[frame_id].close()
                    del frame_images[frame_id]

            if incremental_emit_lines is not None and output_file:
                for frame_id in sorted(incremental_emit_lines):
                    output_file.write(incremental_emit_lines[frame_id])

            if self.decode_failed:
                state, reason_code = STATE_FAILED, REASON_CORRUPT_INPUT
            elif not emitted_row_has_error and emitted_frame_ids == qualified_ids:
                state, reason_code = STATE_ANALYZED, REASON_OK
            else:
                state, reason_code = STATE_FAILED, REASON_ANALYSIS_FAILED

            _promote(state, reason_code)
        finally:
            await batch.aclose()

            # Close output and discard the transient row temp.
            if output_file is not None and not output_file.closed:
                output_file.close()
            if temp_path is not None:
                temp_path.unlink(missing_ok=True)

            # Clean up any remaining frame images (in case of exception)
            if "frame_images" in locals():
                for img in frame_images.values():
                    try:
                        img.close()
                    except Exception:
                        pass
                frame_images.clear()

        # Report any failures
        if failed_frames > 0:
            logger.warning(
                f"{failed_frames}/{total_frames} frame(s) failed categorization."
            )


def output_qualified_frames(
    processor: VideoProcessor, qualified_frames: List[dict]
) -> None:
    """Output qualified frames as JSON."""
    output = {
        "video": str(processor.video_path.name),
        "width": processor.width,
        "height": processor.height,
        "frames": [
            {
                "frame_id": frame["frame_id"],
                "timestamp": frame["timestamp"],
            }
            for frame in qualified_frames
        ],
    }

    sys.stdout.write(json.dumps(output, indent=2) + "\n")


async def async_main():
    """Async CLI entry point."""
    parser = argparse.ArgumentParser(
        description="Describe screencast videos with vision analysis"
    )
    parser.add_argument(
        "video_path",
        type=str,
        help="Path to video file in segment directory",
    )
    parser.add_argument(
        "-j",
        "--jobs",
        type=int,
        default=None,
        help="Max concurrent vision API requests (default: provider policy)",
    )
    parser.add_argument(
        "--frames-only",
        action="store_true",
        help="Only output frame metadata without vision analysis",
    )
    parser.add_argument(
        "--redo",
        action="store_true",
        help="Reprocess file, overwriting existing outputs",
    )
    args = setup_cli(parser)
    require_solstone()

    video_path = Path(args.video_path)
    if not video_path.exists():
        parser.error(f"Video file not found: {video_path}")

    # Files must be in segment directories (YYYYMMDD/HHMMSS_LEN/)
    segment = get_segment_key(video_path)
    if segment is None:
        parser.error(
            f"Video file must be in a segment directory (HHMMSS_LEN/), "
            f"but parent is: {video_path.parent.name}"
        )

    # Determine output path
    output_path = None
    previous_attempts = 0
    incremental_source_path = None
    if not args.frames_only:
        # Output JSONL in same directory, same stem (e.g., center_DP-3_screen.jsonl)
        output_path = video_path.with_suffix(".jsonl")

        # Skip if already processed (unless redo mode)
        if not args.redo and output_path.exists():
            record = read_processing_record_header(output_path)
            if not should_reenter_analysis_output(
                record=record,
                output_path=output_path,
                handler=HANDLER_DESCRIBE,
            ):
                logger.info(f"Already processed: {video_path}")
                return
            previous_attempts = record_attempts(record)
            incremental_source_path = output_path

        if output_path.exists():
            logger.warning(f"Overwriting existing analysis file: {output_path}")

        day = day_from_path(video_path)
        work_key = _describe_work_key(video_path, day, segment)
    else:
        day = day_from_path(video_path)
        work_key = _describe_work_key(video_path, day, segment)

    logger.info(f"Processing video: {video_path}")

    start_time = time.time()

    try:
        processor = VideoProcessor(video_path)

        if args.frames_only:
            # Original behavior: just output frame metadata
            qualified_frames = processor.process()
            output_qualified_frames(processor, qualified_frames)
        else:
            # New behavior: process with vision analysis
            max_concurrent = (
                args.jobs
                if args.jobs is not None
                else fanout_policy.describe_per_proc_jobs(1)
            )
            await processor.process_with_vision(
                max_concurrent=max_concurrent,
                output_path=output_path,
                work_key=work_key,
                previous_attempts=previous_attempts,
                incremental_source_path=incremental_source_path,
            )

            # Emit completion event
            if output_path and output_path.exists():
                journal_path = Path(get_journal())

                try:
                    rel_input = journal_relative_path(journal_path, video_path)
                    rel_output = journal_relative_path(journal_path, output_path)
                except ValueError:
                    rel_input = video_path
                    rel_output = output_path

                duration_ms = int((time.time() - start_time) * 1000)

                event_fields = {
                    "input": str(rel_input),
                    "output": str(rel_output),
                    "duration_ms": duration_ms,
                }
                if day:
                    event_fields["day"] = day
                if segment:
                    event_fields["segment"] = segment
                observer = os.getenv("OBSERVER_NAME")
                if observer:
                    event_fields["observer"] = observer
                callosum_send("observe", "described", **event_fields)
    except Exception as e:
        logger.error(f"Failed to process {video_path}: {e}", exc_info=True)
        raise


def main():
    """CLI entry point."""
    asyncio.run(async_main())


if __name__ == "__main__":
    main()
