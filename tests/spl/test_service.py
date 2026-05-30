# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import asyncio
import contextlib
import sys
from types import SimpleNamespace
from unittest.mock import Mock

import pytest

from solstone.think import link as link_module
from solstone.think import sol_cli
from solstone.think.link import cli as link_cli
from solstone.think.link.paths import service_token_path
from solstone.think.spl import service


class FakeCallosumConnection:
    def start(self, *args, **kwargs) -> None:
        return None

    def emit(self, *args, **kwargs) -> None:
        return None

    def stop(self) -> None:
        return None


async def _cancel(task: asyncio.Task[None]) -> None:
    task.cancel()
    with contextlib.suppress(asyncio.CancelledError):
        await task


def _install_basics(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(service, "CallosumConnection", FakeCallosumConnection)
    monkeypatch.setattr(
        service.LinkState,
        "load_or_create",
        lambda: SimpleNamespace(instance_id="instance.test"),
    )
    monkeypatch.setattr(service, "relay_url", lambda: "https://relay.test")
    monkeypatch.setattr(service, "_POSTURE_POLL_SECONDS", 0.001)


def _install_relay(
    monkeypatch: pytest.MonkeyPatch,
) -> tuple[Mock, list[object], asyncio.Event, asyncio.Event]:
    instances: list[object] = []
    constructed = asyncio.Event()
    stopped = asyncio.Event()

    class FakeRelayClient:
        def __init__(
            self,
            *,
            instance_id: str,
            relay_endpoint: str,
            service_token: str,
            callosum_emit,
        ) -> None:
            self.instance_id = instance_id
            self.relay_endpoint = relay_endpoint
            self.service_token = service_token
            self.callosum_emit = callosum_emit
            self.run_started = asyncio.Event()
            self.run_finished = asyncio.Event()
            self._stop = asyncio.Event()
            self.stop_calls = 0
            instances.append(self)
            constructed.set()

        async def run(self) -> None:
            self.run_started.set()
            try:
                await self._stop.wait()
            finally:
                self.run_finished.set()

        async def stop(self) -> None:
            self.stop_calls += 1
            self._stop.set()
            stopped.set()

    constructor = Mock(side_effect=FakeRelayClient)
    monkeypatch.setattr(service, "RelayClient", constructor)
    return constructor, instances, constructed, stopped


@pytest.mark.asyncio
async def test_direct_posture_never_constructs_relay_client(
    journal_copy, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_basics(monkeypatch)
    constructor, _instances, _constructed, _stopped = _install_relay(monkeypatch)
    monkeypatch.setattr(service, "read_posture", lambda: "direct")
    monkeypatch.setattr(
        service,
        "load_service_token",
        lambda: pytest.fail("direct posture must not read service token"),
    )
    cycles = 0

    async def stop_after_three_cycles(stop_event: asyncio.Event) -> None:
        nonlocal cycles
        cycles += 1
        if cycles >= 3:
            stop_event.set()
        await asyncio.sleep(0)

    monkeypatch.setattr(service, "_wait_for_poll_or_stop", stop_after_three_cycles)

    await service.run_service()

    assert constructor.call_count == 0
    assert cycles == 3
    assert not service_token_path().exists()


@pytest.mark.asyncio
async def test_spl_posture_with_token_parks_relay_client(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _install_basics(monkeypatch)
    constructor, instances, constructed, _stopped = _install_relay(monkeypatch)
    monkeypatch.setattr(service, "read_posture", lambda: "spl")
    monkeypatch.setattr(service, "load_service_token", lambda: "tok.svc")

    task = asyncio.create_task(service.run_service())
    await asyncio.wait_for(constructed.wait(), timeout=1)
    instance = instances[0]
    await asyncio.wait_for(instance.run_started.wait(), timeout=1)
    await _cancel(task)

    constructor.assert_called_once()
    assert constructor.call_args.kwargs["service_token"] == "tok.svc"
    assert instance.stop_calls == 1


@pytest.mark.asyncio
async def test_live_direct_to_spl_to_direct_rereads_posture(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _install_basics(monkeypatch)
    _constructor, instances, constructed, stopped = _install_relay(monkeypatch)
    reads: list[str] = []
    postures = ["direct", "spl", "spl", "direct"]

    def read_posture() -> str:
        value = postures[min(len(reads), len(postures) - 1)]
        reads.append(value)
        return value

    monkeypatch.setattr(service, "read_posture", read_posture)
    monkeypatch.setattr(service, "load_service_token", lambda: "tok.svc")

    async def yield_one_cycle(stop_event: asyncio.Event) -> None:
        await asyncio.sleep(0)
        if stopped.is_set():
            stop_event.set()

    monkeypatch.setattr(service, "_wait_for_poll_or_stop", yield_one_cycle)

    task = asyncio.create_task(service.run_service())
    await asyncio.wait_for(constructed.wait(), timeout=1)
    await asyncio.wait_for(stopped.wait(), timeout=1)
    await _cancel(task)

    assert instances[0].stop_calls == 1
    assert reads[0] == "direct"
    assert "spl" in reads
    assert reads[-1] == "direct"


@pytest.mark.asyncio
async def test_spl_posture_without_token_idles_without_enrolling_or_writing(
    journal_copy, monkeypatch: pytest.MonkeyPatch
) -> None:
    _install_basics(monkeypatch)
    constructor, _instances, _constructed, _stopped = _install_relay(monkeypatch)
    monkeypatch.setattr(service, "read_posture", lambda: "spl")
    monkeypatch.setattr(service, "load_service_token", lambda: None)
    monkeypatch.setattr(
        service.LinkState,
        "load_or_create",
        lambda: pytest.fail("tokenless spl posture must not load link state"),
    )
    cycles_seen = asyncio.Event()
    hold_after_cycles = asyncio.Event()
    cycles = 0

    async def count_cycles(_stop_event: asyncio.Event) -> None:
        nonlocal cycles
        cycles += 1
        if cycles >= 3:
            cycles_seen.set()
            await hold_after_cycles.wait()
            return
        await asyncio.sleep(0)

    monkeypatch.setattr(service, "_wait_for_poll_or_stop", count_cycles)

    task = asyncio.create_task(service.run_service())
    await asyncio.wait_for(cycles_seen.wait(), timeout=1)
    assert not task.done()
    await _cancel(task)

    assert constructor.call_count == 0
    assert cycles >= 3
    assert not service_token_path().exists()


@pytest.mark.asyncio
async def test_posture_read_error_while_parked_keeps_relay_client_running(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _install_basics(monkeypatch)
    _constructor, instances, constructed, _stopped = _install_relay(monkeypatch)
    raised = asyncio.Event()
    calls = 0

    def read_posture() -> str:
        nonlocal calls
        calls += 1
        if calls == 1:
            return "spl"
        raised.set()
        raise RuntimeError("posture read failed")

    monkeypatch.setattr(service, "read_posture", read_posture)
    monkeypatch.setattr(service, "load_service_token", lambda: "tok.svc")
    release_after_raise = asyncio.Event()

    async def hold_after_raise(_stop_event: asyncio.Event) -> None:
        if raised.is_set():
            await release_after_raise.wait()
        else:
            await asyncio.sleep(0)

    monkeypatch.setattr(service, "_wait_for_poll_or_stop", hold_after_raise)

    task = asyncio.create_task(service.run_service())
    await asyncio.wait_for(constructed.wait(), timeout=1)
    instance = instances[0]
    await asyncio.wait_for(instance.run_started.wait(), timeout=1)
    await asyncio.wait_for(raised.wait(), timeout=1)

    assert not task.done()
    assert instance.stop_calls == 0
    assert not instance.run_finished.is_set()

    await _cancel(task)
    assert instance.stop_calls == 1


def test_dispatch_surfaces_for_journal_spl_and_sol_link(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    result: dict[str, object] = {}
    titles: list[str] = []

    def fake_run_command(module_path: str) -> int:
        result["module"] = module_path
        result["argv"] = sys.argv[:]
        return 0

    monkeypatch.setattr(sol_cli, "run_command", fake_run_command)
    monkeypatch.setattr(sol_cli.setproctitle, "setproctitle", titles.append)
    monkeypatch.setattr(sys, "argv", ["journal", "spl"])

    with pytest.raises(SystemExit) as exc:
        sol_cli.journal_main()

    assert exc.value.code == 0
    assert result == {
        "module": "solstone.think.spl",
        "argv": ["journal spl"],
    }
    assert titles == ["journal:spl"]

    module_path, preset_args, surface = sol_cli.resolve_command("link")
    assert (module_path, preset_args, surface) == (
        "solstone.think.link",
        [],
        "access",
    )
    assert link_module.main is link_cli.main
