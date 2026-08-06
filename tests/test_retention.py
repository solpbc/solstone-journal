# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for think.retention — media retention service."""

import json
import os
import shutil
from pathlib import Path

import pytest

from solstone.apps.backup.copy import (
    OFFLOAD_STALL_REASON_LABELS,
    OFFLOAD_STALLED_LEAD,
)
from solstone.observe.processing_record import (
    FAILED_ATTEMPT_BOUND,
    STATE_EMPTY,
    STATE_FAILED,
)
from solstone.think.retention import (
    RetentionConfig,
    RetentionPolicy,
    StorageSummary,
    _human_bytes,
    check_storage_health,
    get_raw_media_files,
    is_raw_media,
    load_retention_config,
    resolve_segment_gate,
)


def _write_jsonl(path: Path, *records: dict) -> None:
    path.write_text(
        "".join(json.dumps(record) + "\n" for record in records),
        encoding="utf-8",
    )


def _processing_record(state: str, *, attempts: int | None = None) -> dict:
    record = {
        "schema": "solstone.processing.v1",
        "state": state,
        "reason_code": "test",
        "handler": "test",
        "attempted_at": "2026-01-01T00:00:00Z",
        "input_size": 1,
    }
    if attempts is not None:
        record["attempts"] = attempts
    return record


def _write_audio_success(path: Path, raw: str = "audio.flac") -> None:
    _write_jsonl(path, {"raw": raw}, {"start": "00:00:00", "text": "ok"})


def _write_screen_success(path: Path, raw: str = "screen.webm") -> None:
    _write_jsonl(path, {"raw": raw}, {"timestamp": 0.0, "content": {}})


def _write_processing_header(path: Path, raw: str, state: str) -> None:
    _write_jsonl(path, {"raw": raw, "_solstone_processing": _processing_record(state)})


# ---------------------------------------------------------------------------
# is_raw_media
# ---------------------------------------------------------------------------


class TestIsRawMedia:
    def test_audio_extensions(self, tmp_path):
        for ext in (".flac", ".opus", ".ogg", ".m4a", ".mp3", ".wav"):
            p = tmp_path / f"audio{ext}"
            p.touch()
            assert is_raw_media(p), f"{ext} should be raw media"

    def test_video_extensions(self, tmp_path):
        for ext in (".webm", ".mov", ".mp4"):
            p = tmp_path / f"screen{ext}"
            p.touch()
            assert is_raw_media(p), f"{ext} should be raw media"

    def test_monitor_diff_png(self, tmp_path):
        p = tmp_path / "monitor_1_diff.png"
        p.touch()
        assert is_raw_media(p)

        p2 = tmp_path / "monitor_2_diff.png"
        p2.touch()
        assert is_raw_media(p2)

    def test_image_extensions(self, tmp_path):
        # Still images joined the media registry with mentra-live photo
        # support: captured/imported images are layer-1 raw media, same
        # lifecycle as raw audio and video.
        for ext in (
            ".png",
            ".jpg",
            ".jpeg",
            ".heic",
            ".heif",
            ".gif",
            ".webp",
            ".tiff",
        ):
            p = tmp_path / f"photo{ext}"
            p.touch()
            assert is_raw_media(p), f"{ext} should be raw media"

    def test_not_raw_media(self, tmp_path):
        for name in (
            "audio.jsonl",
            "screen.jsonl",
            "stream.json",
            "speaker_labels.json",
            "audio.npz",
            "summary.md",
        ):
            p = tmp_path / name
            p.touch()
            assert not is_raw_media(p), f"{name} should NOT be raw media"


# ---------------------------------------------------------------------------
# get_raw_media_files
# ---------------------------------------------------------------------------


class TestGetRawMediaFiles:
    def test_returns_only_raw(self, tmp_path):
        (tmp_path / "audio.flac").write_bytes(b"x" * 100)
        (tmp_path / "screen.webm").write_bytes(b"x" * 200)
        (tmp_path / "audio.jsonl").write_text("transcript")
        (tmp_path / "stream.json").write_text("{}")

        raw = get_raw_media_files(tmp_path)
        names = {f.name for f in raw}
        assert names == {"audio.flac", "screen.webm"}

    def test_empty_dir(self, tmp_path):
        assert get_raw_media_files(tmp_path) == []

    def test_nonexistent_dir(self, tmp_path):
        assert get_raw_media_files(tmp_path / "nope") == []


