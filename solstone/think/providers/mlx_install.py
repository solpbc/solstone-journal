# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Python transport and Hugging Face acquisition seam for MLX installs."""

from __future__ import annotations

import importlib
import json
import platform
import subprocess
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from solstone.think import core_handshake
from solstone.think.models import GEMMA4_26B_A4B_4BIT, QWEN_35_9B
from solstone.think.providers.artifact_proof import ReadinessOutcome
from solstone.think.providers.local import LocalProviderError
from solstone.think.utils import get_journal

MLX_SOFT_TOKEN_BUDGET = 1120
_LOCAL_NAME = "local"
_CORE_SCHEMA = "solstone-local-install-v1"
_BUSY_EXIT_CODE = 75


@dataclass(frozen=True)
class MLXModelSpec:
    name: str
    repo: str
    revision: str
    size_bytes: int


class MLXInstallUnavailableError(RuntimeError):
    """Raised when the host cannot install or run the requested MLX model."""


class MLXVerificationError(RuntimeError):
    """Raised when published snapshot metadata is unavailable or inconsistent."""


class MLXInstallBusyError(MLXInstallUnavailableError):
    """The native installer refused because another local install owns the lease."""


class MlxInstallProcess:
    """Process-like handle for background snapshot acquisition followed by native install."""

    def __init__(self, model_id: str, owner: dict[str, Any] | None) -> None:
        self._cancelled = threading.Event()
        self._done = threading.Event()
        self._lock = threading.Lock()
        self._process: subprocess.Popen[str] | None = None
        self._returncode: int | None = None
        self.launch_error: Exception | None = None
        self._thread = threading.Thread(
            target=self._run,
            args=(model_id, owner),
            name=f"mlx-install-fetch-{model_id}",
            daemon=True,
        )
        self._thread.start()

    @property
    def pending(self) -> bool:
        with self._lock:
            return self._process is None and not self._done.is_set()

    @property
    def returncode(self) -> int | None:
        with self._lock:
            return self._returncode

    def _run(self, model_id: str, owner: dict[str, Any] | None) -> None:
        try:
            request = _run_request(model_id, owner)
            if self._cancelled.is_set():
                return
            process = _spawn_core_install(["run", "mlx"], request)
            with self._lock:
                self._process = process
            if self._cancelled.is_set():
                process.terminate()
            returncode = process.wait()
            with self._lock:
                self._returncode = returncode
        except Exception as exc:  # The bootstrap caller makes the durable launch failure visible.
            self.launch_error = exc
            with self._lock:
                self._returncode = 74
        finally:
            self._done.set()

    def poll(self) -> int | None:
        with self._lock:
            process = self._process
            returncode = self._returncode
        if process is None:
            return returncode
        result = process.poll()
        if result is not None:
            with self._lock:
                self._returncode = result
        return result

    def terminate(self) -> None:
        self._cancelled.set()
        with self._lock:
            process = self._process
        if process is not None and process.poll() is None:
            process.terminate()

    def kill(self) -> None:
        self._cancelled.set()
        with self._lock:
            process = self._process
        if process is not None and process.poll() is None:
            process.kill()

    def wait(self, timeout: float | None = None) -> int | None:
        self._thread.join(timeout)
        if self._thread.is_alive():
            raise subprocess.TimeoutExpired("mlx-install-fetch", timeout)
        return self.returncode


def _core_install_command(verb_words: list[str]) -> list[str]:
    handshake = core_handshake.check_solstone_core_handshake()
    if handshake.status != "ok":
        raise MLXInstallUnavailableError(handshake.message or "solstone-core is unavailable")
    return [str(core_handshake.helper_path_for_executable()), "local", "install", *verb_words]


def _parse_core_envelope(stdout: str, returncode: int) -> dict[str, Any]:
    try:
        envelope = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise MLXInstallUnavailableError("solstone-core returned invalid install JSON") from exc
    if not isinstance(envelope, dict) or envelope.get("schema") != _CORE_SCHEMA:
        raise MLXInstallUnavailableError("solstone-core returned an unknown install response")
    if envelope.get("outcome") == "ok" and returncode == 0 and isinstance(envelope.get("result"), dict):
        return dict(envelope["result"])
    error = envelope.get("error") if isinstance(envelope.get("error"), dict) else {}
    message = str(error.get("message") or "solstone-core local install failed")
    if returncode == _BUSY_EXIT_CODE or error.get("reason_code") == "install_busy":
        raise MLXInstallBusyError(message)
    raise MLXInstallUnavailableError(message)


def _core_install_call(verb_words: list[str], request: dict[str, Any]) -> dict[str, Any]:
    try:
        completed = subprocess.run(_core_install_command(verb_words), input=json.dumps(request), text=True, capture_output=True, check=False)
    except OSError as exc:
        raise MLXInstallUnavailableError(f"solstone-core local install failed to launch: {exc}") from exc
    return _parse_core_envelope(completed.stdout, completed.returncode)


