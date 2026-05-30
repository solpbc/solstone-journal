# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Local provider first-run bootstrap helpers for Settings."""

from __future__ import annotations

import logging
import threading

import psutil

from solstone.apps.settings.install_copy import INSTALL_FAILED_NO_PROGRESS
from solstone.think.models import LOCAL_MODEL
from solstone.think.providers import local_install
from solstone.think.providers.install_state import (
    IN_FLIGHT_STATES,
    InstallStatus,
    is_stalled,
    make_idle_status,
    read_install_status,
    transition_state,
    write_install_status,
)
from solstone.think.providers.local import (
    LOCAL_MODEL_SPECS,
    LocalProviderError,
    normalize_model_id,
)

logger = logging.getLogger(__name__)

_INSTALL_THREADS: dict[str, threading.Thread] = {}
_INSTALL_PROGRESS: dict[str, tuple[int | None, int | None]] = {}
_INSTALL_LOCK = threading.Lock()


class LocalBootstrapUnavailableError(RuntimeError):
    """Raised when the host cannot run the local provider."""


class LocalBootstrapStartError(RuntimeError):
    """Raised when the bootstrap worker could not be started."""


def check_binary_present() -> bool:
    """Return whether the pinned llama-server binary is installed."""
    try:
        return bool(local_install.inspect_readiness(LOCAL_MODEL)["binary_installed"])
    except Exception:
        return False


def check_model_present(model: str) -> bool:
    """Return whether the pinned GGUF model is installed."""
    try:
        model_id = normalize_model_id(model)
        return bool(local_install.inspect_readiness(model_id)["model_installed"])
    except Exception:
        return False


def _platform_supported() -> tuple[bool, str]:
    try:
        local_install.pin_for_current_platform()
    except LocalProviderError as exc:
        return False, str(exc)
    return True, ""


def get_availability_payload(model: str) -> dict[str, bool | float | int | str]:
    """Return the local provider availability payload used by Settings."""
    model_id = normalize_model_id(model)
    spec = LOCAL_MODEL_SPECS[model_id]
    binary_present = check_binary_present()
    model_present = check_model_present(model_id)
    platform_supported, reason = _platform_supported()
    total_memory_bytes = int(psutil.virtual_memory().total)
    total_memory_gb = round(total_memory_bytes / 1024**3, 1)
    ram_sufficient = total_memory_bytes >= spec.min_ram_bytes

    if not platform_supported:
        available = False
    elif not ram_sufficient:
        available = False
        reason = (
            f"insufficient RAM (need {spec.min_ram_bytes // 1024**3} GB, "
            f"have {int(total_memory_bytes / 1024**3)} GB)"
        )
    else:
        available = binary_present and model_present
        if not binary_present:
            reason = "local runtime is not installed"
        elif not model_present:
            reason = "local model files are not installed"
        else:
            reason = ""

    return {
        "model": model_id,
        "platform_supported": platform_supported,
        "total_memory_gb": total_memory_gb,
        "min_ram_gb": spec.min_ram_bytes // 1024**3,
        "binary_present": binary_present,
        "model_present": model_present,
        "available": available,
        "reason": reason,
    }


def _read_status() -> InstallStatus:
    return read_install_status(scope="bundled", name=local_install.LOCAL_PROVIDER_NAME)


def _write_status(status: InstallStatus) -> InstallStatus:
    write_install_status(status, scope="bundled")
    return status


def _has_live_thread(model: str) -> bool:
    with _INSTALL_LOCK:
        thread = _INSTALL_THREADS.get(model)
    return thread is not None and thread.is_alive()


def _set_progress(model: str, received: int | None, total: int | None) -> None:
    received = None if received is None else max(0, int(received))
    total = None if total is None else max(0, int(total))
    with _INSTALL_LOCK:
        _INSTALL_PROGRESS[model] = (received, total)


def _clear_progress(model: str) -> None:
    with _INSTALL_LOCK:
        _INSTALL_PROGRESS.pop(model, None)


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
    # Local downloads refresh progress per chunk, so stale status fails only without a live worker.
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
    model_id = normalize_model_id(model)
    status = _normalize_stalled_status(model_id, _read_status())
    return _payload_for_status(model_id, status)


def start_bootstrap(model: str) -> tuple[dict[str, str], int]:
    """Start the local provider bootstrap worker if needed."""
    model_id = normalize_model_id(model)
    get_state(model_id)
    status = _read_status()
    if status["install_state"] == "installed":
        return {"install_state": "installed"}, 200

    availability = get_availability_payload(model_id)
    blocked_reason = _blocked_reason(availability)
    if blocked_reason:
        raise LocalBootstrapUnavailableError(blocked_reason)

    installed = bool(availability["binary_present"] and availability["model_present"])
    with _INSTALL_LOCK:
        status = _read_status()
        if status["install_state"] == "installed":
            return {"install_state": "installed"}, 200

        if status["install_state"] == "idle" and installed:
            _write_status(
                transition_state(
                    make_idle_status(local_install.LOCAL_PROVIDER_NAME),
                    new_state="installed",
                )
            )
            _INSTALL_PROGRESS.pop(model_id, None)
            return {"install_state": "installed"}, 200

        if status["install_state"] in IN_FLIGHT_STATES:
            return {"install_state": status["install_state"]}, 200

        try:
            thread = threading.Thread(
                target=_run_bootstrap_worker,
                args=(model_id,),
                name=f"local-provider-bootstrap-{model_id}",
                daemon=True,
            )
        except Exception as exc:
            _write_status(transition_state(status, new_state="failed", error=str(exc)))
            _INSTALL_PROGRESS.pop(model_id, None)
            raise LocalBootstrapStartError(str(exc)) from exc

        _write_status(transition_state(status, new_state="downloading"))
        _INSTALL_PROGRESS[model_id] = (0, LOCAL_MODEL_SPECS[model_id].size_bytes)
        _INSTALL_THREADS[model_id] = thread

    try:
        thread.start()
    except Exception as exc:
        with _INSTALL_LOCK:
            if _INSTALL_THREADS.get(model_id) is thread:
                _INSTALL_THREADS.pop(model_id, None)
        _write_status(
            transition_state(_read_status(), new_state="failed", error=str(exc))
        )
        _clear_progress(model_id)
        raise LocalBootstrapStartError(str(exc)) from exc
    return {"install_state": "downloading"}, 202


def _blocked_reason(availability: dict[str, bool | float | int | str]) -> str:
    if not availability["platform_supported"]:
        return str(availability["reason"])
    reason = str(availability["reason"])
    if reason.startswith("insufficient RAM"):
        return reason
    return ""


def _run_bootstrap_worker(model: str) -> None:
    spec = LOCAL_MODEL_SPECS[model]
    current_thread = threading.current_thread()
    try:
        local_install.install_llama_server()
        _write_status(transition_state(_read_status(), new_state="downloading"))
        _set_progress(model, 0, spec.size_bytes)
        local_install.install_model(model)
        _set_progress(model, spec.size_bytes, spec.size_bytes)
    except Exception as exc:
        logger.exception("local provider bootstrap failed")
        _write_status(
            transition_state(_read_status(), new_state="failed", error=str(exc))
        )
        _clear_progress(model)
    finally:
        with _INSTALL_LOCK:
            if _INSTALL_THREADS.get(model) is current_thread:
                _INSTALL_THREADS.pop(model, None)
