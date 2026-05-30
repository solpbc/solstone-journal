# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import importlib
import threading
from datetime import datetime, timedelta, timezone

import pytest

from solstone.apps.settings import local_bootstrap
from solstone.apps.settings.install_copy import INSTALL_FAILED_NO_PROGRESS
from solstone.convey import create_app
from solstone.think.models import LOCAL_MODEL
from solstone.think.providers.install_state import (
    InstallState,
    InstallStatus,
    make_idle_status,
    read_install_status,
    transition_state,
    write_install_status,
)
from solstone.think.providers.local import LOCAL_MODEL_SPECS


def _client(journal_path):
    app = create_app(str(journal_path))
    app.config["TESTING"] = True
    return app.test_client()


def _settings_config() -> dict:
    return {
        "setup": {"completed_at": "2026-05-09T00:00:00Z"},
        "convey": {"trust_localhost": True},
        "providers": {
            "generate": {"provider": "google", "tier": 2, "backup": "anthropic"},
            "cogitate": {"provider": "openai", "tier": 2, "backup": "anthropic"},
            "auth": {"google": "api_key", "openai": "api_key"},
        },
    }


@pytest.fixture(autouse=True)
def _reset_local_state():
    with local_bootstrap._INSTALL_LOCK:
        local_bootstrap._INSTALL_THREADS.clear()
        local_bootstrap._INSTALL_PROGRESS.clear()


class _FakeThread:
    init_count = 0
    start_count = 0

    def __init__(self, *args, **kwargs):
        type(self).init_count += 1
        self.alive = True

    def start(self):
        type(self).start_count += 1

    def is_alive(self):
        return self.alive


def _write_local_status(
    state: InstallState,
    *,
    error: str | None = None,
    last_progress_at: str | None = None,
) -> InstallStatus:
    status = make_idle_status(local_bootstrap.local_install.LOCAL_PROVIDER_NAME)
    status["install_state"] = state
    status["last_transition_at"] = "2026-05-23T00:00:00+00:00"
    status["last_progress_at"] = last_progress_at
    status["install_error"] = error if state == "failed" else None
    write_install_status(status, scope="bundled")
    return status


def _old_progress_iso() -> str:
    return (datetime.now(timezone.utc) - timedelta(seconds=90)).isoformat()


def _fresh_progress_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def test_local_availability_payload_exact_shape(settings_env, monkeypatch):
    journal_path, _config = settings_env(_settings_config())
    monkeypatch.setattr(local_bootstrap, "check_binary_present", lambda: True)
    monkeypatch.setattr(local_bootstrap, "check_model_present", lambda _model: True)
    monkeypatch.setattr(local_bootstrap, "_platform_supported", lambda: (True, ""))
    monkeypatch.setattr(
        local_bootstrap.psutil,
        "virtual_memory",
        lambda: type("VMem", (), {"total": 32 * 1024**3})(),
    )
    client = _client(journal_path)

    response = client.get("/app/settings/api/local/availability")

    assert response.status_code == 200
    payload = response.get_json()
    assert set(payload) == {
        "model",
        "platform_supported",
        "total_memory_gb",
        "min_ram_gb",
        "binary_present",
        "model_present",
        "available",
        "reason",
    }
    assert payload == {
        "model": LOCAL_MODEL,
        "platform_supported": True,
        "total_memory_gb": 32.0,
        "min_ram_gb": 8,
        "binary_present": True,
        "model_present": True,
        "available": True,
        "reason": "",
    }


def test_local_models_route_returns_settings_shape(settings_env):
    journal_path, _config = settings_env(_settings_config())
    client = _client(journal_path)

    response = client.get("/app/settings/api/local/models")

    assert response.status_code == 200
    assert response.get_json() == [
        {
            "name": LOCAL_MODEL,
            "label": "qwen 3.5 4B VLM — 8 GB",
            "min_ram_gb": 8,
            "size_bytes": LOCAL_MODEL_SPECS[LOCAL_MODEL].size_bytes,
        },
    ]


@pytest.mark.parametrize(
    ("method", "path"),
    [
        ("get", "/app/settings/api/local/availability"),
        ("post", "/app/settings/api/local/bootstrap"),
        ("get", "/app/settings/api/local/bootstrap/status"),
    ],
)
def test_local_routes_reject_unknown_model(settings_env, method, path):
    journal_path, _config = settings_env(_settings_config())
    client = _client(journal_path)

    response = getattr(client, method)(f"{path}?model=not-real")

    assert response.status_code == 400
    payload = response.get_json()
    assert payload["reason_code"] == "invalid_request_value"
    assert "not-real" in payload["detail"]
    assert LOCAL_MODEL in payload["detail"]