# ---------------------------------------------------------------------------
# resolve_segment_gate
# ---------------------------------------------------------------------------


def _make_segment(
    tmp_path,
    *,
    audio=False,
    video=False,
    video_name="screen.webm",
    embeddings=False,
    audio_extract=True,
    screen_extract=True,
    speaker_labels=True,
    active_agents=False,
):
    """Create a segment directory with specified contents."""
    seg = tmp_path / "segment"
    seg.mkdir(parents=True, exist_ok=True)
    agents_dir = seg / "talents"
    agents_dir.mkdir(exist_ok=True)

    if audio:
        (seg / "audio.flac").write_bytes(b"audio")
    if video:
        (seg / video_name).write_bytes(b"video")
    if embeddings:
        (seg / "audio.npz").write_bytes(b"npz")
    if audio and audio_extract:
        _write_audio_success(seg / "audio.jsonl")
    if video and screen_extract:
        _write_screen_success(seg / "screen.jsonl", video_name)
    if embeddings and speaker_labels:
        (agents_dir / "speaker_labels.json").write_text("{}")
    if active_agents:
        (agents_dir / "1234_active.jsonl").write_text("{}")

    (seg / "stream.json").write_text('{"stream":"default"}')
    return seg


class TestResolveSegmentGate:
    def test_complete_audio_video_is_eligible(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, video=True, embeddings=True)
        assert resolve_segment_gate(seg).verdict == "eligible"

    def test_complete_audio_only_is_eligible(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True)
        assert resolve_segment_gate(seg).verdict == "eligible"

    def test_complete_video_only_is_eligible(self, tmp_path):
        seg = _make_segment(tmp_path, video=True)
        assert resolve_segment_gate(seg).verdict == "eligible"

    def test_incomplete_missing_audio_extract(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, audio_extract=False)
        assert resolve_segment_gate(seg).verdict == "incomplete"

    def test_incomplete_missing_screen_extract(self, tmp_path):
        seg = _make_segment(tmp_path, video=True, screen_extract=False)
        assert resolve_segment_gate(seg).verdict == "incomplete"

    def test_incomplete_missing_screen_extract_for_mp4(self, tmp_path):
        seg = _make_segment(
            tmp_path, video=True, video_name="screen.mp4", screen_extract=False
        )
        assert resolve_segment_gate(seg).verdict == "incomplete"

    def test_complete_mp4_with_screen_extract(self, tmp_path):
        seg = _make_segment(tmp_path, video=True, video_name="screen.mp4")
        assert resolve_segment_gate(seg).verdict == "eligible"

    def test_incomplete_missing_speaker_labels(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, embeddings=True, speaker_labels=False)
        assert resolve_segment_gate(seg).verdict == "incomplete"

    def test_complete_with_stub_speaker_labels(self, tmp_path):
        """Stub speaker_labels.json (skipped=True, labels=[]) unblocks retention."""
        seg = _make_segment(tmp_path, audio=True, embeddings=True, speaker_labels=False)
        stub = seg / "talents" / "speaker_labels.json"
        stub.write_text(
            json.dumps({"labels": [], "skipped": True, "reason": "no_owner_centroid"})
        )
        assert resolve_segment_gate(seg).verdict == "eligible"

    def test_incomplete_active_agents(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, active_agents=True)
        assert resolve_segment_gate(seg).verdict == "incomplete"

    def test_no_raw_media_is_eligible_with_terminal_extract(self, tmp_path):
        """Segment with only terminal derived content is considered eligible."""
        seg = tmp_path / "segment"
        seg.mkdir()
        _write_audio_success(seg / "audio.jsonl")
        (seg / "stream.json").write_text("{}")
        assert resolve_segment_gate(seg).verdict == "eligible"

    def test_no_agents_dir_is_ok(self, tmp_path):
        """No agents/ directory = no active agents = passes check 1."""
        seg = tmp_path / "segment"
        seg.mkdir()
        (seg / "stream.json").write_text("{}")
        assert resolve_segment_gate(seg).verdict == "eligible"

    def test_failed_audio_record_blocks(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, audio_extract=False)
        _write_processing_header(seg / "audio.jsonl", "audio.flac", STATE_FAILED)

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"audio.jsonl": "failed"}

    def test_failed_final_audio_record_blocks(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, audio_extract=False)
        _write_jsonl(
            seg / "audio.jsonl",
            {
                "raw": "audio.flac",
                "_solstone_processing": _processing_record(
                    STATE_FAILED,
                    attempts=FAILED_ATTEMPT_BOUND,
                ),
            },
        )

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"audio.jsonl": "failed_final"}

    def test_corrupt_analyzing_marker_blocks(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, audio_extract=False)
        _write_jsonl(seg / "audio.jsonl", {"raw": "audio.flac"})
        (seg / ".analyzing_audio").write_text("not json", encoding="utf-8")

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"audio.jsonl": "failed"}

    def test_stale_analyzing_marker_blocks(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, audio_extract=False)
        _write_jsonl(seg / "audio.jsonl", {"raw": "audio.flac"})
        marker = seg / ".analyzing_audio"
        marker.write_text(
            '{"started_at": "2026-04-15T10:00:00Z", "modality": "audio"}\n',
            encoding="utf-8",
        )
        os.utime(marker, (0, 0))

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"audio.jsonl": "failed"}

    def test_failed_marker_blocks(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, audio_extract=False)
        _write_jsonl(seg / "audio.jsonl", {"raw": "audio.flac"})
        (seg / ".analyze_failed_audio").write_text(
            '{"reason": "test"}\n', encoding="utf-8"
        )

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"audio.jsonl": "failed"}

    def test_empty_audio_record_is_eligible(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, audio_extract=False)
        _write_processing_header(seg / "audio.jsonl", "audio.flac", STATE_EMPTY)

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "eligible"
        assert seg / "audio.jsonl" in gate.completion_files

    def test_legacy_chunk_bearing_no_record_is_eligible(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True)

        assert resolve_segment_gate(seg).verdict == "eligible"

    def test_audio_header_start_poisoning_does_not_mask_failure(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, audio_extract=False)
        header = {
            "raw": "audio.flac",
            "start": "not-a-chunk",
            "_solstone_processing": _processing_record(STATE_FAILED),
        }
        _write_jsonl(seg / "audio.jsonl", header)

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"audio.jsonl": "failed"}

    def test_screen_header_timestamp_poisoning_does_not_mask_failure(self, tmp_path):
        seg = _make_segment(tmp_path, video=True, screen_extract=False)
        header = {
            "raw": "screen.webm",
            "timestamp": "not-a-chunk",
            "_solstone_processing": _processing_record(STATE_FAILED),
        }
        _write_jsonl(seg / "screen.jsonl", header)

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"screen.jsonl": "failed"}

    def test_audio_analyzed_sibling_does_not_mask_failed_sibling(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True)
        (seg / "meeting_audio.flac").write_bytes(b"audio")
        _write_processing_header(
            seg / "meeting_audio.jsonl", "meeting_audio.flac", STATE_FAILED
        )

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"meeting_audio.jsonl": "failed"}

    def test_screen_analyzed_sibling_does_not_mask_failed_sibling(self, tmp_path):
        seg = _make_segment(tmp_path, video=True, screen_extract=False)
        (seg / "left_screen.webm").write_bytes(b"video")
        (seg / "right_screen.webm").write_bytes(b"video")
        _write_screen_success(seg / "left_screen.jsonl", "left_screen.webm")
        _write_processing_header(
            seg / "right_screen.jsonl", "right_screen.webm", STATE_FAILED
        )

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"right_screen.jsonl": "failed"}

    def test_screen_analyzed_sibling_does_not_mask_chunk_bearing_failed_sibling(
        self, tmp_path
    ):
        seg = _make_segment(tmp_path, video=True, screen_extract=False)
        (seg / "left_screen.webm").write_bytes(b"video")
        (seg / "right_screen.webm").write_bytes(b"video")
        _write_screen_success(seg / "left_screen.jsonl", "left_screen.webm")
        _write_jsonl(
            seg / "right_screen.jsonl",
            {
                "raw": "right_screen.webm",
                "_solstone_processing": _processing_record(STATE_FAILED),
            },
            {"timestamp": 0.0, "content": {}},
        )

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"right_screen.jsonl": "failed"}

    def test_pending_and_analyzing_are_incomplete_not_blocked(self, tmp_path):
        pending = _make_segment(tmp_path / "pending", audio=True, audio_extract=False)
        _write_jsonl(pending / "audio.jsonl", {"raw": "audio.flac"})
        analyzing = _make_segment(
            tmp_path / "analyzing", audio=True, audio_extract=False
        )
        _write_jsonl(analyzing / "audio.jsonl", {"raw": "audio.flac"})
        (analyzing / ".analyzing_audio").write_text(
            '{"started_at": "2026-04-15T10:00:00Z", "modality": "audio"}\n',
            encoding="utf-8",
        )

        assert resolve_segment_gate(pending).verdict == "incomplete"
        assert resolve_segment_gate(analyzing).verdict == "incomplete"

    def test_zero_nonblank_extraction_blocks_as_malformed(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, audio_extract=False)
        (seg / "audio.jsonl").write_text("\n\n", encoding="utf-8")

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"audio.jsonl": "malformed"}

    def test_invalid_json_header_blocks_as_malformed(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, audio_extract=False)
        (seg / "audio.jsonl").write_text("not json\n", encoding="utf-8")

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"audio.jsonl": "malformed"}

    def test_non_object_header_blocks_as_malformed(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True, audio_extract=False)
        (seg / "audio.jsonl").write_text('"header"\n', encoding="utf-8")

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"audio.jsonl": "malformed"}

    def test_unreadable_extraction_blocks_as_malformed(self, tmp_path, monkeypatch):
        seg = _make_segment(tmp_path, audio=True, audio_extract=False)
        target = seg / "audio.jsonl"
        _write_audio_success(target)
        original_open = Path.open

        def blocked_open(path, *args, **kwargs):
            if path == target:
                raise OSError("blocked")
            return original_open(path, *args, **kwargs)

        monkeypatch.setattr(Path, "open", blocked_open)

        gate = resolve_segment_gate(seg)

        assert gate.verdict == "failed"
        assert gate.failed_files == {"audio.jsonl": "malformed"}

    def test_monitor_diff_rides_segment_gate(self, tmp_path):
        seg = _make_segment(tmp_path, audio=True)
        (seg / "monitor_1_diff.png").write_bytes(b"diff")

        assert resolve_segment_gate(seg).verdict == "eligible"


