# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import ast
import asyncio
import concurrent.futures
import logging
import subprocess
import sys
import threading
import time
from collections import OrderedDict
from collections.abc import Iterator
from dataclasses import replace
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock

import pytest

from solstone.think import supervisor
from solstone.think.providers.artifact_proof import ReadinessOutcome
from solstone.think.providers.brain_state import build_active_brain_fingerprint
from solstone.think.providers.install_state import (
    IN_FLIGHT_STATES,
    TERMINAL_STATES,
    InstallState,
)
from solstone.think.providers.runtime_health import (
    RUNTIME_PHASES,
    ReasonCode,
    RuntimeHealthRecord,
    RuntimeHealthUnavailableError,
    RuntimePhase,
    read_retry_token,
    read_runtime_health,
    request_retry_token,
    request_runtime_retry,
    write_runtime_health,
)


@pytest.fixture
def provider_cache_reset() -> Iterator[None]:
    from solstone.think.providers import local_server, local_vulkan

    local_vulkan.reset_detect_cache()
    local_server.reset_parallel_slots_cache()
    try:
        yield
    finally:
        local_vulkan.reset_detect_cache()
        local_server.reset_parallel_slots_cache()


@pytest.fixture(autouse=True)
def runtime_state_reset(
    tmp_path, monkeypatch, provider_cache_reset, set_test_journal_path
):
    import solstone.think.utils as think_utils

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path / "journal"))
    think_utils._journal_path_cache = None
    states = {
        "local": supervisor.ProviderRuntimeState("local"),
        "parakeet": supervisor.ProviderRuntimeState("parakeet"),
    }
    monkeypatch.setattr(supervisor, "_provider_runtime_states", states)
    monkeypatch.setattr(
        supervisor,
        "_recovery_state",
        {
            "local": supervisor.ProviderRecoveryState(),
            "parakeet": supervisor.ProviderRecoveryState(),
        },
    )
    monkeypatch.setattr(supervisor, "_provider_runtime_executor", None)
    monkeypatch.setattr(
        supervisor,
        "_wedge_state",
        {
            "providers": OrderedDict(),
            "failures": set(),
            "cooldown_until": 0.0,
            "awaiting_recovery": False,
        },
    )
    monkeypatch.setattr(supervisor, "_provider_startup_gate", None)
    monkeypatch.setattr(supervisor, "_parakeet_admission_retry_epoch", 0)
    monkeypatch.setattr(supervisor, "_task_queue", None)
    supervisor._SERVICE_STATE.clear()
    yield
    executor = supervisor._provider_runtime_executor
    if executor is not None:
        executor.shutdown(wait=True, cancel_futures=True)
    think_utils._journal_path_cache = None


class _InlineExecutor:
    def submit(self, fn, *args, **kwargs):
        future: concurrent.futures.Future = concurrent.futures.Future()
        try:
            future.set_result(fn(*args, **kwargs))
        except BaseException as exc:
            future.set_exception(exc)
        return future


def _future_with(result: Any) -> concurrent.futures.Future:
    future: concurrent.futures.Future = concurrent.futures.Future()
    future.set_result(result)
    return future


class _FakeTaskQueue:
    def __init__(self) -> None:
        self.ready_calls = 0
        self.submitted: list[tuple[list[str], tuple[Any, ...], dict[str, Any]]] = []

    def set_ready(self) -> None:
        self.ready_calls += 1

    def submit(self, cmd: list[str], *args: Any, **kwargs: Any) -> None:
        self.submitted.append((cmd, args, kwargs))

    def enforce_deadlines(self, _now: float) -> None:
        return None


class _FakeReservation:
    def __init__(self, port: int = 45123):
        self.port = port
        self.closed = False

    def release_for_spawn(self) -> int:
        self.closed = True
        return self.port

    def close(self) -> None:
        self.closed = True


class _FakeProcess:
    pid = 12345
    returncode = None

    def poll(self) -> None:
        return None


class _FakeManaged:
    def __init__(self, name: str = supervisor.LOCAL_SERVER_PROCESS_NAME) -> None:
        self.name = name
        self.ref = f"ref-{name}"
        self.process = _FakeProcess()
        self.cleanup = MagicMock()
        self.terminate = MagicMock()

    def is_running(self) -> bool:
        return True


class _DeadManaged(_FakeManaged):
    def is_running(self) -> bool:
        return False


def _hold_local_slot_in_child(root: Path):
    ready = root / "slot-holder.ready"
    code = (
        "import fcntl, pathlib, sys, time\n"
        "root = pathlib.Path(sys.argv[1])\n"
        "ready = pathlib.Path(sys.argv[2])\n"
        "root.mkdir(parents=True, exist_ok=True)\n"
        "f = open(root / 'slot-0.lock', 'a+', encoding='utf-8')\n"
        "fcntl.flock(f, fcntl.LOCK_EX)\n"
        "ready.write_text('1', encoding='utf-8')\n"
        "time.sleep(60)\n"
    )
    proc = subprocess.Popen(
        [sys.executable, "-c", code, str(root), str(ready)],
    )
    deadline = time.monotonic() + 2.0
    while time.monotonic() < deadline:
        if ready.exists():
            return proc
        time.sleep(0.01)
    proc.terminate()
    proc.wait(timeout=2)
    raise AssertionError("local admission slot holder did not become ready")


def _readiness(
    status: str,
    reason_code: str,
    *,
    install_state: InstallState = "idle",
    host: dict[str, Any] | None = None,
) -> ReadinessOutcome:
    return ReadinessOutcome(
        provider="parakeet",
        status=status,
        reason_code=reason_code,
        target={},
        install={"install_state": install_state},
        host=host or {},
        artifacts={},
        proof={},
    )


def _local_readiness(
    status: str = "ready",
    reason_code: str = "ready",
    *,
    install_state: InstallState = "idle",
    host_reason: str = "gpu_unavailable",
) -> ReadinessOutcome:
    return ReadinessOutcome(
        provider="local",
        status=status,
        reason_code=reason_code,
        target={"model_id": supervisor.LOCAL_MODEL},
        install={"install_state": install_state},
        host=(
            {
                "backend": "vulkan",
                "backend_reason": "test",
            }
            if status == "ready"
            else {"reason": host_reason}
        ),
        artifacts=(
            {
                "binary_path": "/tmp/llama-server",
                "model_path": "/tmp/model.gguf",
                "mmproj_path": None,
            }
            if status == "ready"
            else {}
        ),
        proof={},
    )


def _status_snapshot(
    *,
    install_state: InstallState = "downloading",
    attempt_id: str | None = "attempt-live",
    target_fingerprint_sha256: str | None = "fp-local",
    revision: int = 7,
) -> dict[str, Any]:
    return {
        "revision": revision,
        "install_state": install_state,
        "attempt_id": attempt_id,
        "target_fingerprint_sha256": target_fingerprint_sha256,
    }


def _local_install_progress_detail(
    *,
    readiness_state: InstallState,
    attempt_id: str,
    revision: int,
) -> dict[str, Any]:
    return {
        "readiness_status": "missing-or-mismatched",
        "readiness_reason_code": "manifest_missing",
        "install_state": readiness_state,
        "install_acquisition_allowed": False,
        "install_attempt_id": attempt_id,
        "install_revision": revision,
    }


def _local_artifact_missing_detail(install_state: InstallState) -> dict[str, Any]:
    return {
        "readiness_status": "missing-or-mismatched",
        "readiness_reason_code": "manifest_missing",
        "install_state": install_state,
        "install_acquisition_allowed": install_state == "idle",
    }


def _assert_local_observation(
    observation: supervisor.ProviderTruthObservation | None,
    *,
    phase: RuntimePhase,
    reason_code: ReasonCode,
    desired_json: str,
    desired_sha: str,
    boot_required: bool,
    detail: dict[str, Any],
) -> None:
    assert observation is not None
    assert observation.phase == phase
    assert observation.reason_code == reason_code
    assert observation.desired_fingerprint_json == desired_json
    assert observation.desired_fingerprint_sha256 == desired_sha
    assert observation.boot_required is boot_required
    assert observation.detail == detail
    assert observation.plan is None


def _patch_lease_state(monkeypatch, state: str) -> None:
    monkeypatch.setattr(
        "solstone.think.providers.install_lease.probe_install_lease_state",
        lambda _provider: state,
    )


def _local_plan() -> supervisor.LocalServerLaunchPlan:
    return supervisor.LocalServerLaunchPlan(
        backend="vulkan",
        desired_fingerprint_json='{"provider":"local"}',
        desired_fingerprint_sha256="fp-local",
        binary_path=Path("/tmp/llama-server"),
        model_path=Path("/tmp/model.gguf"),
        context_tokens=16384,
        parallel_slots=1,
    )


def _cuda_plan() -> supervisor.LocalServerLaunchPlan:
    return supervisor.LocalServerLaunchPlan(
        backend="cuda",
        desired_fingerprint_json='{"provider":"local","backend":"cuda"}',
        desired_fingerprint_sha256="fp-local-cuda",
        binary_path=Path("/tmp/llama-server"),
        model_path=Path("/tmp/model.gguf"),
        lib_dir=Path("/tmp/cuda/lib"),
        gpu_index=0,
        gpu_vram_mib=24576,
        context_tokens=32768,
        parallel_slots=1,
        visible_devices_env="CUDA_VISIBLE_DEVICES",
    )


def _mlx_plan() -> supervisor.LocalServerLaunchPlan:
    return supervisor.LocalServerLaunchPlan(
        backend="mlx",
        desired_fingerprint_json='{"provider":"local","backend":"mlx"}',
        desired_fingerprint_sha256="fp-local-mlx",
        model_id=supervisor.LOCAL_MODEL,
        runtime_dir=Path("/tmp/mlx-runtime"),
    )


def _native_launch_plan_for_test(
    plan: supervisor.LocalServerLaunchPlan,
    port: int,
    *,
    mlx_interpreter_path: Path | None = None,
) -> dict[str, Any]:
    """Return the native-plan contract for supervisor launch tests."""
    if plan.backend == "mlx":
        assert mlx_interpreter_path is not None
        return {
            "outcome": "launch",
            "argv": [
                str(mlx_interpreter_path),
                "--host",
                "127.0.0.1",
                "--port",
                str(port),
                "--model",
                str(plan.runtime_dir),
            ],
            "context_tokens": 0,
            "parallel_slots": 0,
            "prompt_cache_mib": 0,
            "extra_env": {},
        }

    assert plan.binary_path is not None
    assert plan.model_path is not None
    assert plan.context_tokens is not None
    assert plan.parallel_slots is not None
    prompt_cache_mib = 2048 if plan.context_tokens >= 32768 else 0
    argv = [
        str(plan.binary_path),
        "-m",
        str(plan.model_path),
        "--alias",
        plan.model_id,
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--jinja",
        "--n-gpu-layers",
        "999",
        "-c",
        str(plan.context_tokens * plan.parallel_slots),
        "--parallel",
        str(plan.parallel_slots),
        "--kv-unified",
        "--cache-ram",
        str(prompt_cache_mib),
        "--no-context-shift",
        "--device",
        "CUDA0" if plan.backend == "cuda" else "Vulkan0",
    ]
    if plan.mmproj_path is not None:
        argv.extend(["--mmproj", str(plan.mmproj_path)])
    return {
        "outcome": "launch",
        "argv": argv,
        "context_tokens": plan.context_tokens,
        "parallel_slots": plan.parallel_slots,
        "prompt_cache_mib": prompt_cache_mib,
        "extra_env": (
            {"CUDA_VISIBLE_DEVICES": str(plan.gpu_index)}
            if plan.backend == "cuda" and plan.gpu_index is not None
            else (
                {"GGML_VK_VISIBLE_DEVICES": str(plan.gpu_index)}
                if plan.gpu_index is not None
                else {}
            )
        ),
    }


def _core_ready_capacity_for_context(
    local_server: Any,
) -> dict[str, Any]:
    context_tokens = local_server.read_local_context_window()
    capable = context_tokens == 32768
    return {
        "outcome": "ready",
        "server": {
            "parallel_slots": 2 if capable else 1,
            "capacity_source": "local_ctx" if capable else "default",
            "profile": "capable" if capable else "floor",
        },
    }


def _parakeet_plan(backend: str = "cpu") -> supervisor.ParakeetServerLaunchPlan:
    return supervisor.ParakeetServerLaunchPlan(
        binary_backend=backend,
        env_updates={"GGML_VK_VISIBLE_DEVICES": "0"} if backend == "vulkan" else {},
        gpu_index=0 if backend == "vulkan" else None,
        binary_path=Path(f"/tmp/parakeet-{backend}"),
        model_path=Path("/tmp/parakeet-model.bin"),
        threads=4,
        desired_fingerprint_json='{"provider":"parakeet"}',
        desired_fingerprint_sha256="fp-parakeet",
        placement="gpu" if backend == "vulkan" else "cpu",
    )


@pytest.mark.parametrize("install_state", sorted(IN_FLIGHT_STATES))
def test_local_readiness_block_observation_upgrades_live_inflight_states(
    monkeypatch,
    install_state: InstallState,
) -> None:
    target = {"provider": "local", "target": "live-install"}
    desired_json, desired_sha = supervisor._target_fingerprint_pair(target)
    before_status = _status_snapshot(
        install_state=install_state,
        attempt_id=f"attempt-{install_state}",
        target_fingerprint_sha256=desired_sha,
    )
    after_status = dict(before_status)
    statuses = [before_status, after_status]
    target_calls = 0

    def read_status(*, name: str):
        assert name == "local"
        return statuses.pop(0)

    def target_fingerprint() -> dict[str, str]:
        nonlocal target_calls
        target_calls += 1
        return target

    monkeypatch.setattr(supervisor, "read_install_status", read_status)
    _patch_lease_state(monkeypatch, "held")

    observation = supervisor._local_readiness_block_observation(
        readiness=_local_readiness(
            "missing-or-mismatched",
            "manifest_missing",
            install_state=install_state,
        ),
        fingerprint_json=desired_json,
        fingerprint_sha256_value=desired_sha,
        boot_required=True,
        target_fingerprint=target_fingerprint,
    )

    _assert_local_observation(
        observation,
        phase="artifact-not-ready",
        reason_code="install-in-progress",
        desired_json=desired_json,
        desired_sha=desired_sha,
        boot_required=True,
        detail=_local_install_progress_detail(
            readiness_state=install_state,
            attempt_id=str(before_status["attempt_id"]),
            revision=int(before_status["revision"]),
        ),
    )
    assert statuses == []
    assert target_calls == 1


@pytest.mark.parametrize("install_state", sorted(TERMINAL_STATES))
def test_local_readiness_block_observation_returns_shared_object_for_terminal_pregate(
    monkeypatch,
    install_state: InstallState,
) -> None:
    sentinel = supervisor.ProviderTruthObservation(
        provider="local",
        phase="artifact-not-ready",
        reason_code="artifact-missing",
        detail={"sentinel": True},
    )
    monkeypatch.setattr(
        supervisor,
        "_readiness_block_observation",
        lambda **_kwargs: sentinel,
    )
    monkeypatch.setattr(
        supervisor,
        "read_install_status",
        lambda **_kwargs: pytest.fail("canonical status read not expected"),
    )
    monkeypatch.setattr(
        "solstone.think.providers.install_lease.probe_install_lease_state",
        lambda _provider: pytest.fail("lease probe not expected"),
    )

    result = supervisor._local_readiness_block_observation(
        readiness=_local_readiness(
            "missing-or-mismatched",
            "manifest_missing",
            install_state=install_state,
        ),
        fingerprint_json='{"provider":"local"}',
        fingerprint_sha256_value="fp-local",
        boot_required=True,
        target_fingerprint=lambda: pytest.fail("target recompute not expected"),
    )

    assert result is sentinel


@pytest.mark.parametrize(
    "case",
    [
        "canonical-state",
        "attempt-none",
        "attempt-empty",
        "target-mismatch",
        "lease-free",
        "lease-missing",
    ],
)
def test_local_readiness_block_observation_returns_shared_object_for_failed_terms(
    monkeypatch,
    case: str,
) -> None:
    target = {"provider": "local", "target": "live-install"}
    _desired_json, desired_sha = supervisor._target_fingerprint_pair(target)
    before_status = _status_snapshot(target_fingerprint_sha256=desired_sha)
    lease_state = "held"
    if case == "canonical-state":
        before_status["install_state"] = "installed"
    elif case == "attempt-none":
        before_status["attempt_id"] = None
    elif case == "attempt-empty":
        before_status["attempt_id"] = ""
    elif case == "target-mismatch":
        before_status["target_fingerprint_sha256"] = "fp-other"
    elif case == "lease-free":
        lease_state = "free"
    elif case == "lease-missing":
        lease_state = "missing"
    else:
        raise AssertionError(case)

    sentinel = supervisor.ProviderTruthObservation(
        provider="local",
        phase="artifact-not-ready",
        reason_code="artifact-missing",
        detail={"sentinel": True},
    )
    monkeypatch.setattr(
        supervisor,
        "_readiness_block_observation",
        lambda **_kwargs: sentinel,
    )
    monkeypatch.setattr(
        supervisor, "read_install_status", lambda *, name: before_status
    )
    _patch_lease_state(monkeypatch, lease_state)

    result = supervisor._local_readiness_block_observation(
        readiness=_local_readiness(
            "missing-or-mismatched",
            "manifest_missing",
            install_state="downloading",
        ),
        fingerprint_json='{"provider":"local"}',
        fingerprint_sha256_value=desired_sha,
        boot_required=True,
        target_fingerprint=lambda: pytest.fail("target recompute not expected"),
    )

    assert result is sentinel


def test_local_readiness_block_observation_maps_lease_probe_oserror(
    monkeypatch,
) -> None:
    target = {"provider": "local", "target": "live-install"}
    desired_json, desired_sha = supervisor._target_fingerprint_pair(target)
    before_status = _status_snapshot(target_fingerprint_sha256=desired_sha)
    monkeypatch.setattr(
        supervisor, "read_install_status", lambda *, name: before_status
    )

    def fail_probe(_provider: str) -> str:
        raise OSError("lease probe failed")

    monkeypatch.setattr(
        "solstone.think.providers.install_lease.probe_install_lease_state",
        fail_probe,
    )

    observation = supervisor._local_readiness_block_observation(
        readiness=_local_readiness(
            "missing-or-mismatched",
            "manifest_missing",
            install_state="downloading",
        ),
        fingerprint_json=desired_json,
        fingerprint_sha256_value=desired_sha,
        boot_required=True,
        target_fingerprint=lambda: pytest.fail("target recompute not expected"),
    )

    _assert_local_observation(
        observation,
        phase="state-unavailable",
        reason_code="proof-observation-unavailable",
        desired_json=desired_json,
        desired_sha=desired_sha,
        boot_required=True,
        detail={
            "readiness_status": "missing-or-mismatched",
            "readiness_reason_code": "manifest_missing",
            "error": "lease probe failed",
        },
    )


