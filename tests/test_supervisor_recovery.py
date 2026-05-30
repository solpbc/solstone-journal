# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import asyncio
import logging
from unittest.mock import Mock

import solstone.think.supervisor as mod
from solstone.think.providers import local_server


def _set_local_port(monkeypatch, port: int = 9999) -> None:
    monkeypatch.setattr(mod, "read_service_port", lambda service: port)


def _capture_callosum_messages() -> list[dict]:
    received: list[dict] = []
    listener = mod.CallosumConnection()
    listener.start(callback=received.append)
    emitter = mod.CallosumConnection()
    emitter.start()
    mod._supervisor_callosum = emitter
    return received


def _drain_messages(messages: list[dict]) -> list[dict]:
    return [
        message
        for message in messages
        if message.get("tract") == "supervisor" and message.get("event") == "drain"
    ]


class _ProcessStub:
    def __init__(self, returncode: int = 1):
        self.poll = Mock(return_value=returncode)
        self.returncode = returncode
        self.pid = 12345


class _ManagedStub:
    def __init__(self, name: str, cmd: list[str]):
        self.name = name
        self.cmd = cmd
        self.process = _ProcessStub()
        self.ref = f"{name}-ref"
        self.cleanup = Mock()


def test_rising_edge_fires_once(monkeypatch, mock_callosum):
    _set_local_port(monkeypatch)
    received = _capture_callosum_messages()
    monkeypatch.setattr(
        local_server,
        "_probe_health",
        lambda port: (local_server.STATE_READY, None),
    )
    mod._recovery_state["llama_server_down"] = True

    asyncio.run(mod._check_local_server_recovery())

    assert len(_drain_messages(received)) == 1
    assert mod._recovery_state["llama_server_down"] is False


def test_startup_ready_does_not_nudge(monkeypatch, mock_callosum):
    read_service_port = Mock(return_value=9999)
    probe_health = Mock(return_value=(local_server.STATE_READY, None))
    monkeypatch.setattr(mod, "read_service_port", read_service_port)
    monkeypatch.setattr(local_server, "_probe_health", probe_health)
    received = _capture_callosum_messages()
    mod._recovery_state["llama_server_down"] = False

    asyncio.run(mod._check_local_server_recovery())

    read_service_port.assert_not_called()
    probe_health.assert_not_called()
    assert _drain_messages(received) == []


def test_steady_state_no_nudge(monkeypatch, mock_callosum):
    _set_local_port(monkeypatch)
    probe_health = Mock(return_value=(local_server.STATE_READY, None))
    monkeypatch.setattr(local_server, "_probe_health", probe_health)
    received = _capture_callosum_messages()
    mod._recovery_state["llama_server_down"] = False

    asyncio.run(mod._check_local_server_recovery())
    asyncio.run(mod._check_local_server_recovery())

    probe_health.assert_not_called()
    assert _drain_messages(received) == []

    probe_health = Mock(return_value=(local_server.STATE_LOADING, None))
    monkeypatch.setattr(local_server, "_probe_health", probe_health)
    mod._recovery_state["llama_server_down"] = True

    asyncio.run(mod._check_local_server_recovery())

    probe_health.assert_called_once_with(9999)
    assert _drain_messages(received) == []
    assert mod._recovery_state["llama_server_down"] is True


def test_flap_two_nudges(monkeypatch, mock_callosum):
    _set_local_port(monkeypatch)
    probe_health = Mock(return_value=(local_server.STATE_READY, None))
    monkeypatch.setattr(local_server, "_probe_health", probe_health)
    received = _capture_callosum_messages()

    mod._recovery_state["llama_server_down"] = True
    asyncio.run(mod._check_local_server_recovery())

    asyncio.run(mod._check_local_server_recovery())

    mod._recovery_state["llama_server_down"] = True
    asyncio.run(mod._check_local_server_recovery())

    assert len(_drain_messages(received)) == 2
    assert probe_health.call_count == 2
    assert mod._recovery_state["llama_server_down"] is False


