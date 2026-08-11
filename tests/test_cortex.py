# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the file-based Cortex agent manager."""

import asyncio
import json
import os
import signal
import subprocess
import sys
import threading
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from solstone.think.models import GPT_5
from tests.helpers.module_mocks import module_mock


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


def _read_jsonl(path: Path) -> list[dict]:
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


def _cortex_request(use_id: str, name: str = "chat") -> dict:
    return {
        "tract": "cortex",
        "event": "request",
        "use_id": use_id,
        "name": name,
        "day": "20260410",
    }


def _active_path(journal_path: Path, use_id: str, name: str = "chat") -> Path:
    return journal_path / "talents" / name.replace(":", "--") / f"{use_id}_active.jsonl"


def _completed_path(journal_path: Path, use_id: str, name: str = "chat") -> Path:
    return journal_path / "talents" / name.replace(":", "--") / f"{use_id}.jsonl"


def _make_live_agent(
    cortex_service, journal_path: Path, use_id: str, *, name: str = "chat"
):
    from solstone.think.cortex import TalentProcess

    talent_dir = journal_path / "talents" / name.replace(":", "--")
    talent_dir.mkdir(exist_ok=True)
    active_path = talent_dir / f"{use_id}_active.jsonl"
    request = {
        "event": "request",
        "use_id": use_id,
        "ts": 1000,
        "name": name,
        "day": "20260410",
    }
    active_path.write_text(json.dumps(request) + "\n", encoding="utf-8")
    mock_process = MagicMock()
    mock_process.pid = 24680
    mock_process.wait.return_value = 0
    mock_process.stdout = MockPipe([])
    mock_process.stderr = MockPipe([])
    agent = TalentProcess(use_id, mock_process, active_path)
    cortex_service.running_uses[use_id] = agent
    cortex_service.use_requests[use_id] = request
    return agent, active_path


class _FakeClock:
    def __init__(self, now: datetime):
        self.now = now
        self.waits: list[float] = []

    def __call__(self) -> datetime:
        return self.now

    def advance(self, seconds: float) -> None:
        self.now += timedelta(seconds=seconds)

    def wait(self, seconds: float) -> bool:
        self.waits.append(seconds)
        self.advance(seconds)
        return False


class _FakeCallosum:
    def __init__(self) -> None:
        self.emitted: list[tuple[tuple, dict]] = []

    def emit(self, *args, **kwargs) -> None:
        self.emitted.append((args, kwargs))


def _spp_inspector(state: dict):
    def inspect(now: datetime, *, journal_path=None):
        lane = state.get("lane", "spp")
        aggregate = state.get("aggregate", "ready")
        fingerprint = state.get("fingerprint", "a" * 64)
        record_fingerprint = state.get("record_fingerprint", fingerprint)
        observed = state.get("observed", now)
        expires = state.get("expires", now + timedelta(minutes=10))
        record = None
        if lane == "spp" and state.get("record_present", True):
            component = None
            if state.get("component_present", True):
                component = {
                    "status": state.get("component_status", "ok"),
                    "observed_at": observed.isoformat(),
                    "expires_at": expires.isoformat(),
                }
                reason = state.get("component_reason")
                if reason is not None:
                    component["reason_code"] = reason
            record = {
                "active_lane": "spp",
                "fingerprint_sha256": record_fingerprint,
                "evidence": {"lane_prerequisites": component},
            }
        return {
            "status": "ok",
            "path": "/redacted",
            "record": record,
            "projection": {
                "aggregate_state": aggregate,
                "active_lane": lane,
                "fingerprint_sha256": fingerprint,
                "runtime_transition_in_progress": False,
            },
            "reason_code": None,
            "error": None,
        }

    return inspect


def _make_spp_controller(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    *,
    state: dict,
    clock: _FakeClock | None = None,
    logger: MagicMock | None = None,
    callosum: _FakeCallosum | None = None,
):
    from solstone.think import cortex

    clock = clock or _FakeClock(datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc))
    callosum = callosum or _FakeCallosum()
    logger = logger or MagicMock()
    monkeypatch.setattr(cortex, "inspect_brain_state", _spp_inspector(state))
    monkeypatch.setattr(
        cortex,
        "read_active_brain_fingerprint_sha256",
        lambda *, journal_path=None: state.get("fingerprint", "a" * 64),
    )
    controller = cortex.SppRenewalController(
        callosum=callosum,
        stop_event=threading.Event(),
        logger=logger,
        clock=clock,
        wait=clock.wait,
        journal_path=tmp_path,
    )
    return controller, clock, callosum, logger


def _commands(callosum: _FakeCallosum) -> list[list[str]]:
    return [kwargs["cmd"] for _args, kwargs in callosum.emitted]


def _assert_fenced_refresh_command(command: list[str], fingerprint: str) -> None:
    assert command[:4] == ["journal", "brain", "refresh", "--json"]
    assert "--expected-active-fingerprint" in command
    expected_index = command.index("--expected-fingerprint")
    assert command[expected_index + 1] == fingerprint


def _assert_absence_fenced_refresh_command(command: list[str]) -> None:
    assert command == [
        "journal",
        "brain",
        "refresh",
        "--json",
        "--expect-active-fingerprint-absent",
    ]


@pytest.fixture
def mock_journal(tmp_path, monkeypatch):
    """Set up a temporary journal directory."""
    journal_path = tmp_path / "journal"
    journal_path.mkdir()
    agents_path = journal_path / "talents"
    agents_path.mkdir()

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal_path))
    return journal_path


@pytest.fixture(autouse=True)
def _isolate_cortex_stdlib_modules(monkeypatch):
    from solstone.think import cortex

    monkeypatch.setattr(cortex, "threading", module_mock(cortex.threading))
    monkeypatch.setattr(cortex, "subprocess", module_mock(cortex.subprocess))
    monkeypatch.setattr(cortex, "time", module_mock(cortex.time))


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


def test_start_starts_spawn_worker_and_stays_resident(cortex_service, monkeypatch):
    from solstone.think import cortex

    fake_callosum = MagicMock()
    cortex_service.callosum = fake_callosum
    monkeypatch.setattr(cortex_service, "_should_request_brain_refresh", lambda: False)
    monkeypatch.setattr(cortex_service, "_emit_periodic_status", lambda: None)

    resident_loop_entered = threading.Event()
    returned = threading.Event()
    errors: list[BaseException] = []
    real_sleep = time.sleep

    def short_sleep(_seconds):
        resident_loop_entered.set()
        real_sleep(0.01)

    monkeypatch.setattr(cortex.time, "sleep", short_sleep)

    def run_start():
        try:
            cortex_service.start()
        except BaseException as exc:  # pragma: no cover - surfaced below
            errors.append(exc)
        finally:
            returned.set()

    service_thread = threading.Thread(target=run_start, daemon=True)
    service_thread.start()
    try:
        assert resident_loop_entered.wait(1)
        assert fake_callosum.start.called
        assert cortex_service._spawn_worker is not None
        assert cortex_service._spawn_worker.is_alive()
        assert service_thread.is_alive()
        assert not returned.is_set()

        cortex_service.shutdown_requested.set()
        service_thread.join(timeout=1)
        assert returned.is_set()
        assert errors == []
    finally:
        cortex_service.stop_event.set()
        if cortex_service._spawn_worker is not None:
            cortex_service._spawn_worker.join(timeout=1)
        service_thread.join(timeout=1)


def test_spp_renewal_controller_replaces_prerequisites_for_seventy_virtual_minutes(
    tmp_path,
    monkeypatch,
):
    from solstone.think import cortex

    start = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    clock = _FakeClock(start)
    state = {
        "fingerprint": "a" * 64,
        "observed": start,
        "expires": start + timedelta(minutes=10),
    }
    monkeypatch.setattr(cortex, "inspect_brain_state", _spp_inspector(state))
    monkeypatch.setattr(
        cortex,
        "read_active_brain_fingerprint_sha256",
        lambda *, journal_path=None: state["fingerprint"],
    )
    callosum = _FakeCallosum()
    controller = cortex.SppRenewalController(
        callosum=callosum,
        stop_event=threading.Event(),
        logger=MagicMock(),
        clock=clock,
        wait=clock.wait,
        journal_path=tmp_path,
    )

    committed: list[tuple[datetime, datetime]] = []
    while clock.now < start + timedelta(minutes=72):
        delay = controller.step()
        if delay > 0:
            clock.advance(delay)
        assert state["expires"] > clock.now
        controller.step()
        ref = callosum.emitted[-1][1]["ref"]
        controller.handle_supervisor_message(
            {"tract": "supervisor", "event": "started", "ref": ref}
        )
        state["observed"] = clock.now
        state["expires"] = clock.now + timedelta(minutes=10)
        committed.append((state["observed"], state["expires"]))
        controller.handle_supervisor_message(
            {"tract": "supervisor", "event": "stopped", "ref": ref, "exit_code": 0}
        )

    commands = _commands(callosum)
    assert commands
    assert len(committed) >= 9
    assert clock.now >= start + timedelta(minutes=72)
    assert start + timedelta(minutes=72) > start + timedelta(minutes=60)
    assert any(observed >= start + timedelta(minutes=30) for observed, _ in committed)
    assert any(observed >= start + timedelta(minutes=60) for observed, _ in committed)
    assert all(
        later[0] > earlier[0] and later[1] > earlier[1]
        for earlier, later in zip(committed, committed[1:])
    )
    assert all(
        command[:3] == ["journal", "brain", "renew-prerequisites"]
        for command in commands
    )
    assert not any(
        command[:3] == ["journal", "brain", "refresh"] for command in commands
    )
    assert not any(
        "generate" in command or "cogitate" in command for command in commands
    )