@pytest.mark.parametrize(
    "race",
    ["desired", "revision", "attempt", "install-state", "install-target"],
)
def test_local_readiness_block_observation_reports_races(
    monkeypatch,
    race: str,
) -> None:
    target = {"provider": "local", "target": "live-install"}
    desired_json, desired_sha = supervisor._target_fingerprint_pair(target)
    after_target = target
    before_status = _status_snapshot(target_fingerprint_sha256=desired_sha)
    after_status = dict(before_status)
    if race == "desired":
        after_target = {"provider": "local", "target": "new-install"}
    elif race == "revision":
        after_status["revision"] = int(before_status["revision"]) + 1
    elif race == "attempt":
        after_status["attempt_id"] = "attempt-new"
    elif race == "install-state":
        after_status["install_state"] = "verifying"
    elif race == "install-target":
        after_status["target_fingerprint_sha256"] = "fp-new-target"
    else:
        raise AssertionError(race)
    statuses = [before_status, after_status]

    def read_status(*, name: str):
        assert name == "local"
        return statuses.pop(0)

    monkeypatch.setattr(supervisor, "read_install_status", read_status)
    _patch_lease_state(monkeypatch, "held")

    observation = supervisor._local_readiness_block_observation(
        readiness=_local_readiness(
            "missing-or-mismatched",
            "manifest_missing",
            install_state="downloading",
        ),
        fingerprint_json=desired_json,
        fingerprint_sha256_value=desired_sha,
        boot_required=True,
        target_fingerprint=lambda: after_target,
    )

    assert observation is not None
    assert observation.phase == "observing"
    assert observation.reason_code == "observation-raced"
    assert observation.boot_required is True
    assert observation.plan is None
    _after_json, after_sha = supervisor._target_fingerprint_pair(after_target)
    assert observation.detail == {
        "before": supervisor._local_install_progress_identity(
            desired_fingerprint_sha256=desired_sha,
            install_status=before_status,
        ),
        "after": supervisor._local_install_progress_identity(
            desired_fingerprint_sha256=after_sha,
            install_status=after_status,
        ),
    }
    assert statuses == []


def _begin_downloading_local_install(target: dict[str, Any]) -> dict[str, Any]:
    from solstone.think.providers.install_state import begin_or_replace_install_attempt

    return begin_or_replace_install_attempt(
        "local",
        target,
        initial_state="downloading",
    )


def test_mlx_local_observer_preserves_live_install_progress(
    monkeypatch,
) -> None:
    from solstone.think.providers import mlx_install
    from solstone.think.providers.install_lease import acquire_install_lease

    target = {"provider": "local", "runtime": "mlx", "model": "mlx-test"}
    desired_json, desired_sha = supervisor._target_fingerprint_pair(target)
    status = _begin_downloading_local_install(target)
    readiness = _local_readiness(
        "missing-or-mismatched",
        "manifest_missing",
        install_state=status["install_state"],
    )
    forbidden_calls: list[str] = []

    def fail_mlx_install(*_args, **_kwargs):
        forbidden_calls.append("install_local_mlx")
        pytest.fail("install_local_mlx not expected")

    monkeypatch.setattr(mlx_install, "install_local_mlx", fail_mlx_install)
    monkeypatch.setattr(mlx_install, "target_fingerprint", lambda: target)
    monkeypatch.setattr(mlx_install, "inspect_readiness", lambda: readiness)
    lease = acquire_install_lease("local")
    assert lease is not None
    try:
        observation = supervisor._observe_mlx_local_provider_truth()
    finally:
        lease.release()

    _assert_local_observation(
        observation,
        phase="artifact-not-ready",
        reason_code="install-in-progress",
        desired_json=desired_json,
        desired_sha=desired_sha,
        boot_required=True,
        detail=_local_install_progress_detail(
            readiness_state=status["install_state"],
            attempt_id=str(status["attempt_id"]),
            revision=int(status["revision"]),
        ),
    )
    assert forbidden_calls == []

    observation = supervisor._observe_mlx_local_provider_truth()

    _assert_local_observation(
        observation,
        phase="artifact-not-ready",
        reason_code="artifact-missing",
        desired_json=desired_json,
        desired_sha=desired_sha,
        boot_required=True,
        detail=_local_artifact_missing_detail(status["install_state"]),
    )
    assert forbidden_calls == []


def test_linux_local_observer_preserves_live_install_progress(
    monkeypatch,
) -> None:
    from solstone.think.providers import local_cuda, local_install, local_server
    from solstone.think.providers.install_lease import acquire_install_lease

    target = {"provider": "local", "runtime": "llama.cpp", "target": "linux-test"}
    desired_json, desired_sha = supervisor._target_fingerprint_pair(target)
    status = _begin_downloading_local_install(target)
    readiness = _local_readiness(
        "missing-or-mismatched",
        "manifest_missing",
        install_state=status["install_state"],
    )
    forbidden_calls: list[str] = []

    def fail_local_install(*_args, **_kwargs):
        forbidden_calls.append("install_local")
        pytest.fail("install_local not expected")

    def fail_select_server_tier(*_args, **_kwargs):
        forbidden_calls.append("select_server_tier")
        pytest.fail("select_server_tier not expected")

    def fail_probe_nvidia_gpu(*_args, **_kwargs):
        forbidden_calls.append("probe_nvidia_gpu")
        pytest.fail("probe_nvidia_gpu not expected")

    monkeypatch.setattr(local_install, "install_local", fail_local_install)
    monkeypatch.setattr(local_server, "select_server_tier", fail_select_server_tier)
    monkeypatch.setattr(local_cuda, "probe_nvidia_gpu", fail_probe_nvidia_gpu)
    monkeypatch.setattr(local_install, "target_fingerprint", lambda _model: target)
    monkeypatch.setattr(local_install, "inspect_readiness", lambda _model: readiness)
    lease = acquire_install_lease("local")
    assert lease is not None
    try:
        observation = supervisor._observe_linux_local_provider_truth()
    finally:
        lease.release()

    _assert_local_observation(
        observation,
        phase="artifact-not-ready",
        reason_code="install-in-progress",
        desired_json=desired_json,
        desired_sha=desired_sha,
        boot_required=True,
        detail=_local_install_progress_detail(
            readiness_state=status["install_state"],
            attempt_id=str(status["attempt_id"]),
            revision=int(status["revision"]),
        ),
    )
    assert forbidden_calls == []

    observation = supervisor._observe_linux_local_provider_truth()

    _assert_local_observation(
        observation,
        phase="artifact-not-ready",
        reason_code="artifact-missing",
        desired_json=desired_json,
        desired_sha=desired_sha,
        boot_required=True,
        detail=_local_artifact_missing_detail(status["install_state"]),
    )
    assert forbidden_calls == []


def test_linux_local_observer_carries_unified_memory_into_cuda_plan(
    monkeypatch,
) -> None:
    from solstone.think.providers import local_cuda, local_install

    target = {"provider": "local", "runtime": "llama.cpp", "target": "cuda"}
    readiness = replace(
        _local_readiness(),
        host={"backend": "cuda", "backend_reason": "test CUDA"},
    )
    probe = local_cuda.NvidiaProbe(
        index=0,
        compute_cap="sm_121",
        driver_cuda_version=13,
        vram_mib=None,
        tiering_memory_mib=26_724,
        memory_source=local_cuda.MEMORY_SOURCE_SYSTEM_AVAILABLE,
        detected=True,
    )
    monkeypatch.setattr(local_install, "target_fingerprint", lambda _model: target)
    monkeypatch.setattr(local_install, "inspect_readiness", lambda _model: readiness)
    monkeypatch.setattr(local_install, "cuda_binary_dir", lambda: Path("/tmp/cuda"))
    monkeypatch.setattr(
        supervisor, "_local_readiness_block_observation", lambda **_kwargs: None
    )
    monkeypatch.setattr(local_cuda, "probe_nvidia_gpu", lambda: probe)
    monkeypatch.setattr(
        supervisor, "_local_plan_race_identity", lambda _plan: {"stable": True}
    )

    observation = supervisor._observe_linux_local_provider_truth()

    assert observation.phase == "starting"
    assert observation.plan is not None
    assert observation.plan.backend == "cuda"
    assert observation.plan.gpu_vram_mib == 26_724
    assert (observation.plan.context_tokens, observation.plan.parallel_slots) == (
        32_768,
        2,
    )


def test_linux_local_observer_missing_lease_does_not_create_or_mutate_status(
    monkeypatch,
) -> None:
    from solstone.think.providers import local_install
    from solstone.think.providers.install_lease import lease_path
    from solstone.think.providers.install_state import provider_status_path

    target = {"provider": "local", "runtime": "llama.cpp", "target": "linux-test"}
    desired_json, desired_sha = supervisor._target_fingerprint_pair(target)
    status = _begin_downloading_local_install(target)
    readiness = _local_readiness(
        "missing-or-mismatched",
        "manifest_missing",
        install_state=status["install_state"],
    )
    monkeypatch.setattr(local_install, "target_fingerprint", lambda _model: target)
    monkeypatch.setattr(local_install, "inspect_readiness", lambda _model: readiness)
    status_path = provider_status_path("local")
    lease = lease_path("local")
    assert not lease.exists()
    before_bytes = status_path.read_bytes()
    before_mtime = status_path.stat().st_mtime_ns

    observation = supervisor._observe_linux_local_provider_truth()

    _assert_local_observation(
        observation,
        phase="artifact-not-ready",
        reason_code="artifact-missing",
        desired_json=desired_json,
        desired_sha=desired_sha,
        boot_required=True,
        detail=_local_artifact_missing_detail(status["install_state"]),
    )
    assert not lease.exists()
    assert status_path.read_bytes() == before_bytes
    assert status_path.stat().st_mtime_ns == before_mtime


def test_local_ready_side_effects_submit_brain_refresh_once(monkeypatch) -> None:
    from solstone.think.providers import local_server

    plan = _local_plan()
    state = supervisor.ProviderRuntimeState("local")
    state.latest_plan = plan
    queue = _FakeTaskQueue()
    context_windows: list[int] = []

    monkeypatch.setattr(supervisor, "_task_queue", queue)
    monkeypatch.setattr(local_server, "reset_parallel_slots_cache", lambda: None)
    monkeypatch.setattr(
        local_server,
        "write_local_context_window",
        lambda tokens: context_windows.append(tokens),
    )

    supervisor._write_provider_ready_side_effects(
        state,
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={},
        ),
    )

    assert context_windows == [plan.context_tokens]
    assert queue.submitted == [
        (
            [
                "journal",
                "brain",
                "refresh",
                "--expected-fingerprint",
                plan.desired_fingerprint_sha256,
            ],
            (),
            {},
        )
    ]


def test_parakeet_ready_side_effects_do_not_submit_brain_refresh(monkeypatch) -> None:
    from solstone.think.providers import parakeet_server

    plan = _parakeet_plan("vulkan")
    state = supervisor.ProviderRuntimeState("parakeet")
    state.latest_plan = plan
    queue = _FakeTaskQueue()
    placements: list[str] = []

    monkeypatch.setattr(supervisor, "_task_queue", queue)
    monkeypatch.setattr(
        parakeet_server,
        "write_parakeet_placement",
        lambda placement: placements.append(placement),
    )

    supervisor._write_provider_ready_side_effects(
        state,
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={"placement": "gpu"},
        ),
    )

    assert placements == ["gpu"]
    assert queue.submitted == []


def test_local_ready_without_local_launch_plan_does_not_submit_brain_refresh(
    monkeypatch,
) -> None:
    state = supervisor.ProviderRuntimeState("local")
    state.latest_plan = _parakeet_plan()
    queue = _FakeTaskQueue()
    monkeypatch.setattr(supervisor, "_task_queue", queue)

    supervisor._write_provider_ready_side_effects(
        state,
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={},
        ),
    )

    assert queue.submitted == []


def test_local_ready_side_effects_allow_missing_task_queue(monkeypatch) -> None:
    from solstone.think.providers import local_server

    plan = _local_plan()
    state = supervisor.ProviderRuntimeState("local")
    state.latest_plan = plan
    context_windows: list[int] = []

    monkeypatch.setattr(supervisor, "_task_queue", None)
    monkeypatch.setattr(local_server, "reset_parallel_slots_cache", lambda: None)
    monkeypatch.setattr(
        local_server,
        "write_local_context_window",
        lambda tokens: context_windows.append(tokens),
    )

    supervisor._write_provider_ready_side_effects(
        state,
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={},
        ),
    )

    assert context_windows == [plan.context_tokens]


def test_supervisor_bundled_fingerprint_matches_brain_preflight(monkeypatch) -> None:
    target = {
        "provider": "local",
        "runtime": "llama.cpp",
        "backend": "vulkan",
        "runtime_pin": {"kind": "test-runtime", "revision": "1"},
        "model_pin": {"kind": "test-model", "revision": "1"},
    }
    if sys.platform == "darwin":
        from solstone.think.providers import mlx_install

        monkeypatch.setattr(mlx_install, "target_fingerprint", lambda: target)
    else:
        from solstone.think.providers import local_install

        monkeypatch.setattr(
            local_install,
            "target_fingerprint",
            lambda _model_id=supervisor.LOCAL_MODEL: target,
        )

    _target_json, supervisor_sha = supervisor._target_fingerprint_pair(target)
    result = build_active_brain_fingerprint(
        {
            "providers": {"active": {"provider": "local", "model": "local/bundled"}},
            "env": {},
        },
        hmac_key=b"supervisor-brain-fingerprint-test",
    )

    assert result["ok"] is True
    assert result["bundled_runtime_fingerprint_sha256"] == supervisor_sha


def _runtime_record(
    provider: str,
    *,
    phase: RuntimePhase,
    fingerprint: str | None,
    generation: int,
    attempt: int,
    process: dict[str, Any] | None = None,
) -> RuntimeHealthRecord:
    record = read_runtime_health(provider)
    return {
        **record,
        "phase": phase,
        "reason_code": None,
        "detail": {},
        "desired_fingerprint_sha256": fingerprint,
        "incarnation": supervisor._PROVIDER_INCARNATION,
        "generation": generation,
        "attempt": attempt,
        "process": process,
        "updated_at": "2026-07-19T00:00:00+00:00",
        "owner": {"test": "provider-runtime"},
    }


@pytest.mark.parametrize(
    ("status", "expected_phase", "expected_reason"),
    [
        ("missing-or-mismatched", "artifact-not-ready", "artifact-missing"),
        (
            "proof-unavailable",
            "state-unavailable",
            "proof-observation-unavailable",
        ),
    ],
)
def test_readiness_block_table_maps_artifact_and_proof_statuses(
    status: str,
    expected_phase: RuntimePhase,
    expected_reason: ReasonCode,
) -> None:
    observation = supervisor._readiness_block_observation(
        provider="parakeet",
        readiness=_readiness(status, "manifest_missing"),
        fingerprint_json='{"provider":"parakeet"}',
        fingerprint_sha256_value="fp-parakeet",
        boot_required=True,
    )

    assert observation is not None
    assert observation.phase == expected_phase
    assert observation.reason_code == expected_reason


@pytest.mark.parametrize(
    ("readiness_reason", "expected_reason"),
    [
        ("platform_unsupported", "platform-unsupported"),
        ("package_unavailable", "package-unavailable"),
        ("binary_not_runnable", "package-unavailable"),
        ("openmp_runtime_unavailable", "openmp-runtime-unavailable"),
        ("ram_insufficient", "ram-insufficient"),
        ("gpu_probe_failed", "gpu-probe-failed"),
        ("gpu_unavailable", "gpu-unavailable"),
    ],
)
def test_host_ineligible_reason_table(readiness_reason: str, expected_reason: str):
    observation = supervisor._readiness_block_observation(
        provider="local",
        readiness=_readiness(
            "host-ineligible",
            readiness_reason,
            host={"reason": readiness_reason},
        ),
        fingerprint_json='{"provider":"local"}',
        fingerprint_sha256_value="fp-local",
        boot_required=True,
    )

    assert observation is not None
    assert observation.phase == "host-blocked"
    assert observation.reason_code == expected_reason


@pytest.mark.parametrize(
    "install_state",
    [
        "idle",
        "resolving",
        "downloading",
        "verifying",
        "installing",
        "installed",
        "failed",
    ],
)
def test_parakeet_missing_artifact_table_permits_acquisition_only_when_idle(
    install_state: InstallState,
) -> None:
    observation = supervisor._readiness_block_observation(
        provider="parakeet",
        readiness=_readiness(
            "missing-or-mismatched",
            "manifest_missing",
            install_state=install_state,
        ),
        fingerprint_json='{"provider":"parakeet"}',
        fingerprint_sha256_value="fp-parakeet",
        boot_required=True,
    )

    assert observation is not None
    assert observation.phase == "artifact-not-ready"
    assert observation.detail["install_acquisition_allowed"] is (
        install_state == "idle"
    )


@pytest.mark.parametrize("active_provider", ["local", "cloud", None])
@pytest.mark.parametrize("endpoint_bundled", [True, False])
@pytest.mark.parametrize(
    ("readiness_status", "readiness_reason", "expected_phase"),
    [
        ("ready", "ready", "starting"),
        ("missing-or-mismatched", "manifest_missing", "artifact-not-ready"),
        (
            "proof-unavailable",
            "proof_cache_unavailable",
            "state-unavailable",
        ),
        ("host-ineligible", "gpu_unavailable", "host-blocked"),
    ],
)
def test_local_desired_state_table(
    monkeypatch,
    active_provider: str | None,
    endpoint_bundled: bool,
    readiness_status: str,
    readiness_reason: str,
    expected_phase: RuntimePhase,
) -> None:
    from solstone.think.providers import local_install, local_server, local_vulkan

    config = {"providers": {"active": {"provider": active_provider}}}
    inspect_calls = 0

    def inspect_readiness(_model_id):
        nonlocal inspect_calls
        inspect_calls += 1
        return _local_readiness(readiness_status, readiness_reason)

    monkeypatch.setattr(supervisor, "_is_remote_mode", False)
    monkeypatch.setattr(supervisor.sys, "platform", "linux")
    monkeypatch.setattr(supervisor, "read_journal_config", lambda: config)
    monkeypatch.setattr(
        supervisor,
        "is_local_provider_needed",
        lambda config_arg=None: active_provider == "local",
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_endpoint.resolve_local_endpoint",
        lambda: type("Endpoint", (), {"is_bundled": endpoint_bundled})(),
    )
    monkeypatch.setattr(
        local_install,
        "target_fingerprint",
        lambda _model_id: {"provider": "local", "target": "one"},
    )
    monkeypatch.setattr(local_install, "inspect_readiness", inspect_readiness)
    monkeypatch.setattr(local_install, "gpu_device_override", lambda: None)
    monkeypatch.setattr(
        local_vulkan,
        "detect_gpus",
        lambda: [
            local_vulkan.VulkanDevice(0, "GPU", local_vulkan.VK_TYPE_DISCRETE, 12288)
        ],
    )
    monkeypatch.setattr(
        local_vulkan, "select_device", lambda devices, **_kw: devices[0]
    )
    monkeypatch.setattr(local_vulkan, "device_local_used_mib", lambda _index: 0)
    monkeypatch.setattr(local_server, "select_server_tier", lambda _vram: _FakeTier())

    observation = supervisor._observe_local_provider_truth()

    if active_provider != "local" or not endpoint_bundled:
        assert observation.phase == "not-desired"
        assert observation.reason_code == "provider-not-needed"
        assert inspect_calls == 0
    else:
        assert observation.phase == expected_phase
        assert inspect_calls == 1


def test_local_desired_state_table_remote_mode_skips_bundled_observation(
    monkeypatch,
) -> None:
    from solstone.think.providers import local_install

    monkeypatch.setattr(supervisor, "_is_remote_mode", True)
    monkeypatch.setattr(
        local_install,
        "inspect_readiness",
        lambda _model_id: pytest.fail("remote mode must not inspect bundled local"),
    )

    observation = supervisor._observe_local_provider_truth()

    assert observation.phase == "not-desired"
    assert observation.reason_code == "provider-not-needed"
    assert observation.detail["remote_mode"] is True


