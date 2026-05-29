# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the file-based Cortex agent manager."""

import json
import os
import signal
import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from solstone.think.models import GPT_5


class MockPipe:
    """Mock for subprocess stdout/stderr that supports context manager protocol."""

    def __init__(self, lines: list[str]):
        self._lines = lines
        self._iter = None

    def __enter__(self):
        self._iter = iter(self._lines)
        return self

    def __exit__(self, *args):
        pass

    def __iter__(self):
        return self._iter or iter(self._lines)

    def __next__(self):
        if self._iter is None:
            self._iter = iter(self._lines)
        return next(self._iter)


@pytest.fixture
def mock_journal(tmp_path, monkeypatch):
    """Set up a temporary journal directory."""
    journal_path = tmp_path / "journal"
    journal_path.mkdir()
    agents_path = journal_path / "talents"
    agents_path.mkdir()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal_path))
    return journal_path


@pytest.fixture
def cortex_service(mock_journal):
    """Create a CortexService instance for testing."""
    from solstone.think.cortex import CortexService

    return CortexService(str(mock_journal))


def test_agent_process_creation():
    """Test TalentProcess class initialization and methods."""
    from solstone.think.cortex import TalentProcess

    mock_process = MagicMock()
    mock_process.poll.return_value = None  # Running
    mock_process.pid = 12345

    log_path = Path("/tmp/test.jsonl")
    agent = TalentProcess("123456789", mock_process, log_path)

    assert agent.use_id == "123456789"
    assert agent.process == mock_process
    assert agent.log_path == log_path
    assert agent.is_running() is True

    # Test stop
    agent.stop()
    mock_process.terminate.assert_called_once()
    assert agent.stop_event.is_set()


def test_cortex_service_initialization(cortex_service, mock_journal):
    """Test CortexService initialization."""
    assert cortex_service.journal_path == mock_journal
    assert cortex_service.talents_dir == mock_journal / "talents"
    assert cortex_service.running_uses == {}
    assert cortex_service.talents_dir.exists()


def test_cortex_installs_sigterm_handler():
    from solstone.think import cortex

    previous = signal.getsignal(signal.SIGTERM)
    signal.signal(signal.SIGTERM, signal.SIG_DFL)
    try:
        cortex._install_sigterm_handler(MagicMock())
        assert signal.getsignal(signal.SIGTERM) is not signal.SIG_DFL
    finally:
        signal.signal(signal.SIGTERM, previous)


@patch("solstone.think.cortex.subprocess.Popen")
@patch("solstone.think.cortex.threading.Thread")
@patch("solstone.think.cortex.threading.Timer")
def test_spawn_subprocess(
    mock_timer, mock_thread, mock_popen, cortex_service, mock_journal
):
    """Test spawning an agent subprocess."""
    mock_process = MagicMock()
    mock_process.pid = 12345
    mock_process.poll.return_value = None
    mock_process.stdin = MagicMock()
    mock_process.stdout = MagicMock()
    mock_process.stderr = MagicMock()
    mock_popen.return_value = mock_process

    # Setup mock timer
    mock_timer_instance = MagicMock()
    mock_timer.return_value = mock_timer_instance

    use_id = "123456789"
    file_path = mock_journal / "talents" / f"{use_id}_active.jsonl"

    request = {
        "event": "request",
        "ts": 123456789,
        "prompt": "Test prompt",
        "provider": "openai",
        "name": "chat",
        "model": GPT_5,
    }

    cortex_service._spawn_subprocess(
        use_id,
        file_path,
        request,
        [sys.executable, "-m", "solstone.think.talents"],
        "talent",
    )

    # Check subprocess was called
    mock_popen.assert_called_once()
    call_args = mock_popen.call_args
    assert call_args[0][0] == [sys.executable, "-m", "solstone.think.talents"]
    assert call_args[1]["stdin"] is not None
    assert call_args[1]["stdout"] is not None
    assert call_args[1]["stderr"] is not None
    assert call_args[1]["process_group"] == 0

    # Check NDJSON was written to stdin
    mock_process.stdin.write.assert_called_once()
    written_data = mock_process.stdin.write.call_args[0][0]
    ndjson = json.loads(written_data.strip())
    assert ndjson["event"] == "request"
    assert ndjson["prompt"] == "Test prompt"
    assert ndjson["provider"] == "openai"
    assert ndjson["name"] == "chat"
    assert ndjson["model"] == GPT_5

    # Check stdin was closed
    mock_process.stdin.close.assert_called_once()

    # Check agent was tracked
    assert use_id in cortex_service.running_uses
    agent = cortex_service.running_uses[use_id]
    assert agent.use_id == use_id
    assert agent.log_path == file_path

    # Check monitoring threads were started
    assert mock_thread.call_count == 2  # stdout and stderr

    # Check timer was created and started
    mock_timer.assert_called_once()
    mock_timer_instance.start.assert_called_once()


