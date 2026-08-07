# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Public Python transport contract for the native local installer."""

from __future__ import annotations

import json
import importlib
from pathlib import Path

import pytest

from solstone.think.providers import local_install
from solstone.think.providers.local import LocalProviderError
from solstone.think.providers.local_cuda import ArtifactTrust


KEY = "x86_64-unknown-linux-gnu"
JOURNAL = "/journal"


def _pins() -> dict:
    return {
        "llama_server_pins": [{"artifact_key": KEY, "release_tag": "b1", "filename": "llama.tar.gz", "sha256": "v" * 64, "binary_name": "llama-server"}],
        "cuda_server_pin": {
            "cuda_version": 13, "embedded_arch_set": ["sm_89", "sm_120a"], "binary_name": "llama-server", "device_flag_value": "CUDA0", "visible_devices_env": "CUDA_VISIBLE_DEVICES",
            "shared_wanted_files": ["llama-server", "libggml-cuda.so"], "cpu_wanted_files_by_arch": {"amd64": ["libggml-cpu-x64.so"], "arm64": ["libggml-cpu-arm.so"]},
            "artifacts": [{"artifact_key": KEY, "url": "https://example.test/cuda.tar.gz", "sha256": "c" * 64, "size_bytes": 12, "release_tag": "b1", "upstream_image_digest": "sha256:upstream", "llama_cpp_revision": "revision", "repack_revision": "sol1"}],
        },
        "mlx_models": [], "mlx_soft_token_budget": 1120,
    }


def _paths() -> dict:
    return {"artifact_key": KEY, "cache_root": f"{JOURNAL}/cache/providers/local", "binary_path": f"{JOURNAL}/cache/providers/local/bin/{KEY}/b1/llama-server", "cuda_binary_path": f"{JOURNAL}/cache/providers/local/cuda/{KEY}/digest/llama-server", "model_dir": f"{JOURNAL}/cache/providers/local/models/local__qwen3.5-4b"}


def _patch_transport(monkeypatch, calls: list[tuple[list[str], dict]]) -> None:
    def call(verb: list[str], request: dict) -> dict:
        calls.append((verb, request))
        if verb == ["pins", "local"]:
            return _pins()
        if verb == ["paths", "local"]:
            return _paths()
        raise AssertionError((verb, request))

    monkeypatch.setattr(local_install, "_core_install_call", call)
    monkeypatch.setattr(local_install, "get_journal", lambda: JOURNAL)


def test_pin_dataclasses_and_platform_resolution_reconstruct_native_pins(monkeypatch):
    calls: list[tuple[list[str], dict]] = []
    _patch_transport(monkeypatch, calls)
    pin = local_install.cuda_server_pin()
    assert isinstance(pin, local_install.CudaServerPin)
    assert pin.wanted_files_for_arch("amd64") == ("llama-server", "libggml-cuda.so", "libggml-cpu-x64.so")
    assert isinstance(pin.artifacts_by_key[KEY], local_install.CudaArtifactPin)
    assert local_install.llama_server_artifact_key() == KEY
    assert local_install.pin_for_current_platform() == {"release_tag": "b1", "filename": "llama.tar.gz", "sha256": "v" * 64, "binary_name": "llama-server"}
    assert local_install.cuda_artifact_pin_for_current_platform(pin) == pin.artifacts_by_key[KEY]


def test_required_cuda_pin_rejects_absent_platform(monkeypatch):
    pin = local_install.CudaServerPin(13, frozenset(), "llama-server", "CUDA0", "CUDA_VISIBLE_DEVICES", (), {}, {})
    monkeypatch.setattr(local_install, "llama_server_artifact_key", lambda: "unsupported")
    assert local_install.cuda_artifact_pin_for_current_platform(pin) is None
    with pytest.raises(LocalProviderError, match="No pinned CUDA"):
        local_install.require_cuda_artifact_pin_for_current_platform(pin)