def test_local_desired_state_table_darwin_uses_mlx_observation(monkeypatch) -> None:
    from solstone.think.providers import local_install, mlx_install

    monkeypatch.setattr(supervisor, "_is_remote_mode", False)
    monkeypatch.setattr(supervisor.sys, "platform", "darwin")
    monkeypatch.setattr(supervisor, "read_journal_config", lambda: {})
    monkeypatch.setattr(
        supervisor, "is_local_provider_needed", lambda _config=None: True
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_endpoint.resolve_local_endpoint",
        lambda: type("Endpoint", (), {"is_bundled": True})(),
    )
    monkeypatch.setattr(
        local_install,
        "inspect_readiness",
        lambda _model_id: pytest.fail("darwin local observation must use MLX"),
    )
    monkeypatch.setattr(
        mlx_install,
        "target_fingerprint",
        lambda: {"provider": "local", "backend": "mlx"},
    )
    monkeypatch.setattr(
        mlx_install,
        "inspect_readiness",
        lambda: ReadinessOutcome(
            provider="local",
            status="ready",
            reason_code="ready",
            target={"model_id": supervisor.LOCAL_MODEL},
            install={"install_state": "idle"},
            host={"backend": "mlx"},
            artifacts={"runtime_dir": "/tmp/mlx-runtime"},
            proof={},
        ),
    )

    observation = supervisor._observe_local_provider_truth()

    assert observation.phase == "starting"
    assert observation.plan is not None
    assert observation.plan.backend == "mlx"


@pytest.mark.parametrize(
    (
        "remote_mode",
        "platform_can_host",
        "latch",
        "expected_phase",
        "expected_reason",
        "readiness_calls",
    ),
    [
        (
            True,
            True,
            None,
            "not-desired",
            "provider-not-needed",
            0,
        ),
        (
            False,
            False,
            None,
            "not-desired",
            "provider-not-needed",
            0,
        ),
        (
            False,
            True,
            {
                "desired": False,
                "blocked": False,
                "reason_code": "provider-not-needed",
            },
            "not-desired",
            "provider-not-needed",
            0,
        ),
        (
            False,
            True,
            {
                "desired": False,
                "blocked": True,
                "reason_code": "host-admission-blocked",
            },
            "host-blocked",
            "host-admission-blocked",
            0,
        ),
        (
            False,
            True,
            {
                "desired": True,
                "blocked": False,
                "reason_code": "provider-not-needed",
            },
            "starting",
            "launch-requested",
            1,
        ),
    ],
)
def test_parakeet_desired_state_table_remote_platform_and_stt(
    monkeypatch,
    remote_mode: bool,
    platform_can_host: bool,
    latch: dict[str, Any] | None,
    expected_phase: RuntimePhase,
    expected_reason: ReasonCode,
    readiness_calls: int,
) -> None:
    from solstone.think.providers import parakeet_install

    calls = 0
    monkeypatch.setattr(supervisor, "_is_remote_mode", remote_mode)
    monkeypatch.setattr(
        supervisor, "_parakeet_platform_can_host", lambda: platform_can_host
    )
    monkeypatch.setattr(supervisor, "read_journal_config", lambda: {"transcribe": {}})
    monkeypatch.setattr(
        supervisor,
        "_parakeet_stt_admission_latch",
        lambda _transcribe, _confidential: latch,
    )
    monkeypatch.setattr(
        parakeet_install,
        "target_fingerprint",
        lambda *, journal_path=None: {"provider": "parakeet", "target": "one"},
    )

    def inspect_readiness(_journal_path=None):
        nonlocal calls
        calls += 1
        return ReadinessOutcome(
            provider="parakeet",
            status="ready",
            reason_code="ready",
            target={},
            install={"install_state": "idle"},
            host={},
            artifacts={
                "binary_path_cpu": "/tmp/parakeet-cpu",
                "binary_path_vulkan": "/tmp/parakeet-vulkan",
                "model_path": "/tmp/parakeet-model.bin",
            },
            proof={},
        )

    monkeypatch.setattr(parakeet_install, "inspect_readiness", inspect_readiness)
    monkeypatch.setattr(supervisor, "_configured_parakeet_device", lambda: "cpu")
    monkeypatch.setattr(
        supervisor,
        "_resolve_parakeet_backend",
        lambda _device, _selected: ("cpu", {}, None),
    )
    monkeypatch.setattr(supervisor, "parakeet_physical_thread_count", lambda: 4)

    observation = supervisor._observe_parakeet_provider_truth()

    assert observation.phase == expected_phase
    assert observation.reason_code == expected_reason
    assert calls == readiness_calls


class _FakeTier:
    name = "floor"
    context_tokens = 16384
    parallel_slots = 1
    prompt_cache_mib = 0


def test_inactive_local_projection_publishes_not_desired_without_attempt(
    monkeypatch,
) -> None:
    observation = supervisor.ProviderTruthObservation(
        provider="local",
        phase="not-desired",
        reason_code="provider-not-needed",
        detail={"projection": {"status": "reconciling"}},
    )
    starts: list[int] = []
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(
        supervisor, "_observe_local_provider_truth", lambda: observation
    )
    monkeypatch.setattr(
        supervisor,
        "_provider_start_worker",
        lambda provider, plan_arg, fence, _cancel_event: starts.append(fence.attempt),
    )

    asyncio.run(supervisor._reconcile_local_provider_runtime([]))
    asyncio.run(supervisor._reconcile_local_provider_runtime([]))

    state = supervisor._provider_runtime_states["local"]
    record = read_runtime_health("local")
    assert state.latest_phase == "not-desired"
    assert state.retry.attempt_count == 0
    assert starts == []
    assert record["phase"] == "not-desired"
    assert record["detail"]["projection"]["status"] == "reconciling"


def test_byo_local_performs_no_bundled_observation(monkeypatch):
    from solstone.think.providers import local_install

    monkeypatch.setattr(supervisor, "_is_remote_mode", False)
    monkeypatch.setattr(supervisor, "read_journal_config", lambda: {})
    monkeypatch.setattr(
        supervisor, "is_local_provider_needed", lambda _config=None: True
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_endpoint.resolve_local_endpoint",
        lambda: type("Endpoint", (), {"is_bundled": False})(),
    )
    monkeypatch.setattr(
        local_install,
        "inspect_readiness",
        lambda _model_id: pytest.fail(
            "BYO endpoint must not inspect bundled readiness"
        ),
    )

    observation = supervisor._observe_local_provider_truth()

    assert observation.phase == "not-desired"
    assert observation.detail["projection"]["status"] == "reconciling"


def test_truth_observation_runs_off_tick_and_single_flight(monkeypatch):
    started = threading.Event()
    release = threading.Event()
    calls = 0

    def slow_observer():
        nonlocal calls
        calls += 1
        started.set()
        release.wait(timeout=5)
        return supervisor.ProviderTruthObservation(
            provider="local",
            phase="not-desired",
            reason_code="provider-not-needed",
            detail={},
        )

    monkeypatch.setattr(supervisor, "_observe_local_provider_truth", slow_observer)

    asyncio.run(supervisor._reconcile_local_provider_runtime([]))

    assert started.wait(timeout=1)
    state = supervisor._provider_runtime_states["local"]
    assert state.truth_future is not None
    assert not state.truth_future.done()

    asyncio.run(supervisor._reconcile_local_provider_runtime([]))

    assert calls == 1
    release.set()
    state.truth_future.result(timeout=1)
    asyncio.run(supervisor._reconcile_local_provider_runtime([]))

    assert state.latest_phase == "not-desired"
    assert read_runtime_health("local")["phase"] == "not-desired"


def test_no_op_tick_waits_for_truth_cadence_and_backoff_deadline(monkeypatch) -> None:
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "backoff"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry = supervisor.ProviderRetryState(
        attempt_count=1,
        next_at=200.0,
        desired_fingerprint=plan.desired_fingerprint_sha256,
    )
    state.next_truth_at = 160.0

    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 100.0)
    monkeypatch.setattr(
        supervisor,
        "_provider_start_worker",
        lambda *_args: pytest.fail("backoff deadline has not arrived"),
    )
    monkeypatch.setattr(
        supervisor,
        "_observe_local_provider_truth",
        lambda: pytest.fail("truth cadence has not arrived"),
    )

    asyncio.run(supervisor._reconcile_local_provider_runtime([]))

    assert state.start_future is None
    assert state.truth_future is None
    assert state.latest_phase == "backoff"


def test_start_worker_is_single_flight(monkeypatch) -> None:
    plan = _local_plan()
    pending: concurrent.futures.Future = concurrent.futures.Future()
    submitted = 0
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "starting"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.desired_fingerprint = plan.desired_fingerprint_sha256
    state.next_truth_at = 9999.0

    class _PendingExecutor:
        def submit(self, *_args, **_kwargs):
            nonlocal submitted
            submitted += 1
            return pending

    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 100.0)
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _PendingExecutor())

    asyncio.run(supervisor._reconcile_local_provider_runtime([]))
    asyncio.run(supervisor._reconcile_local_provider_runtime([]))

    assert submitted == 1
    assert state.start_future is pending
    assert state.retry.attempt_count == 1


def test_ready_episode_reobserves_on_sixty_second_cadence(monkeypatch) -> None:
    now = 100.0
    observations = 0
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "ready"
    state.desired_fingerprint = "fp-local"
    state.next_truth_at = now + supervisor.PROVIDER_STABLE_READY_REFRESH_SECONDS

    def monotonic() -> float:
        return now

    def observe():
        nonlocal observations
        observations += 1
        return supervisor.ProviderTruthObservation(
            provider="local",
            phase="ready",
            reason_code="ready-existing-owned-process",
            detail={},
            desired_fingerprint_sha256="fp-local",
            boot_required=True,
        )

    monkeypatch.setattr(supervisor.time, "monotonic", monotonic)
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(supervisor, "_observe_local_provider_truth", observe)

    asyncio.run(supervisor._reconcile_local_provider_runtime([]))

    assert observations == 0

    now = 160.0
    asyncio.run(supervisor._reconcile_local_provider_runtime([]))
    asyncio.run(supervisor._reconcile_local_provider_runtime([]))

    assert observations == 1
    assert state.latest_phase in {"ready", "ready-proof-unavailable"}


@pytest.mark.parametrize(
    ("provider", "plan", "managed_name"),
    [
        ("local", _local_plan(), supervisor.LOCAL_SERVER_PROCESS_NAME),
        ("parakeet", _parakeet_plan("vulkan"), supervisor.PARAKEET_SERVER_PROCESS_NAME),
    ],
)
def test_ready_truth_refresh_keeps_same_target_process_authoritative(
    monkeypatch,
    provider: str,
    plan: supervisor.LocalServerLaunchPlan | supervisor.ParakeetServerLaunchPlan,
    managed_name: str,
) -> None:
    state = supervisor._provider_runtime_states[provider]
    managed = _FakeManaged(managed_name)
    process = {
        "name": managed.name,
        "pid": managed.process.pid,
        "ref": managed.ref,
        "port": 45678,
    }
    _set_provider_ready(provider, state, plan)
    state.next_truth_at = 0.0
    state.next_probe_at = 10**12
    write_runtime_health(
        _runtime_record(
            provider,
            phase="ready",
            fingerprint=plan.desired_fingerprint_sha256,
            generation=state.generation,
            attempt=state.retry.attempt_count,
            process=process,
        )
    )
    observation = supervisor.ProviderTruthObservation(
        provider=provider,
        phase="starting",
        reason_code="launch-requested",
        detail={"source": "stable-refresh"},
        desired_fingerprint_json=plan.desired_fingerprint_json,
        desired_fingerprint_sha256=plan.desired_fingerprint_sha256,
        plan=plan,
        boot_required=True,
    )
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 100.0)
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(
        supervisor,
        "_observe_provider_truth",
        lambda provider_arg: observation,
    )

    asyncio.run(supervisor._reconcile_provider_runtime(provider, [managed]))
    submitted_health = read_runtime_health(provider)
    assert state.latest_phase in {"ready", "ready-proof-unavailable"}
    assert submitted_health["phase"] == "ready"
    assert submitted_health["process"] == process

    asyncio.run(supervisor._reconcile_provider_runtime(provider, [managed]))
    refreshed_health = read_runtime_health(provider)
    assert state.latest_phase == "ready"
    assert state.latest_plan is plan
    assert state.pending_stop_request is None
    assert state.stop_cleanup_future is None
    assert refreshed_health["phase"] == "ready"
    assert refreshed_health["process"] == process
    managed.terminate.assert_not_called()
    managed.cleanup.assert_not_called()

    state.truth_fence = supervisor._provider_fence(state, state.retry.attempt_count)
    state.truth_future = _future_with(observation)
    state.generation += 1
    assert supervisor._handle_provider_truth_result(state) is True
    fenced_health = read_runtime_health(provider)
    assert fenced_health["reason_code"] == "stale-result-ignored"
    assert fenced_health["process"] == process


def test_ready_truth_refresh_to_not_desired_retains_stop_ownership(
    monkeypatch,
) -> None:
    plan = _local_plan()
    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    process = {
        "name": managed.name,
        "pid": managed.process.pid,
        "ref": managed.ref,
        "port": 45678,
    }
    _set_provider_ready("local", state, plan)
    state.next_truth_at = 0.0
    state.next_probe_at = 10**12
    write_runtime_health(
        _runtime_record(
            "local",
            phase="ready",
            fingerprint=plan.desired_fingerprint_sha256,
            generation=state.generation,
            attempt=state.retry.attempt_count,
            process=process,
        )
    )
    observation = supervisor.ProviderTruthObservation(
        provider="local",
        phase="not-desired",
        reason_code="provider-not-needed",
        detail={"active_provider": "openai"},
        boot_required=False,
    )
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 100.0)
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(
        supervisor,
        "_observe_provider_truth",
        lambda _provider: observation,
    )

    supervisor._submit_provider_truth_if_needed(state)
    assert state.latest_phase == "ready"
    assert read_runtime_health("local")["process"] == process

    assert supervisor._handle_provider_truth_result(state) is True
    assert state.latest_phase == "stop-deferred"
    assert state.pending_stop_target_phase == "not-desired"
    assert state.pending_stop_admission_exclusive is True

    request = supervisor._deferred_stop_request(state, [managed])
    assert request is not None
    supervisor._set_provider_pending_stop_request(state, request)
    state.next_truth_at = 0.0
    supervisor._submit_provider_truth_if_needed(state)
    assert state.latest_phase == "stop-deferred"
    assert supervisor._handle_provider_truth_result(state) is True

    health = read_runtime_health("local")
    assert state.latest_phase == "stop-deferred"
    assert state.pending_stop_request is request
    assert state.pending_stop_request.managed is managed
    assert health["phase"] == "stop-deferred"
    assert health["process"] == process


def test_retry_token_resets_live_target_without_launching(monkeypatch) -> None:
    plan = _local_plan()
    observations = 0
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "ready"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry = supervisor.ProviderRetryState(
        attempt_count=4,
        next_at=9999.0,
        desired_fingerprint=plan.desired_fingerprint_sha256,
    )
    state.next_truth_at = 9999.0
    managed = _FakeManaged()
    request_retry_token(
        "local",
        desired_fingerprint_sha256=plan.desired_fingerprint_sha256,
        owner={"test": "retry"},
    )
    monkeypatch.setattr(
        supervisor,
        "_provider_start_worker",
        lambda *_args: pytest.fail("live retry token must not duplicate launch"),
    )

    def observe():
        nonlocal observations
        observations += 1
        return supervisor.ProviderTruthObservation(
            provider="local",
            phase="ready",
            reason_code="ready-existing-owned-process",
            detail={},
            desired_fingerprint_sha256=plan.desired_fingerprint_sha256,
            boot_required=True,
        )

    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(supervisor, "_observe_local_provider_truth", observe)

    asyncio.run(supervisor._reconcile_local_provider_runtime([managed]))
    asyncio.run(supervisor._reconcile_local_provider_runtime([managed]))

    assert state.retry.attempt_count == 0
    assert state.latest_phase == "stopped"
    assert observations == 1


def test_owner_retry_is_nonterminal_before_token_is_cleared(monkeypatch) -> None:
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "failed"
    state.desired_fingerprint = "fp-local"
    state.retry = supervisor.ProviderRetryState(
        attempt_count=len(supervisor.PROVIDER_RETRY_SCHEDULE_SECONDS),
        desired_fingerprint="fp-local",
    )
    failed = supervisor._write_provider_runtime(
        state,
        phase="failed",
        reason_code="launch-budget-exhausted",
        detail={},
    )
    assert failed is not None
    token = request_runtime_retry(
        "local",
        expected_health_revision=failed["revision"],
        expected_retry_revision=0,
        desired_fingerprint_sha256="fp-local",
    )

    real_consume = supervisor.consume_retry_token

    def assert_nonterminal_before_consume(*args, **kwargs):
        health = read_runtime_health("local")
        assert health["phase"] == "observing"
        with pytest.raises(
            supervisor.RuntimeHealthConflictError,
            match="terminal failure",
        ):
            request_runtime_retry(
                "local",
                expected_health_revision=health["revision"],
                expected_retry_revision=token["revision"],
                desired_fingerprint_sha256="fp-local",
            )
        return real_consume(*args, **kwargs)

    monkeypatch.setattr(
        supervisor, "consume_retry_token", assert_nonterminal_before_consume
    )

    supervisor._handle_provider_retry_token(state)

    assert read_retry_token("local")["token_id"] is None
    assert state.retry.attempt_count == 0


@pytest.mark.parametrize(
    "observation",
    [
        supervisor.ProviderTruthObservation(
            provider="local",
            phase="starting",
            reason_code="launch-requested",
            detail={},
            desired_fingerprint_json='{"provider":"local","target":"new"}',
            desired_fingerprint_sha256="fp-new",
            plan=_local_plan(),
            boot_required=True,
        ),
        supervisor.ProviderTruthObservation(
            provider="local",
            phase="not-desired",
            reason_code="provider-not-needed",
            detail={},
        ),
        supervisor.ProviderTruthObservation(
            provider="local",
            phase="state-corrupt",
            reason_code="record-malformed",
            detail={},
            desired_fingerprint_sha256="fp-local",
            boot_required=True,
        ),
        supervisor.ProviderTruthObservation(
            provider="local",
            phase="state-unavailable",
            reason_code="record-unavailable",
            detail={},
            desired_fingerprint_sha256="fp-local",
            boot_required=True,
        ),
    ],
)
def test_truth_change_signals_pending_start_cancel_event(
    observation: supervisor.ProviderTruthObservation,
) -> None:
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "starting"
    state.desired_fingerprint = "fp-local"
    state.retry.desired_fingerprint = "fp-local"
    state.start_fence = supervisor._provider_fence(state, 0)
    state.start_future = concurrent.futures.Future()
    state.start_cancel_event = threading.Event()
    state.truth_fence = supervisor._provider_fence(state, 0)
    state.truth_future = _future_with(observation)

    assert supervisor._handle_provider_truth_result(state) is True

    assert state.start_cancel_event is not None
    assert state.start_cancel_event.is_set()