# ---------------------------------------------------------------------------
# RetentionPolicy
# ---------------------------------------------------------------------------


class TestRetentionPolicy:
    def test_keep_never_eligible(self):
        p = RetentionPolicy(mode="keep")
        assert not p.is_eligible(0)
        assert not p.is_eligible(365)

    def test_processed_always_eligible(self):
        p = RetentionPolicy(mode="processed")
        assert p.is_eligible(0)
        assert p.is_eligible(1)

    def test_days_threshold(self):
        p = RetentionPolicy(mode="days", days=30)
        assert not p.is_eligible(29)
        assert p.is_eligible(30)
        assert p.is_eligible(31)

    def test_days_no_value(self):
        p = RetentionPolicy(mode="days", days=None)
        assert not p.is_eligible(100)


class TestRetentionConfig:
    def test_default_policy(self):
        cfg = RetentionConfig()
        assert cfg.policy_for_stream("default").mode == "keep"
        assert cfg.policy_for_stream("default").days is None

    def test_per_stream_override(self):
        cfg = RetentionConfig(
            default=RetentionPolicy(mode="keep"),
            per_stream={
                "archon.plaud": RetentionPolicy(mode="days", days=7),
            },
        )
        assert cfg.policy_for_stream("archon.plaud").mode == "days"
        assert cfg.policy_for_stream("archon.plaud").days == 7
        assert cfg.policy_for_stream("default").mode == "keep"


