# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for observe.sense module."""

import json
import logging
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import Future
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock, patch

import pytest

from solstone.observe.exit_codes import EXIT_PROVIDER_BLOCKED, WATCHDOG_TIMEOUT
from solstone.observe.processing_record import (
    FAILED_ATTEMPT_BOUND,
    HANDLER_DESCRIBE,
    HANDLER_TRANSCRIBE,
    MAX_FIRST_ROW_BYTES,
    REASON_ANALYSIS_FAILED,
    REASON_CORRUPT_INPUT,
    REASON_OK,
    SCREEN_ANALYSIS_ROW_KEY,
    STATE_ANALYZED,
    STATE_EMPTY,
    STATE_FAILED,
    build_processing_record,
    read_processing_record_header,
)
from solstone.observe.sense import FileSensor, HandlerProcess, QueuedItem, _handler_icon
from solstone.think import admission
from solstone.think.processing import (
    DisplayPowersaveSettings,
    GateSettings,
    ProcessingSettings,
    TimeWindowSettings,
)
from solstone.think.providers import fanout_policy
from solstone.think.retention import resolve_segment_gate
from solstone.think.runner import DailyLogWriter as ProcessLogWriter
from solstone.think.runner import _format_log_line


@pytest.fixture(autouse=True)
def _default_thinking_engine_selected(monkeypatch):
    monkeypatch.setattr(
        "solstone.think.models.no_thinking_engine_chosen", lambda: False
    )


@pytest.fixture(autouse=True)
def _speakers_analyze_generation_ready(monkeypatch):
    class Generation:
        generation_id = "test-generation"

        def release(self) -> None:
            pass

    monkeypatch.setattr(
        "solstone.think.speakers_analyze_installation."
        "enter_speakers_analyze_generation",
        lambda **_kwargs: Generation(),
    )


class _ClockShim:
    def __init__(
        self,
        real_time: Any,
        *,
        time: Any = None,
        monotonic: Any = None,
    ) -> None:
        self._real_time = real_time
        if time is not None:
            self.time = time
        if monotonic is not None:
            self.monotonic = monotonic

    def __getattr__(self, name: str) -> Any:
        return getattr(self._real_time, name)


class FakeProcess:
    def __init__(self, exit_code=0, delay=0.0):
        self.exit_code = exit_code
        self.delay = delay
        self.returncode = None
        self.pid = id(self) % 100000
        self.stdout = None
        self.stderr = None
        self.terminated = False
        self.killed = False

    def wait(self, timeout=None):
        if self.delay:
            time.sleep(self.delay)
        if self.returncode is None:
            self.returncode = self.exit_code
        return self.returncode

    def terminate(self):
        self.terminated = True
        self.returncode = -signal.SIGTERM

    def kill(self):
        self.killed = True
        self.returncode = -signal.SIGKILL


class TimeoutProcess(FakeProcess):
    """wait(timeout=...) raises TimeoutExpired until terminate/kill sets returncode."""

    def wait(self, timeout=None):
        if self.returncode is not None:
            return self.returncode
        if timeout is None:
            raise AssertionError("watchdog must never call unbounded wait()")
        raise subprocess.TimeoutExpired(cmd="fake", timeout=timeout)


class BlockingSignalProcess(FakeProcess):
    """wait() blocks until terminate() supplies a signal return code."""

    def __init__(self, exit_code=-signal.SIGTERM):
        super().__init__(exit_code)
        self.wait_entered = threading.Event()
        self.released = threading.Event()

    def wait(self, timeout=None):
        self.wait_entered.set()
        if not self.released.wait(timeout=2):
            raise AssertionError("test process was not released")
        if self.returncode is None:
            self.returncode = self.exit_code
        return self.returncode

    def terminate(self):
        self.terminated = True
        self.returncode = -signal.SIGTERM
        self.released.set()


class FakeManaged:
    def __init__(self, process=None, ref="testref", log_path=None):
        self.process = process or FakeProcess()
        self.ref = ref
        self.log_writer = MagicMock()
        self.log_writer.path = log_path or Path("/tmp/fake.log")
        self.cleanup = MagicMock()


def make_segment_file(
    tmp_path,
    filename="screen.webm",
    day="20250101",
    stream="default",
    segment="143022_300",
):
    segment_dir = tmp_path / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True, exist_ok=True)
    file_path = segment_dir / filename
    file_path.write_text("content")
    return file_path


def _processing_record(
    *,
    state: str,
    handler: str = HANDLER_DESCRIBE,
    reason_code: str = REASON_ANALYSIS_FAILED,
    attempts: int | None = None,
) -> dict:
    record = {
        "schema": "solstone.processing.v1",
        "state": state,
        "reason_code": reason_code,
        "handler": handler,
        "attempted_at": "2026-07-16T00:00:00Z",
        "input_size": 7,
    }
    if attempts is not None:
        record["attempts"] = attempts
    return record


def _write_processing_output(
    media_path: Path,
    record: dict | None,
    rows: list[dict] | None = None,
) -> Path:
    header = {"raw": media_path.name}
    if record is not None:
        header["_solstone_processing"] = record
    output_path = media_path.with_suffix(".jsonl")
    lines = [json.dumps(header)]
    if rows is not None:
        lines.extend(json.dumps(row) for row in rows)
    output_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return output_path


def _processing_settings(mode: str) -> ProcessingSettings:
    return ProcessingSettings(
        mode=mode,
        gate=GateSettings(
            time_window=TimeWindowSettings(
                enabled=True,
                start="02:00",
                end="06:00",
            ),
            display_powersave=DisplayPowersaveSettings(enabled=False),
        ),
    )


BEACON_FIELDS = {
    "name",
    "stream_type",
    "version",
    "uptime",
    "last_successful_sync",
    "pending_queue_depth",
    "recent_error_count",
    "last_error_reason",
    "memory_throttled",
    "memory_throttle_count",
    "memory_floor_mib",
    "memory_available_mib",
}


def test_handler_icon_returns_notification_lucide_names():
    assert _handler_icon("transcribe") == "mic-vocal"
    assert _handler_icon("describe") == "eye"
    assert _handler_icon("depict") == "bot"


def _status_emit_calls(sensor):
    return [
        call
        for call in sensor.callosum.emit.call_args_list
        if call.args[:2] == ("observe", "status")
    ]


# --- QueuedItem Tests ---


def test_queued_item_basic():
    """Test QueuedItem stores file_path and queued_at."""
    path = Path("/tmp/test.flac")
    item = QueuedItem(path)

    assert item.file_path == path
    assert item.queued_at > 0
    assert item.observer is None


def test_queued_item_with_observer():
    """Test QueuedItem stores observer context."""
    path = Path("/tmp/test.flac")
    item = QueuedItem(path, observer="my-observer")

    assert item.file_path == path
    assert item.observer == "my-observer"


def test_sense_installs_sigterm_handler():
    from solstone.observe import sense

    previous = signal.getsignal(signal.SIGTERM)
    signal.signal(signal.SIGTERM, signal.SIG_DFL)
    try:
        sense._install_sigterm_handler(MagicMock())
        assert signal.getsignal(signal.SIGTERM) is not signal.SIG_DFL
    finally:
        signal.signal(signal.SIGTERM, previous)


def test_batch_sigint_handler_stops_sensor_and_raises_keyboard_interrupt():
    from solstone.observe import sense

    previous_int = signal.getsignal(signal.SIGINT)
    previous_term = signal.getsignal(signal.SIGTERM)
    sensor = MagicMock()
    try:
        sense._install_batch_signal_handlers(sensor)
        handler = signal.getsignal(signal.SIGINT)
        assert callable(handler)
        with pytest.raises(KeyboardInterrupt):
            handler(signal.SIGINT, None)
        sensor.stop.assert_called_once()
    finally:
        signal.signal(signal.SIGINT, previous_int)
        signal.signal(signal.SIGTERM, previous_term)


def test_batch_sigterm_handler_stops_sensor_and_exits():
    from solstone.observe import sense

    previous_int = signal.getsignal(signal.SIGINT)
    previous_term = signal.getsignal(signal.SIGTERM)
    sensor = MagicMock()
    try:
        sense._install_batch_signal_handlers(sensor)
        handler = signal.getsignal(signal.SIGTERM)
        assert callable(handler)
        with pytest.raises(SystemExit) as exc_info:
            handler(signal.SIGTERM, None)
        assert exc_info.value.code == 143
        sensor.stop.assert_called_once()
    finally:
        signal.signal(signal.SIGINT, previous_int)
        signal.signal(signal.SIGTERM, previous_term)


def test_resolve_concurrency_applies_to_handler_pools(tmp_path, monkeypatch, caplog):
    """Test handler concurrency config is applied uniformly to pools."""
    import solstone.observe.sense as sense_module

    monkeypatch.setattr(
        sense_module,
        "get_config",
        lambda: {
            "describe": {"max_concurrent": 4},
            "transcribe": {"max_concurrent": 2},
        },
    )

    sensor = FileSensor(tmp_path)

    assert sensor.handler_pools["describe"]._max_workers == 4
    assert sensor.handler_pools["transcribe"]._max_workers == 2

    monkeypatch.setattr(
        sense_module,
        "get_config",
        lambda: {
            "describe": {"max_concurrent": "bad"},
            "transcribe": {"max_concurrent": -1},
        },
    )

    caplog.clear()
    invalid_sensor = FileSensor(tmp_path)

    assert invalid_sensor.handler_pools["describe"]._max_workers == 1
    assert invalid_sensor.handler_pools["transcribe"]._max_workers == 1
    assert "Invalid describe.max_concurrent" in caplog.text
    assert "Invalid transcribe.max_concurrent" in caplog.text


def test_resolve_max_runtime(tmp_path, monkeypatch, caplog):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    sensor = FileSensor(tmp_path)
    assert sensor._resolve_max_runtime("describe") == 1800

    config_dir = tmp_path / "config"
    config_dir.mkdir()
    config_path = config_dir / "journal.json"
    config_path.write_text(
        json.dumps(
            {
                "describe": {"max_runtime": "30m"},
                "transcribe": {"max_runtime": 60},
            }
        )
    )

    assert sensor._resolve_max_runtime("describe") == 1800
    assert sensor._resolve_max_runtime("transcribe") == 60

    caplog.set_level("WARNING")
    config_path.write_text(json.dumps({"describe": {"max_runtime": "banana"}}))

    assert sensor._resolve_max_runtime("describe") == 1800
    assert "Invalid describe.max_runtime" in caplog.text


# --- Existing Tests ---


def test_format_log_line():
    """Test log line formatting."""
    line = _format_log_line("transcribe:test.flac", "stdout", "Processing...\n")
    assert "[transcribe:test.flac:stdout]" in line
    assert "Processing..." in line
    assert line.endswith("\n")


def test_process_log_writer(tmp_path, monkeypatch):
    """Test ProcessLogWriter creates and writes to log file."""
    from solstone.think import runner

    # Mock journal path and current day to use tmp_path
    monkeypatch.setattr(runner, "_get_journal_path", lambda: tmp_path)
    monkeypatch.setattr(runner, "_current_day", lambda: "20241101")

    ref = "1730476800000"
    writer = ProcessLogWriter(ref, "test")

    writer.write("line 1\n")
    writer.write("line 2\n")
    writer.close()

    # Log file uses {ref}_{name}.log format
    log_path = tmp_path / "chronicle" / "20241101" / "health" / f"{ref}_test.log"
    assert log_path.exists()
    content = log_path.read_text()
    assert "line 1\n" in content
    assert "line 2\n" in content

    # Verify symlinks exist
    day_symlink = tmp_path / "chronicle" / "20241101" / "health" / "test.log"
    assert day_symlink.is_symlink()
    journal_symlink = tmp_path / "health" / "test.log"
    assert journal_symlink.is_symlink()


def test_process_log_writer_thread_safe(tmp_path, monkeypatch):
    """Test ProcessLogWriter is thread-safe."""
    from solstone.think import runner

    # Mock journal path and current day to use tmp_path
    monkeypatch.setattr(runner, "_get_journal_path", lambda: tmp_path)
    monkeypatch.setattr(runner, "_current_day", lambda: "20241101")

    ref = "1730476800000"
    writer = ProcessLogWriter(ref, "test")

    def write_lines(prefix):
        for i in range(10):
            writer.write(f"{prefix}-{i}\n")

    threads = [
        threading.Thread(target=write_lines, args=(f"thread{i}",)) for i in range(5)
    ]

    for t in threads:
        t.start()
    for t in threads:
        t.join()

    writer.close()

    # Log file uses {ref}_{name}.log format
    log_path = tmp_path / "chronicle" / "20241101" / "health" / f"{ref}_test.log"
    lines = log_path.read_text().split("\n")
    # Should have 50 lines (5 threads * 10 lines each)
    assert len([line for line in lines if line]) == 50