@pytest.mark.parametrize(
    ("provider", "phase", "plan", "observation"),
    [
        (
            "local",
            "starting",
            _cuda_plan(),
            supervisor.ProviderTruthObservation(
                provider="local",
                phase="host-blocked",
                reason_code="gpu-unavailable",
                detail={"host": {"reason": "transient cuda pressure"}},
                desired_fingerprint_sha256="fp-local-cuda",
                boot_required=True,
            ),
        ),
        (
            "parakeet",
            "starting",
            _parakeet_plan("cpu"),
            supervisor.ProviderTruthObservation(
                provider="parakeet",
                phase="starting",
                reason_code="launch-requested",
                detail={"placement": "gpu"},
                desired_fingerprint_sha256="fp-parakeet",
                plan=_parakeet_plan("vulkan"),
                boot_required=True,
            ),
        ),
        (
            "parakeet",
            "ready",
            _parakeet_plan("cpu"),
            supervisor.ProviderTruthObservation(
                provider="parakeet",
                phase="starting",
                reason_code="launch-requested",
                detail={"placement": "gpu"},
                desired_fingerprint_sha256="fp-parakeet",
                plan=_parakeet_plan("vulkan"),
                boot_required=True,
            ),
        ),
    ],
)
def test_same_target_transient_observation_keeps_captured_plan_authoritative(
    provider: str,
    phase: RuntimePhase,
    plan: supervisor.LocalServerLaunchPlan | supervisor.ParakeetServerLaunchPlan,
    observation: supervisor.ProviderTruthObservation,
) -> None:
    state = supervisor._provider_runtime_states[provider]
    state.latest_phase = phase
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.desired_fingerprint = plan.desired_fingerprint_sha256
    state.truth_fence = supervisor._provider_fence(state, 0)
    state.truth_future = _future_with(observation)
    if phase == "starting":
        state.start_fence = supervisor._provider_fence(state, 0)
        state.start_future = concurrent.futures.Future()
        state.start_cancel_event = threading.Event()

    assert supervisor._handle_provider_truth_result(state) is True

    if state.start_cancel_event is not None:
        assert not state.start_cancel_event.is_set()
    assert state.latest_plan is plan
    assert state.latest_phase == phase


def test_owned_local_host_blocked_defers_admission_exclusive_stop() -> None:
    plan = _cuda_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "ready"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.desired_fingerprint = plan.desired_fingerprint_sha256
    state.truth_fence = supervisor._provider_fence(state, 0)
    state.truth_future = _future_with(
        supervisor.ProviderTruthObservation(
            provider="local",
            phase="host-blocked",
            reason_code="gpu-unavailable",
            detail={"host": {"reason": "transient cuda pressure"}},
            desired_fingerprint_sha256=plan.desired_fingerprint_sha256,
            boot_required=True,
        )
    )

    assert supervisor._handle_provider_truth_result(state) is True

    assert state.latest_plan is plan
    assert state.latest_phase == "stop-deferred"
    assert state.pending_stop_admission_exclusive is True
    assert state.pending_stop_target_phase == "host-blocked"


def _local_host_blocked_observation(
    plan: supervisor.LocalServerLaunchPlan,
    reason_code: ReasonCode,
) -> supervisor.ProviderTruthObservation:
    return supervisor.ProviderTruthObservation(
        provider="local",
        phase="host-blocked",
        reason_code=reason_code,
        detail={"host": {"reason": reason_code}},
        desired_fingerprint_sha256=plan.desired_fingerprint_sha256,
        boot_required=True,
    )


def test_ready_local_ram_insufficient_host_blocked_does_not_defer_stop() -> None:
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    _set_provider_ready("local", state, plan)
    state.truth_fence = supervisor._provider_fence(state, state.retry.attempt_count)
    state.truth_future = _future_with(
        _local_host_blocked_observation(plan, "ram-insufficient")
    )

    assert supervisor._handle_provider_truth_result(state) is True

    assert state.latest_phase == "ready"
    assert state.pending_stop_admission_exclusive is False
    assert state.pending_stop_target_phase == "stopped"
    assert state.pending_stop_request is None


@pytest.mark.parametrize(
    "reason_code",
    [
        "platform-unsupported",
        "package-unavailable",
        "openmp-runtime-unavailable",
        "gpu-probe-failed",
        "gpu-unavailable",
        "host-admission-blocked",
    ],
)
def test_ready_local_liveness_host_blocked_still_defers_admission_exclusive_stop(
    reason_code: ReasonCode,
) -> None:
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    _set_provider_ready("local", state, plan)
    state.truth_fence = supervisor._provider_fence(state, state.retry.attempt_count)
    state.truth_future = _future_with(
        _local_host_blocked_observation(plan, reason_code)
    )

    assert supervisor._handle_provider_truth_result(state) is True

    assert state.latest_phase == "stop-deferred"
    assert state.pending_stop_admission_exclusive is True
    assert state.pending_stop_target_phase == "host-blocked"
    assert state.pending_stop_target_reason_code == reason_code


def test_ready_local_ram_insufficient_decline_preserves_plan_and_process() -> None:
    plan = _local_plan()
    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    process = {
        "name": managed.name,
        "pid": managed.process.pid,
        "ref": managed.ref,
        "port": 45678,
    }
    _set_provider_ready("local", state, plan)
    write_runtime_health(
        _runtime_record(
            "local",
            phase="ready",
            fingerprint=plan.desired_fingerprint_sha256,
            generation=state.generation,
            attempt=state.retry.attempt_count,
            process=process,
        )
    )
    state.truth_fence = supervisor._provider_fence(state, state.retry.attempt_count)
    state.truth_future = _future_with(
        _local_host_blocked_observation(plan, "ram-insufficient")
    )

    assert supervisor._handle_provider_truth_result(state) is True

    health = read_runtime_health("local")
    assert state.latest_phase == "ready"
    assert state.latest_plan is plan
    assert health["process"] == process
    assert health["reason_code"] == "stale-result-ignored"
    assert health["detail"]["slot"] == "truth"
    assert health["detail"]["latched_phase"] == "host-blocked"
    assert health["detail"]["latched_reason_code"] == "ram-insufficient"


def test_ready_local_ram_insufficient_decline_still_submits_probe(monkeypatch) -> None:
    class _RecordingExecutor:
        def __init__(self) -> None:
            self.calls: list[tuple[Any, tuple[Any, ...]]] = []
            self.future: concurrent.futures.Future = concurrent.futures.Future()

        def submit(self, fn, *args):
            self.calls.append((fn, args))
            return self.future

    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    _set_provider_ready("local", state, plan)
    state.truth_fence = supervisor._provider_fence(state, state.retry.attempt_count)
    state.truth_future = _future_with(
        _local_host_blocked_observation(plan, "ram-insufficient")
    )

    assert supervisor._handle_provider_truth_result(state) is True
    assert state.latest_phase == "ready"

    executor = _RecordingExecutor()
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: executor)
    monkeypatch.setattr(supervisor, "read_service_port", lambda _service: 45678)

    supervisor._submit_provider_probe_if_needed(state)

    assert state.probe_future is executor.future
    assert state.probe_fence is not None
    assert len(executor.calls) == 1
    fn, args = executor.calls[0]
    assert fn is supervisor._provider_probe_worker
    assert args == ("local", 45678, state.probe_fence)


def test_ready_local_ram_insufficient_decline_write_unavailable_does_not_defer_stop(
    monkeypatch,
) -> None:
    """The error path must not re-enter the FALL-THROUGH trap.

    The plan is preserved, the process record is preserved, no stop is deferred,
    and the phase is the honest pre-existing write-failure latch rather than a
    false host-blocked.
    """
    plan = _local_plan()
    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    process = {
        "name": managed.name,
        "pid": managed.process.pid,
        "ref": managed.ref,
        "port": 45678,
    }
    _set_provider_ready("local", state, plan)
    write_runtime_health(
        _runtime_record(
            "local",
            phase="ready",
            fingerprint=plan.desired_fingerprint_sha256,
            generation=state.generation,
            attempt=state.retry.attempt_count,
            process=process,
        )
    )

    def fail_write(*_args, **_kwargs):
        raise RuntimeHealthUnavailableError("runtime health write unavailable")

    monkeypatch.setattr(supervisor, "write_runtime_health", fail_write)
    state.truth_fence = supervisor._provider_fence(state, state.retry.attempt_count)
    state.truth_future = _future_with(
        _local_host_blocked_observation(plan, "ram-insufficient")
    )

    assert supervisor._handle_provider_truth_result(state) is True

    assert state.pending_stop_request is None
    assert state.pending_stop_admission_exclusive is False
    assert state.pending_stop_target_phase == "stopped"
    assert state.latest_plan is plan
    assert read_runtime_health("local")["process"] == process
    assert state.latest_phase == "state-unavailable"


def test_parakeet_stt_admission_latch_survives_ready_probe_and_restart(
    monkeypatch,
) -> None:
    monkeypatch.setattr(supervisor.sys, "platform", "linux")
    monkeypatch.setattr(supervisor.platform, "machine", lambda: "x86_64")
    monkeypatch.setattr(supervisor, "local_stt_backend", lambda: "parakeet")
    monkeypatch.setattr(supervisor, "stt_local_floor_bytes", lambda: 4 * 1024**3)
    transcribe: dict[str, Any] = {}
    admission_input = supervisor._parakeet_stt_admission_input(transcribe, False)
    _input_json, input_sha = supervisor._target_fingerprint_pair(admission_input)
    latch = {
        "input_json": "{}",
        "input_sha256": input_sha,
        "retry_epoch": 0,
        "choice": "parakeet",
        "desired": True,
        "blocked": False,
        "reason_code": "launch-requested",
    }
    plan = _parakeet_plan("cpu")
    state = supervisor._provider_runtime_states["parakeet"]
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.generation = 1
    state.retry.attempt_count = 1
    write_runtime_health(
        {
            **read_runtime_health("parakeet"),
            "phase": "starting",
            "reason_code": "launch-requested",
            "detail": {"stt_admission_latch": latch},
            "desired_fingerprint_sha256": plan.desired_fingerprint_sha256,
            "incarnation": supervisor._PROVIDER_INCARNATION,
            "generation": 1,
            "attempt": 1,
            "process": None,
            "updated_at": "2026-07-19T00:00:00+00:00",
            "owner": {"test": "latch"},
        }
    )

    supervisor._write_provider_runtime(
        state,
        phase="ready",
        reason_code="probe-ready",
        detail={"backend": "cpu", "port": 45678},
        process={
            "name": supervisor.PARAKEET_SERVER_PROCESS_NAME,
            "pid": 12345,
            "ref": "ref-parakeet",
            "port": 45678,
        },
    )
    supervisor._write_provider_runtime(
        state,
        phase="ready-proof-unavailable",
        reason_code="proof-observation-unavailable",
        detail={"health_state": "failed"},
        process={
            "name": supervisor.PARAKEET_SERVER_PROCESS_NAME,
            "pid": 12345,
            "ref": "ref-parakeet",
            "port": 45678,
        },
    )
    monkeypatch.setattr(
        supervisor,
        "read_available_bytes",
        lambda: pytest.fail("valid latch must avoid point-in-time RAM recheck"),
    )

    recovered = supervisor._parakeet_stt_admission_latch(transcribe, False)

    assert recovered == latch
    assert read_runtime_health("parakeet")["detail"]["stt_admission_latch"] == latch


def test_handle_shutdown_signals_pending_provider_start(monkeypatch) -> None:
    state = supervisor._provider_runtime_states["local"]
    event = threading.Event()
    state.start_cancel_event = event
    state.start_fence = supervisor.ProviderFence(
        incarnation=supervisor._PROVIDER_INCARNATION,
        generation=1,
        fingerprint="fp-local",
        attempt=1,
    )
    monkeypatch.setattr(supervisor, "_managed_procs", [])
    monkeypatch.setattr(supervisor, "shutdown_requested", False)

    try:
        with pytest.raises(KeyboardInterrupt):
            supervisor.handle_shutdown(15, None)
    finally:
        supervisor.shutdown_requested = False

    assert event.is_set()


def test_local_probe_worker_reports_loading_without_opening_socket(monkeypatch) -> None:
    from solstone.think.providers import local_server

    calls: list[int] = []

    def fake_probe(port: int):
        calls.append(port)
        return local_server.STATE_LOADING, None

    monkeypatch.setattr(local_server, "_probe_health", fake_probe)

    outcome = supervisor._provider_probe_worker(
        "local",
        45678,
        supervisor.ProviderFence(
            incarnation=supervisor._PROVIDER_INCARNATION,
            generation=1,
            fingerprint="fp-local",
            attempt=1,
        ),
    )

    assert calls == [45678]
    assert outcome.status == "not-ready"
    assert outcome.reason_code == "proof-observation-unavailable"
    assert outcome.detail["health_state"] == local_server.STATE_LOADING


def test_parakeet_probe_worker_has_no_loading_state(monkeypatch) -> None:
    from solstone.think.providers import parakeet_server

    calls: list[int] = []

    def fake_probe(port: int):
        calls.append(port)
        return parakeet_server.STATE_FAILED, "warming"

    monkeypatch.setattr(parakeet_server, "_probe_health", fake_probe)

    outcome = supervisor._provider_probe_worker(
        "parakeet",
        45679,
        supervisor.ProviderFence(
            incarnation=supervisor._PROVIDER_INCARNATION,
            generation=1,
            fingerprint="fp-parakeet",
            attempt=1,
        ),
    )

    assert calls == [45679]
    assert outcome.status == "unavailable"
    assert outcome.reason_code == "proof-observation-unavailable"
    assert outcome.detail["health_state"] == parakeet_server.STATE_FAILED


def test_probe_slot_transitions_ready_and_proof_unavailable_with_fakes(
    monkeypatch,
) -> None:
    from solstone.think.providers import local_server

    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    plan = _local_plan()
    _set_provider_ready("local", state, plan)
    state.next_truth_at = 10**12
    state.next_probe_at = 0.0
    supervisor.write_service_port("local", 45678)
    write_runtime_health(
        _runtime_record(
            "local",
            phase="ready",
            fingerprint=plan.desired_fingerprint_sha256,
            generation=1,
            attempt=1,
            process={
                "name": managed.name,
                "pid": managed.process.pid,
                "ref": managed.ref,
                "port": 45678,
            },
        )
    )
    observations = [
        (local_server.STATE_LOADING, None),
        (local_server.STATE_READY, None),
    ]
    calls: list[int] = []

    def fake_probe(port: int):
        calls.append(port)
        return observations.pop(0)

    monkeypatch.setattr(local_server, "_probe_health", fake_probe)
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 100.0)

    asyncio.run(supervisor._reconcile_local_provider_runtime([managed]))
    asyncio.run(supervisor._reconcile_local_provider_runtime([managed]))

    assert state.latest_phase == "ready-proof-unavailable"
    assert calls == [45678]
    assert read_runtime_health("local")["process"]["ref"] == managed.ref

    state.next_probe_at = 0.0
    asyncio.run(supervisor._reconcile_local_provider_runtime([managed]))
    asyncio.run(supervisor._reconcile_local_provider_runtime([managed]))

    assert state.latest_phase == "ready"
    assert calls == [45678, 45678]


def test_wedge_threshold_records_recycle_token_without_sync_termination(
    monkeypatch,
) -> None:
    from solstone.think.providers import local_endpoint, local_server
    from solstone.think.providers.local_endpoint import LocalEndpoint

    state = supervisor._provider_runtime_states["local"]
    plan = _local_plan()
    _set_provider_ready("local", state, plan)
    state.generation = 4
    state.next_truth_at = 9999.0
    managed = _FakeManaged()
    supervisor._managed_procs = [managed]
    supervisor._SERVICE_STATE[managed.name] = {
        "restart": False,
        "shutdown_timeout": 15,
    }
    monkeypatch.setattr(
        local_endpoint,
        "resolve_local_endpoint",
        lambda: LocalEndpoint("", "", None, True),
    )
    monkeypatch.setattr(supervisor, "read_service_port", lambda service: 45678)
    monkeypatch.setattr(
        local_server,
        "_probe_health",
        lambda port: (local_server.STATE_READY, None),
    )
    monkeypatch.setattr(
        supervisor,
        "_restart_service",
        lambda *_args, **_kwargs: pytest.fail("wedge must not restart synchronously"),
    )
    monkeypatch.setattr(
        supervisor,
        "_start_termination_thread",
        lambda *_args, **_kwargs: pytest.fail("wedge must not terminate synchronously"),
    )

    for idx in range(supervisor.LOCAL_WEDGE_THRESHOLD):
        use_id = f"wedge-{idx}"
        supervisor._handle_cortex_outcome(
            {
                "tract": "cortex",
                "event": "start",
                "use_id": use_id,
                "provider": "local",
            }
        )
        supervisor._handle_cortex_outcome(
            {
                "tract": "cortex",
                "event": "error",
                "use_id": use_id,
                "reason_code": "provider_unavailable",
            }
        )

    token = read_retry_token("local")
    assert token["reason_code"] == "local-wedge-provider-unavailable"
    assert token["desired_fingerprint_sha256"] == plan.desired_fingerprint_sha256
    assert state.generation == 5
    assert state.latest_phase == "retry-requested"
    assert state.next_truth_at == 0.0
    assert supervisor._recovery_state["local"].down_generation == 5
    assert supervisor._SERVICE_STATE[managed.name] == {
        "restart": False,
        "shutdown_timeout": 15,
    }
    managed.terminate.assert_not_called()


def _plan_for_backend(
    backend: str,
) -> supervisor.LocalServerLaunchPlan | supervisor.ParakeetServerLaunchPlan:
    if backend == "vulkan":
        return _local_plan()
    if backend == "cuda":
        return _cuda_plan()
    if backend == "mlx":
        return _mlx_plan()
    if backend == "parakeet-vulkan":
        return _parakeet_plan("vulkan")
    return _parakeet_plan("cpu")


def _process_name_for_backend(backend: str) -> str:
    if backend == "mlx":
        return supervisor.MLX_SERVER_PROCESS_NAME
    if backend.startswith("parakeet"):
        return supervisor.PARAKEET_SERVER_PROCESS_NAME
    return supervisor.LOCAL_SERVER_PROCESS_NAME


def _launch_backend_for_test(
    backend: str,
    plan: supervisor.LocalServerLaunchPlan | supervisor.ParakeetServerLaunchPlan,
    reservation: _FakeReservation,
    cancel_event: threading.Event,
) -> supervisor.ProviderLaunchOutcome:
    if backend.startswith("parakeet"):
        return supervisor.start_parakeet_server(
            plan,
            reservation,
            cancel_event,
        )
    return supervisor.start_local_server(plan, reservation, cancel_event)