def test_paths_wrappers_preserve_native_cache_layout(monkeypatch):
    # These functions share the same `paths local` response; asserting them together
    # proves their intentionally common transport input and distinct derived fields.
    calls: list[tuple[list[str], dict]] = []
    _patch_transport(monkeypatch, calls)
    spec = local_install.LOCAL_MODEL_SPECS[local_install.LOCAL_MODEL]
    assert local_install.cache_root() == Path(_paths()["cache_root"])
    assert local_install.binary_install_dir() == Path(_paths()["binary_path"]).parent
    assert local_install.binary_path_for_pin() == Path(_paths()["binary_path"])
    assert local_install.cuda_binary_dir() == Path(_paths()["cuda_binary_path"]).parent
    assert local_install.cuda_binary_path() == Path(_paths()["cuda_binary_path"])
    assert local_install.model_dir(spec.model_id) == Path(_paths()["model_dir"])
    assert local_install.model_path(spec.model_id).name == spec.filename
    assert local_install.mmproj_path(spec.model_id) == (Path(_paths()["model_dir"]) / spec.mmproj_filename if spec.mmproj_filename else None)
    assert all(verb == ["paths", "local"] for verb, _request in calls)


def test_cuda_trust_uses_declared_arch_set(monkeypatch):
    # The suite-wide Vulkan fixture replaces this probe; reload to exercise the
    # public transport wrapper itself rather than that host-selection shortcut.
    importlib.reload(local_install)
    calls: list[tuple[list[str], dict]] = []
    _patch_transport(monkeypatch, calls)
    monkeypatch.setattr(local_install, "cuda_binary_path", lambda: Path("/runtime/llama-server"))
    monkeypatch.setattr(local_install, "_core_install_call", lambda verb, request: calls.append((verb, request)) or ({"trust": "trusted"} if verb == ["cuda", "trust"] else (_paths() if verb == ["paths", "local"] else _pins())))
    assert local_install.probe_cuda_runtime_artifact_trust(local_install.cuda_server_pin()) is ArtifactTrust.TRUSTED
    assert calls[-1] == (["cuda", "trust"], {"artifact_path": "/runtime/llama-server", "declared_arch_set": ["sm_120a", "sm_89"]})


def test_target_fingerprint_parses_native_canonical_json(monkeypatch):
    calls = []
    monkeypatch.setattr(local_install, "get_journal", lambda: JOURNAL)
    monkeypatch.setattr(local_install, "_core_install_call", lambda verb, request: calls.append((verb, request)) or {"target_fingerprint_json": '{"backend":"vulkan","provider":"local"}'})
    assert local_install.target_fingerprint() == {"backend": "vulkan", "provider": "local"}
    assert calls == [(["fingerprint", "local"], {"journal": JOURNAL, "model_id": local_install.LOCAL_MODEL})]


def test_write_vulkan_manifest_builds_migration_request(monkeypatch, tmp_path):
    calls = []
    monkeypatch.setattr(local_install, "binary_install_dir", lambda *_args: tmp_path)
    monkeypatch.setattr(local_install, "_target_sha", lambda *_args: "target")
    monkeypatch.setattr(local_install, "_core_install_call", lambda verb, request: calls.append((verb, request)) or {})
    pin = {"release_tag": "b1", "filename": "llama.tar.gz", "sha256": "v" * 64, "binary_name": "llama-server"}
    local_install.write_vulkan_manifest(artifact_key=KEY, pin=pin, attempt_status={"attempt_id": "attempt"})
    assert calls == [(["manifest", "vulkan"], {"root": str(tmp_path), "manifest_path": str(tmp_path / ".solstone-provider-manifest.json"), "target_fingerprint_sha256": "target", "attempt_id": "attempt", "pin_identity": {"unit": "llama-server-vulkan", "artifact_key": KEY, **pin}, "exclude_names": ["llama.tar.gz"]})]