def test_spp_renewal_controller_tracks_skipped_active_ref_successor(
    tmp_path,
    monkeypatch,
):
    from solstone.think import cortex

    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    clock = _FakeClock(now)
    state = {
        "fingerprint": "b" * 64,
        "observed": now - timedelta(minutes=9),
        "expires": now + timedelta(seconds=30),
    }
    monkeypatch.setattr(cortex, "inspect_brain_state", _spp_inspector(state))
    monkeypatch.setattr(
        cortex,
        "read_active_brain_fingerprint_sha256",
        lambda *, journal_path=None: state["fingerprint"],
    )
    callosum = _FakeCallosum()
    controller = cortex.SppRenewalController(
        callosum=callosum,
        stop_event=threading.Event(),
        logger=MagicMock(),
        clock=clock,
        wait=clock.wait,
        journal_path=tmp_path,
    )

    controller.step()
    first_ref = callosum.emitted[-1][1]["ref"]
    controller.handle_supervisor_message(
        {
            "tract": "supervisor",
            "event": "skipped",
            "ref": first_ref,
            "active_ref": "active-brain",
            "reason": "still_running",
        }
    )
    controller.step()
    assert len(callosum.emitted) == 1

    controller.handle_supervisor_message(
        {
            "tract": "supervisor",
            "event": "stopped",
            "ref": "active-brain",
            "exit_code": 0,
        }
    )
    controller.step()
    assert len(callosum.emitted) == 2
    assert callosum.emitted[-1][1]["ref"] != first_ref


def test_spp_renewal_controller_times_out_missed_successor_stop(
    tmp_path,
    monkeypatch,
):
    from solstone.think import cortex

    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    state = {
        "fingerprint": "b" * 64,
        "observed": now - timedelta(minutes=9),
        "expires": now + timedelta(seconds=30),
    }
    controller, clock, callosum, _logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=state,
        clock=_FakeClock(now),
    )

    controller.step()
    first_ref = callosum.emitted[-1][1]["ref"]
    controller.handle_supervisor_message(
        {
            "tract": "supervisor",
            "event": "skipped",
            "ref": first_ref,
            "active_ref": "active-brain",
            "reason": "still_running",
        }
    )
    clock.advance(cortex.SPP_REFRESH_OBSERVATION_BOUND_S + 1)

    controller.step()

    assert controller._successor_after_ref is None
    assert len(callosum.emitted) == 2
    assert callosum.emitted[-1][1]["ref"] != first_ref


def test_spp_renewal_controller_accepts_full_refresh_exit_zero_without_retry(
    tmp_path,
    monkeypatch,
):
    from solstone.think import cortex

    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    state = {
        "aggregate": "unknown",
        "fingerprint": "e" * 64,
        "observed": now - timedelta(minutes=9),
        "expires": now + timedelta(seconds=30),
    }
    controller, clock, callosum, _logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=state,
        clock=_FakeClock(now),
    )

    controller.step()
    _assert_fenced_refresh_command(callosum.emitted[-1][1]["cmd"], state["fingerprint"])
    ref = callosum.emitted[-1][1]["ref"]
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "started", "ref": ref}
    )
    assert controller._running_deadline is not None
    assert (
        controller._running_deadline - clock.now
    ).total_seconds() == cortex.SPP_REFRESH_OBSERVATION_BOUND_S

    clock.advance(1)
    state["aggregate"] = "ready"
    state["observed"] = clock.now
    state["expires"] = clock.now + timedelta(minutes=10)
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "stopped", "ref": ref, "exit_code": 0}
    )

    assert controller._retry_after is None
    assert controller._running_ref is None
    delay = controller.step()
    assert len(callosum.emitted) == 1
    assert delay > 0


@pytest.mark.parametrize("case", ("unchanged", "unhealthy", "wrong_fingerprint"))
def test_spp_renewal_controller_rejects_exit_zero_without_new_ready_refresh_proof(
    tmp_path,
    monkeypatch,
    case,
):
    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    original_fingerprint = "e" * 64
    state = {
        "aggregate": "unknown",
        "fingerprint": original_fingerprint,
        "observed": now - timedelta(minutes=9),
        "expires": now + timedelta(seconds=30),
    }
    controller, clock, callosum, logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=state,
        clock=_FakeClock(now),
    )

    controller.step()
    _assert_fenced_refresh_command(callosum.emitted[-1][1]["cmd"], original_fingerprint)
    ref = callosum.emitted[-1][1]["ref"]
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "started", "ref": ref}
    )

    clock.advance(1)
    if case == "unchanged":
        state["aggregate"] = "ready"
    elif case == "unhealthy":
        state["aggregate"] = "unhealthy"
        state["component_status"] = "failed"
        state["component_reason"] = "attestation_rejected"
        state["observed"] = clock.now
        state["expires"] = clock.now + timedelta(minutes=10)
    else:
        state["aggregate"] = "ready"
        state["fingerprint"] = "f" * 64
        state["record_fingerprint"] = "f" * 64
        state["observed"] = clock.now
        state["expires"] = clock.now + timedelta(minutes=10)

    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "stopped", "ref": ref, "exit_code": 0}
    )

    assert controller._retry_after is not None
    logged = "\n".join(
        " ".join(str(part) for part in call.args) for call in logger.info.call_args_list
    )
    assert "verified" not in logged
    assert "failed" in logged


def test_spp_renewal_controller_running_timeout_clears_and_retries(
    tmp_path,
    monkeypatch,
):
    from solstone.think import cortex

    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    state = {
        "fingerprint": "f" * 64,
        "observed": now - timedelta(minutes=9),
        "expires": now + timedelta(seconds=30),
    }
    controller, clock, callosum, _logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=state,
        clock=_FakeClock(now),
    )

    controller.step()
    ref = callosum.emitted[-1][1]["ref"]
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "started", "ref": ref}
    )
    assert controller._running_deadline is not None
    assert (
        controller._running_deadline - clock.now
    ).total_seconds() == cortex.SPP_RENEWAL_ATTEMPT_BOUND_S
    clock.advance(cortex.SPP_RENEWAL_ATTEMPT_BOUND_S + 1)
    delay = controller.step()

    assert controller._running_ref is None
    assert controller._retry_after is not None
    assert delay == 5.0

    clock.advance(delay)
    controller.step()
    assert len(callosum.emitted) == 2
    assert callosum.emitted[-1][1]["ref"] != ref


@pytest.mark.parametrize(
    "state_update",
    (
        {"record_present": False},
        {"component_status": "failed", "component_reason": "attestation_not_verified"},
        {"record_fingerprint": "0" * 64},
        {"expires": datetime(2026, 7, 24, 11, 59, tzinfo=timezone.utc)},
    ),
)
def test_spp_renewal_controller_unsafe_spp_records_fall_back_to_full_refresh(
    tmp_path,
    monkeypatch,
    state_update,
):
    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    state = {
        "fingerprint": "9" * 64,
        "observed": now - timedelta(minutes=9),
        "expires": now + timedelta(seconds=30),
        **state_update,
    }
    _controller, _clock, callosum, _logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=state,
        clock=_FakeClock(now),
    )

    _controller.step()

    _assert_fenced_refresh_command(callosum.emitted[-1][1]["cmd"], state["fingerprint"])


def test_spp_renewal_controller_bootstraps_with_absence_fenced_refresh(
    tmp_path,
    monkeypatch,
):
    from solstone.think import cortex

    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    state = {
        "aggregate": "unknown",
        "fingerprint": None,
        "observed": now - timedelta(minutes=9),
        "expires": now + timedelta(seconds=30),
    }
    controller, clock, callosum, logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=state,
        clock=_FakeClock(now),
    )

    assert controller.step() == cortex.SPP_RENEWAL_ACK_TIMEOUT_S
    _assert_absence_fenced_refresh_command(callosum.emitted[-1][1]["cmd"])
    ref = callosum.emitted[-1][1]["ref"]
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "started", "ref": ref}
    )
    clock.advance(1)
    state["aggregate"] = "ready"
    state["fingerprint"] = "a" * 64
    state["record_fingerprint"] = "a" * 64
    state["observed"] = clock.now
    state["expires"] = clock.now + timedelta(minutes=10)
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "stopped", "ref": ref, "exit_code": 0}
    )

    assert controller._retry_after is None
    logged = "\n".join(str(call.args) for call in logger.info.call_args_list)
    assert "verified" in logged