@patch("solstone.think.cortex.subprocess.Popen")
@patch("solstone.think.cortex.threading.Thread")
@patch("solstone.think.cortex.threading.Timer")
def test_spawn_generator_via_subprocess(
    mock_timer, mock_thread, mock_popen, cortex_service, mock_journal
):
    """Test spawning a generator subprocess via _spawn_subprocess."""
    mock_process = MagicMock()
    mock_process.pid = 54321
    mock_process.poll.return_value = None
    mock_process.stdin = MagicMock()
    mock_process.stdout = MagicMock()
    mock_process.stderr = MagicMock()
    mock_popen.return_value = mock_process

    # Setup mock timer
    mock_timer_instance = MagicMock()
    mock_timer.return_value = mock_timer_instance

    use_id = "987654321"
    file_path = mock_journal / "talents" / f"{use_id}_active.jsonl"

    # Generator config has "output" instead of "tools"
    config = {
        "event": "request",
        "ts": 987654321,
        "name": "work",
        "day": "20240101",
        "output": "md",
    }

    # Generators route through _spawn_subprocess
    cortex_service._spawn_subprocess(
        use_id,
        file_path,
        config,
        [sys.executable, "-m", "solstone.think.talents"],
        "talent",
    )

    # Check subprocess was called with agents command (generators route through agents)
    mock_popen.assert_called_once()
    call_args = mock_popen.call_args
    assert call_args[0][0] == [sys.executable, "-m", "solstone.think.talents"]
    assert call_args[1]["stdin"] is not None
    assert call_args[1]["stdout"] is not None
    assert call_args[1]["stderr"] is not None

    # Check NDJSON was written to stdin
    mock_process.stdin.write.assert_called_once()
    written_data = mock_process.stdin.write.call_args[0][0]
    ndjson = json.loads(written_data.strip())
    assert ndjson["event"] == "request"
    assert ndjson["name"] == "work"
    assert ndjson["day"] == "20240101"
    assert ndjson["output"] == "md"

    # Check stdin was closed
    mock_process.stdin.close.assert_called_once()

    # Check generator was tracked
    assert use_id in cortex_service.running_uses
    agent = cortex_service.running_uses[use_id]
    assert agent.use_id == use_id
    assert agent.log_path == file_path

    # Check monitoring threads were started
    assert mock_thread.call_count == 2  # stdout and stderr

    # Check timer was created and started
    mock_timer.assert_called_once()
    mock_timer_instance.start.assert_called_once()


@patch("solstone.think.talent.get_talent")
@patch("solstone.think.cortex.subprocess.Popen")
@patch("solstone.think.cortex.threading.Thread")
@patch("solstone.think.cortex.threading.Timer")
def test_spawn_subprocess_uses_cwd_from_talent(
    mock_timer,
    mock_thread,
    mock_popen,
    mock_get_agent,
    cortex_service,
    mock_journal,
):
    mock_process = MagicMock()
    mock_process.pid = 24680
    mock_process.poll.return_value = None
    mock_process.stdin = MagicMock()
    mock_process.stdout = MagicMock()
    mock_process.stderr = MagicMock()
    mock_popen.return_value = mock_process
    mock_get_agent.return_value = {"type": "cogitate", "cwd": "journal"}

    mock_timer_instance = MagicMock()
    mock_timer.return_value = mock_timer_instance

    use_id = "24680"
    file_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    request = {
        "event": "request",
        "ts": 24680,
        "prompt": "Test prompt",
        "provider": "openai",
        "name": "chat",
        "model": GPT_5,
    }

    cortex_service._spawn_subprocess(
        use_id,
        file_path,
        request,
        [sys.executable, "-m", "solstone.think.talents"],
        "talent",
    )

    assert mock_popen.call_args.kwargs["cwd"] == str(mock_journal)


