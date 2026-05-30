# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for cortex_client module with Callosum."""

import json
import shutil
import tempfile
import threading
import time
from pathlib import Path

import pytest

from solstone.think.callosum import CallosumConnection, CallosumServer
from solstone.think.cortex_client import (
    CortexSpawnUnavailable,
    cortex_request,
    cortex_uses,
    get_use_end_state,
    get_use_log_status,
    read_use_provider_model,
    wait_for_uses,
)
from solstone.think.models import GPT_5
from solstone.think.utils import now_ms


@pytest.fixture
def callosum_server(monkeypatch):
    """Start a Callosum server for testing.

    Uses a short temp path in /tmp to avoid Unix socket path length limits
    (~104 chars on macOS). pytest's tmp_path creates paths that are too long.
    """
    # Create short temp dir to avoid Unix socket path length limits
    tmp_dir = tempfile.mkdtemp(dir="/tmp", prefix="callosum_")
    tmp_path = Path(tmp_dir)

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "talents").mkdir(parents=True, exist_ok=True)

    server = CallosumServer()
    server_thread = threading.Thread(target=server.start, daemon=True)
    server_thread.start()

    # Wait for server to be ready
    socket_path = tmp_path / "health" / "callosum.sock"
    for _ in range(50):
        if socket_path.exists():
            break
        time.sleep(0.1)
    else:
        pytest.fail("Callosum server did not start in time")

    yield tmp_path

    server.stop()
    server_thread.join(timeout=2)
    shutil.rmtree(tmp_dir, ignore_errors=True)


@pytest.fixture
def callosum_listener(callosum_server):
    """Provide a CallosumConnection listener that collects received messages.

    Yields (messages, listener) where messages is a list that accumulates
    all broadcast messages received during the test.
    """
    messages = []

    def callback(msg):
        messages.append(msg)

    listener = CallosumConnection()
    listener.start(callback=callback)
    time.sleep(0.1)  # Allow connection to establish

    yield messages

    listener.stop()


def test_cortex_request_broadcasts_to_callosum(callosum_listener):
    """Test that cortex_request broadcasts request to Callosum."""
    messages = callosum_listener

    # Create a request
    use_id = cortex_request(
        prompt="Test prompt",
        name="chat",
        provider="openai",
        config={"model": GPT_5},
    )

    time.sleep(0.2)

    # Verify broadcast was received
    assert len(messages) == 1
    msg = messages[0]
    assert msg["tract"] == "cortex"
    assert msg["event"] == "request"
    assert msg["prompt"] == "Test prompt"
    assert msg["name"] == "chat"
    assert msg["provider"] == "openai"
    assert msg["model"] == GPT_5
    assert msg["use_id"] == use_id
    assert "ts" in msg


def test_cortex_request_returns_agent_id(callosum_server):
    """Test that cortex_request returns use_id string."""
    _ = callosum_server  # Needed for side effects only

    use_id = cortex_request(prompt="Test", name="chat", provider="openai")

    # Verify use_id is a string timestamp
    assert isinstance(use_id, str)
    assert use_id.isdigit()
    assert len(use_id) == 13  # Millisecond timestamp


def test_cortex_request_uses_explicit_use_id(callosum_listener):
    messages = callosum_listener

    use_id = cortex_request(
        prompt="Test prompt",
        name="chat",
        provider="openai",
        use_id="1713629000000",
    )

    time.sleep(0.2)

    assert use_id == "1713629000000"
    assert messages[-1]["use_id"] == "1713629000000"


def test_cortex_request_unique_agent_ids(callosum_server):
    """Test that cortex_request generates unique agent IDs."""
    _ = callosum_server  # Needed for side effects only

    agent_ids = []
    for i in range(3):
        use_id = cortex_request(prompt=f"Test {i}", name="chat", provider="openai")
        agent_ids.append(use_id)
        time.sleep(0.002)

    # All agent IDs should be unique
    assert len(set(agent_ids)) == 3


def test_cortex_request_raises_when_callosum_unavailable(tmp_path, monkeypatch):
    """Test cortex_request classifies Callosum send failures."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        "solstone.think.cortex_client.callosum_send_classified",
        lambda *a, **kw: "FileNotFoundError",
    )

    with pytest.raises(CortexSpawnUnavailable) as excinfo:
        cortex_request(prompt="Test", name="chat", provider="openai")

    assert excinfo.value.detail == "FileNotFoundError"


def test_cortex_request_empty_journal(tmp_path, monkeypatch):
    """Test cortex_request works with an empty journal directory."""
    monkeypatch.setattr(
        "solstone.think.cortex_client.callosum_send_classified", lambda *a, **kw: ""
    )
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    use_id = cortex_request("test", "chat", "openai")
    assert use_id is not None
    assert len(use_id) > 0


# Tests for cortex_uses remain mostly unchanged as they read from files


def test_cortex_agents_empty(tmp_path, monkeypatch):
    """Test cortex_uses with no agents."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    result = cortex_uses()

    assert result["uses"] == []
    assert result["pagination"]["total"] == 0
    assert result["pagination"]["has_more"] is False
    assert result["live_count"] == 0
    assert result["historical_count"] == 0