def test_spp_renewal_controller_absence_fenced_refresh_retries_when_key_appears(
    tmp_path,
    monkeypatch,
):
    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    state = {
        "aggregate": "unknown",
        "fingerprint": None,
        "observed": now - timedelta(minutes=9),
        "expires": now + timedelta(seconds=30),
    }
    controller, clock, callosum, _logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=state,
        clock=_FakeClock(now),
    )

    controller.step()
    _assert_absence_fenced_refresh_command(callosum.emitted[-1][1]["cmd"])
    ref = callosum.emitted[-1][1]["ref"]
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "started", "ref": ref}
    )
    state["fingerprint"] = "b" * 64
    state["record_fingerprint"] = "b" * 64
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "stopped", "ref": ref, "exit_code": 3}
    )

    assert controller._retry_after is not None
    clock.advance((controller._retry_after - clock.now).total_seconds())
    controller.step()
    _assert_fenced_refresh_command(callosum.emitted[-1][1]["cmd"], "b" * 64)


def test_spp_renewal_controller_restarts_from_ready_vs_stale_record(
    tmp_path,
    monkeypatch,
):
    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    ready_state = {
        "fingerprint": "1" * 64,
        "observed": now,
        "expires": now + timedelta(minutes=10),
    }
    ready, _clock, ready_callosum, _logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=ready_state,
        clock=_FakeClock(now),
    )

    delay = ready.step()
    assert ready_callosum.emitted == []
    assert delay > 0

    stale_state = {
        "fingerprint": "1" * 64,
        "observed": now - timedelta(minutes=20),
        "expires": now - timedelta(seconds=1),
    }
    stale, _clock, stale_callosum, _logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=stale_state,
        clock=_FakeClock(now),
    )

    stale.step()
    _assert_fenced_refresh_command(
        stale_callosum.emitted[-1][1]["cmd"],
        stale_state["fingerprint"],
    )


def test_spp_renewal_controller_pending_fingerprint_switch_is_refenced(
    tmp_path,
    monkeypatch,
):
    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    first_fingerprint = "2" * 64
    second_fingerprint = "3" * 64
    state = {
        "fingerprint": first_fingerprint,
        "observed": now - timedelta(minutes=9),
        "expires": now + timedelta(seconds=30),
    }
    controller, clock, callosum, _logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=state,
        clock=_FakeClock(now),
    )

    controller.step()
    first_ref = callosum.emitted[-1][1]["ref"]
    assert first_fingerprint in callosum.emitted[-1][1]["cmd"]
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "started", "ref": first_ref}
    )
    state["fingerprint"] = second_fingerprint
    state["record_fingerprint"] = second_fingerprint
    state["observed"] = clock.now
    state["expires"] = clock.now + timedelta(seconds=30)
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "stopped", "ref": first_ref, "exit_code": 3}
    )
    assert controller._retry_after is not None

    clock.advance((controller._retry_after - clock.now).total_seconds())
    controller.step()

    assert len(callosum.emitted) == 2
    assert second_fingerprint in callosum.emitted[-1][1]["cmd"]


def test_spp_renewal_controller_refresh_fallback_is_refenced_after_switch(
    tmp_path,
    monkeypatch,
):
    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    first_fingerprint = "2" * 64
    second_fingerprint = "3" * 64
    state = {
        "aggregate": "unknown",
        "fingerprint": first_fingerprint,
        "observed": now - timedelta(minutes=9),
        "expires": now + timedelta(seconds=30),
    }
    controller, clock, callosum, _logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=state,
        clock=_FakeClock(now),
    )

    controller.step()
    first_cmd = callosum.emitted[-1][1]["cmd"]
    _assert_fenced_refresh_command(first_cmd, first_fingerprint)
    first_ref = callosum.emitted[-1][1]["ref"]
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "started", "ref": first_ref}
    )
    state["fingerprint"] = second_fingerprint
    state["record_fingerprint"] = second_fingerprint
    state["observed"] = clock.now
    state["expires"] = clock.now + timedelta(seconds=30)
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "stopped", "ref": first_ref, "exit_code": 3}
    )
    assert controller._retry_after is not None

    clock.advance((controller._retry_after - clock.now).total_seconds())
    controller.step()

    assert len(callosum.emitted) == 2
    _assert_fenced_refresh_command(callosum.emitted[-1][1]["cmd"], second_fingerprint)


def test_spp_renewal_controller_clock_jumps_do_not_duplicate(
    tmp_path,
    monkeypatch,
):
    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    state = {
        "fingerprint": "4" * 64,
        "observed": now,
        "expires": now + timedelta(minutes=10),
    }
    controller, clock, callosum, _logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=state,
        clock=_FakeClock(now),
    )

    delay = controller.step()
    assert delay >= 0
    assert callosum.emitted == []

    clock.advance(delay + 1)
    controller.step()
    assert callosum.emitted[-1][1]["cmd"][:3] == [
        "journal",
        "brain",
        "renew-prerequisites",
    ]

    backward_state = {
        "fingerprint": "5" * 64,
        "observed": now,
        "expires": now + timedelta(minutes=10),
    }
    backward, backward_clock, backward_callosum, _logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=backward_state,
        clock=_FakeClock(now),
    )
    backward_delay = backward.step()
    backward_clock.advance(-600)
    later_delay = backward.step()

    assert backward_delay >= 0
    assert later_delay >= 0
    assert backward_callosum.emitted == []


def test_spp_renewal_controller_recovers_from_inspection_and_fingerprint_errors(
    tmp_path,
    monkeypatch,
):
    from solstone.think import cortex

    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    state = {
        "fingerprint": "6" * 64,
        "observed": now - timedelta(minutes=9),
        "expires": now + timedelta(seconds=30),
    }
    clock = _FakeClock(now)
    callosum = _FakeCallosum()
    logger = MagicMock()
    inspect_calls = 0

    def flaky_inspect(current: datetime, *, journal_path=None):
        nonlocal inspect_calls
        inspect_calls += 1
        if inspect_calls == 1:
            raise OSError("SECRET-SENTINEL path")
        return _spp_inspector(state)(current, journal_path=journal_path)

    monkeypatch.setattr(cortex, "inspect_brain_state", flaky_inspect)
    monkeypatch.setattr(
        cortex,
        "read_active_brain_fingerprint_sha256",
        lambda *, journal_path=None: state["fingerprint"],
    )
    controller = cortex.SppRenewalController(
        callosum=callosum,
        stop_event=threading.Event(),
        logger=logger,
        clock=clock,
        wait=clock.wait,
        journal_path=tmp_path,
    )

    assert controller.step() == 5.0
    clock.advance(5.0)
    controller.step()
    assert callosum.emitted[-1][1]["cmd"][:3] == [
        "journal",
        "brain",
        "renew-prerequisites",
    ]

    fingerprint_calls = 0

    def flaky_fingerprint(*, journal_path=None):
        nonlocal fingerprint_calls
        fingerprint_calls += 1
        if fingerprint_calls == 1:
            raise OSError("SECRET-SENTINEL path")
        return state["fingerprint"]

    callosum = _FakeCallosum()
    logger = MagicMock()
    monkeypatch.setattr(cortex, "inspect_brain_state", _spp_inspector(state))
    monkeypatch.setattr(
        cortex, "read_active_brain_fingerprint_sha256", flaky_fingerprint
    )
    controller = cortex.SppRenewalController(
        callosum=callosum,
        stop_event=threading.Event(),
        logger=logger,
        clock=clock,
        wait=clock.wait,
        journal_path=tmp_path,
    )

    assert controller.step() == 5.0
    clock.advance(5.0)
    controller.step()
    assert callosum.emitted[-1][1]["cmd"][:3] == [
        "journal",
        "brain",
        "renew-prerequisites",
    ]

    logged = "\n".join(
        " ".join(str(part) for part in call.args) for call in logger.info.call_args_list
    )
    assert "SECRET-SENTINEL" not in logged
    assert str(tmp_path) not in logged


def test_spp_renewal_controller_run_contains_unexpected_step_exception(
    tmp_path,
    monkeypatch,
):
    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    state = {
        "fingerprint": "8" * 64,
        "observed": now,
        "expires": now + timedelta(minutes=10),
    }
    controller, _clock, _callosum, logger = _make_spp_controller(
        tmp_path,
        monkeypatch,
        state=state,
        clock=_FakeClock(now),
    )
    controller.step = MagicMock(side_effect=RuntimeError("SECRET-SENTINEL"))

    def stop_after_wait(_seconds: float) -> bool:
        controller.stop_event.set()
        return True

    controller.wait = stop_after_wait
    controller.run()

    logged = "\n".join(
        " ".join(str(part) for part in call.args) for call in logger.info.call_args_list
    )
    assert "RuntimeError" in logged
    assert "retrying" in logged
    assert "SECRET-SENTINEL" not in logged