@patch("solstone.think.talent.get_talent")
@patch("solstone.think.cortex.subprocess.Popen")
@patch("solstone.think.cortex.threading.Thread")
@patch("solstone.think.cortex.threading.Timer")
def test_spawn_subprocess_skips_cwd_for_generate(
    mock_timer,
    mock_thread,
    mock_popen,
    mock_get_agent,
    cortex_service,
    mock_journal,
):
    mock_process = MagicMock()
    mock_process.pid = 13579
    mock_process.poll.return_value = None
    mock_process.stdin = MagicMock()
    mock_process.stdout = MagicMock()
    mock_process.stderr = MagicMock()
    mock_popen.return_value = mock_process
    mock_get_agent.return_value = {"type": "generate"}

    mock_timer_instance = MagicMock()
    mock_timer.return_value = mock_timer_instance

    use_id = "13579"
    file_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    request = {
        "event": "request",
        "ts": 13579,
        "name": "decisions",
        "day": "20240101",
        "output": "md",
    }

    cortex_service._spawn_subprocess(
        use_id,
        file_path,
        request,
        [sys.executable, "-m", "solstone.think.talents"],
        "talent",
    )

    assert mock_popen.call_args.kwargs["cwd"] is None


@pytest.mark.parametrize(
    ("config_timeout", "talent_meta", "expected_timeout"),
    [
        (100, {"type": "cogitate", "cwd": "journal", "timeout_seconds": 200}, 100),
        (None, {"type": "cogitate", "cwd": "journal", "timeout_seconds": 200}, 200),
        (None, {}, 600),
    ],
)
@patch("solstone.think.talent.get_talent")
@patch("solstone.think.cortex.subprocess.Popen")
@patch("solstone.think.cortex.threading.Thread")
@patch("solstone.think.cortex.threading.Timer")
def test_spawn_subprocess_timeout_precedence(
    mock_timer,
    mock_thread,
    mock_popen,
    mock_get_agent,
    cortex_service,
    mock_journal,
    config_timeout,
    talent_meta,
    expected_timeout,
):
    mock_process = MagicMock()
    mock_process.pid = 97531
    mock_process.poll.return_value = None
    mock_process.stdin = MagicMock()
    mock_process.stdout = MagicMock()
    mock_process.stderr = MagicMock()
    mock_popen.return_value = mock_process
    mock_get_agent.return_value = talent_meta

    mock_timer_instance = MagicMock()
    mock_timer.return_value = mock_timer_instance

    use_id = "97531"
    file_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    request = {
        "event": "request",
        "ts": 97531,
        "name": "chat",
        "prompt": "Test prompt",
    }
    if config_timeout is not None:
        request["timeout_seconds"] = config_timeout

    cortex_service._spawn_subprocess(
        use_id,
        file_path,
        request,
        [sys.executable, "-m", "solstone.think.talents"],
        "talent",
    )

    assert mock_timer.call_args.args[0] == expected_timeout


@patch("solstone.think.talent.get_talent")
@patch("solstone.think.cortex.subprocess.Popen")
@patch("solstone.think.cortex.threading.Thread")
@patch("solstone.think.cortex.threading.Timer")
def test_spawn_subprocess_skips_talent_meta_for_generate(
    mock_timer,
    mock_thread,
    mock_popen,
    mock_get_agent,
    cortex_service,
    mock_journal,
):
    mock_process = MagicMock()
    mock_process.pid = 86420
    mock_process.poll.return_value = None
    mock_process.stdin = MagicMock()
    mock_process.stdout = MagicMock()
    mock_process.stderr = MagicMock()
    mock_popen.return_value = mock_process

    mock_timer_instance = MagicMock()
    mock_timer.return_value = mock_timer_instance

    use_id = "86420"
    file_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    request = {
        "event": "request",
        "ts": 86420,
        "name": "chat",
        "prompt": "Test prompt",
    }

    cortex_service._spawn_subprocess(
        use_id,
        file_path,
        request,
        [sys.executable, "-m", "solstone.think.talents"],
        "generate",
    )

    mock_get_agent.assert_not_called()
    assert mock_timer.call_args.args[0] == 600