def test_cortex_agents_with_active(tmp_path, monkeypatch):
    """Test cortex_uses with active (running) agents."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()

    # Create active agent files
    ts1 = now_ms()
    ts2 = ts1 + 1000

    unified_dir = talents_dir / "chat"
    tester_dir = talents_dir / "tester"
    unified_dir.mkdir()
    tester_dir.mkdir()

    active_file1 = unified_dir / f"{ts1}_active.jsonl"
    with open(active_file1, "w") as f:
        json.dump(
            {
                "event": "request",
                "ts": ts1,
                "prompt": "Task 1",
                "name": "chat",
                "provider": "openai",
            },
            f,
        )
        f.write("\n")

    active_file2 = tester_dir / f"{ts2}_active.jsonl"
    with open(active_file2, "w") as f:
        json.dump(
            {
                "event": "request",
                "ts": ts2,
                "prompt": "Task 2",
                "name": "tester",
                "provider": "google",
            },
            f,
        )
        f.write("\n")

    result = cortex_uses()

    assert len(result["uses"]) == 2
    assert result["live_count"] == 2
    assert result["historical_count"] == 0


def test_cortex_agents_with_completed(tmp_path, monkeypatch):
    """Test cortex_uses with completed (historical) agents."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()

    # Create completed agent files
    ts1 = now_ms()
    reviewer_dir = talents_dir / "reviewer"
    reviewer_dir.mkdir()

    completed_file1 = reviewer_dir / f"{ts1}.jsonl"
    with open(completed_file1, "w") as f:
        json.dump(
            {
                "event": "request",
                "ts": ts1,
                "prompt": "Old task",
                "name": "reviewer",
                "provider": "anthropic",
            },
            f,
        )
        f.write("\n")
        json.dump({"event": "finish", "ts": ts1 + 100, "result": "Done"}, f)
        f.write("\n")

    result = cortex_uses()

    assert len(result["uses"]) == 1
    assert result["live_count"] == 0
    assert result["historical_count"] == 1
    assert result["uses"][0]["status"] == "completed"


def test_cortex_agents_pagination(tmp_path, monkeypatch):
    """Test cortex_uses pagination."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()

    # Create multiple agents
    base_ts = now_ms()
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir()
    for i in range(5):
        ts = base_ts + (i * 1000)
        file = unified_dir / f"{ts}.jsonl"
        with open(file, "w") as f:
            json.dump(
                {
                    "event": "request",
                    "ts": ts,
                    "prompt": f"Task {i}",
                    "name": "chat",
                },
                f,
            )
            f.write("\n")

    # Test limit
    result = cortex_uses(limit=2)
    assert len(result["uses"]) == 2
    assert result["pagination"]["limit"] == 2
    assert result["pagination"]["total"] == 5
    assert result["pagination"]["has_more"] is True


def test_cortex_agents_empty_journal(tmp_path, monkeypatch):
    """Test cortex_uses works with an empty journal directory."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    result = cortex_uses()
    assert "uses" in result
    assert "pagination" in result
    assert isinstance(result["uses"], list)


def test_get_agent_log_status_completed(tmp_path, monkeypatch):
    """Test get_use_log_status returns 'completed' for finished agents."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir()

    use_id = "1234567890123"
    (unified_dir / f"{use_id}.jsonl").write_text('{"event": "finish"}\n')

    assert get_use_log_status(use_id) == "completed"


def test_get_agent_log_status_running(tmp_path, monkeypatch):
    """Test get_use_log_status returns 'running' for active agents."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir()

    use_id = "1234567890123"
    (unified_dir / f"{use_id}_active.jsonl").write_text('{"event": "start"}\n')

    assert get_use_log_status(use_id) == "running"


