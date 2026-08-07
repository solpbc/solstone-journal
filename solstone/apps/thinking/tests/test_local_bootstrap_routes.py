# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Settings bootstrap contract after the worker-thread to native-process cutover."""

from __future__ import annotations

import threading
from pathlib import Path

import pytest

from solstone.apps.thinking import local_bootstrap


MODEL = local_bootstrap.LOCAL_MODEL


class _Readiness:
    def __init__(self, *, ready=False, status="missing-or-mismatched", reason="manifest_missing", host=None, artifacts=None, target=None):
        self.ready = ready
        self.status = status
        self.reason_code = reason
        self.host = host or {"platform_supported": True, "package_available": True}
        self.artifacts = artifacts or {"binary_installed": ready, "model_installed": ready}
        self.target = target or {"model_id": MODEL}


class _Process:
    def __init__(self, polls=None):
        self.polls = iter(polls or [None])
        self.returncode = None
        self.terminated = False

    def poll(self):
        value = next(self.polls, self.returncode)
        if value is not None:
            self.returncode = value
        return value

    def terminate(self):
        self.terminated = True
        self.returncode = -15

    def wait(self, timeout=None):
        return self.returncode


def _base_start(monkeypatch, *, readiness=None, statuses=None, mlx=False):
    readiness = readiness or _Readiness()
    statuses = iter(statuses or [{"install_state": "idle", "target_fingerprint_sha256": None}])
    monkeypatch.setattr(local_bootstrap, "resolve_local_endpoint", lambda: type("Endpoint", (), {"is_bundled": True})())
    monkeypatch.setattr(local_bootstrap, "_is_mlx_backend", lambda: mlx)
    target_module = local_bootstrap.mlx_install if mlx else local_bootstrap.local_install
    monkeypatch.setattr(target_module, "inspect_readiness", lambda _model: readiness)
    monkeypatch.setattr(target_module, "target_fingerprint", lambda _model: {"provider": "local"})
    monkeypatch.setattr(local_bootstrap, "get_availability_payload", lambda _model: {"binary_present": False, "model_present": False})
    monkeypatch.setattr(local_bootstrap, "probe_install_lease_free", lambda _provider: True)
    monkeypatch.setattr(local_bootstrap, "_read_status", lambda: next(statuses))
    monkeypatch.setattr(local_bootstrap, "_fit_report_for_model", lambda _model: type("Report", (), {"checks": []})())
    from solstone.think.providers import install_state
    monkeypatch.setattr(install_state, "fingerprint_sha256", lambda _value: "target")
    monkeypatch.setattr(local_bootstrap, "_INSTALL_LOCK", threading.Lock())
    local_bootstrap._INSTALL_PROCESSES.clear()


def test_local_model_list_and_presence_helpers_use_readiness(monkeypatch):
    ready = _Readiness(ready=True, artifacts={"binary_installed": True, "model_installed": True})
    monkeypatch.setattr(local_bootstrap, "_is_mlx_backend", lambda: False)
    monkeypatch.setattr(local_bootstrap.local_install, "inspect_readiness", lambda _model: ready)
    assert local_bootstrap.local_model_ids() == list(local_bootstrap.LOCAL_MODEL_SPECS)
    assert local_bootstrap.check_binary_present() and local_bootstrap.check_model_present(MODEL)
    assert local_bootstrap.list_local_models()[0]["name"] == MODEL


def test_mlx_model_list_and_availability_use_reconstructed_readiness(monkeypatch):
    readiness = _Readiness(ready=True, host={"platform_supported": True, "package_available": True}, artifacts={"model_installed": True})
    monkeypatch.setattr(local_bootstrap, "_is_mlx_backend", lambda: True)
    monkeypatch.setattr(local_bootstrap.mlx_install, "resolve_model_spec", lambda _model=None: type("Spec", (), {"name": local_bootstrap.QWEN_35_9B, "size_bytes": 10})())
    monkeypatch.setattr(local_bootstrap.mlx_install, "inspect_readiness", lambda _model: readiness)
    monkeypatch.setattr(local_bootstrap, "read_total_bytes", lambda: 32 * 1024**3)
    monkeypatch.setattr(local_bootstrap, "assess_memory", lambda *_args, **_kwargs: type("Memory", (), {"severity": "ok", "available_bytes": 32 * 1024**3, "required_bytes": 1})())
    assert local_bootstrap.list_local_models()[0]["name"] == local_bootstrap.QWEN_35_9B
    assert local_bootstrap.get_availability_payload(MODEL)["available"] is True


def test_get_state_reads_status_and_reports_interrupted_without_mutation(monkeypatch):
    status = {"provider": "local", "install_state": "downloading", "progress_bytes_received": 1, "progress_bytes_total": 2, "last_transition_at": None, "last_progress_at": None, "install_error": None}
    monkeypatch.setattr(local_bootstrap, "_is_mlx_backend", lambda: False)
    monkeypatch.setattr(local_bootstrap, "_read_status", lambda: status)
    monkeypatch.setattr(local_bootstrap, "probe_install_lease_free", lambda _provider: True)
    assert local_bootstrap.get_state(MODEL)["install_error"] == "install_interrupted"


