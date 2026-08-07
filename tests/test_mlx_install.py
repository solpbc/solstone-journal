# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Public Python transport contract for native MLX installation."""

from __future__ import annotations

import json
import sys
import threading
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.think.providers import mlx_install


MODEL = mlx_install.QWEN_35_9B
PINS = {"mlx_models": [{"name": MODEL, "repo": "mlx-community/example", "revision": "revision", "size_bytes": 10}]}


def test_mlx_model_spec_is_reconstructed_from_native_pins(monkeypatch):
    monkeypatch.setattr(mlx_install, "_pins", lambda: PINS)
    spec = mlx_install.resolve_model_spec()
    assert spec == mlx_install.MLXModelSpec(MODEL, "mlx-community/example", "revision", 10)


def test_platform_and_package_checks_cover_available_and_import_error(monkeypatch):
    monkeypatch.setattr(mlx_install.platform, "system", lambda: "Darwin")
    monkeypatch.setattr(mlx_install, "_pins", lambda: PINS)
    monkeypatch.setattr(mlx_install.importlib, "import_module", lambda _name: object())
    assert mlx_install.is_mlx_platform_supported()
    assert mlx_install.check_platform_and_package() == (True, "ready")
    monkeypatch.setattr(mlx_install.importlib, "import_module", lambda _name: (_ for _ in ()).throw(ImportError()))
    assert mlx_install.check_platform_and_package() == (False, "package_unavailable")


def test_platform_unsupported_does_not_import_package(monkeypatch):
    monkeypatch.setattr(mlx_install.platform, "system", lambda: "Linux")
    monkeypatch.setattr(mlx_install, "_pins", lambda: PINS)
    assert mlx_install.check_platform_and_package() == (False, "platform_unsupported")


def test_target_fingerprint_parses_native_json(monkeypatch):
    calls = []
    monkeypatch.setattr(mlx_install, "_pins", lambda: PINS)
    monkeypatch.setattr(mlx_install, "get_journal", lambda: "/journal")
    monkeypatch.setattr(mlx_install, "_core_install_call", lambda verb, request: calls.append((verb, request)) or {"target_fingerprint_json": '{"provider":"local","runtime":"mlx"}'})
    assert mlx_install.target_fingerprint() == {"provider": "local", "runtime": "mlx"}
    assert calls == [(["fingerprint", "mlx"], {"journal": "/journal", "model_id": MODEL})]


@pytest.mark.parametrize("status, ready", [("ready", True), ("missing-or-mismatched", False)])
def test_inspect_artifacts_and_readiness_reconstruct_native_response(monkeypatch, status, ready):
    response = {"provider": "local", "ready": ready, "status": status, "reason_code": "ready" if ready else "manifest_missing", "target": {"model_id": MODEL}, "host": {"platform_supported": True, "package_available": True}, "artifacts": {"model_id": MODEL, "model_installed": ready, "snapshot_installed": ready}, "proof": {"snapshot": {}}}
    monkeypatch.setattr(mlx_install, "_pins", lambda: PINS)
    monkeypatch.setattr(mlx_install, "check_platform_and_package", lambda: (True, "ready"))
    monkeypatch.setattr(mlx_install, "is_mlx_platform_supported", lambda: True)
    monkeypatch.setattr(mlx_install, "_core_install_call", lambda _verb, _request: response)
    outcome = mlx_install.inspect_readiness()
    assert outcome.ready is ready and mlx_install.inspect_artifacts()["model_installed"] is ready


def test_install_local_mlx_fetches_metadata_and_passes_real_lfs_hashes(monkeypatch, tmp_path):
    spec = mlx_install.MLXModelSpec(MODEL, "mlx-community/example", "revision", 10)
    snapshot = tmp_path / "snapshot"
    snapshot.mkdir()
    module = SimpleNamespace(snapshot_download=lambda **_kwargs: str(snapshot))
    monkeypatch.setitem(sys.modules, "huggingface_hub", module)
    monkeypatch.setattr(mlx_install, "resolve_model_spec", lambda _model: spec)
    monkeypatch.setattr(mlx_install, "validate_snapshot_sha256", lambda _spec, _snapshot: {"model-00001.safetensors": ("a" * 64, 10)})
    monkeypatch.setattr(mlx_install, "get_journal", lambda: "/journal")
    calls = []
    monkeypatch.setattr(mlx_install, "_core_install_call", lambda verb, request: calls.append((verb, request)) or {"status": {"install_state": "installed"}})
    assert mlx_install.install_local_mlx(owner={"entry": "test"}) == {"install_state": "installed"}
    assert calls[0] == (["run", "mlx"], {"journal": "/journal", "model_id": MODEL, "owner": {"entry": "test"}, "source_snapshot": str(snapshot), "lfs_sha256": {"model-00001.safetensors": "a" * 64}})


def test_install_local_mlx_maps_busy_and_failure(monkeypatch):
    monkeypatch.setattr(mlx_install, "_run_request", lambda *_args: {})
    monkeypatch.setattr(mlx_install, "_core_install_call", lambda *_args: (_ for _ in ()).throw(mlx_install.MLXInstallBusyError("busy")))
    with pytest.raises(mlx_install.MLXInstallBusyError):
        mlx_install.install_local_mlx()
    monkeypatch.setattr(mlx_install, "_core_install_call", lambda *_args: (_ for _ in ()).throw(mlx_install.MLXInstallUnavailableError("failed")))
    with pytest.raises(mlx_install.MLXInstallUnavailableError, match="failed"):
        mlx_install.install_local_mlx()


def test_spawn_install_local_mlx_returns_before_snapshot_fetch_finishes(monkeypatch):
    process = object()
    calls = []
    started = threading.Event()
    release = threading.Event()

    def slow_request(model, owner):
        started.set()
        assert release.wait(timeout=1)
        return {"model_id": model, "owner": owner}

    monkeypatch.setattr(mlx_install, "_run_request", slow_request)
    monkeypatch.setattr(mlx_install, "_spawn_core_install", lambda verb, request: calls.append((verb, request)) or process)
    handle = mlx_install.spawn_install_local_mlx(owner={"entry": "ui"})
    assert started.wait(timeout=1)
    assert handle.pending is True
    assert calls == []
    release.set()
    handle.wait(timeout=1)
    assert calls == [(["run", "mlx"], {"model_id": MODEL, "owner": {"entry": "ui"}})]


def test_remote_metadata_requires_all_requested_lfs_hashes(monkeypatch):
    class File:
        path = "one.safetensors"
        lfs = SimpleNamespace(sha256="a" * 64, size=1)

    hub = SimpleNamespace(RepoFile=File, HfApi=lambda: SimpleNamespace(list_repo_tree=lambda **_kwargs: [File()]))
    monkeypatch.setitem(sys.modules, "huggingface_hub", hub)
    spec = mlx_install.MLXModelSpec(MODEL, "repo", "rev", 1)
    assert mlx_install._remote_safetensors_metadata(spec, ["one.safetensors"]) == {"one.safetensors": ("a" * 64, 1)}
    with pytest.raises(mlx_install.MLXVerificationError):
        mlx_install._remote_safetensors_metadata(spec, ["missing.safetensors"])
