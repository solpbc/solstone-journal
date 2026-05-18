# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Unit tests for the Callosum message bus.

These tests use mocks to test logic in isolation without real I/O.
"""

from pathlib import Path
from unittest.mock import Mock, patch

import pytest

from solstone.think.callosum import CallosumConnection, CallosumServer


@pytest.fixture
def journal_path(tmp_path, monkeypatch):
    """Set up a temporary journal path."""
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    yield journal


def test_server_broadcast_validates_tract_field():
    """Test that messages without tract field are rejected."""
    server = CallosumServer()

    # Message without tract should be rejected and return False
    invalid_msg = {"event": "test"}
    result = server.broadcast(invalid_msg)

    assert result is False
    # Should not be queued
    assert server.broadcast_queue.qsize() == 0


def test_server_broadcast_validates_event_field():
    """Test that messages without event field are rejected."""
    server = CallosumServer()

    # Message without event should be rejected and return False
    invalid_msg = {"tract": "test"}
    result = server.broadcast(invalid_msg)

    assert result is False
    # Should not be queued
    assert server.broadcast_queue.qsize() == 0


def test_server_broadcast_adds_timestamp():
    """Test that server adds timestamp if not present."""
    server = CallosumServer()

    # Valid message without timestamp
    msg = {"tract": "test", "event": "hello"}

    with patch("solstone.think.callosum.time.time", return_value=1234567.890):
        result = server.broadcast(msg)

    assert result is True
    # Message should be queued with timestamp added
    queued_msg = server.broadcast_queue.get_nowait()
    assert queued_msg["tract"] == "test"
    assert queued_msg["event"] == "hello"
    assert queued_msg["ts"] == 1234567890  # milliseconds


def test_server_broadcast_preserves_custom_timestamp():
    """Test that custom timestamp in message is preserved."""
    server = CallosumServer()

    custom_ts = 9999999999
    msg = {"tract": "test", "event": "hello", "ts": custom_ts}

    result = server.broadcast(msg)

    assert result is True
    # Should preserve custom timestamp
    queued_msg = server.broadcast_queue.get_nowait()
    assert queued_msg["ts"] == custom_ts


def test_server_broadcast_removes_dead_clients():
    """Test that _send_to_clients removes clients that fail to receive."""
    server = CallosumServer()

    # Create mock clients - one working, one dead
    working_client = Mock()
    dead_client = Mock()
    dead_client.sendall.side_effect = Exception("Connection broken")
    dead_client.settimeout = Mock()
    working_client.settimeout = Mock()

    server.clients = [working_client, dead_client]

    # Call _send_to_clients directly (the method used by _writer_loop)
    msg = {"tract": "test", "event": "hello", "ts": 12345}
    server._send_to_clients(msg)

    # Dead client should be removed
    assert working_client in server.clients
    assert dead_client not in server.clients
    assert len(server.clients) == 1

    # Dead client socket should be closed
    dead_client.close.assert_called_once()


def test_client_emit_returns_false_when_not_started():
    """Test that emit() returns False and logs warning if start() not called yet."""
    client = CallosumConnection()

    # emit() should return False and log when thread not started
    with patch("solstone.think.callosum.logger") as mock_logger:
        result = client.emit("test", "hello")
        assert result is False
        mock_logger.warning.assert_called_once()
        assert "Thread not running" in mock_logger.warning.call_args[0][0]


def test_client_emit_queues_message():
    """Test that emit() queues message when thread is running."""
    client = CallosumConnection()

    # Setup running thread
    mock_thread = Mock()
    mock_thread.is_alive.return_value = True
    client.thread = mock_thread

    result = client.emit("test", "hello", data="world", count=42)

    assert result is True
    # Message should be in queue
    assert client.send_queue.qsize() == 1
    msg = client.send_queue.get_nowait()
    assert msg["tract"] == "test"
    assert msg["event"] == "hello"
    assert msg["data"] == "world"
    assert msg["count"] == 42


def test_client_emit_returns_false_when_queue_full():
    """Test that emit() returns False when queue is full."""
    client = CallosumConnection()

    # Setup running thread
    mock_thread = Mock()
    mock_thread.is_alive.return_value = True
    client.thread = mock_thread

    # Fill the queue
    for i in range(1000):
        client.send_queue.put({"tract": "test", "event": f"msg{i}"})

    # Next emit should fail
    with patch("solstone.think.callosum.logger") as mock_logger:
        result = client.emit("test", "overflow")
        assert result is False
        mock_logger.warning.assert_called()
        assert "Queue full" in mock_logger.warning.call_args[0][0]


def test_client_start_creates_thread():
    """Test that start() creates and starts background thread."""
    client = CallosumConnection()

    def callback(msg):
        pass

    client.start(callback=callback)

    assert client.thread is not None
    assert client.thread.is_alive()
    assert client.callback is callback

    # Cleanup
    client.stop()


def test_client_start_idempotent():
    """Test that calling start() multiple times is safe."""
    client = CallosumConnection()

    client.start()
    first_thread = client.thread

    # Call start again
    client.start()

    # Should still have same thread (not restarted)
    assert client.thread is first_thread

    # Cleanup
    client.stop()


def test_client_stop_stops_thread():
    """Test that stop() stops the background thread."""
    client = CallosumConnection()

    # Setup running thread
    mock_thread = Mock()
    mock_thread.is_alive.return_value = False
    client.thread = mock_thread

    client.stop()

    # Should set stop event and join thread
    assert client.stop_event.is_set()
    mock_thread.join.assert_called_once_with(timeout=0.5)


def test_server_socket_path_from_env(journal_path):
    """Test that server uses SOLSTONE_JOURNAL env var for socket path."""
    server = CallosumServer()

    expected_path = journal_path / "health" / "callosum.sock"
    assert server.socket_path == expected_path


def test_server_socket_path_custom():
    """Test that server accepts custom socket path."""
    custom_path = Path("/tmp/custom.sock")
    server = CallosumServer(socket_path=custom_path)

    assert server.socket_path == custom_path


def test_client_socket_path_from_env(journal_path):
    """Test that client uses SOLSTONE_JOURNAL env var for socket path."""
    client = CallosumConnection()

    expected_path = journal_path / "health" / "callosum.sock"
    assert client.socket_path == expected_path


def test_client_socket_path_custom():
    """Test that client accepts custom socket path."""
    custom_path = Path("/tmp/custom.sock")
    client = CallosumConnection(socket_path=custom_path)

    assert client.socket_path == custom_path


def test_callosum_send_empty_journal(tmp_path, monkeypatch):
    """Test that callosum_send() works with an empty journal directory."""
    from solstone.think.callosum import callosum_send

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    # No server listening at tmp_path, so send will fail gracefully
    result = callosum_send("test", "event", data="value")
    assert isinstance(result, bool)


def test_callosum_send_with_custom_path():
    """Test that callosum_send() accepts custom socket path."""
    from solstone.think.callosum import callosum_send

    # Use non-existent socket - should return False but not crash
    custom_path = Path("/tmp/nonexistent_callosum.sock")
    result = callosum_send("test", "event", socket_path=custom_path, data="value")

    # Should fail gracefully (no server listening)
    assert result is False


# --- CLI helper tests ---


class TestParseValue:
    """Tests for _parse_value auto-type detection."""

    def test_integer(self):
        from solstone.think.callosum import _parse_value

        assert _parse_value("42") == 42

    def test_float(self):
        from solstone.think.callosum import _parse_value

        assert _parse_value("3.14") == 3.14

    def test_boolean_true(self):
        from solstone.think.callosum import _parse_value

        assert _parse_value("true") is True

    def test_boolean_false(self):
        from solstone.think.callosum import _parse_value

        assert _parse_value("false") is False

    def test_null(self):
        from solstone.think.callosum import _parse_value

        assert _parse_value("null") is None

    def test_plain_string(self):
        from solstone.think.callosum import _parse_value

        assert _parse_value("hello") == "hello"

    def test_string_with_spaces(self):
        from solstone.think.callosum import _parse_value

        assert _parse_value("hello world") == "hello world"

    def test_json_array(self):
        from solstone.think.callosum import _parse_value

        assert _parse_value("[1,2,3]") == [1, 2, 3]


class TestParseKvFields:
    """Tests for _parse_kv_fields key=value parsing."""

    def test_basic_fields(self):
        from solstone.think.callosum import _parse_kv_fields

        result = _parse_kv_fields(["day=20250101", "count=5", "active=true"])
        assert result == {"day": 20250101, "count": 5, "active": True}

    def test_empty_list(self):
        from solstone.think.callosum import _parse_kv_fields

        assert _parse_kv_fields([]) == {}

    def test_value_with_equals(self):
        from solstone.think.callosum import _parse_kv_fields

        # Value containing '=' should keep everything after first '='
        result = _parse_kv_fields(["expr=a=b"])
        assert result == {"expr": "a=b"}

    def test_missing_equals_exits(self):
        from solstone.think.callosum import _parse_kv_fields

        with pytest.raises(SystemExit):
            _parse_kv_fields(["no_equals_here"])


class TestParseJsonMessage:
    """Tests for _parse_json_message validation."""

    def test_valid_json(self):
        from solstone.think.callosum import _parse_json_message

        result = _parse_json_message('{"tract":"test","event":"ping","data":1}')
        assert result == {"tract": "test", "event": "ping", "data": 1}

    def test_missing_tract(self):
        from solstone.think.callosum import _parse_json_message

        with pytest.raises(SystemExit):
            _parse_json_message('{"event":"ping"}')

    def test_missing_event(self):
        from solstone.think.callosum import _parse_json_message

        with pytest.raises(SystemExit):
            _parse_json_message('{"tract":"test"}')

    def test_invalid_json(self):
        from solstone.think.callosum import _parse_json_message

        with pytest.raises(SystemExit):
            _parse_json_message("not json")

    def test_json_array_rejected(self):
        from solstone.think.callosum import _parse_json_message

        with pytest.raises(SystemExit):
            _parse_json_message("[1,2,3]")


class TestCmdSendInputModes:
    """Tests for _cmd_send input mode detection."""

    def test_positional_mode(self):
        """Test tract event key=value positional syntax."""
        from types import SimpleNamespace

        from solstone.think.callosum import _cmd_send

        args = SimpleNamespace(args=["test", "ping", "data=42"])
        with patch(
            "solstone.think.callosum.callosum_send", return_value=True
        ) as mock_send:
            _cmd_send(args)
            mock_send.assert_called_once_with("test", "ping", data=42)

    def test_json_arg_mode(self):
        """Test JSON string argument mode."""
        from types import SimpleNamespace

        from solstone.think.callosum import _cmd_send

        args = SimpleNamespace(args=['{"tract":"test","event":"ping","n":1}'])
        with patch(
            "solstone.think.callosum.callosum_send", return_value=True
        ) as mock_send:
            _cmd_send(args)
            mock_send.assert_called_once_with("test", "ping", n=1)

    def test_stdin_mode(self, monkeypatch):
        """Test reading JSON from stdin."""
        import io
        from types import SimpleNamespace

        from solstone.think.callosum import _cmd_send

        args = SimpleNamespace(args=[])
        fake_stdin = io.StringIO('{"tract":"test","event":"ping"}')
        monkeypatch.setattr("solstone.think.callosum.sys.stdin", fake_stdin)

        with patch(
            "solstone.think.callosum.callosum_send", return_value=True
        ) as mock_send:
            _cmd_send(args)
            mock_send.assert_called_once_with("test", "ping")

    def test_too_few_positional_args_exits(self):
        """Test that a single positional arg (not JSON) exits with usage."""
        from types import SimpleNamespace

        from solstone.think.callosum import _cmd_send

        args = SimpleNamespace(args=["only_one"])
        with pytest.raises(SystemExit):
            _cmd_send(args)

    def test_send_failure_exits(self):
        """Test that failed send exits with code 1."""
        from types import SimpleNamespace

        from solstone.think.callosum import _cmd_send

        args = SimpleNamespace(args=["test", "ping"])
        with patch("solstone.think.callosum.callosum_send", return_value=False):
            with pytest.raises(SystemExit):
                _cmd_send(args)
