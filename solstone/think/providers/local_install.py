# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Transport wrappers for bundled local-provider installation.

The ``solstone-core local install`` commands own artifact resolution, download,
verification, manifests, and install-state mutations.  Python retains only the
typed compatibility surface used by the provider, migration, and UI layers.
"""

from __future__ import annotations

import json
import logging
import platform
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from solstone.think import core_handshake
from solstone.think.journal_config import read_journal_config
from solstone.think.models import LOCAL_MODEL
from solstone.think.providers.artifact_proof import ReadinessOutcome, artifact_manifest_path
from solstone.think.providers.install_state import InstallStatusMalformedError, read_install_status
from solstone.think.providers.local import LOCAL_MODEL_SPECS, LocalProviderError, normalize_model_id
from solstone.think.providers.local_cuda import ArtifactTrust
from solstone.think.utils import get_journal

LOG = logging.getLogger(__name__)
LOCAL_PROVIDER_NAME = "local"
_CORE_SCHEMA = "solstone-local-install-v1"
_BUSY_EXIT_CODE = 75


class LocalInstallBusyError(LocalProviderError):
    """The native installer refused because another local install owns the lease."""


@dataclass(frozen=True)
class CudaArtifactPin:
    url: str
    sha256: str
    size_bytes: int
    release_tag: str
    upstream_image_digest: str
    llama_cpp_revision: str
    repack_revision: str


@dataclass(frozen=True)
class CudaServerPin:
    cuda_version: int
    embedded_arch_set: frozenset[str]
    binary_name: str
    device_flag_value: str
    visible_devices_env: str
    shared_wanted_files: tuple[str, ...]
    cpu_wanted_files_by_arch: dict[str, tuple[str, ...]]
    artifacts_by_key: dict[str, CudaArtifactPin]

    def wanted_files_for_arch(self, arch: str) -> tuple[str, ...]:
        wanted = self.cpu_wanted_files_by_arch.get(arch)
        if wanted is None:
            raise LocalProviderError("unsupported_platform", f"No CUDA wanted-files set for runtime architecture {arch}")
        return self.shared_wanted_files + wanted


@dataclass(frozen=True)
class LocalArtifacts:
    backend: str
    backend_reason: str
    binary_path: Path
    lib_dir: Path | None
    gguf_path: Path
    mmproj_path: Path | None


def _core_install_command(verb_words: list[str]) -> list[str]:
    handshake = core_handshake.check_solstone_core_handshake()
    if handshake.status != "ok":
        raise LocalProviderError("core_unavailable", handshake.message or "solstone-core is unavailable")
    return [str(core_handshake.helper_path_for_executable()), "local", "install", *verb_words]


def _install_error(envelope: dict[str, Any], returncode: int) -> LocalProviderError:
    error = envelope.get("error") if isinstance(envelope.get("error"), dict) else {}
    reason = str(error.get("reason_code") or "native_install_failed")
    message = str(error.get("message") or "solstone-core local install failed")
    if returncode == _BUSY_EXIT_CODE or reason == "install_busy":
        return LocalInstallBusyError(reason, message)
    return LocalProviderError(reason, message)


def _parse_core_envelope(stdout: str, returncode: int) -> dict[str, Any]:
    try:
        envelope = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise LocalProviderError("native_protocol_error", "solstone-core returned invalid install JSON") from exc
    if not isinstance(envelope, dict) or envelope.get("schema") != _CORE_SCHEMA:
        raise LocalProviderError("native_protocol_error", "solstone-core returned an unknown install response")
    if envelope.get("outcome") == "ok" and returncode == 0:
        result = envelope.get("result")
        if not isinstance(result, dict):
            raise LocalProviderError("native_protocol_error", "solstone-core install result is not an object")
        return result
    raise _install_error(envelope, returncode)


def _core_install_call(verb_words: list[str], request: dict[str, Any]) -> dict[str, Any]:
    try:
        completed = subprocess.run(
            _core_install_command(verb_words), input=json.dumps(request), text=True,
            capture_output=True, check=False,
        )
    except OSError as exc:
        raise LocalProviderError("core_launch_failed", f"solstone-core local install failed to launch: {exc}") from exc
    return _parse_core_envelope(completed.stdout, completed.returncode)


def _spawn_core_install(verb_words: list[str], request: dict[str, Any]) -> subprocess.Popen[str]:
    try:
        process = subprocess.Popen(
            _core_install_command(verb_words), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True,
        )
        assert process.stdin is not None
        process.stdin.write(json.dumps(request))
        process.stdin.close()
        return process
    except OSError as exc:
        raise LocalProviderError("core_launch_failed", f"solstone-core local install failed to launch: {exc}") from exc


def _pins() -> dict[str, Any]:
    return _core_install_call(["pins", "local"], {})


def _paths(*, artifact_key: str | None = None, model_id: str | None = None) -> dict[str, Any]:
    request: dict[str, Any] = {"journal": str(get_journal())}
    if artifact_key is not None:
        request["artifact_key"] = artifact_key
    if model_id is not None:
        request["model_id"] = model_id
    return _core_install_call(["paths", "local"], request)


def llama_server_artifact_key() -> str:
    return str(_paths()["artifact_key"])


def pin_for_current_platform() -> dict[str, str]:
    key = llama_server_artifact_key()
    for pin in _pins()["llama_server_pins"]:
        if pin["artifact_key"] == key:
            return {name: str(pin[name]) for name in ("release_tag", "filename", "sha256", "binary_name")}
    raise LocalProviderError("unsupported_platform", f"No pinned llama-server artifact for platform {key}")


def cuda_server_pin() -> CudaServerPin:
    value = _pins()["cuda_server_pin"]
    artifacts = {
        str(item["artifact_key"]): CudaArtifactPin(
            url=str(item["url"]), sha256=str(item["sha256"]), size_bytes=int(item["size_bytes"]),
            release_tag=str(item["release_tag"]), upstream_image_digest=str(item["upstream_image_digest"]),
            llama_cpp_revision=str(item["llama_cpp_revision"]), repack_revision=str(item["repack_revision"]),
        )
        for item in value["artifacts"]
    }
    return CudaServerPin(
        cuda_version=int(value["cuda_version"]), embedded_arch_set=frozenset(value["embedded_arch_set"]),
        binary_name=str(value["binary_name"]), device_flag_value=str(value["device_flag_value"]),
        visible_devices_env=str(value["visible_devices_env"]), shared_wanted_files=tuple(value["shared_wanted_files"]),
        cpu_wanted_files_by_arch={key: tuple(items) for key, items in value["cpu_wanted_files_by_arch"].items()},
        artifacts_by_key=artifacts,
    )


def cuda_artifact_pin_for_current_platform(pin: CudaServerPin | None = None) -> CudaArtifactPin | None:
    return (pin or cuda_server_pin()).artifacts_by_key.get(llama_server_artifact_key())


def require_cuda_artifact_pin_for_current_platform(pin: CudaServerPin | None = None) -> CudaArtifactPin:
    artifact = cuda_artifact_pin_for_current_platform(pin)
    if artifact is None:
        raise LocalProviderError("unsupported_platform", f"No pinned CUDA llama-server artifact for platform {llama_server_artifact_key()}")
    return artifact


def cache_root() -> Path:
    return Path(str(_paths()["cache_root"]))


def binary_install_dir(artifact_key: str | None = None, pin: dict[str, str] | None = None) -> Path:
    del pin
    return Path(str(_paths(artifact_key=artifact_key)["binary_path"])).parent


def binary_path_for_pin(artifact_key: str | None = None, pin: dict[str, str] | None = None) -> Path:
    del pin
    return Path(str(_paths(artifact_key=artifact_key)["binary_path"]))


def cuda_binary_dir() -> Path:
    return Path(str(_paths()["cuda_binary_path"])).parent


def cuda_binary_path() -> Path:
    return Path(str(_paths()["cuda_binary_path"]))


def model_dir(model_id: str) -> Path:
    return Path(str(_paths(model_id=normalize_model_id(model_id))["model_dir"]))


def model_path(model_id: str) -> Path:
    spec = LOCAL_MODEL_SPECS[normalize_model_id(model_id)]
    return model_dir(spec.model_id) / spec.filename


def mmproj_path(model_id: str) -> Path | None:
    spec = LOCAL_MODEL_SPECS[normalize_model_id(model_id)]
    return model_dir(spec.model_id) / spec.mmproj_filename if spec.mmproj_filename else None


def install_hint() -> str:
    return "journal install-provider local"


def gpu_device_override() -> int | None:
    value = read_journal_config().get("providers", {}).get(LOCAL_PROVIDER_NAME, {})
    if not isinstance(value, dict):
        return None
    try:
        index = int(value.get("vulkan_device_index"))
    except (TypeError, ValueError):
        return None
    return index if index >= 0 else None


def _cuda_runtime_arch() -> str:
    machine = platform.machine().lower()
    if machine in {"x86_64", "amd64", "x64"}:
        return "amd64"
    if machine in {"aarch64", "arm64"}:
        return "arm64"
    raise LocalProviderError("unsupported_platform", f"No CUDA runtime architecture mapping for machine {machine}")


def probe_cuda_runtime_artifact_trust(pin: CudaServerPin, *, journal_path: str | Path | None = None) -> ArtifactTrust:
    del journal_path
    artifact = cuda_artifact_pin_for_current_platform(pin)
    if artifact is None:
        return ArtifactTrust.ABSENT
    result = _core_install_call(["cuda", "trust"], {
        "artifact_path": str(cuda_binary_path()), "declared_arch_set": sorted(pin.embedded_arch_set),
    })
    return ArtifactTrust(str(result["trust"]))


def has_persisted_installed_cuda_target(*, journal_path: str | Path | None = None) -> bool:
    try:
        status = read_install_status(name=LOCAL_PROVIDER_NAME, journal_path=journal_path)
        target = json.loads(status["target_fingerprint_json"] or "null")
    except (InstallStatusMalformedError, ValueError):
        LOG.warning("could not read persisted CUDA install target", exc_info=True)
        return False
    return status["install_state"] == "installed" and isinstance(target, dict) and target.get("backend") == "cuda"


def target_fingerprint(model_id: str = LOCAL_MODEL) -> dict[str, Any]:
    result = _core_install_call(["fingerprint", "local"], {"journal": str(get_journal()), "model_id": normalize_model_id(model_id)})
    return json.loads(str(result["target_fingerprint_json"]))


def _target_sha(model_id: str, attempt_status: dict[str, Any] | None, fingerprint: dict[str, Any] | None) -> str:
    if attempt_status and attempt_status.get("target_fingerprint_sha256"):
        return str(attempt_status["target_fingerprint_sha256"])
    if fingerprint is not None:
        # The native endpoint is authoritative for the digest, including canonicalization.
        return str(_core_install_call(["fingerprint", "local"], {"journal": str(get_journal()), "model_id": model_id})["target_fingerprint_sha256"])
    return str(_core_install_call(["fingerprint", "local"], {"journal": str(get_journal()), "model_id": model_id})["target_fingerprint_sha256"])


def _model_pin_identity(model_id: str) -> dict[str, Any]:
    spec = LOCAL_MODEL_SPECS[normalize_model_id(model_id)]
    return {"unit": "local-model", "model_id": spec.model_id, "repo": spec.repo, "revision": spec.revision, "filename": spec.filename, "sha256": spec.sha256, "mmproj_filename": spec.mmproj_filename, "mmproj_sha256": spec.mmproj_sha256}


def write_vulkan_manifest(*, artifact_key: str, pin: dict[str, str], attempt_status: dict[str, Any] | None, fingerprint: dict[str, Any] | None = None, root: Path | None = None) -> None:
    install_root = root or binary_install_dir(artifact_key, pin)
    _core_install_call(["manifest", "vulkan"], {
        "root": str(install_root), "manifest_path": str(artifact_manifest_path(install_root)),
        "target_fingerprint_sha256": _target_sha(LOCAL_MODEL, attempt_status, fingerprint),
        "attempt_id": attempt_status.get("attempt_id") if attempt_status else None,
        "pin_identity": {"unit": "llama-server-vulkan", "artifact_key": artifact_key, **pin},
        "exclude_names": [pin["filename"]],
    })


def write_cuda_manifest(*, artifact_key: str, artifact_pin: CudaArtifactPin, arch: str, wanted_files: tuple[str, ...], attempt_status: dict[str, Any] | None, fingerprint: dict[str, Any] | None = None, root: Path | None = None) -> None:
    install_root = root or cuda_binary_dir()
    pin = cuda_server_pin()
    _core_install_call(["manifest", "cuda"], {
        "root": str(install_root), "manifest_path": str(artifact_manifest_path(install_root)),
        "target_fingerprint_sha256": _target_sha(LOCAL_MODEL, attempt_status, fingerprint),
        "attempt_id": attempt_status.get("attempt_id") if attempt_status else None,
        "pin_identity": {"unit": "llama-server-cuda", "artifact_key": artifact_key, "url": artifact_pin.url, "sha256": artifact_pin.sha256, "size_bytes": artifact_pin.size_bytes, "release_tag": artifact_pin.release_tag, "upstream_image_digest": artifact_pin.upstream_image_digest, "llama_cpp_revision": artifact_pin.llama_cpp_revision, "repack_revision": artifact_pin.repack_revision, "arch": arch, "binary_name": pin.binary_name, "wanted_files": list(wanted_files)},
    })


def write_model_manifest(*, model_id: str, attempt_status: dict[str, Any] | None, fingerprint: dict[str, Any] | None = None) -> None:
    root = model_dir(model_id)
    _core_install_call(["manifest", "model"], {
        "root": str(root), "manifest_path": str(artifact_manifest_path(root)),
        "target_fingerprint_sha256": _target_sha(model_id, attempt_status, fingerprint),
        "attempt_id": attempt_status.get("attempt_id") if attempt_status else None,
        "pin_identity": _model_pin_identity(model_id),
    })


def verify_artifact_sha256(path: Path, expected: str) -> None:
    _core_install_call(["verify", "sha256"], {"path": str(path), "sha256": expected})


def probe_binary_runnable(binary_path: str | Path) -> tuple[bool, str | None]:
    result = _core_install_call(["probe-binary"], {"path": str(binary_path)})
    return bool(result["runnable"]), result.get("reason_code") or result.get("message")


def install_local(model_id: str = LOCAL_MODEL, *, owner: dict[str, Any] | None = None, initial_state: str = "resolving") -> dict[str, Any]:
    del initial_state
    result = _core_install_call(["run", "local"], {"journal": str(get_journal()), "model_id": normalize_model_id(model_id), "owner": owner})
    return dict(result["status"])


def spawn_install_local(model_id: str = LOCAL_MODEL, *, owner: dict[str, Any] | None = None, initial_state: str = "downloading") -> subprocess.Popen[str]:
    del initial_state
    return _spawn_core_install(["run", "local"], {"journal": str(get_journal()), "model_id": normalize_model_id(model_id), "owner": owner})


def _vulkan_observation() -> dict[str, Any]:
    from solstone.think.providers import local_vulkan
    return {"gpu_probe_ok": local_vulkan.gpu_probe_ok(), "device_override": gpu_device_override()}


def _readiness(result: dict[str, Any]) -> ReadinessOutcome:
    return ReadinessOutcome(
        provider=str(result.get("provider", LOCAL_PROVIDER_NAME)), status=str(result["status"]), reason_code=str(result.get("reason_code") or "ready"),
        target=dict(result.get("target") or {}), install=dict(result.get("install") or {}),
        host=dict(result.get("host") or {}), artifacts=dict(result.get("artifacts") or {}), proof=dict(result.get("proof") or {}),
    )


def inspect_artifacts(model_id: str | None = None) -> dict[str, Any]:
    return dict(inspect_readiness(model_id).artifacts)


def inspect_readiness(model_id: str | None = None) -> ReadinessOutcome:
    result = _core_install_call(["inspect", "local"], {"journal": str(get_journal()), "model_id": normalize_model_id(model_id or LOCAL_MODEL), "vulkan_observation": _vulkan_observation()})
    return _readiness(result)


def ensure_artifacts_installed(model_id: str) -> LocalArtifacts:
    readiness = inspect_readiness(model_id)
    if not readiness.artifacts.get("binary_installed"):
        raise LocalProviderError("binary_missing", "Local runtime is not installed.")
    if not readiness.artifacts.get("model_installed"):
        raise LocalProviderError("model_missing", "Local model files are not installed.")
    backend = str(readiness.host.get("backend", "vulkan"))
    mmproj = mmproj_path(model_id)
    return LocalArtifacts(backend, str(readiness.host.get("backend_reason", "native")), Path(str(readiness.artifacts["binary_path"])), cuda_binary_dir() if backend == "cuda" else None, model_path(model_id), mmproj)


__all__ = [
    "CudaArtifactPin", "CudaServerPin", "LocalArtifacts", "LOCAL_PROVIDER_NAME", "LocalInstallBusyError",
    "binary_install_dir", "binary_path_for_pin", "cache_root", "cuda_artifact_pin_for_current_platform", "cuda_binary_dir", "cuda_binary_path", "cuda_server_pin", "ensure_artifacts_installed", "gpu_device_override", "has_persisted_installed_cuda_target", "install_hint", "install_local", "llama_server_artifact_key", "mmproj_path", "model_dir", "model_path", "pin_for_current_platform", "probe_binary_runnable", "probe_cuda_runtime_artifact_trust", "require_cuda_artifact_pin_for_current_platform", "spawn_install_local", "target_fingerprint", "verify_artifact_sha256", "write_cuda_manifest", "write_model_manifest", "write_vulkan_manifest",
]