# ---------------------------------------------------------------------------
# load_retention_config
# ---------------------------------------------------------------------------


class TestLoadRetentionConfig:
    def test_default_config(self, monkeypatch):
        monkeypatch.setattr("solstone.think.utils.get_config", lambda: {})
        cfg = load_retention_config()
        assert cfg.default.mode == "keep"
        assert cfg.default.days is None
        assert cfg.per_stream == {}

    def test_custom_config(self, monkeypatch):
        monkeypatch.setattr(
            "solstone.think.utils.get_config",
            lambda: {
                "retention": {
                    "raw_media": "days",
                    "raw_media_days": 30,
                    "per_stream": {
                        "default": {"raw_media": "processed"},
                    },
                }
            },
        )
        cfg = load_retention_config()
        assert cfg.default.mode == "days"
        assert cfg.default.days == 30
        assert cfg.per_stream["default"].mode == "processed"

    def test_existing_journal_days_config_unchanged(self, monkeypatch):
        monkeypatch.setattr(
            "solstone.think.utils.get_config",
            lambda: {
                "retention": {
                    "raw_media": "days",
                    "raw_media_days": 7,
                }
            },
        )
        cfg = load_retention_config()
        assert cfg.default.mode == "days"
        assert cfg.default.days == 7


# ---------------------------------------------------------------------------
# _human_bytes
# ---------------------------------------------------------------------------