def test_spp_renewal_controller_retries_with_cap_and_clears_on_disable(
    tmp_path,
    monkeypatch,
):
    from solstone.think import cortex

    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    clock = _FakeClock(now)
    state = {
        "fingerprint": "c" * 64,
        "observed": now - timedelta(minutes=9),
        "expires": now + timedelta(seconds=30),
    }
    monkeypatch.setattr(cortex, "inspect_brain_state", _spp_inspector(state))
    monkeypatch.setattr(
        cortex,
        "read_active_brain_fingerprint_sha256",
        lambda *, journal_path=None: state["fingerprint"],
    )
    controller = cortex.SppRenewalController(
        callosum=_FakeCallosum(),
        stop_event=threading.Event(),
        logger=MagicMock(),
        clock=clock,
        wait=clock.wait,
        journal_path=tmp_path,
    )

    delays: list[float] = []
    for _ in range(6):
        controller.step()
        clock.advance(cortex.SPP_RENEWAL_ACK_TIMEOUT_S + 1)
        controller.step()
        assert controller._retry_after is not None
        delays.append((controller._retry_after - clock.now).total_seconds())
        clock.advance(delays[-1])
    assert delays == [5.0, 10.0, 20.0, 40.0, 60.0, 60.0]

    controller.step()
    assert controller._pending_ref is not None
    state["lane"] = "byo-cloud"
    controller.step()
    assert controller._pending_ref is None
    assert controller._retry_after is None


def test_spp_renewal_controller_logs_lifecycle_events_without_secrets(
    tmp_path,
    monkeypatch,
):
    from solstone.think import cortex

    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    clock = _FakeClock(now)
    state = {
        "lane": "byo-cloud",
        "fingerprint": "d" * 64,
        "observed": now - timedelta(minutes=9),
        "expires": now + timedelta(seconds=30),
        "secret": "SECRET-SENTINEL",
    }
    monkeypatch.setattr(cortex, "inspect_brain_state", _spp_inspector(state))
    monkeypatch.setattr(
        cortex,
        "read_active_brain_fingerprint_sha256",
        lambda *, journal_path=None: state["fingerprint"],
    )
    logger = MagicMock()
    callosum = _FakeCallosum()
    controller = cortex.SppRenewalController(
        callosum=callosum,
        stop_event=threading.Event(),
        logger=logger,
        clock=clock,
        wait=clock.wait,
        journal_path=tmp_path,
    )

    assert controller.step() == 30.0
    assert callosum.emitted == []

    state["lane"] = "spp"
    state["expires"] = clock.now + timedelta(minutes=10)
    scheduled_delay = controller.step()
    assert scheduled_delay > 0
    clock.advance(scheduled_delay)
    controller.step()
    assert callosum.emitted[-1][1]["cmd"][:3] == [
        "journal",
        "brain",
        "renew-prerequisites",
    ]
    verified_ref = callosum.emitted[-1][1]["ref"]
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "started", "ref": verified_ref}
    )
    clock.advance(1)
    state["observed"] = clock.now
    state["expires"] = clock.now + timedelta(minutes=10)
    controller.handle_supervisor_message(
        {
            "tract": "supervisor",
            "event": "stopped",
            "ref": verified_ref,
            "exit_code": 0,
        }
    )

    state["expires"] = clock.now + timedelta(seconds=30)
    controller.step()
    failed_ref = callosum.emitted[-1][1]["ref"]
    clock.advance(cortex.SPP_RENEWAL_ACK_TIMEOUT_S + 1)
    controller.step()
    clock.advance(5.0)
    controller.step()
    stale_ref = callosum.emitted[-1][1]["ref"]
    assert stale_ref != failed_ref
    controller.handle_supervisor_message(
        {"tract": "supervisor", "event": "started", "ref": stale_ref}
    )
    clock.advance(cortex.SPP_RENEWAL_ATTEMPT_BOUND_S + 1)
    controller.step()

    logged = "\n".join(
        " ".join(str(part) for part in call.args) for call in logger.info.call_args_list
    )
    for event in (
        "disabled",
        "scheduled",
        "in_flight",
        "verified",
        "failed",
        "stale",
        "retrying",
    ):
        assert event in logged
    assert "SECRET-SENTINEL" not in logged
    assert str(tmp_path) not in logged
    assert "SECRET-SENTINEL" not in json.dumps(callosum.emitted)


def test_cortex_stop_wakes_and_joins_spp_renewal_worker(
    mock_journal,
    monkeypatch,
):
    from solstone.think import cortex
    from solstone.think.cortex import CortexService

    now = datetime(2026, 7, 24, 12, 0, tzinfo=timezone.utc)
    state = {"lane": "byo-cloud", "fingerprint": "7" * 64}
    monkeypatch.setattr(cortex, "inspect_brain_state", _spp_inspector(state))
    service = CortexService(str(mock_journal), clock=lambda: now)
    service.callosum = MagicMock()

    service._start_spp_renewal_controller()
    assert service._spp_renewal_worker is not None
    deadline = time.monotonic() + 1.0
    while not service._spp_renewal_worker.is_alive() and time.monotonic() < deadline:
        time.sleep(0.01)
    assert service._spp_renewal_worker.is_alive()

    service.stop()

    assert not service._spp_renewal_worker.is_alive()


def test_handle_request_dedups_existing_active_file(
    cortex_service, mock_journal, monkeypatch
):
    """A re-broadcast with the same use_id must not spawn twice."""
    spawn_calls = []

    def fake_spawn_subprocess(use_id, file_path, request, cmd, process_type):
        spawn_calls.append((use_id, file_path, request, cmd, process_type))

    monkeypatch.setattr(cortex_service, "_spawn_subprocess", fake_spawn_subprocess)
    request = {
        "tract": "cortex",
        "event": "request",
        "use_id": "1713629000005",
        "prompt": "Test prompt",
        "provider": "openai",
        "name": "chat",
    }

    cortex_service._handle_request(request)
    cortex_service._handle_request(dict(request))

    active_path = mock_journal / "talents" / "chat" / "1713629000005_active.jsonl"
    assert active_path.exists()

    cortex_service._spawn_worker = threading.Thread(
        target=cortex_service._run_spawn_worker,
        daemon=True,
    )
    cortex_service._spawn_worker.start()
    cortex_service.spawn_queue.join()
    cortex_service.stop_event.set()
    cortex_service._spawn_worker.join(timeout=1)

    assert len(spawn_calls) == 1
    assert spawn_calls[0][0] == "1713629000005"


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


def test_slow_spawn_does_not_block_request_callback(cortex_service, mock_journal):
    """AC1: a blocked spawn worker does not block later request claims."""
    block = threading.Event()
    started = threading.Event()
    spawned: list[str] = []

    def slow_spawn(use_id, *_args):
        spawned.append(use_id)
        started.set()
        block.wait(timeout=5)

    with patch.object(cortex_service, "_spawn_subprocess", side_effect=slow_spawn):
        cortex_service._spawn_worker = threading.Thread(
            target=cortex_service._run_spawn_worker,
            daemon=True,
        )
        cortex_service._spawn_worker.start()

        cortex_service._handle_callosum_message(_cortex_request("slow_1"))
        assert started.wait(1)

        before = time.monotonic()
        cortex_service._handle_callosum_message(_cortex_request("slow_2"))
        elapsed = time.monotonic() - before

        assert elapsed < 0.5
        assert _active_path(mock_journal, "slow_2").exists()
        assert cortex_service.spawn_queue.qsize() >= 1
        assert spawned == ["slow_1"]

        cortex_service.stop_event.set()
        block.set()
        cortex_service._spawn_worker.join(timeout=1)


def test_cancel_callback_queues_without_blocking_stop(cortex_service):
    block = threading.Event()
    started = threading.Event()
    cancelled: list[str] = []

    def slow_cancel(use_id: str, _reason_code: str) -> None:
        cancelled.append(use_id)
        started.set()
        block.wait(timeout=5)

    with patch.object(cortex_service, "_cancel_talent_use", side_effect=slow_cancel):
        cortex_service._cancel_worker = threading.Thread(
            target=cortex_service._run_cancel_worker,
            daemon=True,
        )
        cortex_service._cancel_worker.start()

        before = time.monotonic()
        cortex_service._handle_callosum_message(
            {
                "tract": "cortex",
                "event": "cancel",
                "use_id": "cancel_slow_1",
            }
        )
        elapsed = time.monotonic() - before

        assert elapsed < 0.5
        assert started.wait(1)
        assert cancelled == ["cancel_slow_1"]

        cortex_service.stop_event.set()
        block.set()
        cortex_service._cancel_worker.join(timeout=1)