@pytest.mark.parametrize(
    "backend",
    ["vulkan", "cuda", "mlx", "parakeet-cpu", "parakeet-vulkan"],
)
@pytest.mark.parametrize("cancel_point", ["before-probe", "ready", "wait"])
def test_start_worker_cancellation_cleans_child_at_warmup_boundaries(
    monkeypatch,
    backend: str,
    cancel_point: str,
) -> None:
    from solstone.think.providers import local_server, local_vulkan, parakeet_server

    plan = _plan_for_backend(backend)
    cancel_event = threading.Event()
    probe_entered = threading.Event()
    managed = _FakeManaged(_process_name_for_backend(backend))
    service_name = managed.name
    ports: list[tuple[str, int]] = []
    placements: list[str] = []

    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 0.0)
    monkeypatch.setattr(
        supervisor,
        "write_service_port",
        lambda service, port: ports.append((service, port)),
    )
    monkeypatch.setattr(
        local_server,
        "write_local_context_window",
        lambda _tokens: None,
    )
    monkeypatch.setattr(
        local_server,
        "fetch_props",
        lambda _port: pytest.fail("cancelled launch must not fetch props"),
    )
    monkeypatch.setattr(
        local_vulkan,
        "device_local_used_mib",
        lambda _index: pytest.fail("cancelled launch must not inspect post-ready VRAM"),
    )
    monkeypatch.setattr(
        parakeet_server,
        "write_parakeet_placement",
        lambda placement: placements.append(placement),
    )

    def launch_process(name, _cmd, **_kwargs):
        supervisor._SERVICE_STATE[name] = {
            "restart": True,
            "shutdown_timeout": 15,
        }
        if cancel_point == "before-probe":
            cancel_event.set()
        return managed

    def local_probe(_port):
        probe_entered.set()
        if cancel_point == "ready":
            cancel_event.set()
            return local_server.STATE_READY, None
        return local_server.STATE_STARTING, None

    def parakeet_probe(_port):
        probe_entered.set()
        if cancel_point == "ready":
            cancel_event.set()
            return parakeet_server.STATE_READY, None
        return parakeet_server.STATE_FAILED, "warming"

    monkeypatch.setattr(supervisor, "_launch_process", launch_process)
    if not backend.startswith("parakeet"):
        monkeypatch.setattr(
            supervisor,
            "_request_local_launch_plan",
            _native_launch_plan_for_test,
        )
    monkeypatch.setattr(local_server, "_probe_health", local_probe)
    monkeypatch.setattr(parakeet_server, "_probe_health", parakeet_probe)

    outcome_box: dict[str, supervisor.ProviderLaunchOutcome] = {}

    def run_launch() -> None:
        outcome_box["outcome"] = _launch_backend_for_test(
            backend,
            plan,
            _FakeReservation(port=45678),
            cancel_event,
        )

    if cancel_point == "wait":
        thread = threading.Thread(target=run_launch)
        thread.start()
        assert probe_entered.wait(timeout=1.0)
        cancel_event.set()
        thread.join(timeout=1.0)
        assert not thread.is_alive()
    else:
        run_launch()

    outcome = outcome_box["outcome"]
    assert outcome.status == "launch-failed"
    assert outcome.detail["cancelled"] is True
    assert outcome.managed is None
    managed.terminate.assert_called_once()
    managed.cleanup.assert_called_once_with()
    assert service_name not in supervisor._SERVICE_STATE
    assert ports == []
    assert placements == []
    if cancel_point == "before-probe":
        assert not probe_entered.is_set()


def test_cancelled_launch_outcome_preserves_handle_when_cleanup_raises(
    monkeypatch,
) -> None:
    managed = _FakeManaged()
    monkeypatch.setattr(
        supervisor,
        "_terminate_cleanup_handle",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(RuntimeError("cleanup failed")),
    )

    outcome = supervisor._cancelled_launch_outcome(
        "local",
        backend="cuda",
        port=45678,
        managed=managed,
        reason="test cancellation",
    )

    assert outcome.status == "launch-failed"
    assert outcome.managed is managed
    assert outcome.detail["cancelled"] is True
    assert outcome.detail["cleanup_failed"] is True
    assert outcome.detail["cleanup_deferred_to"] == "cleanup-failed-reconciler"


def test_cancelled_ready_result_is_cleaned_without_port_publication(monkeypatch):
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "starting"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    fence = supervisor._provider_fence(state, 1)
    managed = _FakeManaged()
    cancel_event = threading.Event()
    cancel_event.set()
    ports: list[tuple[str, int]] = []
    state.start_fence = fence
    state.start_cancel_event = cancel_event
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={"port": 45678},
            managed=managed,
        )
    )
    monkeypatch.setattr(
        supervisor,
        "write_service_port",
        lambda service, port: ports.append((service, port)),
    )

    assert supervisor._handle_provider_start_result(state, []) is True

    assert ports == []
    managed.terminate.assert_called_once()
    managed.cleanup.assert_called_once_with()
    assert state.latest_phase == "backoff"


def test_superseded_start_cleanup_failure_is_adopted(monkeypatch) -> None:
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_plan = plan
    state.latest_phase = "ready"
    state.desired_fingerprint = "fp-new"
    state.retry.attempt_count = 2
    state.generation = 2
    old_managed = _FakeManaged()
    state.start_fence = supervisor.ProviderFence(
        incarnation=supervisor._PROVIDER_INCARNATION,
        generation=1,
        fingerprint="fp-old",
        attempt=1,
    )
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="warmup-timeout",
            reason_code="warmup-timeout",
            detail={"port": 11111},
            managed=old_managed,
        )
    )
    monkeypatch.setattr(
        supervisor,
        "_terminate_cleanup_handle",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(RuntimeError("cleanup failed")),
    )

    assert supervisor._handle_provider_start_result(state, []) is True

    assert state.latest_phase == "cleanup-failed"
    assert state.pending_stop_request is not None
    assert state.pending_stop_request.managed is old_managed


def test_pending_stop_request_assignment_uses_single_chokepoint() -> None:
    tree = ast.parse(Path(supervisor.__file__).read_text(encoding="utf-8"))
    offenders: list[tuple[str, int]] = []
    stack: list[str] = []

    class Visitor(ast.NodeVisitor):
        def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
            stack.append(node.name)
            self.generic_visit(node)
            stack.pop()

        def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
            stack.append(node.name)
            self.generic_visit(node)
            stack.pop()

        def visit_Assign(self, node: ast.Assign) -> None:
            for target in node.targets:
                self._check_target(target, node.lineno)
            self.generic_visit(node)

        def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
            self._check_target(node.target, node.lineno)
            self.generic_visit(node)

        def _check_target(self, target: ast.expr, lineno: int) -> None:
            if not (
                isinstance(target, ast.Attribute)
                and target.attr == "pending_stop_request"
            ):
                return
            owner = stack[-1] if stack else "<module>"
            if owner != "_set_provider_pending_stop_request":
                offenders.append((owner, lineno))

    Visitor().visit(tree)

    assert offenders == []


def test_pending_cleanup_survives_truth_phase_change_and_blocks_start(
    monkeypatch,
) -> None:
    plan = _local_plan()
    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "cleanup-failed"
    state.latest_plan = None
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    state.pending_stop_request = supervisor._make_stop_request(
        state,
        managed,
        reason_code="cleanup-attempt-failed",
        detail={"source": "preserved-handle"},
        target_phase="stopped",
        target_reason_code="cleanup-succeeded",
    )
    state.cleanup_attempt_count = 1
    state.cleanup_next_at = 50.0
    state.truth_fence = supervisor._provider_fence(state, 1)
    state.truth_future = _future_with(
        supervisor.ProviderTruthObservation(
            provider="local",
            phase="starting",
            reason_code="launch-requested",
            detail={"backend": plan.backend},
            desired_fingerprint_json=plan.desired_fingerprint_json,
            desired_fingerprint_sha256=plan.desired_fingerprint_sha256,
            plan=plan,
            boot_required=True,
        )
    )
    start_submits: list[str] = []
    monkeypatch.setattr(
        supervisor,
        "_provider_start_worker",
        lambda *_args: start_submits.append("start"),
    )

    assert supervisor._handle_provider_truth_result(state) is True
    assert state.latest_phase == "starting"
    assert state.pending_stop_request is not None
    assert state.pending_stop_request.managed is managed

    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 10.0)
    assert supervisor._submit_provider_stop_cleanup_if_needed(state, []) is True
    assert state.stop_cleanup_future is None
    supervisor._submit_provider_start_if_needed(state, [])
    assert state.start_future is None
    assert start_submits == []

    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 50.0)
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    assert supervisor._submit_provider_stop_cleanup_if_needed(state, []) is True
    assert supervisor._handle_provider_stop_cleanup_result(state, []) is True

    managed.terminate.assert_called_once()
    managed.cleanup.assert_called_once_with()
    assert state.pending_stop_request is None
    assert state.latest_phase == "stopped"
    assert start_submits == []


def test_cancelled_stop_cleanup_preserves_unresolved_handle(monkeypatch) -> None:
    plan = _local_plan()
    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "stopping"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    state.pending_stop_request = supervisor._make_stop_request(
        state,
        managed,
        reason_code="cleanup-attempt-failed",
        detail={"source": "preserved-handle"},
        target_phase="stopped",
        target_reason_code="cleanup-succeeded",
    )
    state.stop_cleanup_fence = supervisor._provider_fence(state, 1)
    state.stop_cleanup_future = _future_with(
        supervisor.ProviderStopCleanupOutcome(
            status="cancelled",
            reason_code="stale-result-ignored",
            detail={"cancelled": True},
            managed=managed,
        )
    )
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 100.0)

    assert supervisor._handle_provider_stop_cleanup_result(state, []) is True

    assert state.latest_phase == "cleanup-failed"
    assert state.pending_stop_request is not None
    assert state.pending_stop_request.managed is managed
    assert state.cleanup_attempt_count == 1
    assert state.cleanup_next_at == 102.0
    supervisor._submit_provider_start_if_needed(state, [])
    assert state.start_future is None


def test_late_probe_cannot_declare_ready_with_cleanup_outstanding(
    monkeypatch,
) -> None:
    plan = _local_plan()
    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "cleanup-failed"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    state.pending_stop_request = supervisor._make_stop_request(
        state,
        managed,
        reason_code="cleanup-attempt-failed",
        detail={"source": "preserved-handle"},
        target_phase="stopped",
        target_reason_code="cleanup-succeeded",
    )
    state.probe_fence = supervisor._provider_fence(state, 1)
    state.probe_future = _future_with(
        supervisor.ProviderProbeOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={"port": 45678},
        )
    )
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 100.0)

    assert supervisor._handle_provider_probe_result(state) is True

    assert state.latest_phase == "cleanup-failed"
    assert state.pending_stop_request is not None
    assert state.pending_stop_request.managed is managed
    assert state.next_probe_at == 100.0 + supervisor.PROVIDER_PROBE_INTERVAL_SECONDS
    assert read_runtime_health("local")["reason_code"] == "stale-result-ignored"


def test_deferred_stop_preserves_existing_cleanup_request() -> None:
    old_plan = _local_plan()
    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    _set_provider_ready("local", state, old_plan)
    state.pending_stop_request = supervisor._make_stop_request(
        state,
        managed,
        reason_code="cleanup-attempt-failed",
        detail={"source": "preserved-handle"},
        target_phase="stopped",
        target_reason_code="cleanup-succeeded",
    )
    state.cleanup_attempt_count = 2
    state.cleanup_next_at = 50.0
    observation = supervisor.ProviderTruthObservation(
        provider="local",
        phase="not-desired",
        reason_code="provider-not-needed",
        detail={"active_provider": "cloud"},
        desired_fingerprint_sha256=old_plan.desired_fingerprint_sha256,
        boot_required=False,
    )

    supervisor._defer_provider_stop_for_observation(
        state,
        observation,
        reason_code="admission-exclusive-stop",
        admission_exclusive=True,
    )

    assert state.latest_phase == "stop-deferred"
    assert state.pending_stop_request is not None
    assert state.pending_stop_request.managed is managed
    assert state.pending_stop_request.target_phase == "stop-deferred"
    assert state.pending_stop_request.target_detail["target_phase"] == "not-desired"
    assert state.cleanup_attempt_count == 2
    assert state.cleanup_next_at == 50.0


def test_unresolvable_cleanup_stays_owned_visible_and_blocks_start(
    monkeypatch,
) -> None:
    plan = _local_plan()
    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "cleanup-failed"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    state.pending_stop_request = supervisor._make_stop_request(
        state,
        managed,
        reason_code="cleanup-attempt-failed",
        detail={"source": "preserved-handle"},
        target_phase="stopped",
        target_reason_code="cleanup-succeeded",
    )
    state.cleanup_attempt_count = 0
    state.cleanup_next_at = 0.0
    starts: list[str] = []
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 100.0)
    monkeypatch.setattr(
        supervisor,
        "_terminate_cleanup_handle",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(RuntimeError("still alive")),
    )
    monkeypatch.setattr(
        supervisor,
        "_provider_start_worker",
        lambda *_args: starts.append("start"),
    )

    assert supervisor._submit_provider_stop_cleanup_if_needed(state, []) is True
    assert supervisor._handle_provider_stop_cleanup_result(state, []) is True
    supervisor._submit_provider_start_if_needed(state, [])

    assert state.latest_phase == "cleanup-failed"
    assert state.pending_stop_request is not None
    assert state.pending_stop_request.managed is managed
    assert state.cleanup_attempt_count == 1
    assert state.cleanup_next_at == 102.0
    assert state.start_future is None
    assert starts == []


def test_cancelled_ready_cleanup_failure_is_adopted(monkeypatch) -> None:
    plan = _local_plan()
    queue = _FakeTaskQueue()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "starting"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    fence = supervisor._provider_fence(state, 1)
    managed = _FakeManaged()
    cancel_event = threading.Event()
    cancel_event.set()
    state.start_fence = fence
    state.start_cancel_event = cancel_event
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={"port": 45678},
            managed=managed,
        )
    )
    monkeypatch.setattr(supervisor, "_task_queue", queue)
    monkeypatch.setattr(
        supervisor,
        "_provider_startup_gate",
        supervisor.ProviderStartupGate(
            started_at=0.0,
            required={"local"},
            terminal=set(),
            attempted={},
            first_start_at=None,
            released=False,
        ),
    )
    monkeypatch.setattr(
        supervisor,
        "_terminate_cleanup_handle",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(RuntimeError("cleanup failed")),
    )

    assert supervisor._handle_provider_start_result(state, []) is True

    assert state.latest_phase == "cleanup-failed"
    assert state.pending_stop_request is not None
    assert state.pending_stop_request.managed is managed
    assert supervisor._provider_startup_gate.attempted == {"local": "ready"}


def test_missing_ready_port_cleanup_failure_is_adopted(monkeypatch) -> None:
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "starting"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    managed = _FakeManaged()
    state.start_fence = supervisor._provider_fence(state, 1)
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={},
            managed=managed,
        )
    )
    monkeypatch.setattr(
        supervisor,
        "_terminate_cleanup_handle",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(RuntimeError("cleanup failed")),
    )

    assert supervisor._handle_provider_start_result(state, []) is True

    assert state.latest_phase == "cleanup-failed"
    assert state.pending_stop_request is not None
    assert state.pending_stop_request.managed is managed


def test_port_publication_cleanup_failure_is_adopted(monkeypatch) -> None:
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "starting"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    managed = _FakeManaged()
    state.start_fence = supervisor._provider_fence(state, 1)
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={"port": 45678},
            managed=managed,
        )
    )
    monkeypatch.setattr(
        supervisor,
        "write_service_port",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(OSError("disk full")),
    )
    monkeypatch.setattr(
        supervisor,
        "_terminate_cleanup_handle",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(RuntimeError("cleanup failed")),
    )

    assert supervisor._handle_provider_start_result(state, []) is True

    assert state.latest_phase == "cleanup-failed"
    assert state.pending_stop_request is not None
    assert state.pending_stop_request.managed is managed


def test_observation_raced_when_target_fingerprint_changes_between_reads(
    monkeypatch,
) -> None:
    from solstone.think.providers import local_install, local_server, local_vulkan

    fingerprints = iter(
        [
            {"provider": "local", "target": "one"},
            {"provider": "local", "target": "two"},
        ]
    )
    monkeypatch.setattr(supervisor, "_is_remote_mode", False)
    monkeypatch.setattr(supervisor.sys, "platform", "linux")
    monkeypatch.setattr(supervisor, "read_journal_config", lambda: {})
    monkeypatch.setattr(
        supervisor, "is_local_provider_needed", lambda _config=None: True
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_endpoint.resolve_local_endpoint",
        lambda: type("Endpoint", (), {"is_bundled": True})(),
    )
    monkeypatch.setattr(
        local_install,
        "target_fingerprint",
        lambda _model_id: next(fingerprints),
    )
    monkeypatch.setattr(
        local_install,
        "inspect_readiness",
        lambda _model_id: _local_readiness(),
    )
    monkeypatch.setattr(local_install, "gpu_device_override", lambda: None)
    monkeypatch.setattr(
        local_vulkan,
        "detect_gpus",
        lambda: [
            local_vulkan.VulkanDevice(0, "GPU", local_vulkan.VK_TYPE_DISCRETE, 12288)
        ],
    )
    monkeypatch.setattr(
        local_vulkan, "select_device", lambda devices, **_kw: devices[0]
    )
    monkeypatch.setattr(local_vulkan, "device_local_used_mib", lambda _index: 0)
    monkeypatch.setattr(local_server, "select_server_tier", lambda _vram: _FakeTier())
    monkeypatch.setattr(
        supervisor,
        "_launch_process",
        lambda *_args, **_kwargs: pytest.fail("observation race must not launch"),
    )

    observation = supervisor._observe_local_provider_truth()

    assert observation.phase == "observing"
    assert observation.reason_code == "observation-raced"


def test_observation_raced_when_device_changes_during_plan_construction(
    monkeypatch,
) -> None:
    from solstone.think.providers import local_install, local_server, local_vulkan

    devices = [
        local_vulkan.VulkanDevice(0, "GPU-A", local_vulkan.VK_TYPE_DISCRETE, 12288),
        local_vulkan.VulkanDevice(1, "GPU-B", local_vulkan.VK_TYPE_DISCRETE, 16384),
    ]
    selections = iter([devices[0], devices[0], devices[1]])
    monkeypatch.setattr(supervisor, "_is_remote_mode", False)
    monkeypatch.setattr(supervisor.sys, "platform", "linux")
    monkeypatch.setattr(supervisor, "read_journal_config", lambda: {})
    monkeypatch.setattr(
        supervisor, "is_local_provider_needed", lambda _config=None: True
    )
    monkeypatch.setattr(
        "solstone.think.providers.local_endpoint.resolve_local_endpoint",
        lambda: type("Endpoint", (), {"is_bundled": True})(),
    )
    monkeypatch.setattr(
        local_install,
        "target_fingerprint",
        lambda _model_id: {"provider": "local", "target": "one"},
    )
    monkeypatch.setattr(
        local_install,
        "inspect_readiness",
        lambda _model_id: _local_readiness(),
    )
    monkeypatch.setattr(local_install, "gpu_device_override", lambda: None)
    monkeypatch.setattr(local_vulkan, "detect_gpus", lambda: devices)
    monkeypatch.setattr(
        local_vulkan, "select_device", lambda _devices, **_kw: next(selections)
    )
    monkeypatch.setattr(local_vulkan, "device_local_used_mib", lambda _index: 0)
    monkeypatch.setattr(local_server, "select_server_tier", lambda _vram: _FakeTier())

    observation = supervisor._observe_local_provider_truth()

    assert observation.phase == "observing"
    assert observation.reason_code == "observation-raced"