class TestHumanBytes:
    def test_bytes(self):
        assert _human_bytes(0) == "0 B"
        assert _human_bytes(512) == "512 B"

    def test_kilobytes(self):
        assert _human_bytes(1024) == "1.0 KB"

    def test_megabytes(self):
        assert _human_bytes(1024 * 1024) == "1.0 MB"

    def test_gigabytes(self):
        assert _human_bytes(1024**3) == "1.0 GB"

    def test_large(self):
        result = _human_bytes(12_400_000_000)
        assert "GB" in result


class TestCheckStorageHealth:
    """Tests for check_storage_health threshold evaluation."""

    def _make_summary(self, raw_media_bytes=0, derived_bytes=0):
        return StorageSummary(
            raw_media_bytes=raw_media_bytes,
            derived_bytes=derived_bytes,
            total_segments=10,
            segments_with_raw=5,
            segments_purged=3,
        )

    def test_no_warnings_when_healthy(self, tmp_path, monkeypatch):
        """No warnings when disk is below threshold and raw media GB is null."""
        usage_type = type(shutil.disk_usage(tmp_path))
        monkeypatch.setattr(
            "shutil.disk_usage",
            lambda path: usage_type(1000, 500, 500),  # 50% used
        )
        config = {
            "retention": {
                "storage_warning_disk_percent": 80,
                "storage_warning_raw_media_gb": None,
            }
        }
        summary = self._make_summary()
        warnings = check_storage_health(summary, tmp_path, config=config)
        assert warnings == []

    def test_disk_percent_exceeded(self, tmp_path, monkeypatch):
        """Warning when disk usage exceeds threshold."""
        config = {
            "retention": {
                "storage_warning_disk_percent": 1,
            }
        }
        summary = self._make_summary()
        warnings = check_storage_health(summary, tmp_path, config=config)
        assert len(warnings) == 1
        assert warnings[0]["type"] == "disk_percent"
        assert warnings[0]["level"] == "warning"
        assert warnings[0]["current"] >= 1
        assert warnings[0]["threshold"] == 1
        assert "retention settings" in warnings[0]["message"]
        assert "Clean Up Now" in warnings[0]["message"]

    def test_disk_percent_not_exceeded(self, tmp_path, monkeypatch):
        """No warning when disk is well below threshold."""
        config = {
            "retention": {
                "storage_warning_disk_percent": 100,
            }
        }
        summary = self._make_summary()
        warnings = check_storage_health(summary, tmp_path, config=config)
        assert warnings == []

    def test_raw_media_gb_exceeded(self, tmp_path, monkeypatch):
        """Warning when raw media exceeds GB threshold."""
        raw_bytes = int(5.5 * 1024**3)
        config = {
            "retention": {
                "storage_warning_disk_percent": None,
                "storage_warning_raw_media_gb": 5.0,
            }
        }
        summary = self._make_summary(raw_media_bytes=raw_bytes)
        warnings = check_storage_health(summary, tmp_path, config=config)
        assert len(warnings) == 1
        assert warnings[0]["type"] == "raw_media_gb"
        assert warnings[0]["level"] == "warning"
        assert warnings[0]["current"] >= 5.0
        assert warnings[0]["threshold"] == 5.0
        assert "retention settings" in warnings[0]["message"]

    def test_raw_media_gb_not_exceeded(self, tmp_path, monkeypatch):
        """No warning when raw media is below threshold."""
        raw_bytes = int(2.0 * 1024**3)
        config = {
            "retention": {
                "storage_warning_disk_percent": None,
                "storage_warning_raw_media_gb": 5.0,
            }
        }
        summary = self._make_summary(raw_media_bytes=raw_bytes)
        warnings = check_storage_health(summary, tmp_path, config=config)
        assert warnings == []

    def test_both_thresholds_exceeded(self, tmp_path, monkeypatch):
        """Both warnings when both thresholds exceeded."""
        raw_bytes = int(10 * 1024**3)
        config = {
            "retention": {
                "storage_warning_disk_percent": 1,
                "storage_warning_raw_media_gb": 5.0,
            }
        }
        summary = self._make_summary(raw_media_bytes=raw_bytes)
        warnings = check_storage_health(summary, tmp_path, config=config)
        assert len(warnings) == 2
        types = {w["type"] for w in warnings}
        assert types == {"disk_percent", "raw_media_gb"}

    def test_null_thresholds_disables_checks(self, tmp_path, monkeypatch):
        """Both thresholds null means no warnings ever."""
        raw_bytes = int(100 * 1024**3)
        config = {
            "retention": {
                "storage_warning_disk_percent": None,
                "storage_warning_raw_media_gb": None,
            }
        }
        summary = self._make_summary(raw_media_bytes=raw_bytes)
        warnings = check_storage_health(summary, tmp_path, config=config)
        assert warnings == []

    def test_exact_threshold_triggers(self, tmp_path, monkeypatch):
        """Warning triggers at exactly the threshold (>=, not >)."""
        raw_bytes = int(5.0 * 1024**3)
        config = {
            "retention": {
                "storage_warning_disk_percent": None,
                "storage_warning_raw_media_gb": 5.0,
            }
        }
        summary = self._make_summary(raw_media_bytes=raw_bytes)
        warnings = check_storage_health(summary, tmp_path, config=config)
        assert len(warnings) == 1
        assert warnings[0]["type"] == "raw_media_gb"

    @pytest.mark.parametrize(
        ("backup", "expected_message"),
        [
            ({}, None),
            (
                {
                    "offload": {"enabled": False},
                    "last_offload": {
                        "status": "stalled",
                        "reason": "backup_failing",
                    },
                },
                None,
            ),
            (
                {
                    "offload": {"enabled": True},
                    "last_offload": {"status": "ok", "reason": None},
                },
                None,
            ),
            (
                {
                    "offload": {"enabled": True},
                    "last_offload": {
                        "status": "stalled",
                        "reason": "backup_failing",
                    },
                },
                f"{OFFLOAD_STALLED_LEAD} {OFFLOAD_STALL_REASON_LABELS['backup_failing']}",
            ),
            (
                {
                    "offload": {"enabled": True},
                    "last_offload": {"status": "stalled", "reason": None},
                },
                OFFLOAD_STALLED_LEAD,
            ),
            (
                {
                    "offload": {"enabled": True},
                    "last_offload": {
                        "status": "stalled",
                        "reason": "unknown_reason",
                    },
                },
                OFFLOAD_STALLED_LEAD,
            ),
        ],
    )
    def test_offload_stalled_warning_from_backup_config(
        self,
        tmp_path,
        backup,
        expected_message,
    ):
        config = {
            "retention": {
                "storage_warning_disk_percent": None,
                "storage_warning_raw_media_gb": None,
            },
            "backup": backup,
        }
        summary = self._make_summary()

        warnings = check_storage_health(summary, tmp_path, config=config)

        if expected_message is None:
            assert warnings == []
            return
        assert warnings == [
            {
                "level": "warning",
                "type": "offload_stalled",
                "message": expected_message,
                "current": None,
                "threshold": None,
            }
        ]

    def test_missing_retention_section_uses_defaults(self, tmp_path, monkeypatch):
        """Missing retention section falls back to defaults (80% disk, null raw media)."""
        config = {}
        summary = self._make_summary()
        warnings = check_storage_health(summary, tmp_path, config=config)
        for w in warnings:
            assert w["type"] != "raw_media_gb"

    def test_warning_dict_structure(self, tmp_path, monkeypatch):
        """Each warning has all required keys."""
        config = {
            "retention": {
                "storage_warning_disk_percent": 1,
                "storage_warning_raw_media_gb": 0.001,
            }
        }
        raw_bytes = int(1 * 1024**3)
        summary = self._make_summary(raw_media_bytes=raw_bytes)
        warnings = check_storage_health(summary, tmp_path, config=config)
        for w in warnings:
            assert "level" in w
            assert "type" in w
            assert "message" in w
            assert "current" in w
            assert "threshold" in w