def test_claim_latency_independent_of_queue_depth(cortex_service, mock_journal):
    """AC2: request handling claims and queues without spawning inline."""
    with patch.object(cortex_service, "_spawn_subprocess") as mock_spawn:
        cortex_service._handle_callosum_message(_cortex_request("claim_only"))

    assert _active_path(mock_journal, "claim_only").exists()
    assert cortex_service.spawn_queue.qsize() == 1
    mock_spawn.assert_not_called()


def test_spawn_worker_processes_fifo(cortex_service):
    """AC3: the dedicated worker drains queued spawns in arrival order."""
    use_ids = ["fifo_1", "fifo_2", "fifo_3"]
    spawned: list[str] = []

    with patch.object(
        cortex_service,
        "_spawn_subprocess",
        side_effect=lambda use_id, *_args: spawned.append(use_id),
    ):
        for use_id in use_ids:
            cortex_service._handle_callosum_message(_cortex_request(use_id))

        cortex_service._spawn_worker = threading.Thread(
            target=cortex_service._run_spawn_worker,
            daemon=True,
        )
        cortex_service._spawn_worker.start()

        cortex_service.spawn_queue.join()
        cortex_service.stop_event.set()
        cortex_service._spawn_worker.join(timeout=1)

    assert spawned == use_ids


def test_duplicate_claim_is_not_enqueued_twice(cortex_service, mock_journal):
    """AC4: duplicate use_ids preserve the single active-file claim semantics."""
    use_id = "dedup_1"
    cortex_service._handle_callosum_message(_cortex_request(use_id))
    cortex_service._handle_callosum_message(_cortex_request(use_id))

    assert _active_path(mock_journal, use_id).exists()
    assert len(list((mock_journal / "talents" / "chat").glob("*_active.jsonl"))) == 1
    assert cortex_service.spawn_queue.qsize() == 1
    assert cortex_service._pending_spawns == 1

    with patch.object(cortex_service, "_spawn_subprocess") as mock_spawn:
        cortex_service._spawn_worker = threading.Thread(
            target=cortex_service._run_spawn_worker,
            daemon=True,
        )
        cortex_service._spawn_worker.start()

        cortex_service.spawn_queue.join()
        cortex_service.stop_event.set()
        cortex_service._spawn_worker.join(timeout=1)

    mock_spawn.assert_called_once()


def test_spawn_worker_isolates_per_item_failures(cortex_service, mock_journal):
    """AC5: one unexpected spawn failure terminalizes that use and keeps draining."""
    spawned: list[str] = []

    def spawn_or_raise(use_id, *_args):
        if use_id == "bad":
            raise RuntimeError("boom")
        spawned.append(use_id)

    with patch.object(cortex_service, "_spawn_subprocess", side_effect=spawn_or_raise):
        cortex_service._spawn_worker = threading.Thread(
            target=cortex_service._run_spawn_worker,
            daemon=True,
        )
        cortex_service._spawn_worker.start()

        cortex_service._handle_callosum_message(_cortex_request("bad"))
        cortex_service._handle_callosum_message(_cortex_request("good"))

        cortex_service.spawn_queue.join()

        completed = _completed_path(mock_journal, "bad")
        assert completed.exists()
        assert not _active_path(mock_journal, "bad").exists()
        assert any(event.get("event") == "error" for event in _read_jsonl(completed))
        assert spawned == ["good"]
        assert cortex_service._spawn_worker.is_alive()

        cortex_service.stop_event.set()
        cortex_service._spawn_worker.join(timeout=1)


def test_idle_predicate_includes_pending_spawns(cortex_service):
    """AC6: idle is false while a claimed request is queued for spawn."""
    cortex_service._handle_callosum_message(_cortex_request("idle_1"))

    assert not cortex_service._is_idle()
    assert not cortex_service._is_idle()
    assert cortex_service.spawn_queue.qsize() == 1
    assert cortex_service._pending_spawns == 1

    cortex_service.spawn_queue.get_nowait()
    with cortex_service.lock:
        cortex_service._pending_spawns -= 1

    assert cortex_service._is_idle()
    assert cortex_service._is_idle()
    assert cortex_service.spawn_queue.qsize() == 0
    assert cortex_service._pending_spawns == 0


def test_stop_terminalizes_queued_claims(cortex_service, mock_journal):
    """AC7: stopping drains queued claims after the worker exits."""
    use_ids = ["stop_1", "stop_2", "stop_3"]
    for use_id in use_ids:
        cortex_service._handle_callosum_message(_cortex_request(use_id))

    assert cortex_service.spawn_queue.qsize() == 3

    before = time.monotonic()
    cortex_service.stop()
    elapsed = time.monotonic() - before

    assert elapsed < 1
    assert cortex_service.spawn_queue.qsize() == 0
    assert cortex_service._pending_spawns == 0
    for use_id in use_ids:
        active = _active_path(mock_journal, use_id)
        completed = _completed_path(mock_journal, use_id)
        assert not active.exists()
        assert completed.exists()
        assert any(event.get("event") == "error" for event in _read_jsonl(completed))


def test_emit_status_once_includes_queue_depth(cortex_service):
    """AC8: status emits queue_depth for queued work and stays quiet when idle."""
    cortex_service._handle_callosum_message(_cortex_request("status_1"))
    cortex_service._handle_callosum_message(_cortex_request("status_2"))
    cortex_service.callosum = MagicMock()

    cortex_service._emit_status_once()

    cortex_service.callosum.emit.assert_called_once()
    assert cortex_service.callosum.emit.call_args.args[:2] == ("cortex", "status")
    assert cortex_service.callosum.emit.call_args.kwargs["running_uses"] == 0
    assert cortex_service.callosum.emit.call_args.kwargs["uses"] == []
    assert cortex_service.callosum.emit.call_args.kwargs["queue_depth"] == 2

    while cortex_service.spawn_queue.qsize():
        cortex_service.spawn_queue.get_nowait()
        with cortex_service.lock:
            cortex_service._pending_spawns -= 1
    cortex_service.callosum.emit.reset_mock()

    cortex_service._emit_status_once()

    cortex_service.callosum.emit.assert_not_called()


def test_monitor_stdout_json_events(cortex_service, mock_journal):
    """Test monitoring stdout with JSON events."""
    from io import StringIO

    from solstone.think.cortex import TalentProcess

    use_id = "123456789"
    log_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    log_path.touch()

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
    log_path.touch()

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
    log_path.touch()

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
    """Test monitoring stderr collects trace without writing an event."""
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

    assert agent.stderr_lines == [
        "Error: Something went wrong",
        "Stack trace line 1",
        "Stack trace line 2",
    ]
    assert not log_path.exists()


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


def test_append_day_index_preserves_degraded_marker(cortex_service, mock_journal):
    """Test day-index summaries carry degraded finish markers."""
    use_id = "1234567890000"
    completed_path = mock_journal / "talents" / f"{use_id}.jsonl"
    completed_path.write_text(
        json.dumps(
            {
                "event": "start",
                "ts": 1000,
                "model": "claude-haiku-4-5",
            }
        )
        + "\n"
        + json.dumps(
            {
                "event": "finish",
                "ts": 2000,
                "result": "x",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 7,
                    "total_tokens": 17,
                },
                "degraded": {"reason": "near_empty", "output_tokens": 7},
            }
        )
        + "\n",
        encoding="utf-8",
    )
    request = {
        "name": "morning_briefing",
        "day": "20260410",
        "ts": 1000,
        "provider": "anthropic",
        "model": "claude-haiku-4-5",
    }

    cortex_service._append_day_index(use_id, request, completed_path)

    day_index_path = mock_journal / "talents" / "20260410.jsonl"
    row = json.loads(day_index_path.read_text(encoding="utf-8").strip())
    assert row["degraded"] == {"reason": "near_empty", "output_tokens": 7}


def test_append_day_index_carries_error_reason_code(cortex_service, mock_journal):
    """Test day-index error summaries carry provider reason codes additively."""
    use_id = "1234567890001"
    completed_path = mock_journal / "talents" / f"{use_id}.jsonl"
    completed_path.write_text(
        json.dumps(
            {
                "event": "start",
                "ts": 1000,
                "model": "claude-haiku-4-5",
            }
        )
        + "\n"
        + json.dumps(
            {
                "event": "error",
                "ts": 2000,
                "error": "provider setup blocked",
                "reason_code": "provider_key_missing",
            }
        )
        + "\n",
        encoding="utf-8",
    )
    request = {
        "name": "morning_briefing",
        "day": "20260410",
        "ts": 1000,
        "provider": "anthropic",
        "model": "claude-haiku-4-5",
    }

    cortex_service._append_day_index(use_id, request, completed_path)

    day_index_path = mock_journal / "talents" / "20260410.jsonl"
    row = json.loads(day_index_path.read_text(encoding="utf-8").strip())
    assert row["status"] == "error"
    assert row["reason_code"] == "provider_key_missing"
    assert row["provider"] == "anthropic"
    assert row["model"] == "claude-haiku-4-5"


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


