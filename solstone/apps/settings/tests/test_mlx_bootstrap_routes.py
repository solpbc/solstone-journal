# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import importlib
import json
import sys
import threading
import time
import tomllib
from datetime import datetime, timedelta, timezone
from pathlib import Path

import huggingface_hub
import pytest

from solstone.apps.settings import mlx_bootstrap
from solstone.apps.settings.install_copy import INSTALL_FAILED_NO_PROGRESS
from solstone.convey import create_app
from solstone.think.models import GEMMA4_26B_A4B_4BIT, QWEN_35_9B
from solstone.think.providers import mlx_install
from solstone.think.providers.install_state import (
    InstallState,
    InstallStatus,
    make_idle_status,
    read_install_status,
    transition_state,
    write_install_status,
)
from solstone.think.providers.mlx_install import _MLX_MODEL_REGISTRY


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
def _reset_mlx_state(monkeypatch, tmp_path):
    config_dir = tmp_path / "config"
    config_dir.mkdir(parents=True, exist_ok=True)
    (config_dir / "journal.json").write_text(
        json.dumps(_settings_config(), indent=2) + "\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    with mlx_bootstrap._INSTALL_LOCK:
        mlx_bootstrap._INSTALL_THREADS.clear()
        mlx_bootstrap._INSTALL_PROGRESS.clear()
    monkeypatch.setattr(mlx_install.constants, "HF_HUB_CACHE", str(tmp_path / "hf"))


def _set_state(
    model: str = QWEN_35_9B,
    *,
    state: InstallState = "idle",
    received_bytes: int | None = None,
    total_bytes: int | None = None,
    message: str | None = None,
    thread: threading.Thread | object | None = None,
    last_progress_at: str | None = None,
) -> InstallStatus:
    status = make_idle_status(model)
    status["install_state"] = state
    status["last_transition_at"] = "2026-05-23T00:00:00+00:00"
    status["last_progress_at"] = last_progress_at
    status["install_error"] = message if state == "failed" else None
    write_install_status(status, scope="mlx")
    with mlx_bootstrap._INSTALL_LOCK:
        if thread is None:
            mlx_bootstrap._INSTALL_THREADS.pop(model, None)
        else:
            mlx_bootstrap._INSTALL_THREADS[model] = thread
        if received_bytes is None and total_bytes is None:
            mlx_bootstrap._INSTALL_PROGRESS.pop(model, None)
        else:
            mlx_bootstrap._INSTALL_PROGRESS[model] = (received_bytes, total_bytes)
    return status


def _old_progress_iso() -> str:
    return (datetime.now(timezone.utc) - timedelta(seconds=90)).isoformat()


def _fresh_progress_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


class _FakeThread:
    init_count = 0
    start_count = 0
    count_lock = threading.Lock()

    def __init__(self, *args, **kwargs):
        with type(self).count_lock:
            type(self).init_count += 1
        self.alive = True

    def start(self):
        with type(self).count_lock:
            type(self).start_count += 1

    def is_alive(self):
        return self.alive


class _DeadThread:
    def is_alive(self):
        return False


def test_mlx_availability_payload_exact_shape(settings_env, monkeypatch):
    journal_path, _config = settings_env(_settings_config())
    monkeypatch.setattr(
        mlx_bootstrap, "is_mlx_available_for_model", lambda _spec: (True, "")
    )
    monkeypatch.setattr(mlx_bootstrap, "check_model_present", lambda _model: True)
    monkeypatch.setattr(mlx_bootstrap.platform, "system", lambda: "Darwin")
    monkeypatch.setattr(mlx_bootstrap.platform, "machine", lambda: "arm64")
    monkeypatch.setattr(
        mlx_bootstrap.psutil,
        "virtual_memory",
        lambda: type("VMem", (), {"total": 32 * 1024**3})(),
    )
    monkeypatch.setattr(mlx_bootstrap, "_is_package_installed", lambda _name: True)
    client = _client(journal_path)

    response = client.get("/app/settings/api/mlx/availability")

    assert response.status_code == 200
    payload = response.get_json()
    assert set(payload) == {
        "model",
        "is_apple_silicon",
        "total_memory_gb",
        "mlx_installed",
        "min_ram_gb",
        "model_present",
        "available",
        "reason",
    }
    assert payload == {
        "model": QWEN_35_9B,
        "is_apple_silicon": True,
        "total_memory_gb": 32.0,
        "mlx_installed": True,
        "min_ram_gb": 16,
        "model_present": True,
        "available": True,
        "reason": "",
    }
    assert all(
        isinstance(payload[key], bool)
        for key in (
            "is_apple_silicon",
            "mlx_installed",
            "model_present",
            "available",
        )
    )
    assert isinstance(payload["total_memory_gb"], float)
    assert isinstance(payload["reason"], str)


def test_bootstrap_post_is_idempotent_while_downloading(settings_env, monkeypatch):
    settings_env(_settings_config())
    monkeypatch.setattr(
        mlx_bootstrap, "is_mlx_available_for_model", lambda _spec: (True, "")
    )
    present_barrier = threading.Barrier(2)
    present_lock = threading.Lock()
    present_calls = 0

    def fake_check_model_present(_model):
        nonlocal present_calls
        with present_lock:
            present_calls += 1
            call_number = present_calls
        if call_number <= 2:
            present_barrier.wait(timeout=2)
        return False

    monkeypatch.setattr(mlx_bootstrap, "check_model_present", fake_check_model_present)
    real_thread = threading.Thread
    _FakeThread.init_count = 0
    _FakeThread.start_count = 0
    monkeypatch.setattr(mlx_bootstrap.threading, "Thread", _FakeThread)
    call_barrier = threading.Barrier(3)
    results = []
    errors = []
    results_lock = threading.Lock()

    def call_start_bootstrap():
        try:
            call_barrier.wait(timeout=2)
            result = mlx_bootstrap.start_bootstrap(QWEN_35_9B)
            with results_lock:
                results.append(result)
        except Exception as exc:  # pragma: no cover - surfaced by assertion below
            with results_lock:
                errors.append(exc)

    callers = [
        real_thread(target=call_start_bootstrap),
        real_thread(target=call_start_bootstrap),
    ]
    for caller in callers:
        caller.start()
    call_barrier.wait(timeout=2)
    for caller in callers:
        caller.join(timeout=2)

    assert errors == []
    assert sorted(status for _payload, status in results) == [200, 202]
    assert [payload for payload, _status in results] == [
        {"install_state": "downloading"},
        {"install_state": "downloading"},
    ]
    assert _FakeThread.init_count == 1
    assert _FakeThread.start_count == 1


def test_bootstrap_post_already_installed_returns_installed_without_worker(
    settings_env,
    monkeypatch,
):
    journal_path, _config = settings_env(_settings_config())
    monkeypatch.setattr(
        mlx_bootstrap, "is_mlx_available_for_model", lambda _spec: (True, "")
    )
    monkeypatch.setattr(mlx_bootstrap, "check_model_present", lambda _model: True)
    monkeypatch.setattr(
        mlx_bootstrap.threading,
        "Thread",
        lambda *args, **kwargs: pytest.fail("worker should not be created"),
    )
    client = _client(journal_path)

    response = client.post("/app/settings/api/mlx/bootstrap")

    assert response.status_code == 200
    assert response.get_json() == {"install_state": "installed"}


@pytest.mark.parametrize(
    "reason",
    [
        "not running on macOS",
        "not running on Apple Silicon",
        "insufficient RAM (need 16 GB, have 8 GB)",
        "mlx-vlm package not installed",
    ],
)
def test_bootstrap_post_rejects_unqualified_host(settings_env, monkeypatch, reason):
    journal_path, _config = settings_env(_settings_config())
    monkeypatch.setattr(
        mlx_bootstrap, "is_mlx_available_for_model", lambda _spec: (False, reason)
    )
    client = _client(journal_path)

    response = client.post("/app/settings/api/mlx/bootstrap")

    assert response.status_code == 400
    payload = response.get_json()
    assert payload["reason_code"] == "invalid_request_value"
    assert payload["detail"] == reason


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
def test_mlx_start_bootstrap_payload_for_canonical_states(
    settings_env, monkeypatch, state, expected_payload, expected_status
):
    settings_env(_settings_config())
    _set_state(
        state=state,
        message="failed before" if state == "failed" else None,
        last_progress_at=(
            _fresh_progress_iso() if state in ("downloading", "verifying") else None
        ),
    )
    monkeypatch.setattr(
        mlx_bootstrap, "is_mlx_available_for_model", lambda _spec: (True, "")
    )
    monkeypatch.setattr(mlx_bootstrap, "check_model_present", lambda _model: False)
    _FakeThread.init_count = 0
    _FakeThread.start_count = 0
    monkeypatch.setattr(mlx_bootstrap.threading, "Thread", _FakeThread)

    assert mlx_bootstrap.start_bootstrap(QWEN_35_9B) == (
        expected_payload,
        expected_status,
    )


def test_bootstrap_status_always_returns_canonical_fields(settings_env):
    journal_path, _config = settings_env(_settings_config())
    client = _client(journal_path)

    for state in ("idle", "downloading", "verifying", "installed", "failed"):
        _set_state(
            state=state,
            received_bytes=12,
            total_bytes=24,
            message="bad" if state == "failed" else None,
            thread=_FakeThread() if state in ("downloading", "verifying") else None,
            last_progress_at=(
                _fresh_progress_iso() if state in ("downloading", "verifying") else None
            ),
        )
        response = client.get("/app/settings/api/mlx/bootstrap/status")
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
        assert payload["install_state"] == state
        assert payload["install_error"] == ("bad" if state == "failed" else None)
        if state in ("downloading", "verifying"):
            assert isinstance(payload["progress_bytes_received"], int)
            assert isinstance(payload["progress_bytes_total"], int)


def test_bootstrap_status_transitions_stalled_download_to_failed(settings_env):
    journal_path, _config = settings_env(_settings_config())
    _set_state(
        state="downloading",
        thread=_DeadThread(),
        last_progress_at=_old_progress_iso(),
    )
    client = _client(journal_path)

    payload = client.get("/app/settings/api/mlx/bootstrap/status").get_json()

    assert payload["install_state"] == "failed"
    assert payload["install_error"] == INSTALL_FAILED_NO_PROGRESS


def test_bootstrap_status_transitions_stalled_verifying_to_failed(settings_env):
    journal_path, _config = settings_env(_settings_config())
    _set_state(
        state="verifying",
        thread=_DeadThread(),
        last_progress_at=_old_progress_iso(),
    )
    client = _client(journal_path)

    payload = client.get("/app/settings/api/mlx/bootstrap/status").get_json()

    assert payload["install_state"] == "failed"
    assert payload["install_error"] == INSTALL_FAILED_NO_PROGRESS


def test_mlx_bootstrap_lazy_stall_with_live_thread_stays_in_flight(settings_env):
    settings_env(_settings_config())
    _set_state(
        state="downloading",
        thread=_FakeThread(),
        last_progress_at=_old_progress_iso(),
    )

    payload = mlx_bootstrap.get_state(QWEN_35_9B)

    assert payload["install_state"] == "downloading"
    assert payload["install_error"] is None


@pytest.mark.parametrize("state", ["installed", "failed"])
def test_mlx_bootstrap_restart_terminal_states_have_no_bytes(settings_env, state):
    settings_env(_settings_config())
    _set_state(state=state, message="boom" if state == "failed" else None)

    payload = mlx_bootstrap.get_state(QWEN_35_9B)

    assert payload["install_state"] == state
    assert payload["progress_bytes_received"] is None
    assert payload["progress_bytes_total"] is None


def test_routes_import_without_mlx_vlm_registers_mlx_endpoints(
    monkeypatch, settings_env
):
    monkeypatch.setitem(sys.modules, "mlx_vlm", None)
    routes = importlib.import_module("solstone.apps.settings.routes")
    journal_path, _config = settings_env(_settings_config())
    app = create_app(str(journal_path))
    registered = {rule.rule for rule in app.url_map.iter_rules()}

    assert routes.settings_bp is not None
    assert "/app/settings/api/mlx/availability" in registered
    assert "/app/settings/api/mlx/bootstrap" in registered
    assert "/app/settings/api/mlx/bootstrap/status" in registered
    assert "/app/settings/api/mlx/models" in registered


def test_mlx_models_route_returns_settings_shape(settings_env):
    journal_path, _config = settings_env(_settings_config())
    client = _client(journal_path)

    response = client.get("/app/settings/api/mlx/models")

    assert response.status_code == 200
    assert response.get_json() == [
        {
            "name": QWEN_35_9B,
            "label": "qwen 3.5 — 16 GB Mac",
            "min_ram_gb": 16,
        },
        {
            "name": GEMMA4_26B_A4B_4BIT,
            "label": "gemma 4 (26B) — 24 GB Mac",
            "min_ram_gb": 24,
        },
    ]


@pytest.mark.parametrize(
    ("method", "path"),
    [
        ("get", "/app/settings/api/mlx/availability"),
        ("post", "/app/settings/api/mlx/bootstrap"),
        ("get", "/app/settings/api/mlx/bootstrap/status"),
    ],
)
def test_mlx_routes_reject_unknown_model(settings_env, method, path):
    journal_path, _config = settings_env(_settings_config())
    client = _client(journal_path)

    response = getattr(client, method)(f"{path}?model=not-real")

    assert response.status_code == 400
    payload = response.get_json()
    assert payload["reason_code"] == "invalid_request_value"
    assert "not-real" in payload["detail"]
    assert QWEN_35_9B in payload["detail"]
    assert GEMMA4_26B_A4B_4BIT in payload["detail"]


@pytest.mark.parametrize(
    ("method", "path", "helper_name", "return_value"),
    [
        (
            "get",
            "/app/settings/api/mlx/availability",
            "get_availability_payload",
            {"available": True},
        ),
        (
            "post",
            "/app/settings/api/mlx/bootstrap",
            "start_bootstrap",
            ({"install_state": "installed"}, 200),
        ),
        (
            "get",
            "/app/settings/api/mlx/bootstrap/status",
            "get_state",
            {"install_state": "idle"},
        ),
    ],
)
def test_mlx_routes_default_to_qwen_model(
    settings_env, monkeypatch, method, path, helper_name, return_value
):
    journal_path, _config = settings_env(_settings_config())
    calls = []

    def fake_helper(model):
        calls.append(model)
        return return_value

    monkeypatch.setattr(mlx_bootstrap, helper_name, fake_helper)
    client = _client(journal_path)

    response = getattr(client, method)(path)

    assert response.status_code == 200
    assert calls == [QWEN_35_9B]


def test_update_providers_mlx_round_trip_persists_active_model(
    settings_env, monkeypatch
):
    journal_path, _config = settings_env(_settings_config())
    client = _client(journal_path)

    response = client.put(
        "/app/settings/api/providers",
        json={"mlx": {"active_model": GEMMA4_26B_A4B_4BIT}},
    )

    assert response.status_code == 200
    assert response.get_json()["mlx"]["active_model"] == GEMMA4_26B_A4B_4BIT
    saved = json.loads((journal_path / "config" / "journal.json").read_text())
    assert saved["providers"]["mlx"]["active_model"] == GEMMA4_26B_A4B_4BIT

    from solstone.think.providers import mlx

    monkeypatch.setattr(mlx, "_module_level_cache", {})
    assert mlx._resolve_default_model() == GEMMA4_26B_A4B_4BIT


def test_mlx_bootstrap_status_preserves_active_model_peer_key(
    settings_env, monkeypatch
):
    journal_path, _config = settings_env(_settings_config())
    client = _client(journal_path)
    client.put(
        "/app/settings/api/providers",
        json={"mlx": {"active_model": GEMMA4_26B_A4B_4BIT}},
    )
    monkeypatch.setattr(
        mlx_bootstrap, "is_mlx_available_for_model", lambda _spec: (True, "")
    )
    monkeypatch.setattr(mlx_bootstrap, "check_model_present", lambda _model: False)
    monkeypatch.setattr(mlx_bootstrap.threading, "Thread", _FakeThread)

    response = client.post(f"/app/settings/api/mlx/bootstrap?model={QWEN_35_9B}")

    assert response.status_code == 202
    saved = json.loads((journal_path / "config" / "journal.json").read_text())
    assert saved["providers"]["mlx"]["active_model"] == GEMMA4_26B_A4B_4BIT
    model_record = saved["providers"]["mlx"][QWEN_35_9B]
    assert set(model_record) >= {
        "install_state",
        "last_transition_at",
        "last_progress_at",
        "install_error",
    }
    assert model_record["install_state"] == "downloading"


@pytest.mark.parametrize(
    "payload",
    [
        {"mlx": "string-not-object"},
        {"mlx": {"active_model": 123}},
        {"mlx": {"unknown_field": "x"}},
        {"mlx": {"active_model": "not-a-real-model"}},
    ],
)
def test_update_providers_mlx_rejects_malformed_payload(settings_env, payload):
    journal_path, _config = settings_env(_settings_config())
    client = _client(journal_path)
    before = (journal_path / "config" / "journal.json").read_text()

    response = client.put("/app/settings/api/providers", json=payload)

    assert response.status_code == 400
    assert response.get_json()["reason_code"] == "invalid_config_value"
    assert (journal_path / "config" / "journal.json").read_text() == before


def test_bootstrap_state_is_per_model_under_concurrent_access(monkeypatch):
    releases = {
        QWEN_35_9B: threading.Event(),
        GEMMA4_26B_A4B_4BIT: threading.Event(),
    }
    started = {
        QWEN_35_9B: threading.Event(),
        GEMMA4_26B_A4B_4BIT: threading.Event(),
    }
    monkeypatch.setattr(
        mlx_bootstrap,
        "is_mlx_available_for_model",
        lambda _spec: (True, ""),
    )
    monkeypatch.setattr(mlx_bootstrap, "check_model_present", lambda _model: False)

    def fake_worker(model):
        started[model].set()
        releases[model].wait(timeout=2)
        write_install_status(
            transition_state(
                read_install_status(scope="mlx", name=model),
                new_state="installed",
            ),
            scope="mlx",
        )

    monkeypatch.setattr(mlx_bootstrap, "_run_bootstrap_worker", fake_worker)

    assert mlx_bootstrap.start_bootstrap(QWEN_35_9B) == (
        {"install_state": "downloading"},
        202,
    )
    assert started[QWEN_35_9B].wait(timeout=2)
    assert mlx_bootstrap.start_bootstrap(GEMMA4_26B_A4B_4BIT) == (
        {"install_state": "downloading"},
        202,
    )
    assert started[GEMMA4_26B_A4B_4BIT].wait(timeout=2)
    assert mlx_bootstrap.get_state(QWEN_35_9B)["install_state"] == "downloading"
    assert (
        mlx_bootstrap.get_state(GEMMA4_26B_A4B_4BIT)["install_state"] == "downloading"
    )

    releases[QWEN_35_9B].set()
    deadline = time.monotonic() + 2
    while (
        mlx_bootstrap.get_state(QWEN_35_9B)["install_state"] != "installed"
        and time.monotonic() < deadline
    ):
        time.sleep(0.01)
    assert mlx_bootstrap.get_state(QWEN_35_9B)["install_state"] == "installed"
    assert (
        mlx_bootstrap.get_state(GEMMA4_26B_A4B_4BIT)["install_state"] == "downloading"
    )

    releases[GEMMA4_26B_A4B_4BIT].set()
    deadline = time.monotonic() + 2
    while (
        mlx_bootstrap.get_state(GEMMA4_26B_A4B_4BIT)["install_state"] != "installed"
        and time.monotonic() < deadline
    ):
        time.sleep(0.01)
    assert mlx_bootstrap.get_state(GEMMA4_26B_A4B_4BIT)["install_state"] == "installed"


def _write_snapshot(
    tmp_path: Path,
    monkeypatch,
    files: dict[str, bytes],
    model: str = QWEN_35_9B,
) -> Path:
    monkeypatch.setattr(mlx_install.constants, "HF_HUB_CACHE", str(tmp_path / "hf"))
    snapshot_dir = mlx_bootstrap._snapshot_dir(model)
    snapshot_dir.mkdir(parents=True)
    (snapshot_dir / "model.safetensors.index.json").write_text(
        json.dumps({"weight_map": {f"w{i}": path for i, path in enumerate(files)}}),
        encoding="utf-8",
    )
    for rel_path, content in files.items():
        file_path = snapshot_dir / rel_path
        file_path.parent.mkdir(parents=True, exist_ok=True)
        file_path.write_bytes(content)
    return snapshot_dir


def test_model_present_requires_index_and_all_safetensors(tmp_path, monkeypatch):
    _write_snapshot(
        tmp_path,
        monkeypatch,
        {
            "model-00001-of-00002.safetensors": b"one",
            "model-00002-of-00002.safetensors": b"two",
        },
    )

    assert mlx_bootstrap.check_model_present(QWEN_35_9B) is True

    (
        mlx_bootstrap._snapshot_dir(QWEN_35_9B) / "model-00002-of-00002.safetensors"
    ).unlink()
    assert mlx_bootstrap.check_model_present(QWEN_35_9B) is False


def test_snapshot_download_called_without_resume_download(monkeypatch):
    calls = {}

    def fake_snapshot_download(**kwargs):
        calls.update(kwargs)

    _set_state(state="downloading", thread=threading.current_thread())
    monkeypatch.setattr(
        mlx_bootstrap.huggingface_hub, "snapshot_download", fake_snapshot_download
    )
    monkeypatch.setattr(
        mlx_bootstrap, "_verify_safetensors_sha256_hashes", lambda _model: None
    )

    mlx_bootstrap._run_bootstrap_worker(QWEN_35_9B)

    spec = _MLX_MODEL_REGISTRY[QWEN_35_9B]
    assert calls["repo_id"] == spec.repo
    assert calls["revision"] == spec.revision
    assert "resume_download" not in calls


def test_hfapi_list_repo_tree_called_with_pinned_revision(tmp_path, monkeypatch):
    _write_snapshot(tmp_path, monkeypatch, {"model.safetensors": b"abc"})
    expected = hashlib.sha256(b"abc").hexdigest()
    calls = {}

    class FakeApi:
        def list_repo_tree(self, **kwargs):
            calls.update(kwargs)
            return [
                huggingface_hub.RepoFile(
                    path="model.safetensors",
                    size=3,
                    oid="oid",
                    lfs={"size": 3, "oid": expected, "pointerSize": 123},
                )
            ]

    monkeypatch.setattr(mlx_bootstrap.huggingface_hub, "HfApi", lambda: FakeApi())
    _set_state(state="verifying")

    mlx_bootstrap._verify_safetensors_sha256_hashes(QWEN_35_9B)

    spec = _MLX_MODEL_REGISTRY[QWEN_35_9B]
    assert calls["repo_id"] == spec.repo
    assert calls["revision"] == spec.revision
    assert calls["recursive"] is True


def test_worker_enters_verifying_state_between_download_and_install(monkeypatch):
    entered_verify = threading.Event()
    release_verify = threading.Event()

    def slow_verify(_model):
        entered_verify.set()
        release_verify.wait(timeout=2)

    _set_state(state="downloading", thread=None)
    monkeypatch.setattr(
        mlx_bootstrap.huggingface_hub, "snapshot_download", lambda **_kwargs: None
    )
    monkeypatch.setattr(mlx_bootstrap, "_verify_safetensors_sha256_hashes", slow_verify)
    worker = threading.Thread(
        target=mlx_bootstrap._run_bootstrap_worker, args=(QWEN_35_9B,)
    )
    _set_state(state="downloading", thread=worker)

    worker.start()
    assert entered_verify.wait(timeout=2)
    assert mlx_bootstrap.get_state(QWEN_35_9B)["install_state"] == "verifying"
    release_verify.set()
    worker.join(timeout=2)
    assert mlx_bootstrap.get_state(QWEN_35_9B)["install_state"] == "installed"


def test_worker_verify_mismatch_transitions_to_failed_with_filename(
    tmp_path, monkeypatch
):
    snapshot_dir = _write_snapshot(tmp_path, monkeypatch, {"model.safetensors": b"abc"})

    class FakeApi:
        def list_repo_tree(self, **kwargs):
            return [
                huggingface_hub.RepoFile(
                    path="model.safetensors",
                    size=3,
                    oid="oid",
                    lfs={"size": 3, "oid": "0" * 64, "pointerSize": 123},
                )
            ]

    _set_state(state="downloading", thread=threading.current_thread())
    monkeypatch.setattr(
        mlx_bootstrap.huggingface_hub, "snapshot_download", lambda **_kwargs: None
    )
    monkeypatch.setattr(mlx_bootstrap.huggingface_hub, "HfApi", lambda: FakeApi())

    mlx_bootstrap._run_bootstrap_worker(QWEN_35_9B)
    payload = mlx_bootstrap.get_state(QWEN_35_9B)

    assert payload["install_state"] == "failed"
    assert "model.safetensors" in payload["install_error"]
    assert (snapshot_dir / "model.safetensors").is_file()


def test_worker_exception_sets_failed_message(monkeypatch):
    _set_state(state="downloading", thread=threading.current_thread())
    monkeypatch.setattr(
        mlx_bootstrap.huggingface_hub,
        "snapshot_download",
        lambda **_kwargs: (_ for _ in ()).throw(RuntimeError("download broke")),
    )

    mlx_bootstrap._run_bootstrap_worker(QWEN_35_9B)

    payload = mlx_bootstrap.get_state(QWEN_35_9B)
    assert payload["install_state"] == "failed"
    assert "download broke" in payload["install_error"]


def test_mlx_worker_cleans_registered_thread_after_failure(monkeypatch):
    current = threading.current_thread()
    _set_state(state="downloading", thread=current)
    monkeypatch.setattr(
        mlx_bootstrap.huggingface_hub,
        "snapshot_download",
        lambda **_kwargs: (_ for _ in ()).throw(RuntimeError("download broke")),
    )

    mlx_bootstrap._run_bootstrap_worker(QWEN_35_9B)

    with mlx_bootstrap._INSTALL_LOCK:
        thread = mlx_bootstrap._INSTALL_THREADS.get(QWEN_35_9B)
    assert thread is None or not thread.is_alive()
    payload = mlx_bootstrap.get_state(QWEN_35_9B)
    assert payload["install_state"] == "failed"


def test_mlx_worker_cleans_registered_thread(monkeypatch):
    current = threading.current_thread()
    with mlx_bootstrap._INSTALL_LOCK:
        mlx_bootstrap._INSTALL_THREADS[QWEN_35_9B] = current
    _set_state(state="downloading", thread=current)
    monkeypatch.setattr(
        mlx_bootstrap.huggingface_hub, "snapshot_download", lambda **_kwargs: None
    )
    monkeypatch.setattr(
        mlx_bootstrap, "_verify_safetensors_sha256_hashes", lambda _model: None
    )

    mlx_bootstrap._run_bootstrap_worker(QWEN_35_9B)

    with mlx_bootstrap._INSTALL_LOCK:
        assert QWEN_35_9B not in mlx_bootstrap._INSTALL_THREADS


def test_pyproject_declares_huggingface_hub_top_level_dependency():
    pyproject_path = Path(__file__).resolve().parents[4] / "pyproject.toml"
    data = tomllib.loads(pyproject_path.read_text(encoding="utf-8"))
    deps = data["project"]["dependencies"]
    matches = [dep for dep in deps if dep.startswith("huggingface-hub")]

    assert matches
    assert all(";" not in dep for dep in matches)