def test_process_log_writer_pins_journal_root_at_init(tmp_path, monkeypatch):
    """Env-var drift between construction and flush must not redirect writes."""
    from solstone.think import runner

    journal_a = tmp_path / "a"
    journal_b = tmp_path / "b"
    journal_a.mkdir()
    journal_b.mkdir()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal_a))
    monkeypatch.setattr(runner, "_current_day", lambda: "20241101")

    ref = "test_ref"
    writer = ProcessLogWriter(ref, "echo")

    # Drift: env var changes and day changes before the next flush.
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal_b))
    monkeypatch.setattr(runner, "_current_day", lambda: "20241102")

    writer.write("hello\n")
    writer.close()

    leaked_paths = list(journal_b.rglob("*"))
    assert not leaked_paths, f"writes leaked into drifted journal: {leaked_paths}"
    assert list(journal_a.rglob("*.log")) or list(journal_a.rglob("*echo*"))


def test_process_log_writer_rollover_open_fails_once(tmp_path, monkeypatch):
    from solstone.think import runner

    monkeypatch.setattr(runner, "_get_journal_path", lambda: tmp_path)
    monkeypatch.setattr(runner, "_current_day", lambda: "20241101")

    ref = "1730476800000"
    writer = ProcessLogWriter(ref, "test")
    writer.write("baseline\n")

    real_open = writer._open_log
    calls = {"n": 0}

    def flaky_open(day=None):
        calls["n"] += 1
        if calls["n"] == 1:
            raise OSError("disk full")
        return real_open(day)

    writer._open_log = flaky_open
    monkeypatch.setattr(runner, "_current_day", lambda: "20241102")

    writer.write("during-failure\n")
    writer.write("after-recovery\n")
    writer.close()

    previous_log = tmp_path / "chronicle" / "20241101" / "health" / f"{ref}_test.log"
    current_log = tmp_path / "chronicle" / "20241102" / "health" / f"{ref}_test.log"
    previous_content = previous_log.read_text()
    current_content = current_log.read_text()

    assert "during-failure\n" in previous_content
    assert "after-recovery\n" not in previous_content
    assert "after-recovery\n" in current_content


def test_process_log_writer_rollover_preserves_handle_and_day(tmp_path, monkeypatch):
    from solstone.think import runner

    monkeypatch.setattr(runner, "_get_journal_path", lambda: tmp_path)
    monkeypatch.setattr(runner, "_current_day", lambda: "20241101")

    ref = "1730476800000"
    writer = ProcessLogWriter(ref, "test")

    def flaky_open(day=None):
        raise OSError("disk full")

    writer._open_log = flaky_open
    monkeypatch.setattr(runner, "_current_day", lambda: "20241102")

    writer.write("x\n")

    assert writer._fh.closed is False
    assert writer._current_day == "20241101"

    writer.write("still-writable\n")
    writer.close()

    previous_log = tmp_path / "chronicle" / "20241101" / "health" / f"{ref}_test.log"
    assert "still-writable\n" in previous_log.read_text()


def test_process_log_writer_post_swap_best_effort(tmp_path, monkeypatch):
    from solstone.think import runner

    monkeypatch.setattr(runner, "_get_journal_path", lambda: tmp_path)
    monkeypatch.setattr(runner, "_current_day", lambda: "20241101")

    ref = "1730476800000"
    writer = ProcessLogWriter(ref, "test")
    writer._fh.close()
    failing_fh = MagicMock()
    failing_fh.write.side_effect = OSError("disk full")
    failing_fh.closed = False
    writer._fh = failing_fh

    writer.write("x\n")

    failing_fh.write.assert_called_once_with("x\n")
    writer.close()

    writer = ProcessLogWriter(ref, "test")

    def fail_symlinks():
        raise OSError("symlink failed")

    writer._update_symlinks = fail_symlinks
    monkeypatch.setattr(runner, "_current_day", lambda: "20241102")

    writer.write("rolled\n")
    writer.close()

    current_log = tmp_path / "chronicle" / "20241102" / "health" / f"{ref}_test.log"
    assert writer._current_day == "20241102"
    assert "rolled\n" in current_log.read_text()


def test_handler_process_cleanup():
    """Test HandlerProcess cleanup joins threads and closes logger."""
    mock_managed = MagicMock()
    mock_managed.name = "transcribe"
    mock_managed.process = MagicMock()

    handler = HandlerProcess(Path("/tmp/test.flac"), mock_managed, "transcribe")

    handler.cleanup()

    mock_managed.cleanup.assert_called_once()


def test_file_sensor_register():
    """Test registering handlers."""
    with tempfile.TemporaryDirectory() as tmpdir:
        sensor = FileSensor(Path(tmpdir))

        sensor.register("*.webm", "describe", ["echo", "{file}"])
        sensor.register("*.flac", "transcribe", ["cat", "{file}"])
        sensor.register("*.png", "depict", ["journal", "depict", "{file}"])

        assert "*.webm" in sensor.handlers
        assert "*.flac" in sensor.handlers
        assert "*.png" in sensor.handlers
        assert sensor.handlers["*.webm"][0] == "describe"
        assert sensor.handlers["*.flac"][0] == "transcribe"
        assert sensor.handlers["*.png"][0] == "depict"


def test_file_sensor_match_pattern():
    """Test pattern matching logic.

    Files are expected to be in segment directories: journal/YYYYMMDD/stream/HHMMSS_LEN/file.ext
    """
    with tempfile.TemporaryDirectory() as tmpdir:
        from solstone.observe.utils import (
            AUDIO_EXTENSIONS,
            IMAGE_EXTENSIONS,
            VIDEO_EXTENSIONS,
        )

        # Create journal/day/stream/segment structure
        journal_dir = Path(tmpdir)
        day_dir = journal_dir / "chronicle" / "20250101"
        segment_dir = day_dir / "default" / "123456_300"
        segment_dir.mkdir(parents=True)

        sensor = FileSensor(journal_dir)
        for ext in AUDIO_EXTENSIONS:
            sensor.register(f"*{ext}", "transcribe", ["cat", "{file}"])
        for ext in VIDEO_EXTENSIONS:
            sensor.register(f"*{ext}", "describe", ["echo", "{file}"])
        for ext in IMAGE_EXTENSIONS:
            sensor.register(f"*{ext}", "depict", ["journal", "depict", "{file}"])

        # Should match - files in segment directory
        webm_file = segment_dir / "center_DP-3_screen.webm"
        assert sensor._match_pattern(webm_file) is not None
        assert sensor._match_pattern(webm_file)[0] == "describe"

        flac_file = segment_dir / "audio.flac"
        assert sensor._match_pattern(flac_file) is not None
        assert sensor._match_pattern(flac_file)[0] == "transcribe"

        mp3_file = segment_dir / "imported_audio.mp3"
        assert sensor._match_pattern(mp3_file) is not None
        assert sensor._match_pattern(mp3_file)[0] == "transcribe"

        for filename in (
            "image.png",
            "photo.jpg",
            "camera.heic",
            "snapshot.webp",
            "scan.tiff",
        ):
            image_file = segment_dir / filename
            assert sensor._match_pattern(image_file) is not None
            assert sensor._match_pattern(image_file)[0] == "depict"

        # Should not match - wrong extension
        txt_file = segment_dir / "test.txt"
        assert sensor._match_pattern(txt_file) is None

        # Should not match - file in day root (not in segment dir)
        day_root_file = day_dir / "orphan.webm"
        assert sensor._match_pattern(day_root_file) is None

        # Should not match - jsonl output file
        jsonl_file = segment_dir / "audio.jsonl"
        assert sensor._match_pattern(jsonl_file) is None


