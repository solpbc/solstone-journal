# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""MLX first-run model bootstrap helpers for Settings."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import logging
import platform
import sys
import threading
from pathlib import Path

import huggingface_hub
import psutil

from solstone.apps.settings.install_copy import INSTALL_FAILED_NO_PROGRESS
from solstone.think.providers.install_state import (
    IN_FLIGHT_STATES,
    InstallStatus,
    bump_progress,
    is_stalled,
    make_idle_status,
    read_install_status,
    transition_state,
    write_install_status,
)
from solstone.think.providers.mlx_install import (
    _MLX_MODEL_REGISTRY,
    is_mlx_available_for_model,
    snapshot_dir_for_spec,
)

logger = logging.getLogger(__name__)

_HASH_CHUNK_SIZE = 1024 * 1024
_INSTALL_THREADS: dict[str, threading.Thread] = {}
_INSTALL_PROGRESS: dict[str, tuple[int | None, int | None]] = {}
_INSTALL_LOCK = threading.Lock()


class MlxBootstrapUnavailableError(RuntimeError):
    """Raised when the host cannot run the MLX provider."""


class MlxBootstrapStartError(RuntimeError):
    """Raised when the bootstrap worker could not be started."""


class MlxVerificationError(RuntimeError):
    """Raised when a downloaded file fails sha256 verification."""


def _snapshot_dir(model: str) -> Path:
    return snapshot_dir_for_spec(_MLX_MODEL_REGISTRY[model])


def _safetensors_paths(model: str) -> list[str]:
    snapshot_dir = _snapshot_dir(model)
    index_path = snapshot_dir / "model.safetensors.index.json"
    data = json.loads(index_path.read_text(encoding="utf-8"))
    weight_map = data.get("weight_map")
    if not isinstance(weight_map, dict):
        raise ValueError("model.safetensors.index.json missing weight_map")
    paths = sorted({str(path) for path in weight_map.values() if str(path)})
    if not paths:
        raise ValueError("model.safetensors.index.json has no safetensors paths")
    return paths


def check_model_present(model: str) -> bool:
    """Return whether the pinned MLX snapshot is structurally present."""
    snapshot_dir = _snapshot_dir(model)
    index_path = snapshot_dir / "model.safetensors.index.json"
    if not snapshot_dir.is_dir() or not index_path.is_file():
        return False
    try:
        for rel_path in _safetensors_paths(model):
            file_path = snapshot_dir / rel_path
            if not file_path.is_file() or file_path.stat().st_size <= 0:
                return False
    except (OSError, ValueError, json.JSONDecodeError):
        return False
    return True


def _is_package_installed(package: str) -> bool:
    if package in sys.modules and sys.modules[package] is None:
        return False
    return importlib.util.find_spec(package) is not None


def get_availability_payload(model: str) -> dict[str, bool | float | int | str]:
    """Return the MLX availability payload used by Settings."""
    spec = _MLX_MODEL_REGISTRY[model]
    ok, reason = is_mlx_available_for_model(spec)
    model_present = check_model_present(model)
    available = ok and model_present
    if ok and not model_present:
        reason = "model snapshot not present"
    elif available:
        reason = ""

    total_memory_gb = round(psutil.virtual_memory().total / 1024**3, 1)
    return {
        "model": model,
        "is_apple_silicon": platform.system() == "Darwin"
        and platform.machine() == "arm64",
        "total_memory_gb": total_memory_gb,
        "mlx_installed": _is_package_installed("mlx_vlm"),
        "min_ram_gb": spec.min_ram_bytes // 1024**3,
        "model_present": model_present,
        "available": available,
        "reason": reason,
    }


def _read_status(model: str) -> InstallStatus:
    return read_install_status(scope="mlx", name=model)


def _write_status(status: InstallStatus) -> InstallStatus:
    write_install_status(status, scope="mlx")
    return status


def _has_live_thread(model: str) -> bool:
    with _INSTALL_LOCK:
        thread = _INSTALL_THREADS.get(model)
    return thread is not None and thread.is_alive()


