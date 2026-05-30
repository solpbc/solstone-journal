# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for observe.sense module."""

import signal
import sys
import tempfile
import threading
import time
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from solstone.observe.sense import FileSensor, HandlerProcess, QueuedItem
from solstone.think.runner import DailyLogWriter as ProcessLogWriter
from solstone.think.runner import _format_log_line


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

        assert "*.webm" in sensor.handlers
        assert "*.flac" in sensor.handlers
        assert sensor.handlers["*.webm"][0] == "describe"
        assert sensor.handlers["*.flac"][0] == "transcribe"


def test_file_sensor_match_pattern():
    """Test pattern matching logic.

    Files are expected to be in segment directories: journal/YYYYMMDD/stream/HHMMSS_LEN/file.ext
    """
    with tempfile.TemporaryDirectory() as tmpdir:
        # Create journal/day/stream/segment structure
        journal_dir = Path(tmpdir)
        day_dir = journal_dir / "chronicle" / "20250101"
        segment_dir = day_dir / "default" / "123456_300"
        segment_dir.mkdir(parents=True)

        sensor = FileSensor(journal_dir)
        sensor.register("*.webm", "describe", ["echo", "{file}"])
        sensor.register("*.flac", "transcribe", ["cat", "{file}"])
        sensor.register("*.mp3", "transcribe", ["cat", "{file}"])

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

        # Should not match - wrong extension
        txt_file = segment_dir / "test.txt"
        assert sensor._match_pattern(txt_file) is None

        # Should not match - file in day root (not in segment dir)
        day_root_file = day_dir / "orphan.webm"
        assert sensor._match_pattern(day_root_file) is None

        # Should not match - jsonl output file
        jsonl_file = segment_dir / "audio.jsonl"
        assert sensor._match_pattern(jsonl_file) is None


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


def test_file_sensor_spawn_handler(tmp_path, monkeypatch):
    """Test spawning handler process."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["echo", "{file}"])
    test_file = make_segment_file(tmp_path, "screen.webm")
    log_path = tmp_path / "chronicle" / "20250101" / "health" / "test_echo.log"

    def fake_spawn(cmd, *_args):
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

    def fake_spawn(cmd, file_path, ref, segment, observer, meta, day):
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


def test_run_handler_uses_handler_thread_name_prefix(tmp_path, monkeypatch):
    """Handler spawn happens in the long-lived handler worker thread."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sensor = FileSensor(tmp_path)
    sensor.register("*.webm", "describe", ["journal", "describe", "{file}"])
    test_file = make_segment_file(tmp_path, "screen.webm")
    thread_names = []

    def fake_spawn(*_args):
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

    def fake_spawn(*_args):
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


def test_main_reprocess_screen_passes_stream_and_modality_filter(tmp_path, monkeypatch):
    from solstone.observe import sense

    calls = []

    class SensorStub:
        def __init__(self, *_args, **_kwargs):
            pass

        def register(self, *_args, **_kwargs):
            pass

        def process_day(self, *args, **kwargs):
            calls.append((args, kwargs))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(sense, "require_solstone", lambda: None)
    monkeypatch.setattr(sense, "FileSensor", SensorStub)
    monkeypatch.setattr(sense, "delete_outputs", lambda *_args, **_kwargs: [])
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

    class SensorStub:
        def __init__(self, *_args, **_kwargs):
            pass

        def register(self, *_args, **_kwargs):
            pass

        def process_day(self, *args, **kwargs):
            calls.append((args, kwargs))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(sense, "require_solstone", lambda: None)
    monkeypatch.setattr(sense, "FileSensor", SensorStub)
    monkeypatch.setattr(sense, "delete_outputs", lambda *_args, **_kwargs: [])
    monkeypatch.setattr(
        sys,
        "argv",
        ["sense", "--day", "20250101", "--reprocess", "all"],
    )

    sense.main()

    assert calls[0][1]["modality_filter"] is None


def test_transcribe_cpu_fallback_stays_in_same_worker_thread(tmp_path, monkeypatch):
    """The exit-134 retry is spawned by the same worker thread."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    sensor = FileSensor(tmp_path)
    sensor.register("*.flac", "transcribe", ["journal", "transcribe", "{file}"])
    test_file = make_segment_file(tmp_path, "audio.flac")
    first_started = threading.Event()
    first_release = threading.Event()
    second_started = threading.Event()
    second_release = threading.Event()
    idents = []
    cmds = []

    class ControlledProcess(FakeProcess):
        def __init__(self, exit_code, started, release):
            super().__init__(exit_code)
            self.started = started
            self.release = release

        def wait(self, timeout=None):
            self.started.set()
            assert self.release.wait(timeout=5)
            self.returncode = self.exit_code
            return self.exit_code

    processes = [
        ControlledProcess(134, first_started, first_release),
        ControlledProcess(0, second_started, second_release),
    ]

    def fake_spawn(cmd, *_args):
        cmds.append(cmd)
        idents.append(threading.current_thread().ident)
        return FakeManaged(processes.pop(0))

    original_check = sensor._check_segment_observed
    checks = []

    def record_check(file_path, error=None):
        checks.append((file_path, error))
        return original_check(file_path, error=error)

    monkeypatch.setattr(sensor, "_spawn_managed_process", fake_spawn)
    monkeypatch.setattr(sensor, "_check_segment_observed", record_check)

    sensor._handle_file(test_file)
    assert first_started.wait(timeout=5)
    assert len(sensor.running_handlers["transcribe"]) == 1
    first_release.set()
    assert second_started.wait(timeout=5)
    assert len(sensor.running_handlers["transcribe"]) == 1
    second_release.set()
    sensor.handler_pools["transcribe"].shutdown(wait=True)

    assert idents[0] == idents[1]
    assert "--cpu" not in cmds[0]
    assert "--cpu" in cmds[1]
    assert checks == [(test_file, None)]


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