def test_watchdog_finalizes_hung_stdout_pipe(cortex_service, mock_journal):
    """A grandchild-held stdout pipe is finalized by the watchdog."""
    from solstone.think.cortex import TalentProcess
    from solstone.think.cortex_client import get_use_end_state

    use_id = "1234567891000"
    day = "20260410"
    talent_dir = mock_journal / "talents" / "chat"
    talent_dir.mkdir()
    active_path = talent_dir / f"{use_id}_active.jsonl"
    request = {
        "event": "request",
        "use_id": use_id,
        "ts": 1000,
        "name": "chat",
        "day": day,
    }
    active_path.write_text(json.dumps(request) + "\n", encoding="utf-8")

    child_code = (
        "import os, time\n"
        'print(\'{"event":"start","ts":1001}\', flush=True)\n'
        "pid = os.fork()\n"
        "if pid == 0:\n"
        "    time.sleep(30)\n"
        "    os._exit(0)\n"
        "os._exit(0)\n"
    )
    process = subprocess.Popen(
        [sys.executable, "-c", child_code],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
        process_group=0,
    )
    agent = TalentProcess(use_id, process, active_path)
    cortex_service.running_uses[use_id] = agent
    cortex_service.use_requests[use_id] = request

    monitor_done = threading.Event()
    timeout_done = threading.Event()

    def monitor_stdout():
        try:
            cortex_service._monitor_stdout(agent)
        finally:
            monitor_done.set()

    def timeout_talent():
        try:
            cortex_service._timeout_talent(use_id, agent, 0.1)
        finally:
            timeout_done.set()

    monitor = threading.Thread(target=monitor_stdout)
    monitor.start()
    agent.timeout_timer = threading.Timer(0.1, timeout_talent)
    agent.timeout_timer.start()

    completed = monitor_done.wait(5)
    timeout_completed = timeout_done.wait(5)
    if not completed:
        agent.stop()
    monitor.join(timeout=5)

    assert completed
    assert timeout_completed
    assert not monitor.is_alive()
    completed_path = talent_dir / f"{use_id}.jsonl"
    assert completed_path.exists()
    assert not active_path.exists()
    assert use_id not in cortex_service.running_uses
    assert get_use_end_state(use_id) == "error"
    assert any(event.get("event") == "error" for event in _read_jsonl(completed_path))