def _record_progress(model: str, received: int | None, total: int | None) -> None:
    status = _read_status(model)
    if status["install_state"] not in IN_FLIGHT_STATES:
        return
    updated = bump_progress(status, received=received, total=total)
    with _INSTALL_LOCK:
        _INSTALL_PROGRESS[model] = (
            updated["progress_bytes_received"],
            updated["progress_bytes_total"],
        )
    _write_status(updated)


def _clear_progress(model: str) -> None:
    with _INSTALL_LOCK:
        _INSTALL_PROGRESS.pop(model, None)


def _add_progress(model: str, received_delta: int, total: int | None = None) -> None:
    if received_delta < 0:
        received_delta = 0
    with _INSTALL_LOCK:
        received, current_total = _INSTALL_PROGRESS.get(model, (0, None))
    _record_progress(
        model,
        int(received or 0) + received_delta,
        max(0, int(total)) if total is not None else current_total,
    )


def _set_progress_total(model: str, total: int) -> None:
    with _INSTALL_LOCK:
        received, _current_total = _INSTALL_PROGRESS.get(model, (0, None))
    _record_progress(model, received, max(0, int(total)))


def _set_verify_progress(model: str, received_bytes: int, total_bytes: int) -> None:
    status = _read_status(model)
    if status["install_state"] != "verifying":
        return
    _record_progress(model, max(0, int(received_bytes)), max(0, int(total_bytes)))


def _payload_for_status(
    model: str, status: InstallStatus
) -> dict[str, int | str | None]:
    if status["install_state"] in IN_FLIGHT_STATES:
        with _INSTALL_LOCK:
            received, total = _INSTALL_PROGRESS.get(
                model,
                (
                    status["progress_bytes_received"],
                    status["progress_bytes_total"],
                ),
            )
    else:
        received, total = None, None

    return {
        **status,
        "progress_bytes_received": received,
        "progress_bytes_total": total,
    }


def _normalize_stalled_status(model: str, status: InstallStatus) -> InstallStatus:
    # MLX downloads refresh progress per chunk, so stale status fails only without a live worker.
    if is_stalled(status) and not _has_live_thread(model):
        status = transition_state(
            status,
            new_state="failed",
            error=INSTALL_FAILED_NO_PROGRESS,
        )
        _write_status(status)
        _clear_progress(model)
    return status


def get_state(model: str) -> dict[str, int | str | None]:
    """Return the serialized bootstrap state, applying stall detection."""
    status = _normalize_stalled_status(model, _read_status(model))
    return _payload_for_status(model, status)


def _remote_safetensors_metadata(
    model: str, paths: list[str]
) -> dict[str, tuple[str, int]]:
    spec = _MLX_MODEL_REGISTRY[model]
    wanted = set(paths)
    found: dict[str, tuple[str, int]] = {}
    api = huggingface_hub.HfApi()
    for entry in api.list_repo_tree(
        repo_id=spec.repo,
        revision=spec.revision,
        repo_type="model",
        recursive=True,
    ):
        if not isinstance(entry, huggingface_hub.RepoFile) or entry.path not in wanted:
            continue
        if entry.lfs is None:
            raise MlxVerificationError(f"missing LFS sha256 for {entry.path}")
        found[entry.path] = (entry.lfs.sha256, int(entry.lfs.size))
    missing = sorted(wanted - set(found))
    if missing:
        raise MlxVerificationError(f"missing published sha256 for {missing[0]}")
    return found


def _verify_safetensors_sha256_hashes(model: str) -> None:
    snapshot_dir = _snapshot_dir(model)
    safetensors_paths = _safetensors_paths(model)
    metadata = _remote_safetensors_metadata(model, safetensors_paths)
    total_bytes = sum(size for _sha, size in metadata.values())
    hashed_total = 0
    _set_verify_progress(model, 0, total_bytes)

    for rel_path in safetensors_paths:
        expected_sha, _expected_size = metadata[rel_path]
        file_path = snapshot_dir / rel_path
        digest = hashlib.sha256()
        file_received = 0
        with file_path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(_HASH_CHUNK_SIZE), b""):
                digest.update(chunk)
                file_received += len(chunk)
                _set_verify_progress(model, hashed_total + file_received, total_bytes)
        actual_sha = digest.hexdigest()
        if actual_sha != expected_sha:
            raise MlxVerificationError(f"sha256 mismatch for {rel_path}")
        hashed_total += file_received
        _set_verify_progress(model, hashed_total, total_bytes)