@pytest.mark.parametrize(
    ("method", "path", "helper_name", "return_value"),
    [
        (
            "get",
            "/app/settings/api/local/availability",
            "get_availability_payload",
            {"available": True},
        ),
        (
            "post",
            "/app/settings/api/local/bootstrap",
            "start_bootstrap",
            ({"install_state": "installed"}, 200),
        ),
        (
            "get",
            "/app/settings/api/local/bootstrap/status",
            "get_state",
            {"install_state": "idle"},
        ),
    ],
)
def test_local_routes_default_to_flash_model(
    settings_env, monkeypatch, method, path, helper_name, return_value
):
    journal_path, _config = settings_env(_settings_config())
    calls = []

    def fake_helper(model):
        calls.append(model)
        return return_value

    monkeypatch.setattr(local_bootstrap, helper_name, fake_helper)
    client = _client(journal_path)

    response = getattr(client, method)(path)

    assert response.status_code == 200
    assert calls == [LOCAL_MODEL]


def test_local_bootstrap_post_rejects_unqualified_host(settings_env, monkeypatch):
    journal_path, _config = settings_env(_settings_config())
    monkeypatch.setattr(
        local_bootstrap,
        "start_bootstrap",
        lambda _model: (_ for _ in ()).throw(
            local_bootstrap.LocalBootstrapUnavailableError("unsupported platform")
        ),
    )
    client = _client(journal_path)

    response = client.post("/app/settings/api/local/bootstrap")

    assert response.status_code == 400
    payload = response.get_json()
    assert payload["reason_code"] == "invalid_request_value"
    assert payload["detail"] == "unsupported platform"


@pytest.mark.parametrize(
    ("state", "expected_payload", "expected_status"),
    [
        ("installed", {"install_state": "installed"}, 200),
        ("downloading", {"install_state": "downloading"}, 200),
        ("verifying", {"install_state": "verifying"}, 200),
        ("idle", {"install_state": "downloading"}, 202),
        ("failed", {"install_state": "downloading"}, 202),
    ],
)
def test_start_bootstrap_payload_for_canonical_states(
    settings_env, monkeypatch, state, expected_payload, expected_status
):
    settings_env(_settings_config())
    _write_local_status(
        state,
        error="failed before" if state == "failed" else None,
        last_progress_at=(
            _fresh_progress_iso() if state in ("downloading", "verifying") else None
        ),
    )
    monkeypatch.setattr(
        local_bootstrap,
        "get_availability_payload",
        lambda _model: {
            "platform_supported": True,
            "reason": "local runtime is not installed",
            "binary_present": False,
            "model_present": False,
        },
    )
    _FakeThread.init_count = 0
    _FakeThread.start_count = 0
    monkeypatch.setattr(local_bootstrap.threading, "Thread", _FakeThread)

    assert local_bootstrap.start_bootstrap(LOCAL_MODEL) == (
        expected_payload,
        expected_status,
    )


def test_local_bootstrap_status_returns_canonical_shape(settings_env):
    journal_path, _config = settings_env(_settings_config())
    _write_local_status("downloading", last_progress_at=_fresh_progress_iso())
    with local_bootstrap._INSTALL_LOCK:
        local_bootstrap._INSTALL_PROGRESS[LOCAL_MODEL] = (12, 24)
    client = _client(journal_path)

    response = client.get("/app/settings/api/local/bootstrap/status")

    assert response.status_code == 200
    payload = response.get_json()
    assert {
        "name",
        "install_state",
        "last_transition_at",
        "last_progress_at",
        "progress_bytes_received",
        "progress_bytes_total",
        "install_error",
    } == set(payload)
    assert payload["install_state"] == "downloading"
    assert payload["progress_bytes_received"] == 12
    assert payload["progress_bytes_total"] == 24


def test_local_bootstrap_lazy_stall_without_live_thread_fails(settings_env):
    settings_env(_settings_config())
    _write_local_status("downloading", last_progress_at=_old_progress_iso())

    payload = local_bootstrap.get_state(LOCAL_MODEL)

    assert payload["install_state"] == "failed"
    assert payload["install_error"] == INSTALL_FAILED_NO_PROGRESS
    persisted = read_install_status(scope="bundled", name="local")
    assert persisted["install_state"] == "failed"
    assert persisted["install_error"] == INSTALL_FAILED_NO_PROGRESS


def test_local_bootstrap_lazy_stall_with_live_thread_stays_in_flight(settings_env):
    settings_env(_settings_config())
    _write_local_status("verifying", last_progress_at=_old_progress_iso())
    with local_bootstrap._INSTALL_LOCK:
        local_bootstrap._INSTALL_THREADS[LOCAL_MODEL] = _FakeThread()

    payload = local_bootstrap.get_state(LOCAL_MODEL)

    assert payload["install_state"] == "verifying"
    assert payload["install_error"] is None


@pytest.mark.parametrize("state", ["installed", "failed"])
def test_local_bootstrap_restart_terminal_states_have_no_bytes(settings_env, state):
    settings_env(_settings_config())
    _write_local_status(state, error="boom" if state == "failed" else None)

    payload = local_bootstrap.get_state(LOCAL_MODEL)

    assert payload["install_state"] == state
    assert payload["progress_bytes_received"] is None
    assert payload["progress_bytes_total"] is None