def test_discarded_truth_result_reobserves_immediately_after_retry_fence_change(
    monkeypatch,
) -> None:
    plan = _local_plan()
    now = 100.0
    truth_submits = 0
    first_truth: concurrent.futures.Future = concurrent.futures.Future()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "ready"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.desired_fingerprint = plan.desired_fingerprint_sha256
    state.next_truth_at = 0.0

    class _RaceExecutor:
        def submit(self, fn, *args, **kwargs):
            nonlocal truth_submits
            if fn is supervisor._observe_provider_truth:
                truth_submits += 1
                if truth_submits == 1:
                    return first_truth
                return _future_with(
                    supervisor.ProviderTruthObservation(
                        provider="local",
                        phase="not-desired",
                        reason_code="provider-not-needed",
                        detail={},
                    )
                )
            assert fn is supervisor._provider_start_worker
            return _future_with(
                supervisor.ProviderLaunchOutcome(
                    status="launch-failed",
                    reason_code="launch-failed",
                    detail={},
                )
            )

    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _RaceExecutor())
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: now)

    asyncio.run(supervisor._reconcile_local_provider_runtime([]))

    assert truth_submits == 1
    assert state.truth_future is first_truth
    assert state.next_truth_at == (
        now + supervisor.PROVIDER_TRUTH_OBSERVATION_INTERVAL_SECONDS
    )

    state.latest_phase = "backoff"
    state.retry.next_at = now
    supervisor._submit_provider_start_if_needed(state, [])
    assert state.retry.attempt_count == 1
    first_truth.set_result(
        supervisor.ProviderTruthObservation(
            provider="local",
            phase="not-desired",
            reason_code="provider-not-needed",
            detail={"active_provider": "cloud"},
        )
    )

    asyncio.run(supervisor._reconcile_local_provider_runtime([]))

    assert truth_submits == 2
    assert state.truth_future is not first_truth
    assert state.next_truth_at == (
        now + supervisor.PROVIDER_TRUTH_OBSERVATION_INTERVAL_SECONDS
    )


def _set_provider_ready(
    provider: str,
    state: supervisor.ProviderRuntimeState,
    plan: supervisor.LocalServerLaunchPlan | supervisor.ParakeetServerLaunchPlan,
) -> None:
    state.latest_phase = "ready"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.desired_fingerprint = plan.desired_fingerprint_sha256
    state.generation = 1
    state.retry.attempt_count = 1
    del provider


def test_admission_exclusive_stop_defers_then_stops_when_slot_frees(
    monkeypatch,
) -> None:
    from solstone.think.providers import local_admission

    plan = _local_plan()
    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    _set_provider_ready("local", state, plan)
    request = supervisor._make_stop_request(
        state,
        managed,
        reason_code="admission-exclusive-stop",
        detail={},
        target_phase="not-desired",
        target_reason_code="provider-not-needed",
        admission_exclusive=True,
    )
    cancel_event = threading.Event()
    holder = _hold_local_slot_in_child(local_admission._admission_dir())

    assert supervisor.PROVIDER_ADMISSION_STOP_TIMEOUT_S == 5.0
    monkeypatch.setattr(supervisor, "PROVIDER_ADMISSION_STOP_TIMEOUT_S", 0.0)

    try:
        outcome = supervisor._provider_stop_cleanup_worker(
            "local",
            request,
            supervisor._provider_fence(state, 1),
            cancel_event,
        )

        assert outcome.status == "stop-deferred"
        assert managed.terminate.call_count == 0
        assert list(local_admission._admission_dir().glob("wait-*.ticket")) == []
    finally:
        holder.terminate()
        holder.wait(timeout=2)

    outcome = supervisor._provider_stop_cleanup_worker(
        "local",
        request,
        supervisor._provider_fence(state, 1),
        cancel_event,
    )

    assert outcome.status == "stopped"
    managed.terminate.assert_called_once()
    managed.cleanup.assert_called_once_with()


def test_admission_exclusive_stop_uses_launch_captured_capacity(monkeypatch) -> None:
    from solstone.think.providers import local_admission

    capacities: list[int] = []
    original_acquire = local_admission.acquire_local_slot
    plan = replace(_local_plan(), parallel_slots=3)
    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    _set_provider_ready("local", state, plan)
    state.latest_phase = "stop-deferred"
    state.pending_stop_target_phase = "not-desired"
    state.pending_stop_target_reason_code = "provider-not-needed"
    state.pending_stop_admission_exclusive = True
    state.next_truth_at = 10**12
    procs = [managed]

    def acquire(capacity, timeout_s, **kwargs):
        capacities.append(capacity)
        return original_acquire(capacity, timeout_s, **kwargs)

    monkeypatch.setattr(local_admission, "acquire_local_slot", acquire)
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())

    asyncio.run(supervisor._reconcile_local_provider_runtime(procs))
    asyncio.run(supervisor._reconcile_local_provider_runtime(procs))

    assert capacities == [3]
    assert state.latest_phase == "not-desired"


def test_admission_exclusive_stop_rechecks_reactivation_after_acquisition(
    monkeypatch,
) -> None:
    from solstone.think.providers import local_admission

    original_acquire = local_admission.acquire_local_slot
    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    plan = _local_plan()
    _set_provider_ready("local", state, plan)
    request = supervisor._make_stop_request(
        state,
        managed,
        reason_code="admission-exclusive-stop",
        detail={},
        target_phase="not-desired",
        target_reason_code="provider-not-needed",
        admission_exclusive=True,
    )
    cancel_event = threading.Event()

    def acquire(*args, **kwargs):
        permit = original_acquire(*args, **kwargs)
        cancel_event.set()
        return permit

    monkeypatch.setattr(local_admission, "acquire_local_slot", acquire)

    outcome = supervisor._provider_stop_cleanup_worker(
        "local",
        request,
        supervisor._provider_fence(state, 1),
        cancel_event,
    )

    assert outcome.status == "cancelled"
    assert outcome.managed is managed
    managed.terminate.assert_not_called()
    managed.cleanup.assert_not_called()


@pytest.mark.parametrize(
    ("provider", "old_managed", "new_observation"),
    [
        (
            "local",
            _FakeManaged(supervisor.MLX_SERVER_PROCESS_NAME),
            supervisor.ProviderTruthObservation(
                provider="local",
                phase="starting",
                reason_code="launch-requested",
                detail={},
                desired_fingerprint_json='{"provider":"local","backend":"cuda"}',
                desired_fingerprint_sha256="fp-local-cuda",
                plan=_cuda_plan(),
                boot_required=True,
            ),
        ),
        (
            "parakeet",
            _FakeManaged(supervisor.PARAKEET_SERVER_PROCESS_NAME),
            supervisor.ProviderTruthObservation(
                provider="parakeet",
                phase="starting",
                reason_code="launch-requested",
                detail={},
                desired_fingerprint_json='{"provider":"parakeet","target":"new"}',
                desired_fingerprint_sha256="fp-parakeet-new",
                plan=replace(
                    _parakeet_plan("cpu"),
                    desired_fingerprint_sha256="fp-parakeet-new",
                ),
                boot_required=True,
            ),
        ),
    ],
)
def test_stop_before_replace_runs_before_replacement_start(
    monkeypatch,
    provider: str,
    old_managed: _FakeManaged,
    new_observation: supervisor.ProviderTruthObservation,
) -> None:
    state = supervisor._provider_runtime_states[provider]
    old_plan = _local_plan() if provider == "local" else _parakeet_plan("cpu")
    _set_provider_ready(provider, state, old_plan)
    state.next_truth_at = 10**12
    state.truth_fence = supervisor._provider_fence(state, 1)
    state.truth_future = _future_with(new_observation)
    order: list[str] = []
    procs = [old_managed]

    def cleanup(managed, *, reason, state_name=None):
        order.append(f"cleanup:{managed.name}:{reason}")
        managed.terminate()
        managed.cleanup()
        managed.is_running = lambda: False

    def start_worker(provider_arg, _plan_arg, _fence, _cancel_event):
        order.append(f"start:{provider_arg}")
        return supervisor.ProviderLaunchOutcome(
            status="launch-failed",
            reason_code="launch-failed",
            detail={},
        )

    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(supervisor, "_terminate_cleanup_handle", cleanup)
    monkeypatch.setattr(supervisor, "_provider_start_worker", start_worker)

    asyncio.run(supervisor._reconcile_provider_runtime(provider, procs))
    assert order == ["cleanup:" + old_managed.name + ":target-changed"]

    for _ in range(3):
        if order[-1] == f"start:{provider}":
            break
        asyncio.run(supervisor._reconcile_provider_runtime(provider, procs))
    assert order[-1] == f"start:{provider}"
    assert old_managed.terminate.call_count == 1


def test_local_artifact_failure_before_replacement_keeps_old_child_and_retries(
    monkeypatch,
    tmp_path: Path,
) -> None:
    from solstone.think.providers import local_server
    from solstone.think.providers.install_state import (
        begin_or_replace_install_attempt,
        read_install_status,
        transition_state,
        write_install_status,
    )

    state = supervisor._provider_runtime_states["local"]
    old_managed = _FakeManaged(supervisor.LOCAL_SERVER_PROCESS_NAME)
    prior_tree = tmp_path / "prior-runtime"
    prior_tree.mkdir()
    prior_binary = prior_tree / "llama-server"
    prior_binary.write_text("old runtime", encoding="utf-8")
    old_plan = replace(
        _local_plan(),
        binary_path=prior_binary,
        desired_fingerprint_json='{"provider":"local","backend":"vulkan"}',
        desired_fingerprint_sha256="fp-local-vulkan-old",
    )
    _set_provider_ready("local", state, old_plan)
    write_runtime_health(
        _runtime_record(
            "local",
            phase="ready",
            fingerprint=old_plan.desired_fingerprint_sha256,
            generation=1,
            attempt=1,
            process={
                "name": old_managed.name,
                "pid": old_managed.process.pid,
                "ref": old_managed.ref,
                "port": 45678,
            },
        )
    )
    failed_target = {"provider": "local", "backend": "cuda"}
    attempt = begin_or_replace_install_attempt(
        "local",
        failed_target,
        initial_state="downloading",
    )
    write_install_status(
        transition_state(
            attempt,
            new_state="failed",
            error="cuda_runtime_incomplete",
            error_code="cuda_runtime_incomplete",
        )
    )
    failed_observation = supervisor.ProviderTruthObservation(
        provider="local",
        phase="artifact-not-ready",
        reason_code="artifact-missing",
        detail={
            "readiness_status": "missing-or-mismatched",
            "readiness_reason_code": "cuda_runtime_incomplete",
            "install_state": "failed",
            "install_acquisition_allowed": False,
        },
        desired_fingerprint_json='{"provider":"local","backend":"cuda"}',
        desired_fingerprint_sha256="fp-local-cuda",
        plan=None,
        boot_required=True,
    )
    state.next_truth_at = 10**12
    state.truth_fence = supervisor._provider_fence(state, 1)
    state.truth_future = _future_with(failed_observation)
    state.probe_fence = supervisor._provider_fence(state, 1)
    state.probe_future = _future_with(
        supervisor._probe_outcome("ready", "probe-ready", {"port": 45678})
    )
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())

    asyncio.run(supervisor._reconcile_provider_runtime("local", [old_managed]))

    old_managed.terminate.assert_not_called()
    old_managed.cleanup.assert_not_called()
    assert prior_binary.read_text(encoding="utf-8") == "old runtime"
    status = read_install_status(name="local")
    assert status["install_state"] == "failed"
    assert status["error_code"] == "cuda_runtime_incomplete"
    assert state.latest_phase in {"ready", "ready-proof-unavailable"}
    assert state.desired_fingerprint == old_plan.desired_fingerprint_sha256
    record = read_runtime_health("local")
    assert record["phase"] == "artifact-not-ready"
    assert record["reason_code"] == "artifact-missing"
    assert record["process"]["ref"] == old_managed.ref
    assert state.probe_future is None

    probe_calls: list[int] = []

    def failed_probe(port: int) -> tuple[str, str]:
        probe_calls.append(port)
        return local_server.STATE_FAILED, "timed out"

    monkeypatch.setattr(local_server, "_probe_health", failed_probe)
    state.next_probe_at = 0.0
    supervisor.write_service_port("local", 45678)

    asyncio.run(supervisor._reconcile_provider_runtime("local", [old_managed]))
    assert state.probe_future is not None
    asyncio.run(supervisor._reconcile_provider_runtime("local", [old_managed]))

    assert probe_calls == [45678]
    record = read_runtime_health("local")
    assert record["phase"] == "ready-proof-unavailable"
    assert record["reason_code"] == "proof-observation-unavailable"
    assert record["detail"]["error"] == "timed out"
    assert record["process"]["ref"] == old_managed.ref

    retry_observation = supervisor.ProviderTruthObservation(
        provider="local",
        phase="starting",
        reason_code="launch-requested",
        detail={},
        desired_fingerprint_json='{"provider":"local","backend":"cuda"}',
        desired_fingerprint_sha256="fp-local-cuda",
        plan=_cuda_plan(),
        boot_required=True,
    )
    state.truth_fence = supervisor._provider_fence(state, 1)
    state.truth_future = _future_with(retry_observation)
    order: list[str] = []

    def cleanup(managed, *, reason, state_name=None):
        del state_name
        order.append(f"cleanup:{managed.name}:{reason}")
        managed.terminate()
        managed.cleanup()
        managed.is_running = lambda: False

    def start_worker(provider_arg, _plan_arg, _fence, _cancel_event):
        order.append(f"start:{provider_arg}")
        return supervisor.ProviderLaunchOutcome(
            status="launch-failed",
            reason_code="launch-failed",
            detail={},
        )

    monkeypatch.setattr(supervisor, "_terminate_cleanup_handle", cleanup)
    monkeypatch.setattr(supervisor, "_provider_start_worker", start_worker)

    asyncio.run(supervisor._reconcile_provider_runtime("local", [old_managed]))
    assert order == ["cleanup:" + old_managed.name + ":target-changed"]

    for _ in range(3):
        if order[-1] == "start:local":
            break
        asyncio.run(supervisor._reconcile_provider_runtime("local", [old_managed]))
    assert order[-1] == "start:local"


def test_matching_target_duplicate_convergence_keeps_owner_and_stops_stale(
    monkeypatch,
) -> None:
    keep = _FakeManaged(supervisor.LOCAL_SERVER_PROCESS_NAME)
    stale = _FakeManaged(supervisor.MLX_SERVER_PROCESS_NAME)
    state = supervisor._provider_runtime_states["local"]
    plan = _local_plan()
    _set_provider_ready("local", state, plan)
    state.next_truth_at = 10**12
    procs = [keep, stale]
    write_runtime_health(
        _runtime_record(
            "local",
            phase="ready",
            fingerprint=plan.desired_fingerprint_sha256,
            generation=1,
            attempt=1,
            process={
                "name": keep.name,
                "pid": keep.process.pid,
                "ref": keep.ref,
                "port": 45678,
            },
        )
    )
    starts: list[int] = []
    stopped: list[_FakeManaged] = []
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(
        supervisor,
        "_provider_start_worker",
        lambda *_args: starts.append(1),
    )
    monkeypatch.setattr(
        supervisor,
        "_terminate_cleanup_handle",
        lambda managed, *, reason, state_name=None: (
            stopped.append(managed),
            managed.terminate(),
            managed.cleanup(),
            setattr(managed, "is_running", lambda: False),
        ),
    )

    asyncio.run(supervisor._reconcile_local_provider_runtime(procs))
    asyncio.run(supervisor._reconcile_local_provider_runtime(procs))

    assert stopped == [stale]
    assert keep not in stopped
    assert starts == []


def test_late_cleanup_cannot_clear_newer_generation_port_file() -> None:
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "stopping"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.generation = 2
    state.retry.attempt_count = 2
    newer = _FakeManaged()
    supervisor.write_service_port("local", 22222)
    write_runtime_health(
        _runtime_record(
            "local",
            phase="ready",
            fingerprint=plan.desired_fingerprint_sha256,
            generation=2,
            attempt=2,
            process={
                "name": newer.name,
                "pid": newer.process.pid,
                "ref": newer.ref,
                "port": 22222,
            },
        )
    )
    old = _FakeManaged()
    old_fence = supervisor.ProviderFence(
        incarnation=supervisor._PROVIDER_INCARNATION,
        generation=1,
        fingerprint=plan.desired_fingerprint_sha256,
        attempt=1,
    )
    state.pending_stop_request = supervisor._make_stop_request(
        state,
        old,
        reason_code="target-changed",
        detail={},
    )
    state.stop_cleanup_fence = old_fence
    state.stop_cleanup_future = _future_with(
        supervisor.ProviderStopCleanupOutcome(
            status="stopped",
            reason_code="cleanup-succeeded",
            detail={"port": 11111},
        )
    )

    assert supervisor._handle_provider_stop_cleanup_result(state, [newer]) is True

    assert supervisor.read_service_port("local") == 22222


def test_fenced_out_stop_cleanup_failure_preserves_handle() -> None:
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "ready"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.generation = 2
    state.retry.attempt_count = 2
    stale_managed = _FakeManaged()
    old_fence = supervisor.ProviderFence(
        incarnation=supervisor._PROVIDER_INCARNATION,
        generation=1,
        fingerprint=plan.desired_fingerprint_sha256,
        attempt=1,
    )
    state.pending_stop_request = supervisor._make_stop_request(
        state,
        stale_managed,
        reason_code="target-changed",
        detail={},
    )
    state.stop_cleanup_fence = old_fence
    state.stop_cleanup_future = _future_with(
        supervisor.ProviderStopCleanupOutcome(
            status="cleanup-failed",
            reason_code="cleanup-attempt-failed",
            detail={"error": "terminate failed"},
            managed=stale_managed,
        )
    )

    assert supervisor._handle_provider_stop_cleanup_result(state, []) is True

    assert state.latest_phase == "cleanup-failed"
    assert state.pending_stop_request is not None
    assert state.pending_stop_request.managed is stale_managed


def test_cleanup_failed_cadence_consumes_no_launch_budget(monkeypatch) -> None:
    now = 100.0
    plan = _local_plan()
    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    _set_provider_ready("local", state, plan)
    state.retry.attempt_count = 4
    state.pending_stop_request = supervisor._make_stop_request(
        state,
        managed,
        reason_code="target-changed",
        detail={},
    )
    delays: list[float] = []

    def monotonic() -> float:
        return now

    monkeypatch.setattr(supervisor.time, "monotonic", monotonic)

    for _ in range(5):
        request = state.pending_stop_request
        assert request is not None
        supervisor._schedule_cleanup_failed_retry(
            state,
            request,
            supervisor.ProviderStopCleanupOutcome(
                status="cleanup-failed",
                reason_code="cleanup-attempt-failed",
                detail={},
                managed=managed,
            ),
        )
        delays.append(state.cleanup_next_at - now)

    assert delays == [2.0, 4.0, 8.0, 16.0, 30.0]
    assert state.retry.attempt_count == 4


def test_cleanup_failed_rechecks_dead_child_and_recovers(monkeypatch) -> None:
    plan = _local_plan()
    dead = _DeadManaged()
    state = supervisor._provider_runtime_states["local"]
    _set_provider_ready("local", state, plan)
    state.latest_phase = "cleanup-failed"
    state.latest_plan = None
    state.next_truth_at = 10**12
    state.pending_stop_request = supervisor._make_stop_request(
        state,
        dead,
        reason_code="target-changed",
        detail={},
    )
    state.cleanup_next_at = 0.0
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 10.0)

    procs = [dead]
    asyncio.run(supervisor._reconcile_local_provider_runtime(procs))
    asyncio.run(supervisor._reconcile_local_provider_runtime(procs))

    assert state.latest_phase == "stopped"
    dead.cleanup.assert_called_once_with()