class _BootstrapTqdm:
    _model = next(iter(_MLX_MODEL_REGISTRY))

    def __init__(self, *args, **kwargs):
        self._track_bytes = kwargs.get("unit") == "B"
        self._total = int(kwargs.get("total") or 0)
        if self._track_bytes:
            _set_progress_total(self._model, self._total)
            initial = int(kwargs.get("initial") or 0)
            if initial:
                _add_progress(self._model, initial)

    @property
    def total(self) -> int:
        return self._total

    @total.setter
    def total(self, value: int | float | None) -> None:
        self._total = int(value or 0)
        if self._track_bytes:
            _set_progress_total(self._model, self._total)

    def update(self, n: int | float | None = 1) -> None:
        if self._track_bytes:
            _add_progress(self._model, int(n or 0))

    def refresh(self) -> None:
        return None

    def set_description(self, _description: str) -> None:
        return None

    def close(self) -> None:
        return None

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        self.close()
        return False


def start_bootstrap(model: str) -> tuple[dict[str, str], int]:
    """Start the MLX model bootstrap worker if needed."""
    get_state(model)
    status = _read_status(model)
    if status["install_state"] == "installed":
        return {"install_state": "installed"}, 200

    ok, reason = is_mlx_available_for_model(_MLX_MODEL_REGISTRY[model])
    if not ok:
        raise MlxBootstrapUnavailableError(reason)

    present = check_model_present(model)
    with _INSTALL_LOCK:
        status = _read_status(model)
        if status["install_state"] == "installed":
            return {"install_state": "installed"}, 200

        if status["install_state"] == "idle" and present:
            _write_status(
                transition_state(make_idle_status(model), new_state="installed")
            )
            _INSTALL_PROGRESS.pop(model, None)
            return {"install_state": "installed"}, 200

        if status["install_state"] in IN_FLIGHT_STATES:
            return {"install_state": status["install_state"]}, 200

        try:
            thread = threading.Thread(
                target=_run_bootstrap_worker,
                args=(model,),
                name=f"mlx-model-bootstrap-{model}",
                daemon=True,
            )
        except Exception as exc:
            _write_status(transition_state(status, new_state="failed", error=str(exc)))
            _INSTALL_PROGRESS.pop(model, None)
            raise MlxBootstrapStartError(str(exc)) from exc

        _write_status(transition_state(status, new_state="downloading"))
        _INSTALL_PROGRESS[model] = (0, None)
        _INSTALL_THREADS[model] = thread

    try:
        thread.start()
    except Exception as exc:
        with _INSTALL_LOCK:
            if _INSTALL_THREADS.get(model) is thread:
                _INSTALL_THREADS.pop(model, None)
        _write_status(
            transition_state(_read_status(model), new_state="failed", error=str(exc))
        )
        _clear_progress(model)
        raise MlxBootstrapStartError(str(exc)) from exc
    return {"install_state": "downloading"}, 202


def _run_bootstrap_worker(model: str) -> None:
    spec = _MLX_MODEL_REGISTRY[model]
    current_thread = threading.current_thread()

    class _ModelBoundTqdm(_BootstrapTqdm):
        _model = model

    try:
        # v1.15.0 resumes via .incomplete files automatically; no resume_download kwarg.
        huggingface_hub.snapshot_download(
            repo_id=spec.repo,
            revision=spec.revision,
            tqdm_class=_ModelBoundTqdm,
        )
        _write_status(transition_state(_read_status(model), new_state="verifying"))
        _verify_safetensors_sha256_hashes(model)
        _write_status(transition_state(_read_status(model), new_state="installed"))
        _clear_progress(model)
    except Exception as exc:
        logger.exception("MLX model bootstrap failed")
        _write_status(
            transition_state(_read_status(model), new_state="failed", error=str(exc))
        )
        _clear_progress(model)
    finally:
        with _INSTALL_LOCK:
            if _INSTALL_THREADS.get(model) is current_thread:
                _INSTALL_THREADS.pop(model, None)