def test_start_bootstrap_already_installed_does_not_spawn(monkeypatch):
    _base_start(monkeypatch, readiness=_Readiness(ready=True, status="ready"))
    monkeypatch.setattr(local_bootstrap.local_install, "spawn_install_local", lambda *_args, **_kwargs: (_ for _ in ()).throw(AssertionError("must not spawn")))
    assert local_bootstrap.start_bootstrap(MODEL) == ({"install_state": "installed"}, 200)


def test_start_bootstrap_rejects_byo_endpoint_without_status_or_spawn(monkeypatch):
    monkeypatch.setattr(local_bootstrap, "resolve_local_endpoint", lambda: type("Endpoint", (), {"is_bundled": False})())
    with pytest.raises(local_bootstrap.LocalBootstrapUnavailableError, match="BYO"):
        local_bootstrap.start_bootstrap(MODEL)


def test_start_bootstrap_same_target_deduplicates_without_spawn(monkeypatch):
    _base_start(monkeypatch, statuses=[{"install_state": "downloading", "target_fingerprint_sha256": "target"}])
    monkeypatch.setattr(local_bootstrap.local_install, "spawn_install_local", lambda *_args, **_kwargs: (_ for _ in ()).throw(AssertionError("must not spawn")))
    assert local_bootstrap.start_bootstrap(MODEL) == ({"install_state": "downloading"}, 200)


def test_start_bootstrap_preflight_busy_different_target_is_409(monkeypatch):
    _base_start(monkeypatch)
    monkeypatch.setattr(local_bootstrap, "probe_install_lease_free", lambda _provider: False)
    monkeypatch.setattr(local_bootstrap, "_read_status", lambda: {"install_state": "downloading", "target_fingerprint_sha256": "other"})
    assert local_bootstrap.start_bootstrap(MODEL) == ({"install_state": "downloading", "reason_code": "install_busy"}, 409)


@pytest.mark.parametrize("check_name", ["gpu", "disk"])
def test_start_bootstrap_fit_blocks_without_spawning(monkeypatch, check_name):
    _base_start(monkeypatch)
    report = type("Report", (), {"checks": [type("Check", (), {"name": check_name, "severity": "blocked", "detail": "blocked"})()]})()
    monkeypatch.setattr(local_bootstrap, "_fit_report_for_model", lambda _model: report)
    monkeypatch.setattr(local_bootstrap.local_install, "spawn_install_local", lambda *_args, **_kwargs: (_ for _ in ()).throw(AssertionError("must not spawn")))
    with pytest.raises(local_bootstrap.LocalBootstrapUnavailableError, match="blocked"):
        local_bootstrap.start_bootstrap(MODEL)


def test_start_bootstrap_success_stores_process_and_reaps(monkeypatch):
    process = _Process()
    _base_start(monkeypatch, statuses=[{"install_state": "idle", "target_fingerprint_sha256": None}, {"install_state": "downloading", "target_fingerprint_sha256": "target"}])
    monkeypatch.setattr(local_bootstrap.local_install, "spawn_install_local", lambda *_args, **_kwargs: process)
    monkeypatch.setattr(local_bootstrap, "_reap_process", lambda *_args: None)
    assert local_bootstrap.start_bootstrap(MODEL) == ({"install_state": "downloading"}, 202)
    assert local_bootstrap._INSTALL_PROCESSES[MODEL] is process


def test_start_bootstrap_early_native_busy_returns_409_without_entry(monkeypatch):
    process = _Process([75])
    _base_start(monkeypatch, statuses=[{"install_state": "idle", "target_fingerprint_sha256": None}] * 3)
    monkeypatch.setattr(local_bootstrap.local_install, "spawn_install_local", lambda *_args, **_kwargs: process)
    assert local_bootstrap.start_bootstrap(MODEL) == ({"install_state": "idle", "reason_code": "install_busy"}, 409)
    assert MODEL not in local_bootstrap._INSTALL_PROCESSES


def test_start_bootstrap_timeout_terminates_and_marks_launch_failure(monkeypatch):
    process = _Process([None, None])
    _base_start(monkeypatch, statuses=[{"install_state": "idle", "target_fingerprint_sha256": None}] * 3)
    monkeypatch.setattr(local_bootstrap.local_install, "spawn_install_local", lambda *_args, **_kwargs: process)
    values = iter([0.0, 0.0, 6.0])
    monkeypatch.setattr(local_bootstrap.time, "monotonic", lambda: next(values))
    monkeypatch.setattr(local_bootstrap.time, "sleep", lambda _seconds: None)
    marked = []
    monkeypatch.setattr(local_bootstrap, "_mark_native_launch_failure", lambda target, message, **kwargs: marked.append((target, message, kwargs)))
    with pytest.raises(local_bootstrap.LocalBootstrapStartError):
        local_bootstrap.start_bootstrap(MODEL)
    assert process.terminated and marked[0][0] == {"provider": "local"}