def test_pending_cleanup_dead_child_converges_after_truth_changed_phase(
    monkeypatch,
) -> None:
    plan = _local_plan()
    dead = _DeadManaged()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "starting"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    state.pending_stop_request = supervisor._make_stop_request(
        state,
        dead,
        reason_code="cleanup-attempt-failed",
        detail={"source": "preserved-handle"},
        target_phase="stopped",
        target_reason_code="cleanup-succeeded",
    )
    state.cleanup_attempt_count = 1
    state.cleanup_next_at = 0.0
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 10.0)

    assert supervisor._submit_provider_stop_cleanup_if_needed(state, []) is True
    assert supervisor._handle_provider_stop_cleanup_result(state, [dead]) is True

    assert state.pending_stop_request is None
    assert state.latest_phase == "stopped"
    dead.cleanup.assert_called_once_with()


def test_preserved_cancel_cleanup_handle_is_adopted_not_orphaned() -> None:
    plan = _local_plan()
    managed = _FakeManaged()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "starting"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.generation = 1
    state.retry.attempt_count = 1
    state.start_fence = supervisor._provider_fence(state, 1)
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="launch-failed",
            reason_code="launch-failed",
            detail={
                "cleanup_failed": True,
                "cleanup_deferred_to": "cleanup-failed-reconciler",
            },
            managed=managed,
        )
    )

    assert supervisor._handle_provider_start_result(state, []) is True

    assert state.latest_phase == "cleanup-failed"
    assert state.pending_stop_request is not None
    assert state.pending_stop_request.managed is managed
    managed.terminate.assert_not_called()
    managed.cleanup.assert_not_called()


def test_shutdown_stop_cleanup_signal_does_not_run_cleanup(monkeypatch) -> None:
    state = supervisor._provider_runtime_states["local"]
    event = threading.Event()
    state.stop_cleanup_cancel_event = event
    state.stop_cleanup_fence = supervisor.ProviderFence(
        incarnation=supervisor._PROVIDER_INCARNATION,
        generation=1,
        fingerprint="fp-local",
        attempt=1,
    )
    monkeypatch.setattr(
        supervisor,
        "_terminate_cleanup_handle",
        lambda *_args, **_kwargs: pytest.fail(
            "shutdown signal must not cleanup inline"
        ),
    )

    supervisor._cancel_all_provider_stops("test shutdown")

    assert event.is_set()


@pytest.mark.parametrize(
    ("initial_phase", "observation", "expected_phase", "expected_fingerprint"),
    [
        (
            "ready",
            supervisor.ProviderTruthObservation(
                provider="local",
                phase="not-desired",
                reason_code="provider-not-needed",
                detail={"active_provider": "cloud"},
            ),
            "not-desired",
            None,
        ),
        (
            "backoff",
            supervisor.ProviderTruthObservation(
                provider="local",
                phase="not-desired",
                reason_code="provider-not-needed",
                detail={"active_provider": "cloud"},
            ),
            "not-desired",
            None,
        ),
        (
            "ready",
            supervisor.ProviderTruthObservation(
                provider="local",
                phase="starting",
                reason_code="launch-requested",
                detail={"target": "new"},
                desired_fingerprint_json='{"provider":"local","target":"new"}',
                desired_fingerprint_sha256="fp-new",
                plan=_local_plan(),
                boot_required=True,
            ),
            "starting",
            "fp-new",
        ),
        (
            "host-blocked",
            supervisor.ProviderTruthObservation(
                provider="local",
                phase="starting",
                reason_code="launch-requested",
                detail={"target": "new"},
                desired_fingerprint_json='{"provider":"local","target":"new"}',
                desired_fingerprint_sha256="fp-new",
                plan=_local_plan(),
                boot_required=True,
            ),
            "starting",
            "fp-new",
        ),
        (
            "failed",
            supervisor.ProviderTruthObservation(
                provider="local",
                phase="starting",
                reason_code="launch-requested",
                detail={"target": "new"},
                desired_fingerprint_json='{"provider":"local","target":"new"}',
                desired_fingerprint_sha256="fp-new",
                plan=_local_plan(),
                boot_required=True,
            ),
            "starting",
            "fp-new",
        ),
        (
            "artifact-not-ready",
            supervisor.ProviderTruthObservation(
                provider="local",
                phase="starting",
                reason_code="launch-requested",
                detail={"target": "new"},
                desired_fingerprint_json='{"provider":"local","target":"new"}',
                desired_fingerprint_sha256="fp-new",
                plan=_local_plan(),
                boot_required=True,
            ),
            "starting",
            "fp-new",
        ),
    ],
)
def test_continuous_reobservation_converges_from_steady_phases(
    monkeypatch,
    initial_phase: RuntimePhase,
    observation: supervisor.ProviderTruthObservation,
    expected_phase: RuntimePhase,
    expected_fingerprint: str | None,
) -> None:
    state = supervisor._provider_runtime_states["local"]
    old_plan = _local_plan()
    state.latest_phase = initial_phase
    state.latest_plan = old_plan
    state.desired_fingerprint = old_plan.desired_fingerprint_sha256
    state.retry = supervisor.ProviderRetryState(
        attempt_count=1,
        next_at=9999.0,
        desired_fingerprint=old_plan.desired_fingerprint_sha256,
    )
    state.next_truth_at = 0.0
    starts: list[int] = []

    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 100.0)
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(
        supervisor,
        "_observe_local_provider_truth",
        lambda: observation,
    )
    monkeypatch.setattr(
        supervisor,
        "_provider_start_worker",
        lambda provider, plan_arg, fence, _cancel_event: (
            starts.append(fence.attempt)
            or supervisor.ProviderLaunchOutcome(
                status="launch-failed",
                reason_code="launch-failed",
                detail={},
            )
        ),
    )

    asyncio.run(supervisor._reconcile_local_provider_runtime([]))
    asyncio.run(supervisor._reconcile_local_provider_runtime([]))

    assert state.latest_phase == expected_phase
    assert state.desired_fingerprint == expected_fingerprint
    assert state.truth_future is None


def test_retry_cadence_exhausts_after_six_attempts(monkeypatch):
    now = 1000.0
    launches: list[int] = []
    plan = _local_plan()

    def monotonic() -> float:
        return now

    def observe():
        return supervisor.ProviderTruthObservation(
            provider="local",
            phase="starting",
            reason_code="launch-requested",
            detail={},
            desired_fingerprint_json=plan.desired_fingerprint_json,
            desired_fingerprint_sha256=plan.desired_fingerprint_sha256,
            plan=plan,
            boot_required=True,
        )

    def start_worker(provider, plan_arg, fence, _cancel_event):
        assert provider == "local"
        assert plan_arg is plan
        launches.append(fence.attempt)
        return supervisor.ProviderLaunchOutcome(
            status="launch-failed",
            reason_code="launch-failed",
            detail={"attempt": fence.attempt},
        )

    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(supervisor.time, "monotonic", monotonic)
    monkeypatch.setattr(supervisor, "_observe_local_provider_truth", observe)
    monkeypatch.setattr(supervisor, "_provider_start_worker", start_worker)

    state = supervisor._provider_runtime_states["local"]
    delays: list[float] = []

    for _ in range(40):
        asyncio.run(supervisor._reconcile_local_provider_runtime([]))
        if state.latest_phase == "backoff":
            delays.append(state.retry.next_at - now)
            now = state.retry.next_at
        if state.latest_phase == "failed":
            break

    assert launches == [1, 2, 3, 4, 5, 6]
    assert delays == [2.0, 4.0, 8.0, 16.0, 30.0]
    assert state.latest_phase == "failed"


@pytest.mark.parametrize(
    "status",
    [
        "ready",
        "not-ready",
        "host-blocked",
        "exited",
        "warmup-timeout",
        "launch-failed",
    ],
)
def test_start_result_marks_required_attempt_for_every_outcome_status(
    monkeypatch, status: supervisor.LaunchOutcomeStatus
) -> None:
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "starting"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    state.start_fence = supervisor._provider_fence(state, 1)
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status=status,
            reason_code="launch-failed",
            detail={"status": status},
        )
    )
    monkeypatch.setattr(supervisor, "_task_queue", _FakeTaskQueue())
    monkeypatch.setattr(
        supervisor,
        "_provider_startup_gate",
        supervisor.ProviderStartupGate(
            started_at=0.0,
            required={"local"},
            terminal=set(),
            attempted={},
            first_start_at=None,
            released=False,
        ),
    )

    assert supervisor._handle_provider_start_result(state, []) is True

    assert supervisor._provider_startup_gate.attempted == {"local": status}


def test_superseded_start_result_does_not_mark_attempted_or_release_gate(
    monkeypatch,
) -> None:
    queue = _FakeTaskQueue()
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "starting"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    state.start_fence = supervisor._provider_fence(state, 1)
    state.generation = 2
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="warmup-timeout",
            reason_code="warmup-timeout",
            detail={"backend": plan.backend},
        )
    )
    monkeypatch.setattr(supervisor, "_task_queue", queue)
    monkeypatch.setattr(
        supervisor,
        "_provider_startup_gate",
        supervisor.ProviderStartupGate(
            started_at=0.0,
            required={"local"},
            terminal=set(),
            attempted={},
            first_start_at=None,
            released=False,
        ),
    )

    assert supervisor._handle_provider_start_result(state, []) is True

    assert read_runtime_health("local")["reason_code"] == "stale-result-ignored"
    assert "local" not in supervisor._provider_startup_gate.attempted
    assert queue.ready_calls == 0
    assert supervisor._provider_startup_gate.released is False


def test_startup_gate_releases_on_first_attempt_concluded_before_window(
    caplog, monkeypatch
):
    now = 10.0
    queue = _FakeTaskQueue()
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "starting"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    state.start_fence = supervisor._provider_fence(state, 1)
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="warmup-timeout",
            reason_code="warmup-timeout",
            detail={"backend": plan.backend},
        )
    )
    monkeypatch.setattr(supervisor, "_task_queue", queue)
    monkeypatch.setattr(
        supervisor,
        "_provider_startup_gate",
        supervisor.ProviderStartupGate(
            started_at=0.0,
            required={"local"},
            terminal=set(),
            attempted={},
            first_start_at=None,
            released=False,
        ),
    )
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: now)

    with caplog.at_level(logging.INFO, logger=supervisor.logger.name):
        assert supervisor._handle_provider_start_result(state, []) is True
        supervisor._release_provider_startup_gate_if_ready()

    assert queue.ready_calls == 1
    assert any(
        "provider startup gate released after first launch attempts concluded"
        in record.message
        and "local=warmup-timeout" in record.message
        for record in caplog.records
    )


def test_startup_gate_releases_for_mixed_terminal_and_attempted_required_providers(
    monkeypatch,
) -> None:
    queue = _FakeTaskQueue()
    local = supervisor._provider_runtime_states["local"]
    monkeypatch.setattr(supervisor, "_task_queue", queue)
    monkeypatch.setattr(
        supervisor,
        "_provider_startup_gate",
        supervisor.ProviderStartupGate(
            started_at=0.0,
            required={"local", "parakeet"},
            terminal=set(),
            attempted={"parakeet": "warmup-timeout"},
            first_start_at=None,
            released=False,
        ),
    )

    supervisor._finish_provider_startup_condition(local, "ready")
    supervisor._release_provider_startup_gate_if_ready()
    supervisor._release_provider_startup_gate_if_ready()

    assert queue.ready_calls == 1
    assert supervisor._provider_startup_gate.released is True


def test_startup_gate_window_waits_for_sibling_launch_in_flight(monkeypatch):
    now = supervisor.PROVIDER_STARTUP_GATE_WINDOW_SECONDS + 1.0
    queue = _FakeTaskQueue()
    pending: concurrent.futures.Future = concurrent.futures.Future()
    supervisor._provider_runtime_states["parakeet"].start_future = pending
    monkeypatch.setattr(supervisor, "_task_queue", queue)
    monkeypatch.setattr(
        supervisor,
        "_provider_startup_gate",
        supervisor.ProviderStartupGate(
            started_at=0.0,
            required={"local", "parakeet"},
            terminal=set(),
            attempted={"local": "warmup-timeout"},
            first_start_at=None,
            released=False,
        ),
    )
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: now)

    supervisor._release_provider_startup_gate_if_ready()

    assert queue.ready_calls == 0
    assert supervisor._provider_startup_gate.released is False


def test_provider_startup_gate_ceiling_bounds_worker_warmup_timeouts() -> None:
    slack = 30.0

    assert (
        supervisor.PROVIDER_STARTUP_GATE_CEILING_SECONDS
        >= supervisor.LOCAL_SERVER_READY_TIMEOUT_S + slack
    )
    assert (
        supervisor.PROVIDER_STARTUP_GATE_CEILING_SECONDS
        >= supervisor.PARAKEET_SERVER_READY_TIMEOUT_S + slack
    )
    assert supervisor.PROVIDER_STARTUP_GATE_CEILING_SECONDS <= 420.0


def test_startup_gate_ceiling_uses_first_start_submission_time(monkeypatch):
    first_start_at = 120.0
    now = first_start_at + supervisor.PROVIDER_STARTUP_GATE_CEILING_SECONDS - 1.0
    queue = _FakeTaskQueue()
    pending: concurrent.futures.Future = concurrent.futures.Future()
    supervisor._provider_runtime_states["local"].start_future = pending
    monkeypatch.setattr(supervisor, "_task_queue", queue)
    monkeypatch.setattr(
        supervisor,
        "_provider_startup_gate",
        supervisor.ProviderStartupGate(
            started_at=0.0,
            required={"local"},
            terminal=set(),
            attempted={},
            first_start_at=first_start_at,
            released=False,
        ),
    )
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: now)

    supervisor._release_provider_startup_gate_if_ready()

    assert queue.ready_calls == 0
    assert supervisor._provider_startup_gate.released is False


def test_startup_gate_window_release_logs_unsatisfied_and_no_in_flight(
    caplog, monkeypatch
) -> None:
    now = supervisor.PROVIDER_STARTUP_GATE_WINDOW_SECONDS + 1.0
    queue = _FakeTaskQueue()
    monkeypatch.setattr(supervisor, "_task_queue", queue)
    monkeypatch.setattr(
        supervisor,
        "_provider_startup_gate",
        supervisor.ProviderStartupGate(
            started_at=0.0,
            required={"local", "parakeet"},
            terminal=set(),
            attempted={},
            first_start_at=None,
            released=False,
        ),
    )
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: now)

    with caplog.at_level(logging.WARNING, logger=supervisor.logger.name):
        supervisor._release_provider_startup_gate_if_ready()

    assert queue.ready_calls == 1
    assert any(
        "provider startup gate released after 60.0s window with pending providers: "
        "['local', 'parakeet']"
        in record.message
        and "launch_in_flight=false" in record.message
        for record in caplog.records
    )


@pytest.mark.parametrize("release_mode", ["window", "ceiling"])
def test_supervise_releases_startup_gate_when_provider_reconcile_steps_raise(
    monkeypatch, release_mode: str
) -> None:
    queue = _FakeTaskQueue()
    procs = [_FakeManaged("supervise-test")]
    calls: list[str] = []
    if release_mode == "window":
        now = supervisor.PROVIDER_STARTUP_GATE_WINDOW_SECONDS + 1.0
        first_start_at = None
    else:
        first_start_at = 20.0
        now = first_start_at + supervisor.PROVIDER_STARTUP_GATE_CEILING_SECONDS + 1.0
        supervisor._provider_runtime_states[
            "local"
        ].start_future = concurrent.futures.Future()
        supervisor._provider_runtime_states[
            "parakeet"
        ].start_future = concurrent.futures.Future()
    monkeypatch.setattr(supervisor, "shutdown_requested", False)
    monkeypatch.setattr(supervisor, "_task_queue", queue)
    monkeypatch.setattr(supervisor, "_supervisor_callosum", None)
    monkeypatch.setattr(supervisor, "_last_tick_step_failure", None)
    monkeypatch.setattr(supervisor, "reset_display_powersave_monitor", lambda: None)
    monkeypatch.setattr(supervisor, "_check_segment_flush", lambda: None)
    monkeypatch.setattr(supervisor, "_run_sync_tick", lambda _now: True)

    async def handle_runner_exits_noop(_procs):
        return None

    monkeypatch.setattr(supervisor, "handle_runner_exits", handle_runner_exits_noop)
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: now)
    monkeypatch.setattr(
        supervisor,
        "_provider_startup_gate",
        supervisor.ProviderStartupGate(
            started_at=0.0,
            required={"local", "parakeet"},
            terminal=set(),
            attempted={},
            first_start_at=first_start_at,
            released=False,
        ),
    )

    def fail_retry_token(state: supervisor.ProviderRuntimeState) -> None:
        calls.append(state.provider)
        raise RuntimeError(f"{state.provider} reconcile boom")

    async def stop_after_tick(_seconds: float) -> None:
        supervisor.shutdown_requested = True

    monkeypatch.setattr(supervisor, "_handle_provider_retry_token", fail_retry_token)
    monkeypatch.setattr(supervisor.asyncio, "sleep", stop_after_tick)

    asyncio.run(supervisor.supervise(daily=False, schedule=False, procs=procs))

    assert calls == ["local", "parakeet"]
    assert queue.ready_calls == 1
    assert supervisor._provider_startup_gate.released is True


def test_startup_gate_attempted_survives_retry_backoff(monkeypatch) -> None:
    now = 100.0
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "starting"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    state.start_fence = supervisor._provider_fence(state, 1)
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="launch-failed",
            reason_code="launch-failed",
            detail={"attempt": 1},
        )
    )
    monkeypatch.setattr(supervisor, "_task_queue", _FakeTaskQueue())
    monkeypatch.setattr(
        supervisor,
        "_provider_startup_gate",
        supervisor.ProviderStartupGate(
            started_at=0.0,
            required={"local"},
            terminal=set(),
            attempted={},
            first_start_at=None,
            released=False,
        ),
    )
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: now)

    assert supervisor._handle_provider_start_result(state, []) is True
    assert state.latest_phase == "backoff"
    assert supervisor._provider_startup_gate.attempted == {"local": "launch-failed"}

    now = state.retry.next_at
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(
        supervisor,
        "_provider_start_worker",
        lambda *_args: supervisor.ProviderLaunchOutcome(
            status="warmup-timeout",
            reason_code="warmup-timeout",
            detail={"attempt": 2},
        ),
    )

    supervisor._submit_provider_start_if_needed(state, [])
    assert supervisor._handle_provider_start_result(state, []) is True

    assert state.latest_phase == "backoff"
    assert supervisor._provider_startup_gate.attempted == {"local": "launch-failed"}


def test_startup_gate_releases_on_window_and_reconciler_keeps_retrying(monkeypatch):
    now = supervisor.PROVIDER_STARTUP_GATE_WINDOW_SECONDS + 1.0
    queue = _FakeTaskQueue()
    plan = _local_plan()
    launches: list[int] = []

    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "backoff"
    state.latest_plan = plan
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.retry.next_at = now
    monkeypatch.setattr(supervisor, "_task_queue", queue)
    monkeypatch.setattr(
        supervisor,
        "_provider_startup_gate",
        supervisor.ProviderStartupGate(
            started_at=0.0,
            required={"local"},
            terminal=set(),
            attempted={},
            first_start_at=None,
            released=False,
        ),
    )
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: now)
    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(
        supervisor,
        "_provider_start_worker",
        lambda provider, plan_arg, fence, _cancel_event: (
            launches.append(fence.attempt)
            or supervisor.ProviderLaunchOutcome(
                status="launch-failed",
                reason_code="launch-failed",
                detail={},
            )
        ),
    )

    supervisor._release_provider_startup_gate_if_ready()

    assert queue.ready_calls == 1
    assert supervisor._provider_startup_gate.released is True

    asyncio.run(supervisor._reconcile_local_provider_runtime([]))
    asyncio.run(supervisor._reconcile_local_provider_runtime([]))

    assert launches == [2]
    assert queue.ready_calls == 1