def test_scan_unprocessed_skips_depict_for_import_stream_images(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    make_segment_file(tmp_path, filename="original.png", stream="import.image")
    sensor = FileSensor(tmp_path)
    sensor.register("*.png", "depict", ["journal", "depict", "{file}"])

    to_process, _ = sensor.scan_unprocessed("20250101")

    assert to_process == []


def test_scan_unprocessed_depicts_non_import_stream_images(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    image = make_segment_file(tmp_path, filename="original.png", stream="camera")
    sensor = FileSensor(tmp_path)
    sensor.register("*.png", "depict", ["journal", "depict", "{file}"])

    to_process, _ = sensor.scan_unprocessed("20250101")

    assert to_process == [(image, "depict", ["journal", "depict", "{file}"])]


def test_scan_unprocessed_import_audio_still_transcribes(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    audio = make_segment_file(
        tmp_path,
        filename="imported_audio.mp3",
        stream="import.audio",
    )
    sensor = FileSensor(tmp_path)
    sensor.register("*.mp3", "transcribe", ["journal", "transcribe", "{file}"])

    to_process, _ = sensor.scan_unprocessed("20250101")

    assert to_process == [(audio, "transcribe", ["journal", "transcribe", "{file}"])]


def test_standalone_dry_run(tmp_path, monkeypatch):
    """Test scan_unprocessed finds only unprocessed media files."""
    from solstone.observe.utils import AUDIO_EXTENSIONS, VIDEO_EXTENSIONS

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    day_dir = tmp_path / "chronicle" / "20250101"
    segment_dir = day_dir / "default" / "143022_300"
    segment_dir.mkdir(parents=True)

    (segment_dir / "audio.flac").write_text("audio")
    (segment_dir / "screen.webm").write_text("video")
    (segment_dir / "other.flac").write_text("audio2")
    (segment_dir / "other.jsonl").write_text('{"raw": "test"}')

    sensor = FileSensor(journal_dir=tmp_path)

    for ext in AUDIO_EXTENSIONS:
        sensor.register(f"*{ext}", "transcribe", ["journal", "transcribe", "{file}"])
    for ext in VIDEO_EXTENSIONS:
        sensor.register(f"*{ext}", "describe", ["journal", "describe", "{file}"])

    to_process, _ = sensor.scan_unprocessed("20250101")

    assert len(to_process) == 2
    file_names = {file_path.name for file_path, _, _ in to_process}
    assert file_names == {"audio.flac", "screen.webm"}
    assert "other.flac" not in file_names


def test_standalone_dry_run_with_segment_filter(tmp_path, monkeypatch):
    """Test scan_unprocessed honors segment filters."""
    from solstone.observe.utils import AUDIO_EXTENSIONS, VIDEO_EXTENSIONS

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    day_dir = tmp_path / "chronicle" / "20250101"
    segment_1 = day_dir / "default" / "143022_300"
    segment_2 = day_dir / "default" / "150022_300"
    segment_1.mkdir(parents=True)
    segment_2.mkdir(parents=True)

    (segment_1 / "audio.flac").write_text("audio")
    (segment_2 / "screen.webm").write_text("video")

    sensor = FileSensor(journal_dir=tmp_path)

    for ext in AUDIO_EXTENSIONS:
        sensor.register(f"*{ext}", "transcribe", ["journal", "transcribe", "{file}"])
    for ext in VIDEO_EXTENSIONS:
        sensor.register(f"*{ext}", "describe", ["journal", "describe", "{file}"])

    to_process, _ = sensor.scan_unprocessed("20250101", segment_filter="143022_300")

    assert len(to_process) == 1
    file_names = {file_path.name for file_path, _, _ in to_process}
    assert file_names == {"audio.flac"}


def test_scan_unprocessed_filters_stream_and_modality(tmp_path, monkeypatch):
    """Test scan_unprocessed honors stream and modality filters together."""
    from solstone.observe.utils import AUDIO_EXTENSIONS, VIDEO_EXTENSIONS

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    day_dir = tmp_path / "chronicle" / "20250101"
    alpha = day_dir / "alpha" / "143022_300"
    bravo = day_dir / "bravo" / "143022_300"
    alpha.mkdir(parents=True)
    bravo.mkdir(parents=True)

    (alpha / "audio.flac").write_text("audio")
    (alpha / "screen.webm").write_text("video")
    (bravo / "screen.webm").write_text("video")

    sensor = FileSensor(journal_dir=tmp_path)
    for ext in AUDIO_EXTENSIONS:
        sensor.register(f"*{ext}", "transcribe", ["journal", "transcribe", "{file}"])
    for ext in VIDEO_EXTENSIONS:
        sensor.register(f"*{ext}", "describe", ["journal", "describe", "{file}"])

    to_process, _ = sensor.scan_unprocessed(
        "20250101",
        stream_filter="alpha",
        modality_filter="screen",
    )

    assert [(path.parent.parent.name, path.name) for path, _, _ in to_process] == [
        ("alpha", "screen.webm")
    ]


def test_process_day_filters_stream_and_modality(tmp_path, monkeypatch):
    """Test process_day only dispatches matching stream/modality files."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    make_segment_file(
        tmp_path,
        filename="audio.flac",
        day="20250101",
        stream="alpha",
        segment="143022_300",
    )
    make_segment_file(
        tmp_path,
        filename="screen.webm",
        day="20250101",
        stream="alpha",
        segment="143022_300",
    )
    make_segment_file(
        tmp_path,
        filename="audio.flac",
        day="20250101",
        stream="bravo",
        segment="143022_300",
    )

    sensor = FileSensor(tmp_path)
    sensor.register("*.flac", "transcribe", ["journal", "transcribe", "{file}"])
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    processed = []

    def fake_run(queued_item, *_args):
        processed.append(
            (queued_item.file_path.parent.parent.name, queued_item.file_path.name)
        )

    monkeypatch.setattr(sensor, "_run_handler", fake_run)

    sensor.process_day(
        "20250101",
        max_jobs=1,
        stream_filter="alpha",
        modality_filter="audio",
    )

    assert processed == [("alpha", "audio.flac")]


def test_process_day_reenters_only_retryable_describe_failures(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    cases = [
        (
            "143000_300",
            _processing_record(state=STATE_ANALYZED, reason_code="ok"),
            None,
            False,
        ),
        (
            "143001_300",
            _processing_record(
                state=STATE_EMPTY,
                reason_code="no_decodable_frames",
            ),
            None,
            False,
        ),
        ("143002_300", None, None, True),
        (
            "143008_300",
            None,
            [{"frame_id": 1, SCREEN_ANALYSIS_ROW_KEY: 0.0, "analysis": {}}],
            False,
        ),
        (
            "143003_300",
            _processing_record(
                state=STATE_FAILED,
                reason_code=REASON_CORRUPT_INPUT,
            ),
            None,
            False,
        ),
        (
            "143004_300",
            _processing_record(
                state=STATE_FAILED,
                attempts=FAILED_ATTEMPT_BOUND,
            ),
            None,
            False,
        ),
        (
            "143005_300",
            _processing_record(
                state=STATE_FAILED,
                handler=HANDLER_TRANSCRIBE,
                attempts=1,
            ),
            None,
            False,
        ),
        (
            "143006_300",
            _processing_record(state=STATE_FAILED),
            None,
            True,
        ),
        (
            "143007_300",
            _processing_record(
                state=STATE_FAILED,
                attempts=FAILED_ATTEMPT_BOUND - 1,
            ),
            None,
            True,
        ),
    ]
    for segment, record, rows, _expected in cases:
        media_path = make_segment_file(tmp_path, segment=segment)
        _write_processing_output(media_path, record, rows=rows)

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    processed = []

    def fake_run(queued_item, *_args):
        processed.append(queued_item.file_path.parent.name)

    monkeypatch.setattr(sensor, "_run_handler", fake_run)

    sensor.process_day("20250101", max_jobs=1)

    assert processed == [
        segment for segment, _record, _rows, expected in cases if expected
    ]


def test_process_day_reentry_uses_bounded_first_window(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    media_path = make_segment_file(tmp_path, segment="143022_300")
    record = _processing_record(state=STATE_FAILED)
    output_path = media_path.with_suffix(".jsonl")
    output_path.write_bytes(
        b'{"pad":"'
        + (b"x" * MAX_FIRST_ROW_BYTES)
        + b'","_solstone_processing":'
        + json.dumps(record).encode("utf-8")
        + b"}\n"
    )
    # A record past the bounded first-row window is indeterminate: it re-enters
    # unless analyzed rows provide evidence that the output is already useful.
    row_media_path = make_segment_file(tmp_path, segment="143023_300")
    row_output_path = row_media_path.with_suffix(".jsonl")
    row_output_path.write_bytes(
        b'{"pad":"'
        + (b"x" * MAX_FIRST_ROW_BYTES)
        + b'","_solstone_processing":'
        + json.dumps(record).encode("utf-8")
        + b"}\n"
        + json.dumps(
            {"frame_id": 1, SCREEN_ANALYSIS_ROW_KEY: 0.0, "analysis": {}}
        ).encode("utf-8")
        + b"\n"
    )

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    processed = []

    def fake_run(queued_item, *_args):
        processed.append(queued_item.file_path)

    monkeypatch.setattr(sensor, "_run_handler", fake_run)

    sensor.process_day("20250101", max_jobs=1)

    assert processed == [media_path]


def test_ac5_recordless_audio_output_does_not_reenter(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    audio_path = make_segment_file(tmp_path, filename="audio.flac")
    _write_processing_output(audio_path, None)

    sensor = FileSensor(tmp_path)
    command = ["journal", "transcribe", "{file}"]
    sensor.register("*.flac", "transcribe", command)

    to_process, _ = sensor.scan_unprocessed("20250101")

    assert to_process == []


def test_ac7_missing_screen_output_still_queues_raw_video(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    media_path = make_segment_file(tmp_path)

    sensor = FileSensor(tmp_path)
    command = ["journal", "describe", "{file}"]
    sensor.register("*.webm", "describe", command)

    to_process, _ = sensor.scan_unprocessed("20250101")

    assert to_process == [(media_path, "describe", command)]


def test_process_day_retries_failed_describe_until_attempt_bound(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    media_path = make_segment_file(tmp_path)
    output_path = _write_processing_output(
        media_path,
        _processing_record(state=STATE_FAILED),
    )
    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    processed = []

    def fake_run(queued_item, *_args):
        processed.append(queued_item.file_path)
        record = read_processing_record_header(output_path)
        previous_attempts = record.get("attempts", 0) if record else 0
        _write_processing_output(
            media_path,
            _processing_record(
                state=STATE_FAILED,
                attempts=previous_attempts + 1,
            ),
        )

    monkeypatch.setattr(sensor, "_run_handler", fake_run)

    for _ in range(4):
        sensor.process_day("20250101", max_jobs=1)

    assert processed == [media_path, media_path, media_path]
    assert (
        read_processing_record_header(output_path)["attempts"] == FAILED_ATTEMPT_BOUND
    )


def test_failed_describe_heals_on_daily_pass_and_processed_prune_releases_raw(
    tmp_path, monkeypatch
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    media_path = make_segment_file(tmp_path)
    output_path = _write_processing_output(
        media_path,
        _processing_record(state=STATE_FAILED),
    )
    (media_path.parent / "stream.json").write_text(
        '{"stream":"default"}\n',
        encoding="utf-8",
    )
    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    dispatched = []
    observed_previous_attempts = []
    observed_incremental_paths = []

    def fake_run(queued_item, *_args):
        dispatched.append(queued_item.file_path)
        record = read_processing_record_header(output_path)
        observed_previous_attempts.append(record.get("attempts", 0) if record else 0)
        observed_incremental_paths.append(output_path)
        header = {
            "raw": queued_item.file_path.name,
            "_solstone_processing": build_processing_record(
                state=STATE_ANALYZED,
                reason_code=REASON_OK,
                handler=HANDLER_DESCRIBE,
                input_size=queued_item.file_path.stat().st_size,
            ),
        }
        output_path.write_text(
            json.dumps(header)
            + "\n"
            + json.dumps({"frame_id": 1, "timestamp": 0.0, "content": {}})
            + "\n",
            encoding="utf-8",
        )

    monkeypatch.setattr(sensor, "_run_handler", fake_run)

    sensor.process_day("20250101", max_jobs=1)

    record = read_processing_record_header(output_path)
    assert dispatched == [media_path]
    assert observed_previous_attempts == [0]
    assert observed_incremental_paths == [output_path]
    assert record["state"] == STATE_ANALYZED
    assert record["reason_code"] == REASON_OK
    assert "attempts" not in record

    gate = resolve_segment_gate(media_path.parent)
    assert gate.verdict == "eligible"


def test_process_day_elevates_describe_only_in_batch_mode(tmp_path, monkeypatch):
    """Batch max_jobs elevates screen describe without bypassing other handler pools."""
    import solstone.observe.sense as sense_module

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    for filename in ("screen.webm", "audio.flac", "image.png"):
        make_segment_file(tmp_path, filename=filename)

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    sensor.register("*.flac", "transcribe", ["journal", "transcribe", "{file}"])
    sensor.register("*.png", "depict", ["journal", "depict", "{file}"])
    monkeypatch.setattr(sensor, "_resolve_concurrency", lambda _handler: 1)

    class RecordingPool:
        def __init__(self, name: str) -> None:
            self.name = name
            self.submitted = []
            self.shutdown_calls = []

        def submit(self, fn, *args):
            self.submitted.append((fn, args))
            future = Future()
            try:
                future.set_result(fn(*args))
            except Exception as exc:
                future.set_exception(exc)
            return future

        def shutdown(self, **kwargs):
            self.shutdown_calls.append(kwargs)

    class RecordingTempExecutor(RecordingPool):
        instances = []

        def __init__(self, *, max_workers: int, thread_name_prefix: str) -> None:
            super().__init__(thread_name_prefix)
            self.max_workers = max_workers
            self.thread_name_prefix = thread_name_prefix
            self.instances.append(self)

    configured_pools = {
        name: RecordingPool(name) for name in ("describe", "transcribe", "depict")
    }
    sensor.handler_pools.update(configured_pools)
    monkeypatch.setattr(sense_module, "ThreadPoolExecutor", RecordingTempExecutor)
    processed = []

    def fake_run(queued_item, handler_name, *_args):
        processed.append((handler_name, queued_item.file_path.name))

    monkeypatch.setattr(sensor, "_run_handler", fake_run)

    sensor.process_day("20250101", max_jobs=3)

    assert [
        (executor.max_workers, executor.thread_name_prefix)
        for executor in RecordingTempExecutor.instances
    ] == [(3, "describe-batch")]
    assert configured_pools["describe"].submitted == []
    assert len(RecordingTempExecutor.instances[0].submitted) == 1
    assert len(configured_pools["transcribe"].submitted) == 1
    assert len(configured_pools["depict"].submitted) == 1
    assert set(processed) == {
        ("describe", "screen.webm"),
        ("transcribe", "audio.flac"),
        ("depict", "image.png"),
    }


def test_file_sensor_spawn_handler(tmp_path, monkeypatch):
    """Test spawning handler process."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["echo", "{file}"])
    test_file = make_segment_file(tmp_path, "screen.webm")
    log_path = tmp_path / "chronicle" / "20250101" / "health" / "test_echo.log"

    def fake_spawn(cmd, *_args, **_kwargs):
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_text("")
        return FakeManaged(FakeProcess(0), log_path=log_path)

    with patch.object(sensor, "_spawn_managed_process", side_effect=fake_spawn) as mock:
        sensor._handle_file(test_file)
        sensor.handler_pools["describe"].shutdown(wait=True)

    mock.assert_called_once()
    assert mock.call_args[0][0] == ["echo", str(test_file)]

    health_dir = tmp_path / "chronicle" / "20250101" / "health"
    log_files = list(health_dir.glob("*_echo.log"))
    assert len(log_files) == 1, f"Expected 1 echo log file, found {len(log_files)}"


def test_file_sensor_spawn_handler_duplicate(tmp_path):
    """Test that duplicate file processing is prevented."""
    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["echo", "hello"])
    test_file = make_segment_file(tmp_path, "screen.webm")

    class StubPool:
        def __init__(self):
            self.submitted = []

        def submit(self, *args):
            self.submitted.append(args)
            return MagicMock()

        def shutdown(self, **_kwargs):
            pass

    stub_pool = StubPool()
    sensor.handler_pools["describe"] = stub_pool

    sensor._handle_file(test_file)
    sensor._handle_file(test_file)

    assert len(sensor.queued_handlers["describe"]) == 1
    assert sensor.queued_handlers["describe"][0].file_path == test_file
    assert sensor.running_handlers["describe"] == []
    assert len(stub_pool.submitted) == 1


# This proves describe/transcribe/depict enter the memory gate.
@pytest.mark.parametrize(
    ("handler_name", "filename"),
    [
        ("describe", "screen.webm"),
        ("transcribe", "audio.flac"),
        ("depict", "image.png"),
    ],
)
def test_describe_transcribe_depict_enter_memory_gate(
    tmp_path,
    monkeypatch,
    handler_name,
    filename,
):
    sensor = FileSensor(tmp_path)
    test_file = make_segment_file(tmp_path, filename)
    queued_item = QueuedItem(test_file)
    sensor.queued_handlers[handler_name].append(queued_item)
    calls = []
    spawns = []

    def fake_wait(stage, **kwargs):
        calls.append((stage, kwargs["should_stop"]()))

    def fake_spawn(*_args, **_kwargs):
        spawns.append(handler_name)
        return FakeManaged(FakeProcess(0))

    monkeypatch.setattr(
        "solstone.observe.sense.admission.wait_for_memory_headroom",
        fake_wait,
    )
    monkeypatch.setattr(sensor, "_spawn_managed_process", fake_spawn)

    sensor._run_handler(
        queued_item,
        handler_name,
        ["journal", handler_name, "{file}"],
        "143022_300",
        "20250101",
        False,
    )

    assert calls == [(handler_name, False)]
    assert spawns == [handler_name]
    assert sensor.queued_handlers[handler_name] == []


def test_duplicate_observing_during_memory_throttle_does_not_spawn_twice(
    tmp_path,
    monkeypatch,
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    test_file = make_segment_file(tmp_path, "screen.webm")
    entered_gate = threading.Event()
    release_gate = threading.Event()
    gate_calls = []
    spawn_calls = []

    def fake_wait(stage, **_kwargs):
        gate_calls.append(stage)
        entered_gate.set()
        assert release_gate.wait(timeout=5)

    def fake_spawn(*_args, **_kwargs):
        spawn_calls.append(test_file)
        return FakeManaged(FakeProcess(0))

    monkeypatch.setattr(
        "solstone.observe.sense.admission.wait_for_memory_headroom",
        fake_wait,
    )
    monkeypatch.setattr(sensor, "_spawn_managed_process", fake_spawn)

    sensor._handle_file(test_file)
    assert entered_gate.wait(timeout=5)
    sensor._handle_file(test_file)

    with sensor.lock:
        assert len(sensor.queued_handlers["describe"]) == 1
        assert sensor.running_handlers["describe"] == []

    release_gate.set()
    sensor.handler_pools["describe"].shutdown(wait=True)

    assert gate_calls == ["describe"]
    assert spawn_calls == [test_file]


@patch("solstone.think.runner._current_day")
def test_file_sensor_spawn_handler_real_process(
    mock_day, tmp_path, monkeypatch, mock_callosum
):
    """Test spawning a real process and monitoring completion."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    mock_day.return_value = "20241101"

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["echo", "hello"])

    test_file = make_segment_file(tmp_path, "screen.webm")

    sensor._handle_file(test_file)
    sensor.handler_pools["describe"].shutdown(wait=True)

    assert sensor.running_handlers["describe"] == []

    # Check log file contains output with {ref}_echo.log format
    health_dir = tmp_path / "chronicle" / "20250101" / "health"
    log_files = list(health_dir.glob("*_echo.log"))
    assert len(log_files) == 1, f"Expected 1 echo log file, found {len(log_files)}"

    log_content = log_files[0].read_text()
    assert "hello" in log_content
    # New format is [command_name:stream]
    assert "[echo:stdout]" in log_content


@patch("solstone.think.runner._current_day")
def test_file_sensor_spawn_handler_failing_process(mock_day, tmp_path, monkeypatch):
    """Test handling of failing process."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    mock_day.return_value = "20241101"

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["false"])

    test_file = make_segment_file(tmp_path, "screen.webm")

    sensor._handle_file(test_file)
    sensor.handler_pools["describe"].shutdown(wait=True)

    assert sensor.running_handlers["describe"] == []


def test_file_sensor_failing_process_notifies(tmp_path, monkeypatch):
    """Test that a failing handler process emits a notification event."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["false"])
    # Mock callosum on sensor to capture emitted events
    sensor.callosum = MagicMock()
    test_file = make_segment_file(tmp_path, "screen.webm")
    log_path = tmp_path / "chronicle" / "20250101" / "health" / "test_false.log"

    with patch.object(
        sensor,
        "_spawn_managed_process",
        return_value=FakeManaged(FakeProcess(1), log_path=log_path),
    ):
        sensor._handle_file(test_file)
        sensor.handler_pools["describe"].shutdown(wait=True)

    # Check that a notification event was emitted
    # sensor.callosum.emit is called with ('notification', 'show', ...)
    # Search for a call where the first two args are 'notification' and 'show'
    notif_call = None
    for call in sensor.callosum.emit.call_args_list:
        args, kwargs = call
        if len(args) >= 2 and args[0] == "notification" and args[1] == "show":
            notif_call = call
            break

    assert notif_call is not None
    _, kwargs = notif_call
    assert "describe failed" in kwargs.get("message").lower()
    assert kwargs.get("title") == "Describe Error"


def test_file_sensor_provider_blocked_suppresses_describe_error(tmp_path, monkeypatch):
    """Provider-blocked describe exits stay pending and do not emit error cards."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    sensor.callosum = MagicMock()
    test_file = make_segment_file(tmp_path, "screen.webm")
    log_path = tmp_path / "chronicle" / "20250101" / "health" / "test_describe.log"

    with patch.object(
        sensor,
        "_spawn_managed_process",
        return_value=FakeManaged(
            FakeProcess(EXIT_PROVIDER_BLOCKED),
            log_path=log_path,
        ),
    ):
        sensor._handle_file(test_file)
        sensor.handler_pools["describe"].shutdown(wait=True)

    notification_calls = [
        call
        for call in sensor.callosum.emit.call_args_list
        if call.args[:2] == ("notification", "show")
    ]
    observed_calls = [
        call
        for call in sensor.callosum.emit.call_args_list
        if call.args[:2] == ("observe", "observed")
    ]

    assert notification_calls == []
    assert len(observed_calls) == 1
    assert "error" not in observed_calls[0].kwargs
    assert not test_file.with_suffix(".jsonl").exists()
    assert sensor.running_handlers["describe"] == []


def _callosum_emit_calls(sensor: FileSensor, tract: str, event: str):
    return [
        call
        for call in sensor.callosum.emit.call_args_list
        if call.args[:2] == (tract, event)
    ]


def test_handler_shutdown_signal_exit_suppresses_failure_reporting(
    tmp_path, monkeypatch, caplog
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    sensor.callosum = MagicMock()
    sensor.recent_error_count = 4
    sensor.last_error_reason = "prior failure"
    test_file = make_segment_file(tmp_path, "screen.webm")
    process = BlockingSignalProcess()
    managed = FakeManaged(
        process,
        log_path=tmp_path / "chronicle" / "20250101" / "health" / "describe.log",
    )
    remove_calls = []
    original_remove = sensor._remove_running_handler

    def record_remove(handler_name, handler_proc):
        remove_calls.append((handler_name, id(handler_proc)))
        return original_remove(handler_name, handler_proc)

    monkeypatch.setattr(
        sensor,
        "_spawn_managed_process",
        lambda *_args, **_kwargs: managed,
    )
    monkeypatch.setattr(sensor, "_remove_running_handler", record_remove)
    caplog.set_level(logging.DEBUG, logger="solstone.observe.sense")

    sensor._handle_file(test_file)
    # Do not pre-set _stopping: that hits the pre-spawn/terminate_due_to_stop
    # returns and never proves the post-wait shutdown classification.
    assert process.wait_entered.wait(timeout=2)

    sensor.stop()

    assert process.terminated is True
    assert _callosum_emit_calls(sensor, "notification", "show") == []
    observed_calls = _callosum_emit_calls(sensor, "observe", "observed")
    assert len(observed_calls) == 1
    assert observed_calls[0].kwargs["error"] is True
    assert observed_calls[0].kwargs["errors"] == ["describe exit -15"]
    assert sensor.recent_error_count == 4
    assert sensor.last_error_reason == "prior failure"
    assert [
        record
        for record in caplog.records
        if record.name == "solstone.observe.sense" and record.levelno >= logging.ERROR
    ] == []
    assert "Segment observed with errors: 20250101/143022_300" in caplog.text
    assert "['describe exit -15']" in caplog.text
    assert sensor.running_handlers["describe"] == []
    assert managed.cleanup.call_count == 2
    assert len(remove_calls) == 2


def test_handler_shutdown_nonzero_exit_outside_stop_reports_failure(
    tmp_path, monkeypatch, caplog
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    sensor.callosum = MagicMock()
    test_file = make_segment_file(tmp_path, "screen.webm")
    log_path = tmp_path / "chronicle" / "20250101" / "health" / "describe.log"
    monkeypatch.setattr(
        sensor,
        "_spawn_managed_process",
        lambda *_args, **_kwargs: FakeManaged(FakeProcess(2), log_path=log_path),
    )
    caplog.set_level(logging.ERROR, logger="solstone.observe.sense")

    sensor._handle_file(test_file)
    sensor.handler_pools["describe"].shutdown(wait=True)

    assert "describe failed with exit 2" in caplog.text
    assert len(_callosum_emit_calls(sensor, "notification", "show")) == 1
    assert sensor.recent_error_count == 1
    assert sensor.last_error_reason == "describe exit 2"


def test_handler_shutdown_signal_exit_outside_stop_reports_failure(
    tmp_path, monkeypatch, caplog
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    sensor.callosum = MagicMock()
    test_file = make_segment_file(tmp_path, "screen.webm")
    log_path = tmp_path / "chronicle" / "20250101" / "health" / "describe.log"
    monkeypatch.setattr(
        sensor,
        "_spawn_managed_process",
        lambda *_args, **_kwargs: FakeManaged(FakeProcess(-9), log_path=log_path),
    )
    caplog.set_level(logging.ERROR, logger="solstone.observe.sense")

    sensor._handle_file(test_file)
    sensor.handler_pools["describe"].shutdown(wait=True)

    # This excludes implementations keyed on exit-code sign: they pass the
    # shutdown case while swallowing every kernel-killed handler.
    assert "describe failed with exit -9" in caplog.text
    assert len(_callosum_emit_calls(sensor, "notification", "show")) == 1
    assert sensor.recent_error_count == 1
    assert sensor.last_error_reason == "describe exit -9"


def test_handler_watchdog_timeout_terminates_and_surfaces(
    tmp_path, monkeypatch, caplog
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    sensor.callosum = MagicMock()
    test_file = make_segment_file(tmp_path, "screen.webm")
    process = TimeoutProcess()
    log_path = tmp_path / "chronicle" / "20250101" / "health" / "describe.log"

    monkeypatch.setattr(sensor, "_resolve_max_runtime", lambda _handler: 1)
    monkeypatch.setattr(
        sensor,
        "_spawn_managed_process",
        lambda *_args, **_kwargs: FakeManaged(process, log_path=log_path),
    )

    original_check = sensor._check_segment_observed
    checks = []

    def record_check(file_path, error=None):
        checks.append((file_path, error))
        return original_check(file_path, error=error)

    monkeypatch.setattr(sensor, "_check_segment_observed", record_check)
    caplog.set_level("ERROR")

    sensor._handle_file(test_file)
    sensor.handler_pools["describe"].shutdown(wait=True)

    notification_calls = [
        call
        for call in sensor.callosum.emit.call_args_list
        if call.args[:2] == ("notification", "show")
    ]
    observed_calls = [
        call
        for call in sensor.callosum.emit.call_args_list
        if call.args[:2] == ("observe", "observed")
    ]

    assert process.terminated is True
    assert checks == [(test_file, f"describe {WATCHDOG_TIMEOUT} after 1s")]
    assert notification_calls
    assert notification_calls[0].kwargs["title"] == "Describe Timeout"
    assert WATCHDOG_TIMEOUT in notification_calls[0].kwargs["message"]
    assert observed_calls
    assert observed_calls[0].kwargs["error"] is True
    assert "Unhandled exception in handler worker" not in caplog.text


def test_start_emits_native_observe_health_beacon(tmp_path, monkeypatch):
    import solstone.observe.sense as sense_module

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    callosum = MagicMock()
    monkeypatch.setattr(sense_module, "CallosumConnection", lambda **_kwargs: callosum)

    sensor = FileSensor(tmp_path)
    sensor.running_flag = False

    sensor.start()

    status_calls = _status_emit_calls(sensor)
    assert len(status_calls) == 1
    kwargs = status_calls[0].kwargs
    assert kwargs["name"] == "native.observe"
    assert kwargs["stream_type"] == "screen_audio"
    assert BEACON_FIELDS <= set(kwargs)


def test_emit_status_idle_emits_beacon_without_handler_sections(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sensor = FileSensor(tmp_path)
    sensor.callosum = MagicMock()

    sensor._emit_status()

    status_calls = _status_emit_calls(sensor)
    assert len(status_calls) == 1
    kwargs = status_calls[0].kwargs
    assert BEACON_FIELDS <= set(kwargs)
    for handler_name in ("describe", "transcribe", "depict"):
        assert handler_name not in kwargs


def test_emit_status_pending_depth_counts_work_without_path_leaks(
    tmp_path, monkeypatch
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sensor = FileSensor(tmp_path)
    sensor.callosum = MagicMock()
    running_file = make_segment_file(tmp_path, "running_secret.webm")
    queued_file = make_segment_file(
        tmp_path,
        "queued_secret.flac",
        segment="143023_300",
    )

    sensor.running_handlers["describe"].append(
        HandlerProcess(running_file, FakeManaged(ref="running-ref"), "describe")
    )
    sensor.queued_handlers["transcribe"].append(QueuedItem(queued_file))

    sensor._emit_status()

    kwargs = _status_emit_calls(sensor)[0].kwargs
    assert kwargs["pending_queue_depth"] == 2
    leaked_substrings = (
        running_file.name,
        queued_file.name,
        str(running_file),
        str(queued_file),
    )
    for field in BEACON_FIELDS:
        value = str(kwargs[field])
        for leaked in leaked_substrings:
            assert leaked not in value


def test_memory_throttle_beacon_uses_count_until_last_waiter_finishes(
    tmp_path,
    monkeypatch,
):
    import solstone.observe.sense as sense_module

    admission.reset_admission_state()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        sense_module,
        "get_config",
        lambda: {"describe": {"max_concurrent": 2}},
    )
    monkeypatch.setattr(
        admission, "get_config", lambda: {"memory": {"floor_mib": 5120}}
    )
    monkeypatch.setattr(admission, "_POLL_INTERVAL_SECONDS", 0.01)
    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    files = [
        make_segment_file(tmp_path, "screen1.webm"),
        make_segment_file(tmp_path, "screen2.webm", segment="143023_300"),
    ]
    release_first = threading.Event()
    release_second = threading.Event()
    thread_order: dict[int, int] = {}
    order_lock = threading.Lock()
    callback_lock = threading.Lock()
    throttled_twice = threading.Event()
    first_waiter_finished = threading.Event()
    all_waiters_finished = threading.Event()
    throttle_starts = 0
    throttle_finishes = 0

    def fake_available() -> int:
        ident = threading.get_ident()
        with order_lock:
            index = thread_order.setdefault(ident, len(thread_order) + 1)
        if index == 1 and release_first.is_set():
            return 6 * 1024**3
        if index == 2 and release_second.is_set():
            return 6 * 1024**3
        return 2 * 1024**3

    def throttle_started(**_fields):
        nonlocal throttle_starts
        with callback_lock:
            throttle_starts += 1
            if throttle_starts == 2:
                throttled_twice.set()

    def throttle_completed(**_fields):
        nonlocal throttle_finishes
        with callback_lock:
            throttle_finishes += 1
            if throttle_finishes == 1:
                first_waiter_finished.set()
            if throttle_finishes == 2:
                all_waiters_finished.set()

    monkeypatch.setattr(admission, "read_available_bytes", fake_available)
    monkeypatch.setattr(sensor, "_emit_memory_throttle_started", throttle_started)
    monkeypatch.setattr(sensor, "_emit_memory_throttle_completed", throttle_completed)
    monkeypatch.setattr(
        sensor,
        "_spawn_managed_process",
        lambda *_args, **_kwargs: FakeManaged(FakeProcess(0)),
    )

    try:
        for file_path in files:
            sensor._handle_file(file_path)
        assert throttled_twice.wait(timeout=2)

        beacon = sensor._build_health_beacon()
        assert beacon["memory_throttled"] is True
        assert beacon["memory_throttle_count"] == 2
        assert beacon["memory_floor_mib"] == 5120
        assert beacon["memory_available_mib"] == 2048

        release_first.set()
        assert first_waiter_finished.wait(timeout=2)

        beacon = sensor._build_health_beacon()
        assert beacon["memory_throttled"] is True
        assert beacon["memory_throttle_count"] == 1

        release_second.set()
        assert all_waiters_finished.wait(timeout=2)
        sensor.handler_pools["describe"].shutdown(wait=True)

        beacon = sensor._build_health_beacon()
        assert beacon["memory_throttled"] is False
        assert beacon["memory_throttle_count"] == 0
        assert beacon["memory_floor_mib"] is None
        assert beacon["memory_available_mib"] is None
    finally:
        release_first.set()
        release_second.set()
        admission.reset_admission_state()


def test_stop_releases_blocked_memory_waiters_and_spawns_nothing(
    tmp_path,
    monkeypatch,
):
    """stop() drains memory waiters without spawning handlers."""
    import solstone.observe.sense as sense_module

    admission.reset_admission_state()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        sense_module,
        "get_config",
        lambda: {"describe": {"max_concurrent": 2}},
    )
    monkeypatch.setattr(
        admission, "get_config", lambda: {"memory": {"floor_mib": 5120}}
    )
    monkeypatch.setattr(admission, "read_available_bytes", lambda: 2 * 1024**3)
    monkeypatch.setattr(admission, "_POLL_INTERVAL_SECONDS", 0.01)
    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    files = [
        make_segment_file(tmp_path, "stop1.webm"),
        make_segment_file(tmp_path, "stop2.webm", segment="143023_300"),
    ]
    spawns = []
    callback_lock = threading.Lock()
    throttled_twice = threading.Event()
    throttle_starts = 0
    throttle_finishes = 0

    def throttle_started(**_fields):
        nonlocal throttle_starts
        with callback_lock:
            throttle_starts += 1
            if throttle_starts == 2:
                throttled_twice.set()

    def throttle_completed(**_fields):
        nonlocal throttle_finishes
        with callback_lock:
            throttle_finishes += 1

    monkeypatch.setattr(sensor, "_emit_memory_throttle_started", throttle_started)
    monkeypatch.setattr(sensor, "_emit_memory_throttle_completed", throttle_completed)
    monkeypatch.setattr(
        sensor,
        "_spawn_managed_process",
        lambda *_args, **_kwargs: spawns.append("spawn") or FakeManaged(FakeProcess(0)),
    )

    try:
        for file_path in files:
            sensor._handle_file(file_path)
        assert throttled_twice.wait(timeout=2)

        sensor.stop()

        assert throttle_finishes == 2
        assert spawns == []
        assert admission.throttle_state().count == 0
    finally:
        admission.reset_admission_state()


def test_successful_contact_and_idle_status_reset_recent_errors(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sensor = FileSensor(tmp_path)
    sensor.recent_error_count = 3

    sensor._record_successful_contact()

    assert sensor.recent_error_count == 0
    assert sensor.last_successful_sync is not None

    sensor.recent_error_count = 3
    sensor.last_successful_sync = None
    sensor.callosum = MagicMock()

    sensor._emit_status()

    kwargs = _status_emit_calls(sensor)[0].kwargs
    assert kwargs["recent_error_count"] == 0
    assert kwargs["last_successful_sync"] is not None


def test_handler_failure_count_reason_cap_and_provider_blocked_exclusion(
    tmp_path, monkeypatch
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sensor = FileSensor(tmp_path)

    sensor._record_handler_failure("describe exit 7")

    assert sensor.recent_error_count == 1
    assert sensor.last_error_reason == "describe exit 7"
    assert "\n" not in sensor.last_error_reason
    assert len(sensor.last_error_reason) <= 200

    for _ in range(150):
        sensor._record_handler_failure("describe exit 7")

    assert sensor.recent_error_count == 99

    provider_sensor = FileSensor(tmp_path)
    provider_sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    provider_sensor.callosum = MagicMock()
    test_file = make_segment_file(tmp_path, "provider_blocked.webm")
    queued_item = QueuedItem(test_file)
    provider_sensor.queued_handlers["describe"].append(queued_item)
    log_path = tmp_path / "chronicle" / "20250101" / "health" / "provider.log"

    monkeypatch.setattr(
        provider_sensor,
        "_spawn_managed_process",
        lambda *_args, **_kwargs: FakeManaged(
            FakeProcess(EXIT_PROVIDER_BLOCKED),
            log_path=log_path,
        ),
    )

    provider_sensor._run_handler(
        queued_item,
        "describe",
        ["journal", "describe", "{file}"],
        "143022_300",
        "20250101",
        False,
    )

    assert provider_sensor.recent_error_count == 0


def test_watchdog_timeout_counts_path_free_reason(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    sensor.callosum = MagicMock()
    test_file = make_segment_file(tmp_path, "timeout_secret.webm")
    process = TimeoutProcess()

    monkeypatch.setattr(sensor, "_resolve_max_runtime", lambda _handler: 1)
    monkeypatch.setattr(
        sensor,
        "_spawn_managed_process",
        lambda *_args, **_kwargs: FakeManaged(process),
    )

    sensor._handle_file(test_file)
    sensor.handler_pools["describe"].shutdown(wait=True)

    assert sensor.recent_error_count == 1
    assert sensor.last_error_reason is not None
    assert WATCHDOG_TIMEOUT in sensor.last_error_reason
    assert test_file.name not in sensor.last_error_reason
    assert str(test_file) not in sensor.last_error_reason


def test_health_beacon_allowlist(tmp_path):
    sensor = FileSensor(tmp_path)

    beacon = sensor._build_health_beacon()

    assert set(beacon.keys()) == BEACON_FIELDS


def test_emit_status_soft_warns_once_before_handler_cap(tmp_path, monkeypatch):
    import solstone.observe.sense as sense_module

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    sensor = FileSensor(tmp_path)
    sensor.callosum = MagicMock()
    slow_file = make_segment_file(tmp_path, "slow.webm")
    fast_file = make_segment_file(
        tmp_path,
        "fast.flac",
        segment="143023_300",
    )
    slow_proc = HandlerProcess(slow_file, FakeManaged(ref="slow-ref"), "describe")
    fast_proc = HandlerProcess(fast_file, FakeManaged(ref="fast-ref"), "transcribe")
    slow_proc.started_at = 925.0
    fast_proc.started_at = 951.0
    sensor.running_handlers["describe"].append(slow_proc)
    sensor.running_handlers["transcribe"].append(fast_proc)
    monkeypatch.setattr(sensor, "_resolve_max_runtime", lambda _handler: 100)
    monkeypatch.setattr(
        sense_module,
        "time",
        _ClockShim(sense_module.time, time=lambda: 1000.0),
    )

    sensor._emit_status()
    sensor._emit_status()
    sensor._emit_status()

    notification_calls = [
        call
        for call in sensor.callosum.emit.call_args_list
        if call.args[:2] == ("notification", "show")
    ]

    assert len(notification_calls) == 1
    notification = notification_calls[0].kwargs
    assert notification["title"] == "Describe Slow"
    assert notification["title"] != "Describe Timeout"
    assert WATCHDOG_TIMEOUT not in notification["message"]
    assert "taking longer than usual" in notification["message"]
    assert "Transcribe Slow" not in [
        call.kwargs["title"] for call in notification_calls
    ]


def test_healthy_job_under_cap_not_killed(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    sensor.callosum = MagicMock()
    test_file = make_segment_file(tmp_path, "screen.webm")
    process = FakeProcess(0)

    monkeypatch.setattr(
        sensor,
        "_spawn_managed_process",
        lambda *_args, **_kwargs: FakeManaged(process),
    )

    original_check = sensor._check_segment_observed
    checks = []

    def record_check(file_path, error=None):
        checks.append((file_path, error))
        return original_check(file_path, error=error)

    monkeypatch.setattr(sensor, "_check_segment_observed", record_check)

    sensor._handle_file(test_file)
    sensor.handler_pools["describe"].shutdown(wait=True)

    observed_calls = [
        call
        for call in sensor.callosum.emit.call_args_list
        if call.args[:2] == ("observe", "observed")
    ]

    assert checks == [(test_file, None)]
    assert process.terminated is False
    assert observed_calls
    assert "error" not in observed_calls[0].kwargs


def test_file_sensor_handle_file(tmp_path):
    """Test file handling dispatches to correct handler."""
    with patch.object(FileSensor, "_run_handler") as mock_run:
        # Create journal/day/stream/segment structure
        day_dir = tmp_path / "chronicle" / "20250101"
        segment_dir = day_dir / "default" / "143022_300"
        segment_dir.mkdir(parents=True)

        sensor = FileSensor(tmp_path)
        sensor.register("*.webm", "describe", ["echo", "{file}"])

        test_file = segment_dir / "center_DP-3_screen.webm"
        test_file.write_text("content")

        sensor._handle_file(test_file)
        sensor.handler_pools["describe"].shutdown(wait=True)

        mock_run.assert_called_once()
        call_args = mock_run.call_args[0]
        assert call_args[0].file_path == test_file
        assert call_args[1] == "describe"


def test_file_sensor_handle_nonexistent_file(tmp_path):
    """Test handling of nonexistent file is graceful."""
    with patch.object(FileSensor, "_run_handler") as mock_run:
        sensor = FileSensor(tmp_path)
        sensor.register("*.webm", "describe", ["echo", "{file}"])

        nonexistent = tmp_path / "nonexistent.webm"
        sensor._handle_file(nonexistent)

        # Should not spawn handler for nonexistent file
        mock_run.assert_not_called()


def test_file_sensor_stop():
    """Test stopping the sensor."""
    with tempfile.TemporaryDirectory() as tmpdir:
        sensor = FileSensor(Path(tmpdir))

        # Mock callosum
        sensor.callosum = MagicMock()
        process = FakeProcess()
        managed = FakeManaged(process)
        handler_proc = HandlerProcess(Path(tmpdir) / "test.webm", managed, "describe")
        sensor.running_handlers["describe"].append(handler_proc)

        sensor.stop()

        assert sensor.running_flag is False
        assert sensor._stopping.is_set()
        sensor.callosum.stop.assert_called_once()
        assert process.terminated is True
        managed.cleanup.assert_called_once()
        assert all(pool._shutdown for pool in sensor.handler_pools.values())


def test_file_sensor_handle_callosum_message(tmp_path):
    """Test handling of observe.observing Callosum events."""
    with patch.object(FileSensor, "_handle_file") as mock_handle:
        # Create journal/day/stream/segment structure
        day_dir = tmp_path / "chronicle" / "20250101"
        segment_dir = day_dir / "default" / "143022_300"
        segment_dir.mkdir(parents=True)

        sensor = FileSensor(tmp_path)
        sensor.register("*.flac", "transcribe", ["echo", "{file}"])
        sensor.register("*.webm", "describe", ["echo", "{file}"])

        # Create test files with simple names in segment directory
        audio_file = segment_dir / "audio.flac"
        audio_file.write_text("audio content")
        video_file = segment_dir / "center_DP-3_screen.webm"
        video_file.write_text("video content")

        # Simulate observing event with simple filenames
        message = {
            "tract": "observe",
            "event": "observing",
            "day": "20250101",
            "stream": "default",
            "segment": "143022_300",
            "files": ["audio.flac", "center_DP-3_screen.webm"],
        }

        sensor._handle_callosum_message(message)

        # Should have called _handle_file for each file
        assert mock_handle.call_count == 2
        called_paths = [call[0][0] for call in mock_handle.call_args_list]
        assert audio_file in called_paths
        assert video_file in called_paths

        # Should have pre-registered segment tracking
        assert "143022_300" in sensor.segment_files
        assert audio_file in sensor.segment_files["143022_300"]
        assert video_file in sensor.segment_files["143022_300"]
        assert "143022_300" in sensor.segment_start_time
        assert sensor.segment_day["143022_300"] == "20250101"


def test_file_sensor_handle_callosum_message_ignores_other_events(tmp_path):
    """Test that non-observing events are ignored."""
    with patch.object(FileSensor, "_handle_file") as mock_handle:
        sensor = FileSensor(tmp_path)

        # Simulate a different event type
        message = {
            "tract": "observe",
            "event": "status",
            "some_data": "value",
        }

        sensor._handle_callosum_message(message)

        # Should not call _handle_file
        mock_handle.assert_not_called()


def test_file_sensor_handle_callosum_message_invalid_event(tmp_path):
    """Test that invalid observing events are handled gracefully."""
    with patch.object(FileSensor, "_handle_file") as mock_handle:
        sensor = FileSensor(tmp_path)

        # Simulate event missing required fields
        message = {
            "tract": "observe",
            "event": "observing",
            "segment": "143022_300",
            # missing 'day' and 'files'
        }

        sensor._handle_callosum_message(message)

        # Should not call _handle_file
        mock_handle.assert_not_called()


def test_file_sensor_deferred_live_observing_suppresses_handlers(
    tmp_path, monkeypatch, mock_callosum
):
    import solstone.observe.sense as sense_module
    from solstone.think.callosum import CallosumConnection

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        sense_module,
        "load_processing_settings",
        lambda: _processing_settings("deferred"),
    )

    segment_dir = tmp_path / "chronicle" / "20250101" / "default" / "143022_300"
    segment_dir.mkdir(parents=True)
    audio_file = segment_dir / "audio.flac"
    audio_file.write_text("audio content")

    sensor = FileSensor(tmp_path)
    sensor.register("*.flac", "transcribe", ["echo", "{file}"])
    emitted_events = []
    sensor.callosum = CallosumConnection()
    sensor.callosum.start(callback=lambda msg: emitted_events.append(msg))

    with patch.object(sensor, "_handle_file") as mock_handle:
        sensor._handle_callosum_message(
            {
                "tract": "observe",
                "event": "observing",
                "day": "20250101",
                "stream": "default",
                "segment": "143022_300",
                "files": ["audio.flac"],
            }
        )

    mock_handle.assert_not_called()
    observed_events = [
        event
        for event in emitted_events
        if event.get("tract") == "observe" and event.get("event") == "observed"
    ]
    assert len(observed_events) == 1
    assert observed_events[0].get("day") == "20250101"
    assert observed_events[0].get("segment") == "143022_300"
    assert (tmp_path / "chronicle" / "20250101" / "health" / "stream.updated").exists()
    assert audio_file.exists()


def test_file_sensor_no_engine_live_observing_suppresses_handlers(
    tmp_path, monkeypatch, mock_callosum
):
    import solstone.observe.sense as sense_module
    from solstone.think.callosum import CallosumConnection

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        sense_module,
        "load_processing_settings",
        lambda: _processing_settings("realtime"),
    )
    monkeypatch.setattr("solstone.think.models.no_thinking_engine_chosen", lambda: True)

    segment_dir = tmp_path / "chronicle" / "20250101" / "default" / "143022_300"
    segment_dir.mkdir(parents=True)
    audio_file = segment_dir / "audio.flac"
    audio_file.write_text("audio content")

    sensor = FileSensor(tmp_path)
    sensor.register("*.flac", "transcribe", ["echo", "{file}"])
    emitted_events = []
    sensor.callosum = CallosumConnection()
    sensor.callosum.start(callback=lambda msg: emitted_events.append(msg))

    with patch.object(sensor, "_handle_file") as mock_handle:
        sensor._handle_callosum_message(
            {
                "tract": "observe",
                "event": "observing",
                "day": "20250101",
                "stream": "default",
                "segment": "143022_300",
                "files": ["audio.flac"],
            }
        )

    mock_handle.assert_not_called()
    observed_events = [
        event
        for event in emitted_events
        if event.get("tract") == "observe" and event.get("event") == "observed"
    ]
    assert len(observed_events) == 1
    assert observed_events[0].get("day") == "20250101"
    assert observed_events[0].get("segment") == "143022_300"
    assert (tmp_path / "chronicle" / "20250101" / "health" / "stream.updated").exists()
    assert audio_file.exists()


def test_file_sensor_realtime_live_observing_dispatches_handler(tmp_path, monkeypatch):
    import solstone.observe.sense as sense_module

    monkeypatch.setattr(
        sense_module,
        "load_processing_settings",
        lambda: _processing_settings("realtime"),
    )

    segment_dir = tmp_path / "chronicle" / "20250101" / "default" / "143022_300"
    segment_dir.mkdir(parents=True)
    audio_file = segment_dir / "audio.flac"
    audio_file.write_text("audio content")

    sensor = FileSensor(tmp_path)
    sensor.register("*.flac", "transcribe", ["echo", "{file}"])

    with patch.object(sensor, "_handle_file") as mock_handle:
        sensor._handle_callosum_message(
            {
                "tract": "observe",
                "event": "observing",
                "day": "20250101",
                "stream": "default",
                "segment": "143022_300",
                "files": ["audio.flac"],
            }
        )

    mock_handle.assert_called_once()
    assert mock_handle.call_args.args[0] == audio_file


def test_file_sensor_deferred_batch_observing_dispatches_handler(tmp_path, monkeypatch):
    import solstone.observe.sense as sense_module

    monkeypatch.setattr(
        sense_module,
        "load_processing_settings",
        lambda: _processing_settings("deferred"),
    )

    segment_dir = tmp_path / "chronicle" / "20250101" / "default" / "143022_300"
    segment_dir.mkdir(parents=True)
    audio_file = segment_dir / "audio.flac"
    audio_file.write_text("audio content")

    sensor = FileSensor(tmp_path)
    sensor.register("*.flac", "transcribe", ["echo", "{file}"])

    with patch.object(sensor, "_handle_file") as mock_handle:
        sensor._handle_callosum_message(
            {
                "tract": "observe",
                "event": "observing",
                "day": "20250101",
                "stream": "default",
                "segment": "143022_300",
                "files": ["audio.flac"],
                "batch": True,
            }
        )

    mock_handle.assert_called_once()
    assert mock_handle.call_args.args[0] == audio_file


@patch("solstone.think.runner._current_day")
def test_file_sensor_segment_observed_includes_day(
    mock_day, tmp_path, monkeypatch, mock_callosum
):
    """Test that observe.observed event includes day field."""
    from solstone.think.callosum import CallosumConnection

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    mock_day.return_value = "20250101"

    # Create journal/day/stream/segment structure
    day_dir = tmp_path / "chronicle" / "20250101"
    segment_dir = day_dir / "default" / "143022_300"
    segment_dir.mkdir(parents=True)

    sensor = FileSensor(tmp_path)
    sensor.register("*.flac", "transcribe", ["echo", "{file}"])

    # Set up callosum on sensor to capture emitted events
    emitted_events = []
    sensor.callosum = CallosumConnection()
    sensor.callosum.start(callback=lambda msg: emitted_events.append(msg))

    # Create test file with simple name in segment directory
    audio_file = segment_dir / "audio.flac"
    audio_file.write_text("audio content")

    # Simulate observing event to set up segment tracking (simple filenames)
    message = {
        "tract": "observe",
        "event": "observing",
        "day": "20250101",
        "stream": "default",
        "segment": "143022_300",
        "files": ["audio.flac"],
    }
    sensor._handle_callosum_message(message)
    sensor.handler_pools["transcribe"].shutdown(wait=True)

    # Check that segment_day was cleaned up (handler completed)
    assert "143022_300" not in sensor.segment_day

    # Check observe.observed event was emitted with day field
    observed_events = [
        e
        for e in emitted_events
        if e.get("tract") == "observe" and e.get("event") == "observed"
    ]
    assert len(observed_events) == 1
    assert observed_events[0].get("day") == "20250101"
    assert observed_events[0].get("segment") == "143022_300"


def _observed_event_from_observing(
    tmp_path, monkeypatch, mock_callosum, *, batch: bool
):
    from solstone.think.callosum import CallosumConnection

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    segment_dir = tmp_path / "chronicle" / "20250101" / "default" / "143022_300"
    segment_dir.mkdir(parents=True)
    audio_file = segment_dir / "audio.flac"
    audio_file.write_text("audio content")

    sensor = FileSensor(tmp_path)
    sensor.register("*.flac", "transcribe", ["echo", "{file}"])

    emitted_events = []
    sensor.callosum = CallosumConnection()
    sensor.callosum.start(callback=lambda msg: emitted_events.append(msg))

    message = {
        "tract": "observe",
        "event": "observing",
        "day": "20250101",
        "stream": "default",
        "segment": "143022_300",
        "files": ["audio.flac"],
    }
    if batch:
        message["batch"] = True

    with patch.object(sensor, "_handle_file"):
        sensor._handle_callosum_message(message)

    sensor._check_segment_observed(audio_file)

    observed_events = [
        event
        for event in emitted_events
        if event.get("tract") == "observe" and event.get("event") == "observed"
    ]
    assert len(observed_events) == 1
    return observed_events[0]


def test_file_sensor_observing_batch_propagates_to_observed(
    tmp_path, monkeypatch, mock_callosum
):
    observed = _observed_event_from_observing(
        tmp_path, monkeypatch, mock_callosum, batch=True
    )

    assert observed.get("batch") is True


def test_file_sensor_observing_without_batch_emits_live_observed(
    tmp_path, monkeypatch, mock_callosum
):
    observed = _observed_event_from_observing(
        tmp_path, monkeypatch, mock_callosum, batch=False
    )

    assert "batch" not in observed


def test_file_sensor_live_observed_touches_stream_updated(
    tmp_path, monkeypatch, mock_callosum
):
    _observed_event_from_observing(tmp_path, monkeypatch, mock_callosum, batch=False)

    marker = tmp_path / "chronicle" / "20250101" / "health" / "stream.updated"
    assert marker.exists()


def test_file_sensor_live_observed_logs_stream_updated_failure(
    tmp_path, monkeypatch, mock_callosum, caplog
):
    def fail_day_path(_day):
        raise OSError("boom")

    monkeypatch.setattr("solstone.observe.sense.day_path", fail_day_path)

    with caplog.at_level("DEBUG", logger="solstone.observe.sense"):
        observed = _observed_event_from_observing(
            tmp_path, monkeypatch, mock_callosum, batch=False
        )

    assert observed["segment"] == "143022_300"
    assert "failed to touch stream.updated marker" in caplog.text


def test_file_sensor_batch_observed_does_not_touch_stream_updated(
    tmp_path, monkeypatch, mock_callosum
):
    _observed_event_from_observing(tmp_path, monkeypatch, mock_callosum, batch=True)

    marker = tmp_path / "chronicle" / "20250101" / "health" / "stream.updated"
    assert not marker.exists()


def test_file_sensor_segment_observed_no_handlers(tmp_path, monkeypatch, mock_callosum):
    """Test that observe.observed is emitted immediately for segments with no matching handlers.

    This covers the case of tmux-only segments where files like .jsonl don't match
    any registered patterns (*.flac, *.webm, etc.).
    """
    from solstone.think.callosum import CallosumConnection

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # Create journal/day/stream/segment structure
    day_dir = tmp_path / "chronicle" / "20250101"
    segment_dir = day_dir / "default" / "143022_300"
    segment_dir.mkdir(parents=True)

    sensor = FileSensor(tmp_path)
    # Only register handlers for audio/video (not .jsonl)
    sensor.register("*.flac", "transcribe", ["echo", "{file}"])
    sensor.register("*.webm", "describe", ["echo", "{file}"])

    # Set up callosum on sensor to capture emitted events
    emitted_events = []
    sensor.callosum = CallosumConnection()
    sensor.callosum.start(callback=lambda msg: emitted_events.append(msg))

    # Create test file that doesn't match any pattern (like tmux captures)
    jsonl_file = segment_dir / "tmux_0_screen.jsonl"
    jsonl_file.write_text('{"content": "terminal output"}')

    # Simulate observing event with only .jsonl file
    message = {
        "tract": "observe",
        "event": "observing",
        "day": "20250101",
        "stream": "default",
        "segment": "143022_300",
        "files": ["tmux_0_screen.jsonl"],
    }
    sensor._handle_callosum_message(message)

    # Segment tracking should be cleaned up immediately (no handlers to wait for)
    assert "143022_300" not in sensor.segment_files
    assert "143022_300" not in sensor.segment_day

    # Check observe.observed event was emitted immediately
    observed_events = [
        e
        for e in emitted_events
        if e.get("tract") == "observe" and e.get("event") == "observed"
    ]
    assert len(observed_events) == 1
    assert observed_events[0].get("day") == "20250101"
    assert observed_events[0].get("segment") == "143022_300"


def test_file_sensor_queued_describes_complete_on_long_lived_worker(
    tmp_path, monkeypatch, mock_callosum
):
    """Two queued describe files both complete on the long-lived handler worker without spurious termination."""
    import solstone.observe.sense as sense_module
    from solstone.think.callosum import CallosumConnection

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        sense_module,
        "get_config",
        lambda: {
            "describe": {"max_concurrent": 1},
            "transcribe": {"max_concurrent": 1},
        },
    )

    segment_dir = tmp_path / "chronicle" / "20250101" / "default" / "143022_300"
    segment_dir.mkdir(parents=True)
    first = segment_dir / "first.webm"
    second = segment_dir / "second.webm"
    first.write_text("video")
    second.write_text("video")

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    emitted_events = []
    sensor.callosum = CallosumConnection()
    sensor.callosum.start(callback=lambda msg: emitted_events.append(msg))
    terminated = []

    def fake_spawn(cmd, file_path, ref, segment, observer, meta, day, **_kwargs):
        process = FakeProcess(0, delay=0.02)

        def terminate():
            terminated.append(file_path)
            FakeProcess.terminate(process)

        process.terminate = terminate
        file_path.with_suffix(".jsonl").write_text("{}\n")
        return FakeManaged(process, ref=ref, log_path=tmp_path / "describe.log")

    original_check = sensor._check_segment_observed
    checked = []

    def record_check(file_path, error=None):
        checked.append((file_path, error))
        return original_check(file_path, error=error)

    monkeypatch.setattr(sensor, "_spawn_managed_process", fake_spawn)
    monkeypatch.setattr(sensor, "_check_segment_observed", record_check)

    sensor._handle_callosum_message(
        {
            "tract": "observe",
            "event": "observing",
            "day": "20250101",
            "stream": "default",
            "segment": "143022_300",
            "files": ["first.webm", "second.webm"],
        }
    )
    sensor.handler_pools["describe"].shutdown(wait=True)

    assert terminated == []
    assert checked == [(first, None), (second, None)]
    observed_events = [
        event
        for event in emitted_events
        if event.get("tract") == "observe" and event.get("event") == "observed"
    ]
    assert len(observed_events) == 1
    assert "errors" not in observed_events[0]
    assert first.with_suffix(".jsonl").exists()
    assert second.with_suffix(".jsonl").exists()


def test_handler_watchdog_timeout_drains_queue(tmp_path, monkeypatch, mock_callosum):
    """A timed-out head job frees the serialized worker for queued tail jobs."""
    import solstone.observe.sense as sense_module
    from solstone.think.callosum import CallosumConnection

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        sense_module,
        "get_config",
        lambda: {
            "describe": {"max_concurrent": 1},
            "transcribe": {"max_concurrent": 1},
        },
    )

    segment_dir = tmp_path / "chronicle" / "20250101" / "default" / "143022_300"
    segment_dir.mkdir(parents=True)
    files = [
        segment_dir / "first.webm",
        segment_dir / "second.webm",
        segment_dir / "third.webm",
    ]
    for file_path in files:
        file_path.write_text("video")

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    emitted_events = []
    sensor.callosum = CallosumConnection()
    sensor.callosum.start(callback=lambda msg: emitted_events.append(msg))

    timeout_process = TimeoutProcess()

    def fake_spawn(cmd, file_path, ref, segment, observer, meta, day, **_kwargs):
        if file_path == files[0]:
            process = timeout_process
        else:
            process = FakeProcess(0, delay=0.01)
            file_path.with_suffix(".jsonl").write_text("{}\n")
        return FakeManaged(process, ref=ref, log_path=tmp_path / "describe.log")

    original_check = sensor._check_segment_observed
    checks = []

    def record_check(file_path, error=None):
        checks.append((file_path, error))
        return original_check(file_path, error=error)

    monkeypatch.setattr(sensor, "_resolve_max_runtime", lambda _handler: 1)
    monkeypatch.setattr(sensor, "_spawn_managed_process", fake_spawn)
    monkeypatch.setattr(sensor, "_check_segment_observed", record_check)

    sensor._handle_callosum_message(
        {
            "tract": "observe",
            "event": "observing",
            "day": "20250101",
            "stream": "default",
            "segment": "143022_300",
            "files": [file_path.name for file_path in files],
        }
    )
    sensor.handler_pools["describe"].shutdown(wait=True)

    observed_events = [
        event
        for event in emitted_events
        if event.get("tract") == "observe" and event.get("event") == "observed"
    ]

    assert timeout_process.terminated is True
    assert checks == [
        (files[0], f"describe {WATCHDOG_TIMEOUT} after 1s"),
        (files[1], None),
        (files[2], None),
    ]
    assert len(observed_events) == 1
    assert observed_events[0].get("error") is True
    assert files[2].with_suffix(".jsonl").exists()


def test_run_handler_uses_handler_thread_name_prefix(tmp_path, monkeypatch):
    """Handler spawn happens in the long-lived handler worker thread."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    test_file = make_segment_file(tmp_path, "screen.webm")
    thread_names = []

    def fake_spawn(*_args, **_kwargs):
        thread_names.append(threading.current_thread().name)
        return FakeManaged(FakeProcess(0))

    monkeypatch.setattr(sensor, "_spawn_managed_process", fake_spawn)

    sensor._handle_file(test_file)
    sensor.handler_pools["describe"].shutdown(wait=True)

    assert thread_names
    assert thread_names[0].startswith("describe-worker")


def test_file_sensor_stop_during_spawn_gap_drains_worker(tmp_path, monkeypatch):
    """stop() handles a worker between spawn return and running append."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    test_file = make_segment_file(tmp_path, "screen.webm")
    spawn_started = threading.Event()
    release_spawn = threading.Event()
    process = FakeProcess(0)

    def fake_spawn(*_args, **_kwargs):
        spawn_started.set()
        assert release_spawn.wait(timeout=5)
        return FakeManaged(process)

    monkeypatch.setattr(sensor, "_spawn_managed_process", fake_spawn)

    sensor._handle_file(test_file)
    assert spawn_started.wait(timeout=5)
    stop_thread = threading.Thread(target=sensor.stop)
    stop_thread.start()
    time.sleep(0.05)
    release_spawn.set()
    stop_thread.join(timeout=10)

    assert not stop_thread.is_alive()
    assert process.terminated is True
    assert all(pool._shutdown for pool in sensor.handler_pools.values())


def test_process_day_survives_one_batch_worker_failure(tmp_path, monkeypatch, caplog):
    """One failing batch future does not abort the day."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    day_dir = tmp_path / "chronicle" / "20250101"
    segment_dir = day_dir / "default" / "143022_300"
    segment_dir.mkdir(parents=True)
    for name in ("good1.webm", "bad.webm", "good2.webm"):
        (segment_dir / name).write_text("video")

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    processed = []

    def fake_run(queued_item, *_args):
        if queued_item.file_path.name == "bad.webm":
            raise RuntimeError("boom")
        processed.append(queued_item.file_path.name)

    monkeypatch.setattr(sensor, "_run_handler", fake_run)
    caplog.set_level("INFO")

    sensor.process_day("20250101", max_jobs=2)

    assert set(processed) == {"good1.webm", "good2.webm"}
    assert "Batch worker failed for" in caplog.text
    assert "Batch processing complete" in caplog.text


def test_main_rejects_invalid_stream_filter(tmp_path, monkeypatch):
    from solstone.observe import sense

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(sense, "require_solstone", lambda: None)
    monkeypatch.setattr(
        sys,
        "argv",
        ["sense", "--day", "20250101", "--stream", "bad/stream"],
    )

    with pytest.raises(SystemExit) as exc_info:
        sense.main()

    assert exc_info.value.code == 2


def test_main_speakers_analyze_failure_prints_canonical_message_once(
    tmp_path, monkeypatch, capsys
):
    from solstone.observe import sense
    from solstone.think.speakers_analyze_installation import (
        speakers_analyze_repair_text,
    )

    message = (
        "Speakers-analyze installation is incomplete "
        f"(asset-missing: wespeaker). {speakers_analyze_repair_text()}"
    )

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(sense, "require_solstone", lambda: None)
    monkeypatch.setattr(sys, "argv", ["sense", "--day", "20250101"])

    def fail_generation(**_kwargs):
        raise RuntimeError(message)

    monkeypatch.setattr(
        "solstone.think.speakers_analyze_installation."
        "enter_speakers_analyze_generation",
        fail_generation,
    )

    with pytest.raises(SystemExit) as exc_info:
        sense.main()

    captured = capsys.readouterr()
    assert exc_info.value.code == 78
    assert captured.err.count("Speakers-analyze installation is incomplete") == 1
    assert message in captured.err


def test_main_live_generation_id_without_fd_still_prints_canonical_message_once(
    tmp_path, monkeypatch, capsys
):
    from solstone.observe import sense

    message = "speakers-analyze generation lease is already held"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setenv(
        "SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_ID",
        "live-generation",
    )
    monkeypatch.setenv("SOL_SUPERVISOR_SPAWNED", "1")
    monkeypatch.delenv("SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_FD", raising=False)
    monkeypatch.delenv(
        "SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_TOKEN",
        raising=False,
    )
    monkeypatch.setattr(sense, "require_solstone", lambda: None)
    monkeypatch.setattr(sys, "argv", ["sense", "--day", "20250101"])

    def fail_generation(**_kwargs):
        raise RuntimeError(message)

    monkeypatch.setattr(
        "solstone.think.speakers_analyze_installation."
        "enter_speakers_analyze_generation",
        fail_generation,
    )

    with pytest.raises(SystemExit) as exc_info:
        sense.main()

    captured = capsys.readouterr()
    assert exc_info.value.code == 78
    assert captured.err.count(message) == 1


def _registered_describe_commands(sensor: FileSensor) -> list[list[str]]:
    return [
        command
        for handler_name, command in sensor.handlers.values()
        if handler_name == "describe"
    ]


@pytest.mark.parametrize(
    ("argv", "configured", "expected_effective"),
    [
        (["journal sense"], 4, 4),
        (["journal sense", "--day", "20250101", "-j", "2"], 4, 4),
        (["journal sense", "--day", "20250101", "-j", "2"], 1, 2),
    ],
)
def test_main_registers_describe_with_policy_per_proc_jobs(
    tmp_path, monkeypatch, argv, configured, expected_effective
):
    from solstone.observe import sense

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(sense, "require_solstone", lambda: None)
    monkeypatch.setattr("sys.argv", argv)
    effective_calls = []
    observed_commands = []

    def fake_resolve_concurrency(self, handler_name):
        del self
        return configured if handler_name == "describe" else 1

    def fake_per_proc(effective_procs):
        effective_calls.append(effective_procs)
        return 9

    def record_watch(self):
        observed_commands.extend(_registered_describe_commands(self))
        self.stop()

    def record_day(self, *args, **kwargs):
        del args, kwargs
        observed_commands.extend(_registered_describe_commands(self))
        self.stop()

    monkeypatch.setattr(
        sense.FileSensor, "_resolve_concurrency", fake_resolve_concurrency
    )
    monkeypatch.setattr(fanout_policy, "describe_per_proc_jobs", fake_per_proc)
    monkeypatch.setattr(sense.FileSensor, "start", record_watch)
    monkeypatch.setattr(sense.FileSensor, "process_day", record_day)

    sense.main()

    assert effective_calls == [expected_effective]
    assert observed_commands
    assert all(command[-2:] == ["-j", "9"] for command in observed_commands)


def test_main_reprocess_screen_passes_stream_and_modality_filter(tmp_path, monkeypatch):
    from solstone.observe import sense

    calls = []
    installed = []

    class SensorStub:
        def __init__(self, *_args, **_kwargs):
            pass

        def register(self, *_args, **_kwargs):
            pass

        def _resolve_concurrency(self, _handler_name):
            return 1

        def process_day(self, *args, **kwargs):
            calls.append((args, kwargs))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(sense, "require_solstone", lambda: None)
    monkeypatch.setattr(sense, "FileSensor", SensorStub)
    monkeypatch.setattr(sense, "delete_outputs", lambda *_args, **_kwargs: [])
    monkeypatch.setattr(
        sense,
        "_install_batch_signal_handlers",
        lambda sensor: installed.append(sensor),
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "sense",
            "--day",
            "20250101",
            "--stream",
            "alpha",
            "--reprocess",
            "screen",
        ],
    )

    sense.main()

    assert len(installed) == 1
    assert calls == [
        (
            ("20250101",),
            {
                "max_jobs": 1,
                "segment_filter": None,
                "stream_filter": "alpha",
                "modality_filter": "screen",
            },
        )
    ]


def test_main_reprocess_all_keeps_modality_filter_unset(tmp_path, monkeypatch):
    from solstone.observe import sense

    calls = []
    installed = []

    class SensorStub:
        def __init__(self, *_args, **_kwargs):
            pass

        def register(self, *_args, **_kwargs):
            pass

        def _resolve_concurrency(self, _handler_name):
            return 1

        def process_day(self, *args, **kwargs):
            calls.append((args, kwargs))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(sense, "require_solstone", lambda: None)
    monkeypatch.setattr(sense, "FileSensor", SensorStub)
    monkeypatch.setattr(sense, "delete_outputs", lambda *_args, **_kwargs: [])
    monkeypatch.setattr(
        sense,
        "_install_batch_signal_handlers",
        lambda sensor: installed.append(sensor),
    )
    monkeypatch.setattr(
        sys,
        "argv",
        ["sense", "--day", "20250101", "--reprocess", "all"],
    )

    sense.main()

    assert len(installed) == 1
    assert calls[0][1]["modality_filter"] is None


def test_queue_wait_ms_reaches_child_env(tmp_path, monkeypatch):
    """The handler needs the queue wait sense measured; it cannot compute it itself."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sensor = FileSensor(tmp_path)
    test_file = make_segment_file(tmp_path, "audio.flac")
    captured = {}

    def fake_runner_spawn(cmd, ref, callosum, env, day):
        captured["env"] = env
        return FakeManaged(FakeProcess(0), ref=ref)

    monkeypatch.setattr(
        "solstone.observe.sense.RunnerManagedProcess.spawn", fake_runner_spawn
    )

    sensor._spawn_managed_process(
        ["journal", "transcribe", str(test_file)],
        test_file,
        "ref",
        "143022_300",
        None,
        None,
        "20250101",
        queue_wait_ms=4200,
    )
    assert captured["env"]["SOL_QUEUE_WAIT_MS"] == "4200"

    sensor._spawn_managed_process(
        ["journal", "transcribe", str(test_file)],
        test_file,
        "ref",
        "143022_300",
        None,
        None,
        "20250101",
    )
    assert "SOL_QUEUE_WAIT_MS" not in captured["env"]


def test_generation_env_reaches_event_and_batch_transcribe_children(
    tmp_path, monkeypatch
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    fd = os.open(tmp_path / "generation.lock", os.O_RDWR | os.O_CREAT, 0o600)
    monkeypatch.setenv(
        "SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_ID",
        "generation",
    )
    monkeypatch.setenv("SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_FD", str(fd))
    monkeypatch.setenv("SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_TOKEN", "123")
    sensor = FileSensor(tmp_path)
    test_file = make_segment_file(tmp_path, "audio.flac")
    captured: list[dict[str, str]] = []

    def fake_runner_spawn(cmd, ref, callosum, env, day):
        captured.append(env)
        return FakeManaged(FakeProcess(0), ref=ref)

    monkeypatch.setattr(
        "solstone.observe.sense.RunnerManagedProcess.spawn", fake_runner_spawn
    )

    try:
        sensor._spawn_managed_process(
            ["journal", "transcribe", str(test_file)],
            test_file,
            "event-ref",
            "143022_300",
            None,
            None,
            None,
        )
        sensor._spawn_managed_process(
            ["journal", "transcribe", str(test_file)],
            test_file,
            "batch-ref",
            "143022_300",
            None,
            None,
            "20250101",
        )
    finally:
        os.close(fd)

    assert [
        (
            env["SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_ID"],
            env["SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_FD"],
            env["SOL_SPEAKERS_ANALYZE_INSTALL_GENERATION_TOKEN"],
        )
        for env in captured
    ] == [("generation", str(fd), "123"), ("generation", str(fd), "123")]


def test_run_handler_passes_queue_wait_from_queued_at(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sensor = FileSensor(tmp_path)
    sensor.register("*.flac", "transcribe", ["journal", "transcribe", "{file}"])
    test_file = make_segment_file(tmp_path, "audio.flac")
    # queued_at is time.time()-based, so the wait must be measured with time.time().
    queued_item = QueuedItem(test_file, queued_at=time.time() - 2.0)
    sensor.queued_handlers["transcribe"].append(queued_item)
    captured = {}

    def fake_spawn(cmd, *_args, **kwargs):
        captured.update(kwargs)
        return FakeManaged(FakeProcess(0))

    monkeypatch.setattr(sensor, "_spawn_managed_process", fake_spawn)

    sensor._run_handler(
        queued_item,
        "transcribe",
        ["journal", "transcribe", "{file}"],
        "143022_300",
        "20250101",
        False,
    )

    assert 1900 <= captured["queue_wait_ms"] <= 3000


def test_memory_gate_wait_does_not_consume_handler_cap(
    tmp_path,
    monkeypatch,
):
    """The memory gate fires once and its wait does not eat into the handler cap."""
    import solstone.observe.sense as sense_module

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sensor = FileSensor(tmp_path)
    sensor.register("*.flac", "transcribe", ["journal", "transcribe", "{file}"])
    test_file = make_segment_file(tmp_path, "audio.flac")
    queued_item = QueuedItem(test_file)
    sensor.queued_handlers["transcribe"].append(queued_item)
    gate_calls = []
    wait_timeouts = []
    cmds = []
    monotonic_now = {"value": 0.0}

    class RecordingProcess(FakeProcess):
        def wait(self, timeout=None):
            wait_timeouts.append(timeout)
            return super().wait(timeout=timeout)

    def fake_gate(stage, **_kwargs):
        gate_calls.append(stage)
        monotonic_now["value"] += 100.0

    def fake_spawn(cmd, *_args, **_kwargs):
        cmds.append(cmd)
        return FakeManaged(RecordingProcess(0))

    monkeypatch.setattr(
        "solstone.observe.sense.admission.wait_for_memory_headroom",
        fake_gate,
    )
    monkeypatch.setattr(sensor, "_resolve_max_runtime", lambda _handler: 30)
    monkeypatch.setattr(
        sense_module,
        "time",
        _ClockShim(
            sense_module.time,
            monotonic=lambda: monotonic_now["value"],
        ),
    )
    monkeypatch.setattr(sensor, "_spawn_managed_process", fake_spawn)

    sensor._run_handler(
        queued_item,
        "transcribe",
        ["journal", "transcribe", "{file}"],
        "143022_300",
        "20250101",
        False,
    )

    assert gate_calls == ["transcribe"]
    # One attempt, one wait, at the full cap: the deadline is set after the gate.
    assert wait_timeouts == [30.0]
    assert len(cmds) == 1
    # The exit-134 --cpu fallback is gone; journal transcribe has no such flag.
    assert not any("--cpu" in cmd for cmd in cmds)


def test_delete_outputs_screen(tmp_path):
    """Test delete_outputs with screen type."""
    from solstone.observe.sense import delete_outputs

    # Create journal/day/stream/segment structure
    day_dir = tmp_path / "chronicle" / "20250101"
    segment_dir = day_dir / "default" / "143022_300"
    segment_dir.mkdir(parents=True)

    # Create source files and outputs
    (segment_dir / "center_DP-3_screen.webm").write_text("video")
    (segment_dir / "center_DP-3_screen.jsonl").write_text('{"raw": "test"}')
    (segment_dir / "audio.flac").write_text("audio")
    (segment_dir / "audio.jsonl").write_text('{"raw": "test"}')

    # Delete screen outputs
    deleted = delete_outputs(day_dir, "screen")

    assert len(deleted) == 1
    assert deleted[0].name == "center_DP-3_screen.jsonl"
    assert not (segment_dir / "center_DP-3_screen.jsonl").exists()
    assert (segment_dir / "audio.jsonl").exists()  # Audio untouched


def test_delete_outputs_audio(tmp_path):
    """Test delete_outputs with audio type."""
    from solstone.observe.sense import delete_outputs

    # Create journal/day/stream/segment structure
    day_dir = tmp_path / "chronicle" / "20250101"
    segment_dir = day_dir / "default" / "143022_300"
    segment_dir.mkdir(parents=True)

    # Create source files and outputs
    (segment_dir / "center_DP-3_screen.webm").write_text("video")
    (segment_dir / "center_DP-3_screen.jsonl").write_text('{"raw": "test"}')
    (segment_dir / "audio.flac").write_text("audio")
    (segment_dir / "audio.jsonl").write_text('{"raw": "test"}')

    # Delete audio outputs
    deleted = delete_outputs(day_dir, "audio")

    assert len(deleted) == 1
    assert deleted[0].name == "audio.jsonl"
    assert not (segment_dir / "audio.jsonl").exists()
    assert (segment_dir / "center_DP-3_screen.jsonl").exists()  # Screen untouched


def test_delete_outputs_dry_run(tmp_path):
    """Test delete_outputs with dry_run=True."""
    from solstone.observe.sense import delete_outputs

    # Create journal/day/stream/segment structure
    day_dir = tmp_path / "chronicle" / "20250101"
    segment_dir = day_dir / "default" / "143022_300"
    segment_dir.mkdir(parents=True)

    # Create source files and outputs
    (segment_dir / "screen.webm").write_text("video")
    (segment_dir / "screen.jsonl").write_text('{"raw": "test"}')

    # Dry run should return files but not delete
    deleted = delete_outputs(day_dir, "screen", dry_run=True)

    assert len(deleted) == 1
    assert (segment_dir / "screen.jsonl").exists()  # Still exists


def test_delete_outputs_segment_filter(tmp_path):
    """Test delete_outputs with segment filter."""
    from solstone.observe.sense import delete_outputs

    # Create journal/day/stream/segments structure
    day_dir = tmp_path / "chronicle" / "20250101"
    segment1 = day_dir / "default" / "143022_300"
    segment2 = day_dir / "default" / "150022_300"
    segment1.mkdir(parents=True)
    segment2.mkdir(parents=True)

    # Create outputs in both segments
    (segment1 / "screen.webm").write_text("video")
    (segment1 / "screen.jsonl").write_text('{"raw": "test"}')
    (segment2 / "screen.webm").write_text("video")
    (segment2 / "screen.jsonl").write_text('{"raw": "test"}')

    # Delete only from segment1
    deleted = delete_outputs(day_dir, "screen", segment_filter="143022_300")

    assert len(deleted) == 1
    assert not (segment1 / "screen.jsonl").exists()
    assert (segment2 / "screen.jsonl").exists()  # Other segment untouched


def test_delete_outputs_stream_filter(tmp_path):
    """Test delete_outputs with stream filter."""
    from solstone.observe.sense import delete_outputs

    day_dir = tmp_path / "chronicle" / "20250101"
    alpha = day_dir / "alpha" / "143022_300"
    bravo = day_dir / "bravo" / "143022_300"
    alpha.mkdir(parents=True)
    bravo.mkdir(parents=True)

    (alpha / "screen.webm").write_text("video")
    (alpha / "screen.jsonl").write_text('{"raw": "test"}')
    (bravo / "screen.webm").write_text("video")
    (bravo / "screen.jsonl").write_text('{"raw": "test"}')

    deleted = delete_outputs(day_dir, "screen", stream_filter="alpha")

    assert [path.parent.parent.name for path in deleted] == ["alpha"]
    assert not (alpha / "screen.jsonl").exists()
    assert (bravo / "screen.jsonl").exists()


def test_delete_outputs_all_keeps_modality_behavior_with_stream_filter(tmp_path):
    """Test reprocess_type=all still deletes audio and screen outputs."""
    from solstone.observe.sense import delete_outputs

    day_dir = tmp_path / "chronicle" / "20250101"
    alpha = day_dir / "alpha" / "143022_300"
    bravo = day_dir / "bravo" / "143022_300"
    alpha.mkdir(parents=True)
    bravo.mkdir(parents=True)

    for segment_dir in (alpha, bravo):
        (segment_dir / "screen.webm").write_text("video")
        (segment_dir / "screen.jsonl").write_text('{"raw": "test"}')
        (segment_dir / "audio.flac").write_text("audio")
        (segment_dir / "audio.jsonl").write_text('{"raw": "test"}')

    deleted = delete_outputs(day_dir, "all", stream_filter="alpha")

    assert {path.name for path in deleted} == {"screen.jsonl", "audio.jsonl"}
    assert not (alpha / "screen.jsonl").exists()
    assert not (alpha / "audio.jsonl").exists()
    assert (bravo / "screen.jsonl").exists()
    assert (bravo / "audio.jsonl").exists()