def test_start_bootstrap_popen_error_marks_launch_failure(monkeypatch):
    _base_start(monkeypatch)
    monkeypatch.setattr(local_bootstrap.local_install, "spawn_install_local", lambda *_args, **_kwargs: (_ for _ in ()).throw(OSError("missing core")))
    marked = []
    monkeypatch.setattr(local_bootstrap, "_mark_native_launch_failure", lambda target, message, **kwargs: marked.append((target, message, kwargs)))
    with pytest.raises(local_bootstrap.LocalBootstrapStartError, match="missing core"):
        local_bootstrap.start_bootstrap(MODEL)
    assert marked == [({"provider": "local"}, "missing core", {})]


def test_guarded_timeout_failure_uses_captured_attempt_and_ignores_newer_attempt(monkeypatch):
    class Lease:
        def release(self):
            pass

    writes = []
    captured = {
        "provider": "local", "install_state": "downloading", "attempt_id": "old",
        "target_fingerprint_json": "{}", "target_fingerprint_sha256": "target",
        "revision": 1, "schema_version": 1, "started_at": None, "last_transition_at": None,
        "last_progress_at": None, "completed_at": None, "progress_bytes_received": None,
        "progress_bytes_total": None, "install_error": None, "error_code": None, "owner": None,
    }
    monkeypatch.setattr(local_bootstrap, "acquire_install_lease", lambda _provider: Lease())
    def stale_write(status):
        writes.append(status)
        raise RuntimeError("attempt id changed")

    monkeypatch.setattr(local_bootstrap, "_write_status", stale_write)
    local_bootstrap._mark_native_launch_failure({"provider": "local"}, "timeout", attempt=captured)
    assert writes[0]["attempt_id"] == "old"


def test_launch_failure_begins_and_persists_a_durable_failed_attempt(monkeypatch):
    class Lease:
        def release(self):
            pass

    begun = []
    writes = []
    attempt = {
        "provider": "local", "install_state": "resolving", "attempt_id": "new",
        "target_fingerprint_json": "{}", "target_fingerprint_sha256": "target",
        "revision": 1, "schema_version": 1, "started_at": None, "last_transition_at": None,
        "last_progress_at": None, "completed_at": None, "progress_bytes_received": None,
        "progress_bytes_total": None, "install_error": None, "error_code": None, "owner": None,
    }
    monkeypatch.setattr(local_bootstrap, "acquire_install_lease", lambda _provider: Lease())
    monkeypatch.setattr(
        local_bootstrap,
        "begin_or_replace_install_attempt",
        lambda provider, target, **kwargs: begun.append((provider, target, kwargs)) or attempt,
    )
    monkeypatch.setattr(local_bootstrap, "_write_status", lambda status: writes.append(status))
    local_bootstrap._mark_native_launch_failure({"provider": "local"}, "missing core")
    assert begun[0][1] == {"provider": "local"}
    assert writes[0]["install_state"] == "failed"


def test_reaper_removes_completed_process(monkeypatch):
    process = _Process()
    process.returncode = 0
    local_bootstrap._INSTALL_PROCESSES[MODEL] = process
    monkeypatch.setattr(local_bootstrap, "_INSTALL_LOCK", threading.Lock())
    local_bootstrap._reap_process(MODEL, process)
    assert MODEL not in local_bootstrap._INSTALL_PROCESSES


def test_mlx_branch_spawns_mlx_native_process(monkeypatch):
    process = _Process()
    _base_start(monkeypatch, mlx=True, statuses=[{"install_state": "idle", "target_fingerprint_sha256": None}, {"install_state": "resolving", "target_fingerprint_sha256": "target"}])
    monkeypatch.setattr(local_bootstrap.mlx_install, "spawn_install_local_mlx", lambda *_args, **_kwargs: process)
    monkeypatch.setattr(local_bootstrap, "_reap_process", lambda *_args: None)
    assert local_bootstrap.start_bootstrap(MODEL) == ({"install_state": "resolving"}, 202)


def test_mlx_pending_fetch_returns_202_without_waiting_for_native_status(monkeypatch):
    process = _Process([None, None])
    started = threading.Event()
    release = threading.Event()

    def slow_request(_model, _owner):
        started.set()
        assert release.wait(timeout=1)
        return {"model_id": local_bootstrap.QWEN_35_9B}

    _base_start(monkeypatch, mlx=True, statuses=[{"install_state": "idle", "target_fingerprint_sha256": None}] * 3)
    monkeypatch.setattr(local_bootstrap.mlx_install, "_run_request", slow_request)
    monkeypatch.setattr(local_bootstrap.mlx_install, "_spawn_core_install", lambda *_args: process)
    monkeypatch.setattr(local_bootstrap, "_reap_process", lambda *_args: None)
    values = iter([0.0, 0.0, 6.0])
    monkeypatch.setattr(local_bootstrap.time, "monotonic", lambda: next(values))
    monkeypatch.setattr(local_bootstrap.time, "sleep", lambda _seconds: None)
    assert local_bootstrap.start_bootstrap(MODEL) == ({"install_state": "resolving"}, 202)
    assert started.wait(timeout=1)
    handle = local_bootstrap._INSTALL_PROCESSES[local_bootstrap.QWEN_35_9B]
    assert handle.pending is True
    release.set()
    handle.wait(timeout=1)