def test_write_cuda_manifest_builds_wanted_file_identity(monkeypatch, tmp_path):
    calls = []
    artifact = local_install.CudaArtifactPin("https://example.test/cuda", "c" * 64, 12, "b1", "sha256:upstream", "revision", "sol1")
    monkeypatch.setattr(local_install, "cuda_binary_dir", lambda: tmp_path)
    monkeypatch.setattr(local_install, "cuda_server_pin", lambda: local_install.CudaServerPin(13, frozenset(), "llama-server", "CUDA0", "CUDA_VISIBLE_DEVICES", (), {}, {}))
    monkeypatch.setattr(local_install, "_target_sha", lambda *_args: "target")
    monkeypatch.setattr(local_install, "_core_install_call", lambda verb, request: calls.append((verb, request)) or {})
    local_install.write_cuda_manifest(artifact_key=KEY, artifact_pin=artifact, arch="amd64", wanted_files=("llama-server",), attempt_status=None)
    request = calls[0][1]
    assert calls[0][0] == ["manifest", "cuda"]
    assert request["attempt_id"] is None and request["pin_identity"]["wanted_files"] == ["llama-server"]
    assert request["pin_identity"]["url"] == artifact.url


def test_write_model_manifest_builds_model_identity(monkeypatch, tmp_path):
    calls = []
    monkeypatch.setattr(local_install, "model_dir", lambda _model: tmp_path)
    monkeypatch.setattr(local_install, "_target_sha", lambda *_args: "target")
    monkeypatch.setattr(local_install, "_core_install_call", lambda verb, request: calls.append((verb, request)) or {})
    local_install.write_model_manifest(model_id=local_install.LOCAL_MODEL, attempt_status={"attempt_id": "attempt"})
    assert calls[0][0] == ["manifest", "model"]
    assert calls[0][1]["pin_identity"] == local_install._model_pin_identity(local_install.LOCAL_MODEL)
    assert calls[0][1]["attempt_id"] == "attempt"


def test_verify_sha_and_probe_binary_parse_native_results(monkeypatch, tmp_path):
    calls = []
    responses = iter([{"verified": True}, {"runnable": False, "reason_code": "binary_exit", "exit_code": 1}])
    monkeypatch.setattr(local_install, "_core_install_call", lambda verb, request: calls.append((verb, request)) or next(responses))
    path = tmp_path / "llama-server"
    local_install.verify_artifact_sha256(path, "a" * 64)
    assert local_install.probe_binary_runnable(path) == (False, "binary_exit")
    assert calls[0] == (["verify", "sha256"], {"path": str(path), "sha256": "a" * 64})


def test_install_local_maps_success_busy_and_native_failure(monkeypatch):
    monkeypatch.setattr(local_install, "get_journal", lambda: JOURNAL)
    monkeypatch.setattr(local_install, "_core_install_call", lambda *_args: {"status": {"install_state": "installed"}})
    assert local_install.install_local(owner={"entry": "test"}) == {"install_state": "installed"}
    monkeypatch.setattr(local_install, "_core_install_call", lambda *_args: (_ for _ in ()).throw(local_install.LocalInstallBusyError("install_busy", "busy")))
    with pytest.raises(local_install.LocalInstallBusyError):
        local_install.install_local()
    monkeypatch.setattr(local_install, "_core_install_call", lambda *_args: (_ for _ in ()).throw(LocalProviderError("download_failed", "nope")))
    with pytest.raises(LocalProviderError, match="nope") as error:
        local_install.install_local()
    assert error.value.reason_code == "download_failed"


def test_spawn_install_local_builds_request_without_waiting(monkeypatch):
    process = object()
    calls = []
    monkeypatch.setattr(local_install, "get_journal", lambda: JOURNAL)
    monkeypatch.setattr(local_install, "_spawn_core_install", lambda verb, request: calls.append((verb, request)) or process)
    assert local_install.spawn_install_local(owner={"entry": "ui"}) is process
    assert calls == [(["run", "local"], {"journal": JOURNAL, "model_id": local_install.LOCAL_MODEL, "owner": {"entry": "ui"}})]