def test_monitor_stdout_json_events(cortex_service, mock_journal):
    """Test monitoring stdout with JSON events."""
    from io import StringIO

    from solstone.think.cortex import TalentProcess

    use_id = "123456789"
    log_path = mock_journal / "talents" / f"{use_id}_active.jsonl"

    mock_process = MagicMock()
    mock_process.poll.return_value = 0  # Process exits
    mock_process.stdout = StringIO(
        '{"event": "start", "ts": 1234567890}\n'
        '{"event": "finish", "ts": 1234567891, "result": "Done"}\n'
    )

    agent = TalentProcess(use_id, mock_process, log_path)
    cortex_service.running_uses[use_id] = agent
    cortex_service.use_requests[use_id] = {
        "name": "weekly_reflection",
        "day": "20260308",
    }

    with patch.object(cortex_service, "_complete_use_file") as mock_complete:
        cortex_service._monitor_stdout(agent)

        # Check events were written to file
        assert log_path.exists()
        lines = log_path.read_text().strip().split("\n")
        assert len(lines) == 2
        start_event = json.loads(lines[0])
        finish_event = json.loads(lines[1])
        assert start_event["event"] == "start"
        assert start_event["name"] == "weekly_reflection"
        assert start_event["day"] == "20260308"
        assert finish_event["event"] == "finish"
        assert finish_event["name"] == "weekly_reflection"
        assert finish_event["day"] == "20260308"

        # Check file was completed
        mock_complete.assert_called_once_with(use_id, log_path)

    # Check agent was removed
    assert use_id not in cortex_service.running_uses


def test_monitor_stdout_non_json_output(cortex_service, mock_journal):
    """Test monitoring stdout with non-JSON output."""
    from io import StringIO

    from solstone.think.cortex import TalentProcess

    use_id = "123456789"
    log_path = mock_journal / "talents" / f"{use_id}_active.jsonl"

    mock_process = MagicMock()
    mock_process.poll.return_value = 0
    mock_process.stdout = StringIO(
        'Plain text output\n{"event": "finish", "ts": 1234567890}\n'
    )

    agent = TalentProcess(use_id, mock_process, log_path)
    cortex_service.running_uses[use_id] = agent

    with patch.object(cortex_service, "_complete_use_file"):
        cortex_service._monitor_stdout(agent)

        # Check info event was created for non-JSON
        lines = log_path.read_text().strip().split("\n")
        assert len(lines) == 2

        info_event = json.loads(lines[0])
        assert info_event["event"] == "info"
        assert info_event["message"] == "Plain text output"
        assert "ts" in info_event


def test_monitor_stdout_no_finish_event(cortex_service, mock_journal):
    """Test monitoring stdout when process exits without finish event."""
    from io import StringIO

    from solstone.think.cortex import TalentProcess

    use_id = "123456789"
    log_path = mock_journal / "talents" / f"{use_id}_active.jsonl"

    mock_process = MagicMock()
    mock_process.wait.return_value = 1  # Non-zero exit
    mock_process.stdout = StringIO('{"event": "start", "ts": 1234567890}\n')

    agent = TalentProcess(use_id, mock_process, log_path)
    cortex_service.running_uses[use_id] = agent

    with patch.object(cortex_service, "_complete_use_file"):
        cortex_service._monitor_stdout(agent)

        # Check error event was added
        lines = log_path.read_text().strip().split("\n")
        assert len(lines) == 2

        error_event = json.loads(lines[1])
        assert error_event["event"] == "error"
        assert "exit_code" in error_event
        assert error_event["exit_code"] == 1


def test_monitor_stderr(cortex_service, mock_journal):
    """Test monitoring stderr for errors."""
    from io import StringIO

    from solstone.think.cortex import TalentProcess

    use_id = "123456789"
    log_path = mock_journal / "talents" / f"{use_id}_active.jsonl"

    mock_process = MagicMock()
    mock_process.poll.return_value = 1  # Error exit
    mock_process.stderr = StringIO(
        "Error: Something went wrong\nStack trace line 1\nStack trace line 2\n"
    )

    agent = TalentProcess(use_id, mock_process, log_path)

    cortex_service._monitor_stderr(agent)

    # Check error event was written
    assert log_path.exists()
    lines = log_path.read_text().strip().split("\n")
    assert len(lines) == 1

    error_event = json.loads(lines[0])
    assert error_event["event"] == "error"
    assert "trace" in error_event
    assert "Error: Something went wrong" in error_event["trace"]
    assert error_event["exit_code"] == 1