def test_local_bootstrap_migrates_preexisting_install_without_worker(
    settings_env, monkeypatch
):
    settings_env(_settings_config())
    monkeypatch.setattr(
        local_bootstrap.local_install,
        "inspect_readiness",
        lambda _model=None: {
            "binary_installed": True,
            "model_installed": True,
            "ram_sufficient": True,
            "binary_path": "/tmp/llama-server",
            "model_path": "/tmp/model.gguf",
        },
    )
    monkeypatch.setattr(local_bootstrap, "_platform_supported", lambda: (True, ""))
    monkeypatch.setattr(
        local_bootstrap.psutil,
        "virtual_memory",
        lambda: type("VMem", (), {"total": 32 * 1024**3})(),
    )
    monkeypatch.setattr(
        local_bootstrap.threading,
        "Thread",
        lambda *args, **kwargs: pytest.fail("worker should not be created"),
    )

    assert local_bootstrap.start_bootstrap(LOCAL_MODEL) == (
        {"install_state": "installed"},
        200,
    )
    status = read_install_status(scope="bundled", name="local")
    assert status["install_state"] == "installed"


def test_local_worker_resets_progress_between_binary_and_model(
    settings_env, monkeypatch
):
    settings_env(_settings_config())
    observed = {}
    _write_local_status("downloading", last_progress_at=_fresh_progress_iso())

    def fake_llama_server():
        _write_local_status("installed")

    def fake_install_model(model):
        observed.update(local_bootstrap.get_state(model))
        status = read_install_status(scope="bundled", name="local")
        write_install_status(
            transition_state(status, new_state="installed"),
            scope="bundled",
        )

    monkeypatch.setattr(
        local_bootstrap.local_install, "install_llama_server", fake_llama_server
    )
    monkeypatch.setattr(
        local_bootstrap.local_install, "install_model", fake_install_model
    )

    local_bootstrap._run_bootstrap_worker(LOCAL_MODEL)

    gguf_size = LOCAL_MODEL_SPECS[LOCAL_MODEL].size_bytes
    assert observed["install_state"] == "downloading"
    assert observed["progress_bytes_total"] == gguf_size
    assert observed["progress_bytes_received"] <= gguf_size // 100


def test_local_worker_cleans_registered_thread(settings_env, monkeypatch):
    settings_env(_settings_config())
    current = threading.current_thread()
    with local_bootstrap._INSTALL_LOCK:
        local_bootstrap._INSTALL_THREADS[LOCAL_MODEL] = current
    _write_local_status("downloading", last_progress_at=_fresh_progress_iso())

    def fake_install_model(_model):
        status = read_install_status(scope="bundled", name="local")
        write_install_status(
            transition_state(status, new_state="installed"),
            scope="bundled",
        )

    monkeypatch.setattr(
        local_bootstrap.local_install, "install_llama_server", lambda: None
    )
    monkeypatch.setattr(
        local_bootstrap.local_install, "install_model", fake_install_model
    )

    local_bootstrap._run_bootstrap_worker(LOCAL_MODEL)

    with local_bootstrap._INSTALL_LOCK:
        assert LOCAL_MODEL not in local_bootstrap._INSTALL_THREADS


def test_local_worker_cleans_registered_thread_after_failure(settings_env, monkeypatch):
    settings_env(_settings_config())
    current = threading.current_thread()
    with local_bootstrap._INSTALL_LOCK:
        local_bootstrap._INSTALL_THREADS[LOCAL_MODEL] = current
    _write_local_status("downloading", last_progress_at=_fresh_progress_iso())

    monkeypatch.setattr(
        local_bootstrap.local_install,
        "install_llama_server",
        lambda: (_ for _ in ()).throw(RuntimeError("binary download broke")),
    )

    local_bootstrap._run_bootstrap_worker(LOCAL_MODEL)

    with local_bootstrap._INSTALL_LOCK:
        thread = local_bootstrap._INSTALL_THREADS.get(LOCAL_MODEL)
    assert thread is None or not thread.is_alive()
    status = read_install_status(scope="bundled", name="local")
    assert status["install_state"] == "failed"
    assert status["install_error"] == "binary download broke"


def test_routes_import_registers_local_endpoints(settings_env):
    routes = importlib.import_module("solstone.apps.settings.routes")
    journal_path, _config = settings_env(_settings_config())
    app = create_app(str(journal_path))
    registered = {rule.rule for rule in app.url_map.iter_rules()}

    assert routes.settings_bp is not None
    assert "/app/settings/api/providers/local/status" in registered
    assert "/app/settings/api/local/availability" in registered
    assert "/app/settings/api/local/bootstrap" in registered
    assert "/app/settings/api/local/bootstrap/status" in registered
    assert "/app/settings/api/local/models" in registered