class TestStorageHealthNudge:
    def _make_summary(self):
        return StorageSummary(
            raw_media_bytes=int(10 * 1024**3),
            derived_bytes=0,
            total_segments=10,
            segments_with_raw=5,
            segments_purged=3,
        )

    def _config(self, mode: str) -> dict:
        return {
            "retention": {
                "raw_media": mode,
                "storage_warning_disk_percent": 1,
                "storage_warning_raw_media_gb": 5.0,
            }
        }

    def _force_disk_warning(self, tmp_path, monkeypatch) -> None:
        usage_type = type(shutil.disk_usage(tmp_path))
        monkeypatch.setattr(
            "shutil.disk_usage",
            lambda path: usage_type(1000, 950, 50),
        )

    def test_keep_mode_appends_nudge(self, tmp_path, monkeypatch):
        self._force_disk_warning(tmp_path, monkeypatch)
        warnings = check_storage_health(
            self._make_summary(),
            tmp_path,
            config=self._config("keep"),
        )
        assert {warning["type"] for warning in warnings} == {
            "disk_percent",
            "raw_media_gb",
        }
        assert all(
            "always retain original media" in warning["message"] for warning in warnings
        )

    def test_days_mode_does_not_append_nudge(self, tmp_path, monkeypatch):
        self._force_disk_warning(tmp_path, monkeypatch)
        warnings = check_storage_health(
            self._make_summary(),
            tmp_path,
            config=self._config("days"),
        )
        assert all(
            "always retain original media" not in warning["message"]
            for warning in warnings
        )

    def test_processed_mode_does_not_append_nudge(self, tmp_path, monkeypatch):
        self._force_disk_warning(tmp_path, monkeypatch)
        warnings = check_storage_health(
            self._make_summary(),
            tmp_path,
            config=self._config("processed"),
        )
        assert all(
            "always retain original media" not in warning["message"]
            for warning in warnings
        )


class TestRetentionDerivationRule:
    @staticmethod
    def derive_retention(days_value: str, dont_retain: bool) -> tuple[str, int | None]:
        # Mirrors the JS deriveRetention helper in convey/templates/init.html
        # and apps/settings/workspace.html.
        if dont_retain:
            return ("processed", None)
        try:
            days = int(days_value)
        except (TypeError, ValueError):
            days = None
        if days is not None and days >= 1:
            return ("days", days)
        return ("keep", None)

    def test_empty_days_defaults_to_keep(self):
        assert self.derive_retention("", False) == ("keep", None)

    def test_numeric_days_uses_days_mode(self):
        assert self.derive_retention("30", False) == ("days", 30)

    def test_checkbox_wins_over_numeric_days(self):
        assert self.derive_retention("30", True) == ("processed", None)

    def test_checkbox_wins_when_days_empty(self):
        assert self.derive_retention("", True) == ("processed", None)
