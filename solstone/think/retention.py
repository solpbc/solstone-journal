# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Raw-media retention policy and safety predicates for solstone journals.

Retention policy controls whether raw media in a processed segment is eligible to
be marked for removal. This module does not remove media.

Scope: raw media ONLY. Chronicle JSONL, derived outputs, talents/ directories,
and all other journal content persist indefinitely and are never touched by
retention. Days mode requires an explicit days value; without one, raw media is
not eligible.
"""

from __future__ import annotations

import json
import shutil
from dataclasses import dataclass, field
from pathlib import Path

from solstone.apps.backup.copy import OFFLOAD_STALL_REASON_LABELS, OFFLOAD_STALLED_LEAD
from solstone.think.backup.state import merge_backup_config
from solstone.think.data_state import DataState, derive_modality_state
from solstone.think.media import AUDIO_EXTENSIONS as RAW_AUDIO_EXTENSIONS
from solstone.think.media import MEDIA_EXTENSIONS as RAW_MEDIA_EXTENSIONS
from solstone.think.media import VIDEO_EXTENSIONS as RAW_VIDEO_EXTENSIONS
from solstone.think.utils import day_dirs, iter_segments

# ---------------------------------------------------------------------------
# Raw media file identification
# ---------------------------------------------------------------------------


def is_raw_media(path: Path) -> bool:
    """Check if a file is raw media (layer 1 capture).

    Raw media: every registry container format in ``solstone.think.media.FORMATS``
    — audio (*.flac, *.opus, *.ogg, *.m4a, *.mp3, *.wav), video (*.webm, *.mp4,
    *.mov), and still images (*.png, *.jpg, *.jpeg, *.heic, *.heif, *.gif,
    *.webp, *.tiff — e.g. mentra-live photos) — plus monitor_*_diff.png screen
    diffs (covered by the registry since images joined it; kept explicit).
    """
    if path.suffix.lower() in RAW_MEDIA_EXTENSIONS:
        return True
    if (
        path.suffix.lower() == ".png"
        and path.name.startswith("monitor_")
        and "_diff" in path.name
    ):
        return True
    return False


def get_raw_media_files(segment_path: Path) -> list[Path]:
    """Return all raw media files in a segment directory."""
    if not segment_path.is_dir():
        return []
    return [f for f in segment_path.iterdir() if f.is_file() and is_raw_media(f)]


# ---------------------------------------------------------------------------
# Completion detection (safety invariant)
# ---------------------------------------------------------------------------


@dataclass
class SegmentGate:
    """Deletion gate verdict for one segment."""

    verdict: str
    failed_files: dict[str, str] = field(default_factory=dict)
    completion_files: list[Path] = field(default_factory=list)


def _matching_extraction_files(
    segment_path: Path, patterns: tuple[str, ...]
) -> list[Path]:
    return sorted(
        {
            path
            for pattern in patterns
            for path in segment_path.glob(pattern)
            if path.is_file()
        }
    )


def _derive_extraction_file_state(
    seg_path: Path,
    jsonl_path: Path,
    *,
    modality: str,
    marker_key: str,
    has_raw: bool,
) -> str:
    """Read one extraction file strictly enough for irreversible deletion."""
    try:
        lines: list[str] = []
        with jsonl_path.open("r", encoding="utf-8") as handle:
            for line in handle:
                if not line.strip():
                    continue
                lines.append(line)
                if len(lines) == 2:
                    break
    except OSError:
        return "malformed"

    if not lines:
        return "malformed"

    try:
        header = json.loads(lines[0])
    except (json.JSONDecodeError, ValueError):
        return "malformed"
    if not isinstance(header, dict):
        return "malformed"

    record = header.get("_solstone_processing")
    if not isinstance(record, dict):
        record = None

    # Row 1 is always the metadata header. A stray marker key merged through
    # SEGMENT_META must never make a header-only file look chunk-bearing when
    # retention is deciding whether raw media can be irreversibly deleted.
    has_chunks = False
    if len(lines) == 2:
        try:
            first_chunk = json.loads(lines[1])
        except (json.JSONDecodeError, ValueError):
            first_chunk = None
        has_chunks = isinstance(first_chunk, dict) and marker_key in first_chunk

    return derive_modality_state(
        seg_path,
        modality,
        has_chunks=has_chunks,
        has_jsonl=True,
        has_raw=has_raw,
        record=record,
    )


def _collect_extraction_states(
    segment_path: Path,
    paths: list[Path],
    *,
    modality: str,
    marker_key: str,
    has_raw: bool,
    failed_files: dict[str, str],
    completion_files: list[Path],
) -> bool:
    incomplete = False
    for path in paths:
        state = _derive_extraction_file_state(
            segment_path,
            path,
            modality=modality,
            marker_key=marker_key,
            has_raw=has_raw,
        )
        if state == "malformed":
            failed_files[path.name] = "malformed"
        elif state in {DataState.FAILED.value, DataState.FAILED_FINAL.value}:
            failed_files[path.name] = state
        elif state in {DataState.PENDING.value, DataState.ANALYZING.value}:
            incomplete = True
        elif state in {DataState.ANALYZED.value, DataState.EMPTY.value}:
            completion_files.append(path)
        else:  # pragma: no cover - derive_modality_state owns this closed vocabulary.
            raise RuntimeError(f"unexpected {modality} data state for {path}: {state}")
    return incomplete


def resolve_segment_gate(segment_path: Path) -> SegmentGate:
    """Resolve whether a segment's raw media is safe to purge."""
    agents_dir = segment_path / "talents"
    incomplete = False
    failed_files: dict[str, str] = {}
    completion_files: list[Path] = []

    if agents_dir.is_dir():
        for f in agents_dir.iterdir():
            if f.is_file() and f.name.endswith("_active.jsonl"):
                incomplete = True
                break

    files = [f for f in segment_path.iterdir() if f.is_file()]
    file_suffixes = {f.suffix.lower() for f in files}
    has_audio_raw = bool(file_suffixes & RAW_AUDIO_EXTENSIONS)
    has_video_raw = bool(file_suffixes & RAW_VIDEO_EXTENSIONS)
    audio_extracts = _matching_extraction_files(
        segment_path, ("audio.jsonl", "*_audio.jsonl")
    )
    screen_extracts = _matching_extraction_files(
        segment_path, ("screen.jsonl", "*_screen.jsonl")
    )

    if has_audio_raw and not audio_extracts:
        incomplete = True

    # monitor_*_diff.png files are raw media but have no extraction record. They
    # ride the whole-segment gate and are only deleted when audio/video checks pass.
    if has_video_raw and not screen_extracts:
        incomplete = True

    speaker_labels = agents_dir / "speaker_labels.json"
    if ".npz" in file_suffixes:
        if not agents_dir.is_dir() or not speaker_labels.exists():
            incomplete = True

    incomplete = (
        _collect_extraction_states(
            segment_path,
            audio_extracts,
            modality="audio",
            marker_key="start",
            has_raw=has_audio_raw,
            failed_files=failed_files,
            completion_files=completion_files,
        )
        or incomplete
    )
    incomplete = (
        _collect_extraction_states(
            segment_path,
            screen_extracts,
            modality="screen",
            marker_key="timestamp",
            has_raw=has_video_raw,
            failed_files=failed_files,
            completion_files=completion_files,
        )
        or incomplete
    )

    if failed_files:
        return SegmentGate("failed", failed_files=failed_files)
    if speaker_labels.exists():
        completion_files.append(speaker_labels)
    if incomplete:
        return SegmentGate("incomplete", completion_files=completion_files)
    return SegmentGate("eligible", completion_files=completion_files)