def test_has_finish_event(cortex_service, mock_journal):
    """Test checking for finish event in JSONL file."""
    file_path = mock_journal / "talents" / "test.jsonl"

    # File with finish event
    file_path.write_text(
        '{"event": "start", "ts": 123}\n{"event": "finish", "ts": 124}\n'
    )
    assert cortex_service._has_finish_event(file_path) is True

    # File with error event
    file_path.write_text(
        '{"event": "start", "ts": 123}\n{"event": "error", "ts": 124}\n'
    )
    assert cortex_service._has_finish_event(file_path) is True

    # File without finish/error
    file_path.write_text('{"event": "start", "ts": 123}\n')
    assert cortex_service._has_finish_event(file_path) is False

    # Empty file
    file_path.write_text("")
    assert cortex_service._has_finish_event(file_path) is False


def test_complete_use_file(cortex_service, mock_journal):
    """Test completing an agent file (rename from active to completed)."""
    use_id = "123456789"
    unified_dir = mock_journal / "talents" / "chat"
    unified_dir.mkdir()
    active_path = unified_dir / f"{use_id}_active.jsonl"
    active_path.touch()
    cortex_service.use_requests[use_id] = {"name": "chat", "use_id": use_id}

    cortex_service._complete_use_file(use_id, active_path)

    # Check file was renamed
    assert not active_path.exists()
    completed_path = unified_dir / f"{use_id}.jsonl"
    assert completed_path.exists()
    symlink_path = mock_journal / "talents" / "chat.log"
    assert symlink_path.is_symlink()
    assert os.readlink(symlink_path) == f"chat/{use_id}.jsonl"


def test_complete_use_file_replaces_symlink(cortex_service, mock_journal):
    """Test completing agent file replaces convenience symlink for same name."""
    unified_dir = mock_journal / "talents" / "chat"
    unified_dir.mkdir()

    first_agent_id = "111"
    first_active_path = unified_dir / f"{first_agent_id}_active.jsonl"
    first_active_path.touch()
    cortex_service.use_requests[first_agent_id] = {"name": "chat"}

    cortex_service._complete_use_file(first_agent_id, first_active_path)

    second_agent_id = "222"
    second_active_path = unified_dir / f"{second_agent_id}_active.jsonl"
    second_active_path.touch()
    cortex_service.use_requests[second_agent_id] = {"name": "chat"}

    cortex_service._complete_use_file(second_agent_id, second_active_path)

    symlink_path = mock_journal / "talents" / "chat.log"
    assert symlink_path.is_symlink()
    assert os.readlink(symlink_path) == f"chat/{second_agent_id}.jsonl"


def test_complete_use_file_colon_name(cortex_service, mock_journal):
    """Test completing agent file sanitizes colon in convenience symlink name."""
    use_id = "123456789"
    entities_dir = mock_journal / "talents" / "entities--entity_assist"
    entities_dir.mkdir()
    active_path = entities_dir / f"{use_id}_active.jsonl"
    active_path.touch()
    cortex_service.use_requests[use_id] = {"name": "entities:entity_assist"}

    cortex_service._complete_use_file(use_id, active_path)

    symlink_path = mock_journal / "talents" / "entities--entity_assist.log"
    assert symlink_path.is_symlink()
    assert os.readlink(symlink_path) == f"entities--entity_assist/{use_id}.jsonl"


def test_complete_use_file_no_name(cortex_service, mock_journal):
    """Test completing agent file skips symlink when request name is missing."""
    use_id = "123456789"
    active_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    active_path.touch()

    cortex_service._complete_use_file(use_id, active_path)

    completed_path = mock_journal / "talents" / f"{use_id}.jsonl"
    assert completed_path.exists()
    assert not any(path.is_symlink() for path in (mock_journal / "talents").iterdir())


def test_write_error_and_complete(cortex_service, mock_journal):
    """Test writing error and completing file."""
    use_id = "123456789"
    file_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    file_path.touch()

    cortex_service._write_error_and_complete(file_path, "Test error message")

    # Check error was written
    completed_path = mock_journal / "talents" / f"{use_id}.jsonl"
    assert completed_path.exists()
    assert not file_path.exists()

    content = completed_path.read_text()
    error_event = json.loads(content)
    assert error_event["event"] == "error"
    assert error_event["error"] == "Test error message"
    assert "ts" in error_event