def _spawn_core_install(verb_words: list[str], request: dict[str, Any]) -> subprocess.Popen[str]:
    try:
        process = subprocess.Popen(_core_install_command(verb_words), stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        assert process.stdin is not None
        process.stdin.write(json.dumps(request))
        process.stdin.close()
        return process
    except OSError as exc:
        raise MLXInstallUnavailableError(f"solstone-core local install failed to launch: {exc}") from exc


def _pins() -> dict[str, Any]:
    return _core_install_call(["pins", "local"], {})


def is_mlx_platform_supported() -> bool:
    return platform.system() == "Darwin" and bool(_pins().get("mlx_models"))


def check_platform_and_package() -> tuple[bool, str]:
    if not is_mlx_platform_supported():
        return False, "platform_unsupported"
    try:
        importlib.import_module("mlx_vlm")
    except ImportError:
        return False, "package_unavailable"
    return True, "ready"


def resolve_model_spec(model_id: str | None = None) -> MLXModelSpec:
    wanted = model_id or QWEN_35_9B
    for value in _pins()["mlx_models"]:
        if value["name"] == wanted:
            return MLXModelSpec(str(value["name"]), str(value["repo"]), str(value["revision"]), int(value["size_bytes"]))
    raise MLXInstallUnavailableError(f"No MLX model pin for {wanted}")


def target_fingerprint(model_id: str | None = None) -> dict[str, Any]:
    spec = resolve_model_spec(model_id)
    result = _core_install_call(["fingerprint", "mlx"], {"journal": str(get_journal()), "model_id": spec.name})
    return json.loads(str(result["target_fingerprint_json"]))


def snapshot_dir_for_spec(spec: MLXModelSpec) -> Path:
    return Path(get_journal()) / "cache" / "providers" / "local" / "mlx" / spec.repo.replace("/", "--") / spec.revision / "snapshot"


def variant_dir_for_snapshot(snapshot_dir: Path) -> Path:
    return snapshot_dir.parent / "variant-solstone-budget1120"


def _safetensors_paths(snapshot_dir: Path) -> list[str]:
    data = json.loads((snapshot_dir / "model.safetensors.index.json").read_text(encoding="utf-8"))
    weight_map = data.get("weight_map")
    if not isinstance(weight_map, dict):
        raise ValueError("model.safetensors.index.json missing weight_map")
    paths = sorted({str(path) for path in weight_map.values() if str(path)})
    if not paths:
        raise ValueError("model.safetensors.index.json has no safetensors paths")
    return paths


def _remote_safetensors_metadata(spec: MLXModelSpec, paths: list[str]) -> dict[str, tuple[str, int]]:
    import huggingface_hub

    wanted, found = set(paths), {}
    for entry in huggingface_hub.HfApi().list_repo_tree(repo_id=spec.repo, revision=spec.revision, repo_type="model", recursive=True):
        if isinstance(entry, huggingface_hub.RepoFile) and entry.path in wanted:
            if entry.lfs is None:
                raise MLXVerificationError(f"missing LFS sha256 for {entry.path}")
            found[entry.path] = (entry.lfs.sha256, int(entry.lfs.size))
    missing = sorted(wanted - set(found))
    if missing:
        raise MLXVerificationError(f"missing published sha256 for {missing[0]}")
    return found


def validate_snapshot_sha256(spec: MLXModelSpec, snapshot_dir: Path) -> dict[str, tuple[str, int]]:
    """Fetch expected LFS digests; native code compares the local bytes."""
    return _remote_safetensors_metadata(spec, _safetensors_paths(snapshot_dir))


def _readiness(result: dict[str, Any]) -> ReadinessOutcome:
    return ReadinessOutcome(
        provider=str(result.get("provider", _LOCAL_NAME)), status=str(result["status"]), reason_code=str(result.get("reason_code") or "ready"),
        target=dict(result.get("target") or {}), install=dict(result.get("install") or {}), host=dict(result.get("host") or {}),
        artifacts=dict(result.get("artifacts") or {}), proof=dict(result.get("proof") or {}),
    )


def inspect_artifacts(model_id: str | None = None) -> dict[str, Any]:
    return dict(inspect_readiness(model_id).artifacts)


def inspect_readiness(model_id: str | None = None) -> ReadinessOutcome:
    spec = resolve_model_spec(model_id)
    package_available, _reason = check_platform_and_package()
    result = _core_install_call(["inspect", "mlx"], {"journal": str(get_journal()), "model_id": spec.name, "mlx_vlm_importable": package_available, "platform_supported": is_mlx_platform_supported()})
    return _readiness(result)


def _run_request(model_id: str, owner: dict[str, Any] | None) -> dict[str, Any]:
    import huggingface_hub

    spec = resolve_model_spec(model_id)
    snapshot = Path(huggingface_hub.snapshot_download(repo_id=spec.repo, revision=spec.revision))
    metadata = validate_snapshot_sha256(spec, snapshot)
    return {"journal": str(get_journal()), "model_id": spec.name, "owner": owner, "source_snapshot": str(snapshot), "lfs_sha256": {path: digest for path, (digest, _size) in metadata.items()}}


def install_local_mlx(model_id: str = QWEN_35_9B, *, owner: dict[str, Any] | None = None, initial_state: str = "resolving") -> dict[str, Any]:
    del initial_state
    return dict(_core_install_call(["run", "mlx"], _run_request(model_id, owner))["status"])


def spawn_install_local_mlx(model_id: str = QWEN_35_9B, *, owner: dict[str, Any] | None = None, initial_state: str = "downloading") -> MlxInstallProcess:
    del initial_state
    return MlxInstallProcess(model_id, owner)


__all__ = [
    "GEMMA4_26B_A4B_4BIT", "MLXInstallBusyError", "MLXInstallUnavailableError", "MLXModelSpec", "MLXVerificationError", "MLX_SOFT_TOKEN_BUDGET", "MlxInstallProcess", "QWEN_35_9B", "check_platform_and_package", "inspect_artifacts", "inspect_readiness", "install_local_mlx", "is_mlx_platform_supported", "resolve_model_spec", "snapshot_dir_for_spec", "spawn_install_local_mlx", "target_fingerprint", "validate_snapshot_sha256", "variant_dir_for_snapshot",
]