@pytest.mark.parametrize("status, ready", [("ready", True), ("missing-or-mismatched", False)])
def test_inspect_readiness_reconstructs_native_outcome(monkeypatch, status, ready):
    response = {"provider": "local", "ready": ready, "status": status, "reason_code": "ready" if ready else "manifest_missing", "target": {"model_id": local_install.LOCAL_MODEL}, "install": {"install_state": "idle"}, "host": {"backend": "vulkan", "backend_reason": "no NVIDIA GPU detected", "platform_supported": True}, "artifacts": {"binary_installed": ready, "model_installed": ready, "binary_path": "/bin/llama", "model_path": "/model.gguf"}, "proof": {"binary": {}}}
    monkeypatch.setattr(local_install, "get_journal", lambda: JOURNAL)
    monkeypatch.setattr(local_install, "_vulkan_observation", lambda: {"gpu_probe_ok": True})
    monkeypatch.setattr(local_install, "_core_install_call", lambda verb, request: response)
    outcome = local_install.inspect_readiness()
    assert outcome.ready is ready and outcome.status == status
    # Supervisor uses direct indexing for both of these native-owned fields.
    assert outcome.host["backend_reason"] == "no NVIDIA GPU detected"
    assert outcome.host["platform_supported"] is True
    assert local_install.inspect_artifacts()["binary_installed"] is ready


def test_ensure_artifacts_and_persisted_cuda_status(monkeypatch):
    ready = local_install._readiness({"provider": "local", "status": "ready", "reason_code": "ready", "target": {}, "install": {}, "host": {"backend": "cuda", "backend_reason": "native"}, "artifacts": {"binary_installed": True, "model_installed": True, "binary_path": "/bin/llama", "model_path": "/model.gguf"}, "proof": {}})
    monkeypatch.setattr(local_install, "inspect_readiness", lambda _model: ready)
    monkeypatch.setattr(local_install, "cuda_binary_dir", lambda: Path("/cuda"))
    monkeypatch.setattr(local_install, "mmproj_path", lambda _model: None)
    monkeypatch.setattr(local_install, "model_path", lambda _model: Path("/model.gguf"))
    assert isinstance(local_install.ensure_artifacts_installed(local_install.LOCAL_MODEL), local_install.LocalArtifacts)
    importlib.reload(local_install)
    monkeypatch.setattr(local_install, "read_install_status", lambda **_kwargs: {"install_state": "installed", "target_fingerprint_json": '{"provider":"local","backend":"cuda"}'})
    monkeypatch.setattr(local_install, "_core_install_call", lambda *_args: (_ for _ in ()).throw(AssertionError("must not shell out")))
    assert local_install.has_persisted_installed_cuda_target()


def test_core_unavailable_and_exit_75_raise_without_python_fallback(monkeypatch):
    monkeypatch.setattr(local_install.core_handshake, "check_solstone_core_handshake", lambda: type("Handshake", (), {"status": "fail", "message": "missing binary"})())
    with pytest.raises(LocalProviderError, match="missing binary"):
        local_install._core_install_command(["run", "local"])
    envelope = json.dumps({"schema": "solstone-local-install-v1", "outcome": "error", "error": {"kind": "busy", "reason_code": "install_busy", "message": "held"}})
    with pytest.raises(local_install.LocalInstallBusyError):
        local_install._parse_core_envelope(envelope, 75)


def test_install_hint_and_gpu_override_remain_python_configuration_helpers(monkeypatch):
    monkeypatch.setattr(local_install, "read_journal_config", lambda: {"providers": {"local": {"vulkan_device_index": "2"}}})
    assert local_install.install_hint() == "journal install-provider local"
    assert local_install.gpu_device_override() == 2