@pytest.mark.parametrize("terminal_phase", ["ready", "host-blocked"])
def test_startup_gate_releases_on_terminal_provider_state(
    monkeypatch,
    terminal_phase: RuntimePhase,
) -> None:
    queue = _FakeTaskQueue()
    state = supervisor._provider_runtime_states["local"]
    monkeypatch.setattr(supervisor, "_task_queue", queue)
    monkeypatch.setattr(
        supervisor,
        "_provider_startup_gate",
        supervisor.ProviderStartupGate(
            started_at=0.0,
            required={"local"},
            terminal=set(),
            attempted={},
            first_start_at=None,
            released=False,
        ),
    )
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 1.0)

    supervisor._finish_provider_startup_condition(state, terminal_phase)
    supervisor._release_provider_startup_gate_if_ready()
    supervisor._release_provider_startup_gate_if_ready()

    assert queue.ready_calls == 1
    assert supervisor._provider_startup_gate.released is True


def test_fenced_ready_result_publishes_port(monkeypatch):
    from solstone.think.providers import local_server

    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_plan = plan
    state.latest_phase = "starting"
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 3
    fence = supervisor._provider_fence(state, 1)
    managed = _FakeManaged()
    ports: list[tuple[str, int]] = []
    order: list[str] = []
    state.start_fence = fence
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={"port": 45678},
            managed=managed,
        )
    )
    monkeypatch.setattr(
        supervisor,
        "write_service_port",
        lambda service, port: order.append("port") or ports.append((service, port)),
    )
    monkeypatch.setattr(
        local_server,
        "write_local_context_window",
        lambda _tokens: order.append("context"),
    )
    original_write = supervisor._write_provider_runtime

    def write_with_order(*args, **kwargs):
        order.append(f"runtime:{kwargs['phase']}")
        return original_write(*args, **kwargs)

    monkeypatch.setattr(supervisor, "_write_provider_runtime", write_with_order)

    assert supervisor._handle_provider_start_result(state, []) is True

    assert ports == [("local", 45678)]
    assert order[:3] == ["runtime:ready", "context", "port"]
    record = read_runtime_health("local")
    assert record["phase"] == "ready"
    assert record["generation"] == 3
    assert record["attempt"] == 1
    assert record["process"]["port"] == 45678


def test_ready_ownership_write_failure_does_not_publish_port_or_ready(
    monkeypatch,
) -> None:
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_plan = plan
    state.latest_phase = "starting"
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    managed = _FakeManaged()
    state.start_fence = supervisor._provider_fence(state, 1)
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={"port": 45678},
            managed=managed,
        )
    )
    writes: list[RuntimePhase] = []
    monkeypatch.setattr(
        supervisor,
        "write_service_port",
        lambda *_args, **_kwargs: pytest.fail("port must not be published"),
    )

    def failed_ready_write(*args, **kwargs):
        phase = kwargs["phase"]
        writes.append(phase)
        if phase == "ready":
            return None
        return {
            **read_runtime_health("local"),
            "phase": phase,
            "reason_code": kwargs["reason_code"],
            "detail": kwargs["detail"],
            "desired_fingerprint_sha256": state.desired_fingerprint,
            "incarnation": supervisor._PROVIDER_INCARNATION,
            "generation": state.generation,
            "attempt": state.retry.attempt_count,
            "process": kwargs.get("process"),
            "updated_at": "2026-07-19T00:00:00+00:00",
            "owner": {"test": "failed-ready-write"},
        }

    monkeypatch.setattr(supervisor, "_write_provider_runtime", failed_ready_write)

    assert supervisor._handle_provider_start_result(state, []) is True

    assert writes == ["ready", "state-unavailable"]
    assert state.latest_phase == "state-unavailable"
    assert supervisor.read_service_port("local") is None
    managed.terminate.assert_called_once()
    managed.cleanup.assert_called_once_with()


def test_mlx_ready_clears_stale_local_context_and_capacity_cache(
    monkeypatch,
) -> None:
    from solstone.think.providers import local_server

    journal = Path(supervisor.get_journal())
    health = journal / "health"
    health.mkdir(parents=True, exist_ok=True)
    (health / "local.ctx").write_text("32768", encoding="utf-8")
    supervisor.write_service_port("local", 11111)
    monkeypatch.setattr(
        local_server,
        "_core_connect_outcome",
        lambda: _core_ready_capacity_for_context(local_server),
    )
    local_server.reset_parallel_slots_cache()
    assert local_server.read_server_capacity().parallel_slots == 2

    plan = _mlx_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_plan = plan
    state.latest_phase = "starting"
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    managed = _FakeManaged(supervisor.MLX_SERVER_PROCESS_NAME)
    state.start_fence = supervisor._provider_fence(state, 1)
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={"port": 45678},
            managed=managed,
        )
    )

    assert supervisor._handle_provider_start_result(state, []) is True

    assert not (health / "local.ctx").exists()
    assert supervisor.read_service_port("local") == 45678
    assert local_server.read_server_capacity().parallel_slots == 1
    assert local_server.read_server_capacity().source == "default"


def test_local_capacity_observed_before_ready_is_reset_on_ready(
    monkeypatch,
) -> None:
    from solstone.think.providers import local_server

    monkeypatch.setattr(
        local_server,
        "_core_connect_outcome",
        lambda: _core_ready_capacity_for_context(local_server),
    )
    local_server.reset_parallel_slots_cache()
    assert local_server.read_server_capacity().parallel_slots == 1

    plan = _cuda_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_plan = plan
    state.latest_phase = "starting"
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    managed = _FakeManaged()
    state.start_fence = supervisor._provider_fence(state, 1)
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={"port": 45678},
            managed=managed,
        )
    )

    assert supervisor._handle_provider_start_result(state, []) is True

    assert supervisor.read_service_port("local") == 45678
    assert local_server.read_local_context_window() == plan.context_tokens
    assert local_server.read_server_capacity().parallel_slots == 2
    assert local_server.read_server_capacity().source == "local_ctx"


def test_fenced_parakeet_ready_result_writes_placement_after_ownership(
    monkeypatch,
) -> None:
    from solstone.think.providers import parakeet_server

    plan = _parakeet_plan("vulkan")
    state = supervisor._provider_runtime_states["parakeet"]
    state.latest_plan = plan
    state.latest_phase = "starting"
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    managed = _FakeManaged(supervisor.PARAKEET_SERVER_PROCESS_NAME)
    ports: list[tuple[str, int]] = []
    order: list[str] = []
    state.start_fence = supervisor._provider_fence(state, 1)
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={"port": 45678, "placement": "gpu"},
            managed=managed,
        )
    )
    monkeypatch.setattr(
        supervisor,
        "write_service_port",
        lambda service, port: order.append("port") or ports.append((service, port)),
    )
    monkeypatch.setattr(
        parakeet_server,
        "write_parakeet_placement",
        lambda placement: order.append(f"placement:{placement}"),
    )
    original_write = supervisor._write_provider_runtime

    def write_with_order(*args, **kwargs):
        order.append(f"runtime:{kwargs['phase']}")
        return original_write(*args, **kwargs)

    monkeypatch.setattr(supervisor, "_write_provider_runtime", write_with_order)

    assert supervisor._handle_provider_start_result(state, []) is True

    assert ports == [("parakeet-cpp", 45678)]
    assert order[:3] == ["runtime:ready", "placement:gpu", "port"]


def test_boot_incarnation_invalidates_late_start_result(monkeypatch):
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_plan = plan
    state.latest_phase = "starting"
    state.desired_fingerprint = plan.desired_fingerprint_sha256
    state.retry.attempt_count = 1
    state.generation = 1
    stale_fence = supervisor.ProviderFence(
        incarnation="old-boot",
        generation=1,
        fingerprint=plan.desired_fingerprint_sha256,
        attempt=1,
    )
    managed = _FakeManaged()
    state.start_fence = stale_fence
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={"port": 45678},
            managed=managed,
        )
    )
    published: list[tuple[str, int]] = []
    monkeypatch.setattr(
        supervisor,
        "write_service_port",
        lambda service, port: published.append((service, port)),
    )

    assert supervisor._handle_provider_start_result(state, []) is True

    assert published == []
    managed.terminate.assert_called_once()
    managed.cleanup.assert_called_once_with()
    assert read_runtime_health("local")["reason_code"] == "stale-result-ignored"


def test_superseded_attempt_cannot_publish_or_clear_newer_port(monkeypatch):
    plan = _local_plan()
    state = supervisor._provider_runtime_states["local"]
    state.latest_plan = plan
    state.latest_phase = "ready"
    state.desired_fingerprint = "fp-new"
    state.retry.attempt_count = 2
    state.generation = 2
    supervisor.write_service_port("local", 22222)
    write_runtime_health(
        _runtime_record(
            "local",
            phase="ready",
            fingerprint="fp-new",
            generation=2,
            attempt=2,
            process={
                "name": supervisor.LOCAL_SERVER_PROCESS_NAME,
                "pid": 12345,
                "ref": "ref-new",
                "port": 22222,
            },
        )
    )
    old_fence = supervisor.ProviderFence(
        incarnation=supervisor._PROVIDER_INCARNATION,
        generation=1,
        fingerprint="fp-old",
        attempt=1,
    )
    old_managed = _FakeManaged()
    state.start_fence = old_fence
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail={"port": 11111},
            managed=old_managed,
        )
    )
    published: list[tuple[str, int]] = []
    monkeypatch.setattr(
        supervisor,
        "write_service_port",
        lambda service, port: published.append((service, port)),
    )

    assert supervisor._handle_provider_start_result(state, []) is True

    assert published == []
    assert supervisor.read_service_port("local") == 22222
    old_managed.terminate.assert_called_once()
    old_managed.cleanup.assert_called_once_with()


@pytest.mark.parametrize(
    ("provider", "plan", "managed_name", "detail"),
    [
        (
            "local",
            _local_plan(),
            supervisor.LOCAL_SERVER_PROCESS_NAME,
            {"port": 11111},
        ),
        (
            "parakeet",
            _parakeet_plan("vulkan"),
            supervisor.PARAKEET_SERVER_PROCESS_NAME,
            {"port": 11111, "placement": "gpu"},
        ),
    ],
)
def test_superseded_ready_result_writes_no_context_or_placement(
    monkeypatch,
    provider: str,
    plan: supervisor.LocalServerLaunchPlan | supervisor.ParakeetServerLaunchPlan,
    managed_name: str,
    detail: dict[str, Any],
) -> None:
    from solstone.think.providers import local_server, parakeet_server

    state = supervisor._provider_runtime_states[provider]
    state.latest_plan = plan
    state.latest_phase = "ready"
    state.desired_fingerprint = "fp-new"
    state.retry.attempt_count = 2
    state.generation = 2
    state.start_fence = supervisor.ProviderFence(
        incarnation=supervisor._PROVIDER_INCARNATION,
        generation=1,
        fingerprint="fp-old",
        attempt=1,
    )
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="ready",
            reason_code="probe-ready",
            detail=detail,
            managed=_FakeManaged(managed_name),
        )
    )
    context_writes: list[int] = []
    placement_writes: list[str] = []
    monkeypatch.setattr(
        local_server,
        "write_local_context_window",
        lambda tokens: context_writes.append(tokens),
    )
    monkeypatch.setattr(
        parakeet_server,
        "write_parakeet_placement",
        lambda placement: placement_writes.append(placement),
    )

    assert supervisor._handle_provider_start_result(state, []) is True

    assert context_writes == []
    assert placement_writes == []


def test_provider_reconcilers_keep_local_and_parakeet_state_independent(
    monkeypatch,
) -> None:
    local_plan = _local_plan()
    parakeet_plan = _parakeet_plan()
    launches: list[tuple[str, int]] = []
    local = supervisor._provider_runtime_states["local"]
    parakeet = supervisor._provider_runtime_states["parakeet"]
    local.latest_phase = "starting"
    local.latest_plan = local_plan
    local.desired_fingerprint = local_plan.desired_fingerprint_sha256
    local.retry.desired_fingerprint = local_plan.desired_fingerprint_sha256
    local.next_truth_at = 9999.0
    parakeet.latest_phase = "starting"
    parakeet.latest_plan = parakeet_plan
    parakeet.desired_fingerprint = parakeet_plan.desired_fingerprint_sha256
    parakeet.retry.desired_fingerprint = parakeet_plan.desired_fingerprint_sha256
    parakeet.next_truth_at = 9999.0

    def start_worker(provider, _plan_arg, fence, _cancel_event):
        launches.append((provider, fence.attempt))
        return supervisor.ProviderLaunchOutcome(
            status="launch-failed",
            reason_code="launch-failed",
            detail={"provider": provider},
        )

    monkeypatch.setattr(supervisor, "_provider_executor", lambda: _InlineExecutor())
    monkeypatch.setattr(supervisor.time, "monotonic", lambda: 100.0)
    monkeypatch.setattr(supervisor, "_provider_start_worker", start_worker)

    asyncio.run(supervisor._reconcile_local_provider_runtime([]))
    asyncio.run(supervisor._reconcile_local_provider_runtime([]))
    asyncio.run(supervisor._reconcile_parakeet_provider_runtime([]))
    asyncio.run(supervisor._reconcile_parakeet_provider_runtime([]))

    assert launches == [("local", 1), ("parakeet", 1)]
    assert local.latest_phase == "backoff"
    assert parakeet.latest_phase == "backoff"
    assert local.retry.desired_fingerprint == local_plan.desired_fingerprint_sha256
    assert parakeet.retry.desired_fingerprint == (
        parakeet_plan.desired_fingerprint_sha256
    )


def test_spawn_failure_leaves_no_port_file(monkeypatch):
    from solstone.think.providers import local_server

    plan = _local_plan()
    monkeypatch.setattr(
        local_server, "write_local_context_window", lambda _tokens: None
    )
    monkeypatch.setattr(
        supervisor,
        "_launch_process",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(RuntimeError("spawn failed")),
    )

    outcome = supervisor.start_local_server(plan, _FakeReservation(port=34567))

    assert outcome.status == "launch-failed"
    assert supervisor.read_service_port("local") is None


@pytest.mark.parametrize(
    "backend",
    ["vulkan", "cuda", "mlx", "parakeet-vulkan", "parakeet-cpu"],
)
@pytest.mark.parametrize("status", ["warmup-timeout", "exited", "launch-failed"])
def test_non_ready_outcome_cleanup_runs_before_backoff_record(
    monkeypatch,
    backend: str,
    status: supervisor.LaunchOutcomeStatus,
) -> None:
    provider = "parakeet" if backend.startswith("parakeet") else "local"
    state = supervisor._provider_runtime_states[provider]
    state.latest_phase = "starting"
    state.desired_fingerprint = f"fp-{backend}"
    state.retry.attempt_count = 1
    state.retry.desired_fingerprint = state.desired_fingerprint
    state.generation = 1
    fence = supervisor._provider_fence(state, 1)
    managed_name = (
        supervisor.PARAKEET_SERVER_PROCESS_NAME
        if provider == "parakeet"
        else (
            supervisor.MLX_SERVER_PROCESS_NAME
            if backend == "mlx"
            else supervisor.LOCAL_SERVER_PROCESS_NAME
        )
    )
    managed = _FakeManaged(managed_name)
    state.start_fence = fence
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status=status,
            reason_code=(
                "warmup-timeout"
                if status == "warmup-timeout"
                else ("process-exited" if status == "exited" else "launch-failed")
            ),
            detail={"backend": backend, "port": 45678},
            managed=managed,
        )
    )
    order: list[str] = []

    monkeypatch.setattr(
        supervisor,
        "_terminate_cleanup_handle",
        lambda managed_arg, *, reason, state_name=None: order.append(
            f"cleanup:{managed_arg.name}:{reason}"
        ),
    )
    original_write = supervisor._write_provider_runtime

    def write_with_order(*args, **kwargs):
        order.append(f"write:{kwargs['phase']}")
        return original_write(*args, **kwargs)

    monkeypatch.setattr(supervisor, "_write_provider_runtime", write_with_order)

    assert supervisor._handle_provider_start_result(state, []) is True

    assert order[0].startswith(f"cleanup:{managed_name}:")
    assert order[1] == "write:backoff"
    assert state.latest_phase == "backoff"


def test_non_ready_cleanup_runs_before_failed_record(monkeypatch):
    state = supervisor._provider_runtime_states["local"]
    state.latest_phase = "starting"
    state.desired_fingerprint = "fp-local"
    state.retry.attempt_count = len(supervisor.PROVIDER_RETRY_SCHEDULE_SECONDS)
    state.generation = 1
    fence = supervisor._provider_fence(
        state, len(supervisor.PROVIDER_RETRY_SCHEDULE_SECONDS)
    )
    managed = _FakeManaged()
    state.start_fence = fence
    state.start_future = _future_with(
        supervisor.ProviderLaunchOutcome(
            status="warmup-timeout",
            reason_code="warmup-timeout",
            detail={"port": 45678},
            managed=managed,
        )
    )
    order: list[str] = []
    monkeypatch.setattr(
        supervisor,
        "_terminate_cleanup_handle",
        lambda managed_arg, *, reason, state_name=None: order.append("cleanup"),
    )
    original_write = supervisor._write_provider_runtime

    def write_with_order(*args, **kwargs):
        order.append(f"write:{kwargs['phase']}")
        return original_write(*args, **kwargs)

    monkeypatch.setattr(supervisor, "_write_provider_runtime", write_with_order)

    assert supervisor._handle_provider_start_result(state, []) is True

    assert order[:2] == ["cleanup", "write:failed"]
    assert state.latest_phase == "failed"


def test_launch_helper_returns_reserved_port_without_publishing(monkeypatch):
    from solstone.think.providers import local_server, local_vulkan

    plan = _local_plan()
    managed = _FakeManaged()
    ports: list[tuple[str, int]] = []
    spawned: list[list[str]] = []

    monkeypatch.setattr(
        supervisor,
        "write_service_port",
        lambda service, port: ports.append((service, port)),
    )
    monkeypatch.setattr(
        local_server,
        "write_local_context_window",
        lambda _tokens: None,
    )
    monkeypatch.setattr(
        local_server,
        "_probe_health",
        lambda _port: (local_server.STATE_READY, None),
    )
    monkeypatch.setattr(local_server, "fetch_props", lambda _port: None)
    monkeypatch.setattr(local_vulkan, "device_local_used_mib", lambda _index: None)
    monkeypatch.setattr(
        supervisor,
        "_request_local_launch_plan",
        _native_launch_plan_for_test,
    )
    monkeypatch.setattr(
        supervisor,
        "_launch_process",
        lambda name, cmd, **_kwargs: spawned.append(cmd) or managed,
    )

    assert not hasattr(plan, "port")

    outcome = supervisor.start_local_server(plan, _FakeReservation(port=45678))

    assert outcome.status == "ready"
    assert outcome.managed is managed
    assert ports == []
    assert spawned[0][spawned[0].index("--port") + 1] == "45678"
    assert RUNTIME_PHASES >= {"starting", "backoff", "ready"}