def test_timeout_finalize_claim_beats_late_stdout_cleanup(cortex_service, mock_journal):
    """Timeout finalization wins once; later stdout cleanup is a no-op loser."""
    from solstone.think.cortex import TalentProcess

    use_id = "1234567891001"
    day = "20260410"
    talent_dir = mock_journal / "talents" / "chat"
    talent_dir.mkdir()
    active_path = talent_dir / f"{use_id}_active.jsonl"
    request = {
        "event": "request",
        "use_id": use_id,
        "ts": 1000,
        "name": "chat",
        "day": day,
    }
    active_path.write_text(json.dumps(request) + "\n", encoding="utf-8")

    mock_process = MagicMock()
    mock_process.pid = 24680
    mock_process.wait.return_value = 0
    mock_process.stdout = MockPipe([])
    agent = TalentProcess(use_id, mock_process, active_path)
    cortex_service.running_uses[use_id] = agent
    cortex_service.use_requests[use_id] = request

    with patch.object(agent, "_signal_process_group"):
        cortex_service._timeout_talent(use_id, agent, 1)

    completed_path = talent_dir / f"{use_id}.jsonl"
    after_timeout = completed_path.read_bytes()
    day_index = mock_journal / "talents" / f"{day}.jsonl"
    assert len(day_index.read_text(encoding="utf-8").splitlines()) == 1

    cortex_service._monitor_stdout(agent)

    assert not active_path.exists()
    assert completed_path.read_bytes() == after_timeout
    summaries = [
        json.loads(line)
        for line in day_index.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    assert [summary["use_id"] for summary in summaries].count(use_id) == 1


def test_cancel_edge_cases_are_idempotent_without_spawning(
    cortex_service, mock_journal
):
    cortex_service.callosum = MagicMock()
    with patch.object(cortex_service, "_spawn_subprocess") as mock_spawn:
        cortex_service._cancel_talent_use("unknown_use", "chat_watchdog_cancelled")
        cortex_service._handle_callosum_message({"tract": "cortex", "event": "cancel"})
        cortex_service._handle_callosum_message(
            {"tract": "cortex", "event": "cancel", "use_id": {"bad": "shape"}}
        )

        completed_dir = mock_journal / "talents" / "chat"
        completed_dir.mkdir()
        finalized = completed_dir / "already_done.jsonl"
        finalized.write_text('{"event":"finish","use_id":"already_done"}\n')
        before_finalized = finalized.read_bytes()
        cortex_service._cancel_talent_use("already_done", "chat_watchdog_cancelled")

        agent, active_path = _make_live_agent(
            cortex_service, mock_journal, "live_cancel_twice"
        )
        with patch.object(agent, "_signal_process_group"):
            cortex_service._cancel_talent_use(
                "live_cancel_twice", "chat_watchdog_cancelled"
            )
            cortex_service._cancel_talent_use(
                "live_cancel_twice", "chat_watchdog_cancelled"
            )

    completed_path = _completed_path(mock_journal, "live_cancel_twice")
    rows = _read_jsonl(completed_path)
    terminal_errors = [row for row in rows if row.get("event") == "error"]
    assert not _active_path(mock_journal, "unknown_use").exists()
    assert finalized.read_bytes() == before_finalized
    assert not active_path.exists()
    assert len(terminal_errors) == 1
    assert terminal_errors[0]["reason_code"] == "chat_watchdog_cancelled"
    mock_spawn.assert_not_called()


def test_cancel_reason_code_does_not_change_timeout_reason_shape(
    cortex_service, mock_journal
):
    cortex_service.callosum = MagicMock()
    cancel_agent, _cancel_active_path = _make_live_agent(
        cortex_service, mock_journal, "reason_cancel"
    )
    with patch.object(cancel_agent, "_signal_process_group"):
        cortex_service._cancel_talent_use("reason_cancel", "chat_watchdog_cancelled")

    timeout_agent, _timeout_active_path = _make_live_agent(
        cortex_service, mock_journal, "reason_timeout"
    )
    with patch.object(timeout_agent, "_signal_process_group"):
        cortex_service._timeout_talent("reason_timeout", timeout_agent, 1)

    cancel_error = [
        row
        for row in _read_jsonl(_completed_path(mock_journal, "reason_cancel"))
        if row.get("event") == "error"
    ][0]
    timeout_error = [
        row
        for row in _read_jsonl(_completed_path(mock_journal, "reason_timeout"))
        if row.get("event") == "error"
    ][0]
    assert cancel_error["reason_code"] == "chat_watchdog_cancelled"
    assert "reason_code" not in timeout_error


def test_cancel_racing_real_finish_completes_once(cortex_service, mock_journal):
    cortex_service.callosum = MagicMock()
    use_id = "cancel_race_finish"
    agent, active_path = _make_live_agent(cortex_service, mock_journal, use_id)
    agent.process.stdout = MockPipe(
        [
            json.dumps(
                {
                    "event": "finish",
                    "use_id": use_id,
                    "ts": 1001,
                    "result": "already emitted",
                }
            )
            + "\n"
        ]
    )
    finish_appended = threading.Event()
    release_finish = threading.Event()
    original_append = cortex_service._append_use_event

    def append_spy(use_id_arg, active_path_arg, event):
        appended = original_append(use_id_arg, active_path_arg, event)
        if event.get("event") == "finish":
            finish_appended.set()
            release_finish.wait(timeout=1)
        return appended

    monitor = threading.Thread(target=cortex_service._monitor_stdout, args=(agent,))
    with patch.object(cortex_service, "_append_use_event", side_effect=append_spy):
        monitor.start()
        assert finish_appended.wait(1)
        with patch.object(agent, "_signal_process_group"):
            cortex_service._cancel_talent_use(use_id, "chat_watchdog_cancelled")
        release_finish.set()
        monitor.join(timeout=1)

    assert not monitor.is_alive()

    completed_path = _completed_path(mock_journal, use_id)
    rows = _read_jsonl(completed_path)
    day_index = mock_journal / "talents" / "20260410.jsonl"
    assert completed_path.exists()
    assert not active_path.exists()
    assert sum(1 for row in rows if row.get("event") == "finish") == 1
    terminal_errors = [row for row in rows if row.get("event") == "error"]
    assert len(terminal_errors) == 1
    assert terminal_errors[0]["reason_code"] == "chat_watchdog_cancelled"
    assert len(day_index.read_text(encoding="utf-8").splitlines()) == 1


def test_cancel_claim_first_then_late_finish_appends_before_complete(
    cortex_service, mock_journal
):
    from solstone.think.cortex_client import get_use_end_state

    cortex_service.callosum = MagicMock()
    use_id = "cancel_claim_first_late_finish"
    agent, active_path = _make_live_agent(cortex_service, mock_journal, use_id)
    agent.process.stdout = MockPipe(
        [
            json.dumps(
                {
                    "event": "finish",
                    "use_id": use_id,
                    "ts": 1002,
                    "result": "late emitted",
                }
            )
            + "\n"
        ]
    )
    cancel_error_appended = threading.Event()
    allow_cancel_complete = threading.Event()
    cancel_done = threading.Event()
    original_append = cortex_service._append_use_event

    def append_spy(use_id_arg, active_path_arg, event):
        appended = original_append(use_id_arg, active_path_arg, event)
        if (
            event.get("event") == "error"
            and event.get("reason_code") == "chat_watchdog_cancelled"
        ):
            cancel_error_appended.set()
            allow_cancel_complete.wait(timeout=1)
        return appended

    def cancel_use():
        try:
            cortex_service._cancel_talent_use(use_id, "chat_watchdog_cancelled")
        finally:
            cancel_done.set()

    cancel_thread = threading.Thread(target=cancel_use)
    with patch.object(cortex_service, "_append_use_event", side_effect=append_spy):
        with patch.object(agent, "_signal_process_group"):
            cancel_thread.start()
            try:
                assert cancel_error_appended.wait(1)
                assert use_id not in cortex_service.running_uses
                assert active_path.exists()
                cortex_service._monitor_stdout(agent)
            finally:
                allow_cancel_complete.set()
                cancel_thread.join(timeout=1)

    assert cancel_done.is_set()
    assert not cancel_thread.is_alive()

    completed_path = _completed_path(mock_journal, use_id)
    rows = _read_jsonl(completed_path)
    day_index = mock_journal / "talents" / "20260410.jsonl"
    day_rows = _read_jsonl(day_index)
    assert completed_path.exists()
    assert not active_path.exists()
    assert [row.get("event") for row in rows] == ["request", "error", "finish"]
    assert rows[1]["reason_code"] == "chat_watchdog_cancelled"
    assert rows[2]["result"] == "late emitted"
    assert cortex_service._has_finish_event(completed_path) is True
    assert get_use_end_state(use_id) == "finish"
    assert len(day_rows) == 1
    assert day_rows[0]["status"] == "completed"


def test_monitor_stderr_after_completion_does_not_resurrect_active_log(
    cortex_service, mock_journal
):
    """Late stderr collection after completion does not recreate active logs."""
    from io import StringIO

    from solstone.think.cortex import TalentProcess

    use_id = "1234567891002"
    talent_dir = mock_journal / "talents" / "chat"
    talent_dir.mkdir()
    active_path = talent_dir / f"{use_id}_active.jsonl"
    active_path.write_text('{"event":"request","name":"chat"}\n', encoding="utf-8")
    cortex_service.use_requests[use_id] = {"name": "chat"}
    cortex_service._complete_use_file(use_id, active_path)

    mock_process = MagicMock()
    mock_process.stderr = StringIO("late stderr\n")
    mock_process.poll.return_value = 1
    agent = TalentProcess(use_id, mock_process, active_path)

    cortex_service._monitor_stderr(agent)

    assert not active_path.exists()
    assert (talent_dir / f"{use_id}.jsonl").exists()
    assert agent.stderr_lines == ["late stderr"]


def test_nonzero_exit_with_stderr_writes_single_terminal_error(
    cortex_service, mock_journal
):
    """Stdout finalizer writes the only terminal error and includes stderr trace."""
    from io import StringIO

    from solstone.think.cortex import TalentProcess

    use_id = "1234567891003"
    day = "20260410"
    talent_dir = mock_journal / "talents" / "chat"
    talent_dir.mkdir()
    active_path = talent_dir / f"{use_id}_active.jsonl"
    request = {
        "event": "request",
        "use_id": use_id,
        "ts": 1000,
        "name": "chat",
        "day": day,
    }
    active_path.write_text(json.dumps(request) + "\n", encoding="utf-8")

    mock_process = MagicMock()
    mock_process.poll.return_value = 1
    mock_process.wait.return_value = 1
    mock_process.stderr = StringIO("stderr line 1\nstderr line 2\n")
    mock_process.stdout = StringIO('{"event": "start", "ts": 1001}\n')
    agent = TalentProcess(use_id, mock_process, active_path)
    cortex_service.running_uses[use_id] = agent
    cortex_service.use_requests[use_id] = request

    cortex_service._monitor_stderr(agent)
    cortex_service._monitor_stdout(agent)

    completed_path = talent_dir / f"{use_id}.jsonl"
    events = _read_jsonl(completed_path)
    terminal_errors = [
        event
        for event in events
        if event.get("event") == "error" and event.get("terminal", True)
    ]
    assert len(terminal_errors) == 1
    assert terminal_errors[0]["trace"] == "stderr line 1\nstderr line 2"
    assert terminal_errors[0]["exit_code"] == 1


def test_spawn_failure_completes_and_clears_state(cortex_service, mock_journal):
    """Spawn failures complete as errors and clear request/running state."""
    use_id = "1234567891004"
    active_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    request = {"event": "request", "use_id": use_id, "name": "chat", "day": "20260410"}
    active_path.write_text(json.dumps(request) + "\n", encoding="utf-8")

    mock_process = MagicMock()
    mock_process.stdin.write.side_effect = BrokenPipeError()
    mock_process.pid = 12345

    with patch("solstone.think.cortex.subprocess.Popen", return_value=mock_process):
        cortex_service._spawn_subprocess(
            use_id,
            active_path,
            request,
            [sys.executable, "-m", "solstone.think.talents"],
            "generate",
        )

    completed_path = mock_journal / "talents" / f"{use_id}.jsonl"
    assert completed_path.exists()
    assert use_id not in cortex_service.running_uses
    assert use_id not in cortex_service.use_requests
    mock_process.kill.assert_called_once()
    mock_process.wait.assert_called_once_with(timeout=5)
    assert any(event.get("event") == "error" for event in _read_jsonl(completed_path))

    popen_use_id = "1234567891005"
    popen_active_path = mock_journal / "talents" / f"{popen_use_id}_active.jsonl"
    popen_request = {
        "event": "request",
        "use_id": popen_use_id,
        "name": "chat",
        "day": "20260410",
    }
    popen_active_path.write_text(json.dumps(popen_request) + "\n", encoding="utf-8")

    with patch(
        "solstone.think.cortex.subprocess.Popen", side_effect=OSError("spawn boom")
    ):
        cortex_service._spawn_subprocess(
            popen_use_id,
            popen_active_path,
            popen_request,
            [sys.executable, "-m", "solstone.think.talents"],
            "generate",
        )

    popen_completed_path = mock_journal / "talents" / f"{popen_use_id}.jsonl"
    assert popen_completed_path.exists()
    assert popen_use_id not in cortex_service.running_uses
    assert popen_use_id not in cortex_service.use_requests
    assert any(
        event.get("event") == "error" for event in _read_jsonl(popen_completed_path)
    )


def test_normal_completion_cancels_timeout_timer(
    cortex_service, mock_journal, monkeypatch
):
    """A normally completed use cancels its timer before the watchdog fires."""
    from io import StringIO

    from solstone.think.cortex import TalentProcess

    use_id = "1234567891006"
    day = "20260410"
    talent_dir = mock_journal / "talents" / "chat"
    talent_dir.mkdir()
    active_path = talent_dir / f"{use_id}_active.jsonl"
    request = {
        "event": "request",
        "use_id": use_id,
        "ts": 1000,
        "name": "chat",
        "day": day,
    }
    active_path.write_text(json.dumps(request) + "\n", encoding="utf-8")

    fired = threading.Event()

    def fake_timeout(*_args):
        fired.set()

    monkeypatch.setattr(cortex_service, "_timeout_talent", fake_timeout)

    mock_process = MagicMock()
    mock_process.stdout = StringIO('{"event": "finish", "ts": 1001}\n')
    mock_process.wait.return_value = 0
    agent = TalentProcess(use_id, mock_process, active_path)
    cortex_service.running_uses[use_id] = agent
    cortex_service.use_requests[use_id] = request
    agent.timeout_timer = threading.Timer(
        0.05, lambda: cortex_service._timeout_talent(use_id, agent, 0.05)
    )
    agent.timeout_timer.start()

    cortex_service._monitor_stdout(agent)

    assert not fired.wait(0.2)


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
    active_path.touch()
    cortex_service.use_requests = {
        use_id: {
            "event": "request",
            "prompt": "test",
            "name": "test_agent",
            "model": "claude-haiku-4-5",
            "type": "cogitate",
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


def test_native_finish_usage_handoffs_to_cortex_token_logging(
    cortex_service, mock_journal, tmp_path, monkeypatch
):
    """A native finish usage event survives the talent-to-Cortex handoff."""
    from solstone.think import talents
    from solstone.think.cortex import TalentProcess

    usage = {
        "input_tokens": 10,
        "output_tokens": 5,
        "total_tokens": 15,
        "model_version": "gemini-native",
    }
    binary = tmp_path / "fake-solstone-core"
    binary.write_text(
        "#!/usr/bin/env python3\n"
        "import json\n"
        "import sys\n"
        "sys.stdin.read()\n"
        f"print({json.dumps({'event': 'finish', 'terminal': True, 'result': 'done', 'usage': usage})!r}, flush=True)\n",
        encoding="utf-8",
    )
    binary.chmod(0o700)
    monkeypatch.setattr("solstone.think.cogitate_client._native_binary", lambda: binary)

    talent_events: list[dict] = []
    asyncio.run(
        talents._execute_with_tools(
            {
                "provider": "google",
                "model": "gemini-test",
                "name": "native_usage",
                "type": "cogitate",
                "prompt": "test",
                "output": "md",
                "output_path": None,
            },
            talent_events.append,
        )
    )

    use_id = "native_usage_handoff"
    active_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    active_path.touch()
    cortex_service.use_requests = {
        use_id: {
            "event": "request",
            "name": "native_usage",
            "model": "gemini-test",
            "type": "cogitate",
        }
    }
    mock_process = MagicMock()
    mock_process.stdout = MockPipe([json.dumps(talent_events[0]) + "\n"])
    mock_process.wait.return_value = 0
    agent = TalentProcess(use_id, mock_process, active_path)

    with patch("solstone.think.models.log_token_usage") as mock_log_token_usage:
        with patch.object(cortex_service, "_complete_use_file"):
            cortex_service._monitor_stdout(agent)

    mock_log_token_usage.assert_called_once()
    assert mock_log_token_usage.call_args.kwargs["usage"] == usage
    assert mock_log_token_usage.call_args.kwargs["model"] == "gemini-native"


def test_monitor_stdout_finish_falls_back_to_request_model(
    cortex_service, mock_journal
):
    """Test finish usage without model_version uses request model for token logging."""
    from solstone.think.cortex import TalentProcess

    use_id = "request_model_test"
    active_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    active_path.touch()
    cortex_service.use_requests = {
        use_id: {
            "event": "request",
            "prompt": "test",
            "name": "test_agent",
            "model": "claude-haiku-4-5",
            "type": "cogitate",
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


def test_monitor_stdout_finish_generate_skips_token_logging(
    cortex_service, mock_journal
):
    """Test generate finish usage is not logged again by cortex."""
    from solstone.think.cortex import TalentProcess

    use_id = "generate_usage_test"
    active_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    active_path.touch()
    cortex_service.use_requests = {
        use_id: {
            "event": "request",
            "prompt": "test",
            "name": "test_agent",
            "model": "claude-haiku-4-5",
            "type": "generate",
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

    # Generate talents self-log inside the subprocess; cortex must not duplicate it.
    mock_log_token_usage.assert_not_called()


def test_monitor_stdout_terminal_error_logs_cogitate_usage_only(
    cortex_service, mock_journal
):
    """Test terminal error usage is logged only for cogitate requests."""
    from solstone.think.cortex import TalentProcess

    usage = {
        "input_tokens": 10,
        "output_tokens": 5,
        "total_tokens": 15,
        "model_version": "claude-haiku-4-5-20251001",
    }

    def run_terminal_event(use_id: str, request_type: str, event: dict):
        active_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
        active_path.touch()
        cortex_service.use_requests = {
            use_id: {
                "event": "request",
                "prompt": "test",
                "name": "test_agent",
                "model": "claude-haiku-4-5",
                "type": request_type,
            }
        }

        mock_process = MagicMock()
        mock_process.stdout = MockPipe(
            [
                '{"event": "start", "ts": 1000}\n',
                json.dumps(event) + "\n",
            ]
        )
        mock_process.wait.return_value = 0

        agent = TalentProcess(use_id, mock_process, active_path)

        with patch("solstone.think.models.log_token_usage") as mock_log_token_usage:
            with patch.object(cortex_service, "_complete_use_file"):
                cortex_service._monitor_stdout(agent)
        return mock_log_token_usage

    logged = run_terminal_event(
        "terminal_error_usage",
        "cogitate",
        {
            "event": "error",
            "terminal": True,
            "reason_code": "token_budget_exceeded",
            "usage": usage,
        },
    )
    logged.assert_called_once()
    assert logged.call_args.kwargs["usage"] == usage
    assert logged.call_args.kwargs["model"] == "claude-haiku-4-5-20251001"

    no_usage = run_terminal_event(
        "terminal_error_no_usage",
        "cogitate",
        {
            "event": "error",
            "terminal": True,
            "reason_code": "token_budget_exceeded",
        },
    )
    no_usage.assert_not_called()

    generate = run_terminal_event(
        "terminal_error_generate",
        "generate",
        {
            "event": "error",
            "terminal": True,
            "reason_code": "token_budget_exceeded",
            "usage": usage,
        },
    )
    generate.assert_not_called()


def test_monitor_stdout_nonterminal_error_logs_terminal_stuck_usage(
    cortex_service, mock_journal
):
    from solstone.think.cortex import TalentProcess

    use_id = "nonterminal_error_then_stuck"
    active_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    active_path.touch()
    usage = {
        "input_tokens": 10,
        "output_tokens": 5,
        "total_tokens": 15,
        "model_version": "claude-haiku-4-5-20251001",
    }
    cortex_service.use_requests = {
        use_id: {
            "event": "request",
            "prompt": "test",
            "name": "test_agent",
            "model": "claude-haiku-4-5",
            "type": "cogitate",
        }
    }

    mock_process = MagicMock()
    mock_process.stdout = MockPipe(
        [
            '{"event": "start", "ts": 1000}\n',
            json.dumps(
                {
                    "event": "error",
                    "terminal": False,
                    "error": "agent reported a recoverable error",
                }
            )
            + "\n",
            json.dumps(
                {
                    "event": "error",
                    "terminal": True,
                    "reason_code": "agent_stuck",
                    "usage": usage,
                }
            )
            + "\n",
        ]
    )
    mock_process.wait.return_value = 0

    agent = TalentProcess(use_id, mock_process, active_path)

    with patch("solstone.think.models.log_token_usage") as mock_log_token_usage:
        with patch.object(cortex_service, "_complete_use_file"):
            cortex_service._monitor_stdout(agent)

    assert mock_log_token_usage.call_count == 1
    assert mock_log_token_usage.call_args.kwargs["usage"] == usage
    assert mock_log_token_usage.call_args.kwargs["model"] == (
        "claude-haiku-4-5-20251001"
    )


def test_monitor_stdout_nonterminal_error_logs_finish_usage_once(
    cortex_service, mock_journal
):
    from solstone.think.cortex import TalentProcess

    use_id = "nonterminal_error_then_finish"
    active_path = mock_journal / "talents" / f"{use_id}_active.jsonl"
    active_path.touch()
    usage = {
        "input_tokens": 20,
        "output_tokens": 7,
        "total_tokens": 27,
        "model_version": "claude-haiku-4-5-20251001",
    }
    cortex_service.use_requests = {
        use_id: {
            "event": "request",
            "prompt": "test",
            "name": "test_agent",
            "model": "claude-haiku-4-5",
            "type": "cogitate",
        }
    }

    mock_process = MagicMock()
    mock_process.stdout = MockPipe(
        [
            '{"event": "start", "ts": 1000}\n',
            json.dumps(
                {
                    "event": "error",
                    "terminal": False,
                    "error": "agent reported a recoverable error",
                }
            )
            + "\n",
            json.dumps(
                {
                    "event": "finish",
                    "result": "done",
                    "usage": usage,
                }
            )
            + "\n",
        ]
    )
    mock_process.wait.return_value = 0

    agent = TalentProcess(use_id, mock_process, active_path)

    with patch("solstone.think.models.log_token_usage") as mock_log_token_usage:
        with patch.object(cortex_service, "_complete_use_file"):
            cortex_service._monitor_stdout(agent)

    assert mock_log_token_usage.call_count == 1
    assert mock_log_token_usage.call_args.kwargs["usage"] == usage
    assert mock_log_token_usage.call_args.kwargs["model"] == (
        "claude-haiku-4-5-20251001"
    )


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