def test_undeliverable_callosum_none(monkeypatch, caplog):
    _set_local_port(monkeypatch)
    monkeypatch.setattr(
        local_server,
        "_probe_health",
        lambda port: (local_server.STATE_READY, None),
    )
    mod._supervisor_callosum = None
    mod._recovery_state["llama_server_down"] = True
    caplog.set_level(logging.WARNING)

    asyncio.run(mod._check_local_server_recovery())

    assert mod._recovery_state["llama_server_down"] is False
    assert "supervisor callosum unavailable" in caplog.text


def test_undeliverable_emit_raises(monkeypatch, caplog):
    _set_local_port(monkeypatch)
    probe_health = Mock(return_value=(local_server.STATE_READY, None))
    monkeypatch.setattr(local_server, "_probe_health", probe_health)
    callosum = Mock()
    callosum.emit.side_effect = RuntimeError("boom")
    mod._supervisor_callosum = callosum
    mod._recovery_state["llama_server_down"] = True
    caplog.set_level(logging.WARNING)

    asyncio.run(mod._check_local_server_recovery())
    asyncio.run(mod._check_local_server_recovery())

    assert mod._recovery_state["llama_server_down"] is False
    callosum.emit.assert_called_once_with("supervisor", "drain")
    probe_health.assert_called_once_with(9999)
    assert "Cannot nudge catchup drain: boom" in caplog.text


def test_nudge_no_targeting():
    callosum = Mock()
    mod._supervisor_callosum = callosum

    mod._nudge_catchup_drain()

    callosum.emit.assert_called_once_with("supervisor", "drain")


def test_remote_mode_inert(monkeypatch, mock_callosum):
    read_service_port = Mock(return_value=9999)
    probe_health = Mock(return_value=(local_server.STATE_READY, None))
    monkeypatch.setattr(mod, "read_service_port", read_service_port)
    monkeypatch.setattr(local_server, "_probe_health", probe_health)
    received = _capture_callosum_messages()
    mod._is_remote_mode = True
    mod._recovery_state["llama_server_down"] = True

    asyncio.run(mod._check_local_server_recovery())

    read_service_port.assert_not_called()
    probe_health.assert_not_called()
    assert _drain_messages(received) == []
    assert mod._recovery_state["llama_server_down"] is True


def test_handle_runner_exits_sets_flag_for_llama_server(monkeypatch):
    monkeypatch.setattr(mod, "_SERVICE_STATE", {})
    monkeypatch.setattr(mod, "_RESTART_POLICIES", {})
    monkeypatch.setattr(mod, "shutdown_requested", False)
    monkeypatch.setattr(mod, "_supervisor_callosum", None)
    mod._SERVICE_STATE[mod.LOCAL_SERVER_PROCESS_NAME] = {"restart": True}
    managed = _ManagedStub(
        mod.LOCAL_SERVER_PROCESS_NAME,
        ["/tmp/llama-server", "-m", "/tmp/model.gguf"],
    )
    replacement = _ManagedStub(mod.LOCAL_SERVER_PROCESS_NAME, managed.cmd)

    def fake_launch(name, cmd, *, restart=False, shutdown_timeout=15, ref=None):
        return replacement

    monkeypatch.setattr(mod, "_launch_process", fake_launch)
    mod._recovery_state["llama_server_down"] = False

    asyncio.run(mod.handle_runner_exits([managed]))

    assert mod._recovery_state["llama_server_down"] is True


def test_handle_runner_exits_no_flag_for_other_service(monkeypatch):
    monkeypatch.setattr(mod, "_SERVICE_STATE", {})
    monkeypatch.setattr(mod, "_RESTART_POLICIES", {})
    monkeypatch.setattr(mod, "shutdown_requested", False)
    monkeypatch.setattr(mod, "_supervisor_callosum", None)
    mod._SERVICE_STATE["convey"] = {"restart": True}
    managed = _ManagedStub("convey", ["journal", "convey"])
    replacement = _ManagedStub("convey", managed.cmd)

    def fake_launch(name, cmd, *, restart=False, shutdown_timeout=15, ref=None):
        return replacement

    monkeypatch.setattr(mod, "_launch_process", fake_launch)
    mod._recovery_state["llama_server_down"] = False

    asyncio.run(mod.handle_runner_exits([managed]))

    assert mod._recovery_state["llama_server_down"] is False