def test_get_status(cortex_service):
    """Test getting service status."""
    from solstone.think.cortex import TalentProcess

    # Empty status
    status = cortex_service.get_status()
    assert status["running_uses"] == 0
    assert status["use_ids"] == []

    # Add running agents
    mock_process = MagicMock()
    agent1 = TalentProcess("111", mock_process, Path("/tmp/1.jsonl"))
    agent2 = TalentProcess("222", mock_process, Path("/tmp/2.jsonl"))

    cortex_service.running_uses["111"] = agent1
    cortex_service.running_uses["222"] = agent2

    status = cortex_service.get_status()
    assert status["running_uses"] == 2
    assert set(status["use_ids"]) == {"111", "222"}


def test_monitor_stdout_finish_prefers_model_version(cortex_service, mock_journal):
    """Test finish usage model_version is preferred for token logging."""
    from solstone.think.cortex import TalentProcess

    use_id = "model_version_test"
    active_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    cortex_service.use_requests = {
        use_id: {
            "event": "request",
            "prompt": "test",
            "name": "test_agent",
            "model": "claude-haiku-4-5",
        }
    }

    mock_process = MagicMock()
    mock_stdout = [
        '{"event": "start", "ts": 1000}\n',
        json.dumps(
            {
                "event": "finish",
                "ts": 2000,
                "result": "X",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "total_tokens": 15,
                    "model_version": "claude-haiku-4-5-20251001",
                },
            }
        )
        + "\n",
    ]
    mock_process.stdout = MockPipe(mock_stdout)
    mock_process.wait.return_value = 0

    agent = TalentProcess(use_id, mock_process, active_path)

    with patch("solstone.think.models.log_token_usage") as mock_log_token_usage:
        with patch.object(cortex_service, "_complete_use_file"):
            cortex_service._monitor_stdout(agent)

    assert mock_log_token_usage.call_args.kwargs["model"] == (
        "claude-haiku-4-5-20251001"
    )


def test_monitor_stdout_finish_falls_back_to_request_model(
    cortex_service, mock_journal
):
    """Test finish usage without model_version uses request model for token logging."""
    from solstone.think.cortex import TalentProcess

    use_id = "request_model_test"
    active_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    cortex_service.use_requests = {
        use_id: {
            "event": "request",
            "prompt": "test",
            "name": "test_agent",
            "model": "claude-haiku-4-5",
        }
    }

    mock_process = MagicMock()
    mock_stdout = [
        '{"event": "start", "ts": 1000}\n',
        json.dumps(
            {
                "event": "finish",
                "ts": 2000,
                "result": "X",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "total_tokens": 15,
                },
            }
        )
        + "\n",
    ]
    mock_process.stdout = MockPipe(mock_stdout)
    mock_process.wait.return_value = 0

    agent = TalentProcess(use_id, mock_process, active_path)

    with patch("solstone.think.models.log_token_usage") as mock_log_token_usage:
        with patch.object(cortex_service, "_complete_use_file"):
            cortex_service._monitor_stdout(agent)

    assert mock_log_token_usage.call_args.kwargs["model"] == "claude-haiku-4-5"


def test_recover_orphaned_uses(cortex_service, mock_journal):
    """Test recovery of orphaned active agent files."""
    # Create orphaned active files
    talents_dir = mock_journal / "talents"
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir()
    agent1_active = unified_dir / "111_active.jsonl"
    agent2_active = unified_dir / "222_active.jsonl"

    agent1_active.write_text('{"event": "start", "ts": 1000}\n')
    agent2_active.write_text('{"event": "start", "ts": 2000}\n')

    active_files = [agent1_active, agent2_active]
    cortex_service._recover_orphaned_uses(active_files)

    # Check active files were renamed to completed
    assert not agent1_active.exists()
    assert not agent2_active.exists()
    assert (unified_dir / "111.jsonl").exists()
    assert (unified_dir / "222.jsonl").exists()

    # Check error events were appended
    content1 = (unified_dir / "111.jsonl").read_text()
    lines1 = content1.strip().split("\n")
    assert len(lines1) == 2
    error_event = json.loads(lines1[1])
    assert error_event["event"] == "error"
    assert "Recovered" in error_event["error"]
    assert error_event["use_id"] == "111"

    content2 = (unified_dir / "222.jsonl").read_text()
    lines2 = content2.strip().split("\n")
    assert len(lines2) == 2
    assert json.loads(lines2[1])["event"] == "error"