# ---------------------------------------------------------------------------
# Retention configuration
# ---------------------------------------------------------------------------


@dataclass
class RetentionPolicy:
    """Retention policy for a single scope (global or per-stream)."""

    mode: str = "keep"  # "keep", "days", or "processed"
    days: int | None = None

    def is_eligible(self, segment_age_days: int) -> bool:
        """Check if a segment's raw media should be purged under this policy."""
        if self.mode == "keep":
            return False
        if self.mode == "processed":
            return True
        if self.mode == "days" and self.days is not None:
            return segment_age_days >= self.days
        return False


@dataclass
class RetentionConfig:
    """Retention configuration from journal.json."""

    default: RetentionPolicy = field(default_factory=RetentionPolicy)
    per_stream: dict[str, RetentionPolicy] = field(default_factory=dict)

    def policy_for_stream(self, stream: str) -> RetentionPolicy:
        """Return the effective policy for a stream."""
        return self.per_stream.get(stream, self.default)


def load_retention_config() -> RetentionConfig:
    """Load retention configuration from journal.json."""
    from solstone.think.utils import get_config

    config = get_config()
    retention = config.get("retention", {})

    mode = retention.get("raw_media", "keep")
    days = retention.get("raw_media_days", None)
    default = RetentionPolicy(mode=mode, days=days)

    per_stream: dict[str, RetentionPolicy] = {}
    for stream_name, stream_config in retention.get("per_stream", {}).items():
        per_stream[stream_name] = RetentionPolicy(
            mode=stream_config.get("raw_media", mode),
            days=stream_config.get("raw_media_days", days),
        )

    return RetentionConfig(default=default, per_stream=per_stream)