def test_read_use_provider_model_reads_active_log(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents" / "chat"
    talents_dir.mkdir(parents=True)

    use_id = "1234567890123"
    (talents_dir / f"{use_id}_active.jsonl").write_text(
        json.dumps({"event": "request", "provider": "openai", "model": "wrong"})
        + "\n"
        + json.dumps(
            {
                "event": "start",
                "provider": "anthropic",
                "model": "claude-opus-4-1",
            }
        )
        + "\n",
        encoding="utf-8",
    )

    assert read_use_provider_model(use_id) == ("anthropic", "claude-opus-4-1")


def test_get_agent_log_status_not_found(tmp_path, monkeypatch):
    """Test get_use_log_status returns 'not_found' for missing agents."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "talents").mkdir()

    assert get_use_log_status("nonexistent") == "not_found"


def test_get_agent_log_status_prefers_completed(tmp_path, monkeypatch):
    """Test get_use_log_status returns 'completed' when both files exist."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir()

    # Edge case: both files exist (shouldn't happen, but check precedence)
    use_id = "1234567890123"
    (unified_dir / f"{use_id}.jsonl").write_text('{"event": "finish"}\n')
    (unified_dir / f"{use_id}_active.jsonl").write_text('{"event": "start"}\n')

    assert get_use_log_status(use_id) == "completed"


def test_get_agent_end_state_finish(tmp_path, monkeypatch):
    """Test get_use_end_state returns 'finish' for successful agents."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir()

    use_id = "1234567890123"
    (unified_dir / f"{use_id}.jsonl").write_text(
        '{"event": "request", "prompt": "hello"}\n'
        '{"event": "finish", "result": "done"}\n'
    )

    assert get_use_end_state(use_id) == "finish"


def test_get_agent_end_state_error(tmp_path, monkeypatch):
    """Test get_use_end_state returns 'error' for failed agents."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir()

    use_id = "1234567890123"
    (unified_dir / f"{use_id}.jsonl").write_text(
        '{"event": "request", "prompt": "hello"}\n'
        '{"event": "error", "error": "something went wrong"}\n'
    )

    assert get_use_end_state(use_id) == "error"


def test_get_agent_end_state_no_output_maps_to_error(tmp_path, monkeypatch):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir()

    use_id = "1234567890123"
    (unified_dir / f"{use_id}.jsonl").write_text(
        json.dumps({"event": "request", "prompt": "hello"})
        + "\n"
        + json.dumps(
            {
                "event": "error",
                "error": "no_output: expects-final cogitate run finished without "
                "emitting a final result",
                "reason_code": "no_output",
                "terminal": True,
            }
        )
        + "\n"
    )

    assert get_use_end_state(use_id) == "error"


def test_get_agent_end_state_running(tmp_path, monkeypatch):
    """Test get_use_end_state returns 'running' for active agents."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir()

    use_id = "1234567890123"
    (unified_dir / f"{use_id}_active.jsonl").write_text(
        '{"event": "request", "prompt": "hello"}\n'
    )

    assert get_use_end_state(use_id) == "running"


def test_get_agent_end_state_unknown(tmp_path, monkeypatch):
    """Test get_use_end_state returns 'unknown' for missing agents."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "talents").mkdir()

    assert get_use_end_state("nonexistent") == "unknown"


# Tests for wait_for_uses


def test_wait_for_agents_already_complete(tmp_path, monkeypatch):
    """Test wait_for_uses returns immediately if agents already completed."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir()
    (tmp_path / "health").mkdir()

    # Create completed agents
    agent_ids = ["1000", "2000"]
    for use_id in agent_ids:
        (unified_dir / f"{use_id}.jsonl").write_text('{"event": "finish"}\n')

    completed, timed_out = wait_for_uses(agent_ids, timeout=1)

    assert set(completed.keys()) == set(agent_ids)
    assert all(v == "finish" for v in completed.values())
    assert timed_out == []


def test_wait_for_agents_event_completion(callosum_server):
    """Test wait_for_uses completes when finish event is received."""
    tmp_path = callosum_server
    talents_dir = tmp_path / "talents"
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir(exist_ok=True)

    use_id = "1234567890123"

    # Start wait in background thread
    result = {"completed": None, "timed_out": None}

    def wait_thread():
        result["completed"], result["timed_out"] = wait_for_uses([use_id], timeout=5)

    waiter = threading.Thread(target=wait_thread)
    waiter.start()

    # Give the waiter time to set up listener
    time.sleep(0.2)

    # Create the completed file and emit finish event
    (unified_dir / f"{use_id}.jsonl").write_text('{"event": "finish"}\n')

    # Emit finish event via Callosum
    client = CallosumConnection()
    client.start()
    time.sleep(0.1)
    client.emit("cortex", "finish", use_id=use_id, result="done")
    time.sleep(0.2)
    client.stop()

    waiter.join(timeout=3)

    assert result["completed"] == {use_id: "finish"}
    assert result["timed_out"] == []


def test_wait_for_agents_error_event(callosum_server):
    """Test wait_for_uses completes on error event too."""
    tmp_path = callosum_server
    talents_dir = tmp_path / "talents"
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir(exist_ok=True)

    use_id = "1234567890124"

    result = {"completed": None, "timed_out": None}

    def wait_thread():
        result["completed"], result["timed_out"] = wait_for_uses([use_id], timeout=5)

    waiter = threading.Thread(target=wait_thread)
    waiter.start()
    time.sleep(0.2)

    # Create completed file and emit error event
    (unified_dir / f"{use_id}.jsonl").write_text('{"event": "error"}\n')

    client = CallosumConnection()
    client.start()
    time.sleep(0.1)
    client.emit("cortex", "error", use_id=use_id, error="something failed")
    time.sleep(0.2)
    client.stop()

    waiter.join(timeout=3)

    assert result["completed"] == {use_id: "error"}
    assert result["timed_out"] == []


def test_wait_for_agents_initial_file_check(tmp_path, monkeypatch):
    """Test wait_for_uses finds already-completed agents via initial file check."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir()
    (tmp_path / "health").mkdir()

    use_id = "1234567890125"

    # Agent already completed before we start waiting
    (unified_dir / f"{use_id}.jsonl").write_text('{"event": "finish"}\n')

    completed, timed_out = wait_for_uses([use_id], timeout=1)

    # Should find via initial file check
    assert completed == {use_id: "finish"}
    assert timed_out == []


def test_wait_for_agents_timeout_actual(tmp_path, monkeypatch):
    """Test wait_for_uses times out for agents that never complete."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir()
    (tmp_path / "health").mkdir()

    use_id = "1234567890126"
    # Create active file (not completed)
    (unified_dir / f"{use_id}_active.jsonl").write_text('{"event": "start"}\n')

    completed, timed_out = wait_for_uses([use_id], timeout=1)

    assert completed == {}
    assert timed_out == [use_id]


def test_wait_for_agents_partial(callosum_server):
    """Test wait_for_uses with some completing and some timing out."""
    tmp_path = callosum_server
    talents_dir = tmp_path / "talents"
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir(exist_ok=True)

    completing_agent = "1111"
    timeout_agent = "2222"

    # Create active file for timeout agent
    (unified_dir / f"{timeout_agent}_active.jsonl").write_text('{"event": "start"}\n')

    result = {"completed": None, "timed_out": None}

    def wait_thread():
        result["completed"], result["timed_out"] = wait_for_uses(
            [completing_agent, timeout_agent], timeout=1
        )

    waiter = threading.Thread(target=wait_thread)
    waiter.start()
    time.sleep(0.2)

    # Complete one agent
    (unified_dir / f"{completing_agent}.jsonl").write_text('{"event": "finish"}\n')

    client = CallosumConnection()
    client.start()
    time.sleep(0.1)
    client.emit("cortex", "finish", use_id=completing_agent, result="done")
    time.sleep(0.1)
    client.stop()

    waiter.join(timeout=5)

    assert result["completed"] == {completing_agent: "finish"}
    assert result["timed_out"] == [timeout_agent]


def test_wait_for_agents_missed_event_recovery(tmp_path, monkeypatch, caplog):
    """Test that missed events are recovered via final file check with INFO log."""
    import logging

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()
    unified_dir = talents_dir / "chat"
    unified_dir.mkdir()
    (tmp_path / "health").mkdir()

    use_id = "1234567890127"

    # Start with active file
    (unified_dir / f"{use_id}_active.jsonl").write_text('{"event": "start"}\n')

    result = {"completed": None, "timed_out": None}

    def wait_and_complete():
        # Wait a bit then "complete" the agent by renaming file
        time.sleep(0.3)
        (unified_dir / f"{use_id}.jsonl").write_text('{"event": "finish"}\n')
        (unified_dir / f"{use_id}_active.jsonl").unlink()

    completer = threading.Thread(target=wait_and_complete)
    completer.start()

    with caplog.at_level(logging.INFO):
        result["completed"], result["timed_out"] = wait_for_uses([use_id], timeout=1)

    completer.join()

    # Should recover via final file check
    assert result["completed"] == {use_id: "finish"}
    assert result["timed_out"] == []

    # Should log about missed event
    assert any(
        "completion event not received but use completed" in record.message
        for record in caplog.records
    )