# ---------------------------------------------------------------------------
# Storage summary
# ---------------------------------------------------------------------------


def _human_bytes(size: int) -> str:
    """Format byte count as human-readable string."""
    n = float(size)
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if abs(n) < 1024:
            if unit == "B":
                return f"{int(n)} B"
            return f"{n:.1f} {unit}"
        n /= 1024
    return f"{n:.1f} PB"


@dataclass
class StorageSummary:
    """Storage usage summary for a journal."""

    raw_media_bytes: int = 0
    derived_bytes: int = 0
    total_segments: int = 0
    segments_with_raw: int = 0
    segments_purged: int = 0

    @property
    def raw_media_human(self) -> str:
        return _human_bytes(self.raw_media_bytes)

    @property
    def derived_human(self) -> str:
        return _human_bytes(self.derived_bytes)


def compute_storage_summary() -> StorageSummary:
    """Compute storage summary across all journal segments."""
    summary = StorageSummary()

    for day_name in sorted(day_dirs().keys()):
        for _stream, _seg_key, seg_path in iter_segments(day_name):
            summary.total_segments += 1

            raw_files = get_raw_media_files(seg_path)
            summary.raw_media_bytes += sum(f.stat().st_size for f in raw_files)

            if raw_files:
                summary.segments_with_raw += 1
            elif (seg_path / "audio.jsonl").exists() or (
                seg_path / "screen.jsonl"
            ).exists():
                summary.segments_purged += 1

            for f in seg_path.rglob("*"):
                if f.is_file() and not is_raw_media(f):
                    summary.derived_bytes += f.stat().st_size

    return summary


def check_storage_health(
    summary: StorageSummary,
    journal_path: str | Path,
    config: dict | None = None,
) -> list[dict]:
    """Check storage health against configured thresholds.

    Parameters
    ----------
    summary
        Pre-computed storage summary (avoids recomputation).
    journal_path
        Journal root path, used for disk usage check.
    config
        Full journal config dict. Loaded via get_config() if not provided.

    Returns
    -------
    list[dict]
        List of warning dicts. Empty when healthy.
    """
    if config is None:
        from solstone.think.utils import get_config

        config = get_config()

    retention = config.get("retention", {})
    keep_mode_nudge = (
        " you're currently set to always retain original media, so no eligible "
        "originals are marked. originals stay on disk until you act."
    )
    always_retain_enabled = retention.get("raw_media", "keep") == "keep"
    warnings = []

    # Check disk usage percentage
    disk_threshold = retention.get("storage_warning_disk_percent", 80)
    if disk_threshold is not None:
        try:
            usage = shutil.disk_usage(str(journal_path))
            disk_percent = round(usage.used / usage.total * 100, 1)
            if disk_percent >= disk_threshold:
                message = (
                    f"Disk is {disk_percent}% full (threshold: {disk_threshold}%). "
                    "You can adjust retention settings or select \"mark eligible "
                    "originals\" to add eligible originals to the held list; "
                    "originals stay on disk until you act."
                )
                if always_retain_enabled:
                    message += keep_mode_nudge
                warnings.append(
                    {
                        "level": "warning",
                        "type": "disk_percent",
                        "message": message,
                        "current": disk_percent,
                        "threshold": disk_threshold,
                    }
                )
        except OSError:
            pass

    # Check raw media size
    raw_media_gb_threshold = retention.get("storage_warning_raw_media_gb")
    if raw_media_gb_threshold is not None:
        raw_media_gb = round(summary.raw_media_bytes / (1024**3), 2)
        if raw_media_gb >= raw_media_gb_threshold:
            message = (
                f"Raw media is {raw_media_gb} GB (threshold: {raw_media_gb_threshold} GB). "
                "You can adjust retention settings or select \"mark eligible "
                "originals\" to add eligible originals to the held list; "
                "originals stay on disk until you act."
            )
            if always_retain_enabled:
                message += keep_mode_nudge
            warnings.append(
                {
                    "level": "warning",
                    "type": "raw_media_gb",
                    "message": message,
                    "current": raw_media_gb,
                    "threshold": raw_media_gb_threshold,
                }
            )

    backup = merge_backup_config(config)
    offload = backup["offload"]
    last_offload = backup["last_offload"]
    if offload.get("enabled") is True and last_offload.get("status") == "stalled":
        reason = last_offload.get("reason")
        reason_label = (
            OFFLOAD_STALL_REASON_LABELS.get(reason) if isinstance(reason, str) else None
        )
        warnings.append(
            {
                "level": "warning",
                "type": "offload_stalled",
                "message": " ".join(
                    part for part in (OFFLOAD_STALLED_LEAD, reason_label) if part
                ),
                "current": None,
                "threshold": None,
            }
        )

    return warnings
