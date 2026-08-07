# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Local provider first-run bootstrap helpers for Thinking."""

from __future__ import annotations

import logging
import subprocess
import sys
import threading
import time
from typing import Any

from solstone.apps.thinking.install_copy import (
    LOCAL_MEMORY_WARNING_LOW_TEMPLATE,
    LOCAL_MEMORY_WARNING_UNKNOWN,
    LOCAL_MLX_MEMORY_WARNING_UNKNOWN,
)
from solstone.think.models import LOCAL_MODEL, QWEN_35_9B
from solstone.think.providers import local_install, mlx_install
from solstone.think.providers.fit_report import FitReport
from solstone.think.providers.install_lease import acquire_install_lease, probe_install_lease_free
from solstone.think.providers.install_state import (
    IN_FLIGHT_STATES,
    InstallStatus,
    begin_or_replace_install_attempt,
    read_install_status,
    transition_state,
    write_install_status,
)
from solstone.think.providers.local import (
    LOCAL_MODEL_SPECS,
    LocalModelSpec,
    LocalProviderError,
    normalize_model_id,
)
from solstone.think.providers.local_endpoint import resolve_local_endpoint
from solstone.think.providers.memory import (
    MLX_AVAILABLE_FLOOR_BYTES,
    assess_memory,
    gb,
    gb_label,
    read_total_bytes,
)

logger = logging.getLogger(__name__)

_INSTALL_PROCESSES: dict[str, Any] = {}
_INSTALL_LOCK = threading.Lock()
_MLX_MODEL_LABEL = f"qwen 3.5 9B VLM — {gb_label(MLX_AVAILABLE_FLOOR_BYTES)} GB"


class LocalBootstrapUnavailableError(RuntimeError):
    """Raised when the host cannot run the local provider."""


class LocalBootstrapStartError(RuntimeError):
    """Raised when the bootstrap worker could not be started."""


def _is_mlx_backend() -> bool:
    return sys.platform == "darwin"


def _resolve_model_id(model: str | None) -> str:
    if _is_mlx_backend():
        return QWEN_35_9B
    return normalize_model_id(model)


def accepted_request_model(model: str | None) -> str | None:
    """Return the canonical local model id for this backend, if recognized."""
    candidate = model or LOCAL_MODEL
    if _is_mlx_backend():
        return QWEN_35_9B if candidate in {LOCAL_MODEL, QWEN_35_9B} else None
    return candidate if candidate in LOCAL_MODEL_SPECS else None


def local_model_ids() -> list[str]:
    """Selectable canonical model ids for this backend."""
    if _is_mlx_backend():
        return [QWEN_35_9B]
    return list(LOCAL_MODEL_SPECS)


def list_local_models() -> list[dict[str, object]]:
    """Return backend-aware local model descriptors for Settings."""
    if _is_mlx_backend():
        spec = mlx_install.resolve_model_spec()
        return [
            {
                "name": spec.name,
                "label": _MLX_MODEL_LABEL,
                "min_ram_gb": MLX_AVAILABLE_FLOOR_BYTES // 1024**3,
                "size_bytes": spec.size_bytes,
            }
        ]
    return [
        {
            "name": name,
            "label": "qwen 3.5 4B VLM — 8 GB",
            "min_ram_gb": spec.min_ram_bytes // 1024**3,
            "size_bytes": spec.size_bytes,
        }
        for name, spec in LOCAL_MODEL_SPECS.items()
    ]


def check_binary_present() -> bool:
    """Return whether the pinned llama-server binary is installed."""
    try:
        return bool(
            local_install.inspect_readiness(LOCAL_MODEL).artifacts["binary_installed"]
        )
    except Exception:
        return False


def check_model_present(model: str) -> bool:
    """Return whether the pinned GGUF model is installed."""
    try:
        model_id = normalize_model_id(model)
        return bool(
            local_install.inspect_readiness(model_id).artifacts["model_installed"]
        )
    except Exception:
        return False


def _platform_supported() -> tuple[bool, str]:
    try:
        local_install.pin_for_current_platform()
    except LocalProviderError as exc:
        return False, str(exc)
    return True, ""


def _download_bytes_for_local_spec(spec: LocalModelSpec) -> int:
    return int(spec.size_bytes + (spec.mmproj_size_bytes or 0))


def get_availability_payload(model: str) -> dict[str, bool | float | int | str | None]:
    """Return the local provider availability payload used by Settings."""
    model_id = _resolve_model_id(model)
    if _is_mlx_backend():
        spec = mlx_install.resolve_model_spec(model_id)
        readiness = mlx_install.inspect_readiness(model_id)
        memory_verdict = assess_memory(
            MLX_AVAILABLE_FLOOR_BYTES, block_below_floor=True
        )
        total_memory_bytes = read_total_bytes()
        min_ram_gb = MLX_AVAILABLE_FLOOR_BYTES // 1024**3
        memory_blocked = memory_verdict.severity == "blocked"
        available = bool(
            readiness.host["platform_supported"]
            and readiness.host["package_available"]
            and not memory_blocked
            and readiness.artifacts["model_installed"]
        )
        warning = (
            LOCAL_MLX_MEMORY_WARNING_UNKNOWN
            if memory_verdict.severity == "warning"
            else ""
        )
        if not readiness.host["platform_supported"]:
            reason = "requires Apple Silicon macOS"
        elif memory_blocked:
            assert memory_verdict.available_bytes is not None
            reason = (
                "insufficient RAM "
                f"(need {gb_label(memory_verdict.required_bytes)} GB available, "
                f"have {gb_label(memory_verdict.available_bytes)} GB available)"
            )
        elif not readiness.host["package_available"]:
            reason = "mlx-vlm runtime is not installed"
        elif not readiness.artifacts["model_installed"]:
            reason = "local model files are not installed"
        else:
            reason = ""
        return {
            "model": readiness.target["model_id"],
            "platform_supported": readiness.host["platform_supported"],
            "total_memory_gb": gb(total_memory_bytes),
            "available_memory_gb": gb(memory_verdict.available_bytes),
            "min_ram_gb": min_ram_gb,
            "binary_present": readiness.host["package_available"],
            "model_present": readiness.artifacts["model_installed"],
            "available": available,
            "reason": reason,
            "warning": warning,
            "download_bytes": spec.size_bytes,
        }

    spec = LOCAL_MODEL_SPECS[model_id]
    readiness = local_install.inspect_readiness(model_id)
    binary_present = bool(readiness.artifacts["binary_installed"])
    model_present = bool(readiness.artifacts["model_installed"])
    platform_supported, reason = _platform_supported()
    total_memory_gb = gb(read_total_bytes())
    memory_verdict = assess_memory(spec.min_ram_bytes, block_below_floor=False)
    warning = ""
    if memory_verdict.severity == "warning":
        if memory_verdict.available_bytes is None:
            warning = LOCAL_MEMORY_WARNING_UNKNOWN
        else:
            warning = LOCAL_MEMORY_WARNING_LOW_TEMPLATE.format(
                ram_gb=spec.min_ram_bytes // 1024**3
            )

    if not platform_supported:
        available = False
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
        "available_memory_gb": gb(memory_verdict.available_bytes),
        "min_ram_gb": spec.min_ram_bytes // 1024**3,
        "binary_present": binary_present,
        "model_present": model_present,
        "available": available,
        "reason": reason,
        "warning": warning,
        "download_bytes": _download_bytes_for_local_spec(spec),
    }


def _read_status() -> InstallStatus:
    return read_install_status(name=local_install.LOCAL_PROVIDER_NAME)


def _write_status(status: InstallStatus) -> InstallStatus:
    write_install_status(status)
    return status


def _payload_for_status(
    _model: str, status: InstallStatus
) -> dict[str, int | str | None]:
    if status["install_state"] in IN_FLIGHT_STATES:
        received, total = (
            status["progress_bytes_received"],
            status["progress_bytes_total"],
        )
    else:
        received, total = None, None

    return {
        "name": status["provider"],
        "install_state": status["install_state"],
        "last_transition_at": status["last_transition_at"],
        "last_progress_at": status["last_progress_at"],
        "progress_bytes_received": received,
        "progress_bytes_total": total,
        "install_error": status["install_error"],
    }


def _payload_for_read_status(
    model: str,
    status: InstallStatus,
) -> dict[str, int | str | None]:
    if status["install_state"] in IN_FLIGHT_STATES and probe_install_lease_free(
        local_install.LOCAL_PROVIDER_NAME
    ):
        payload = _payload_for_status(model, status)
        payload["install_state"] = "failed"
        payload["install_error"] = "install_interrupted"
        return payload
    return _payload_for_status(model, status)


def get_state(model: str) -> dict[str, int | str | None]:
    """Return the serialized bootstrap state without mutating on-disk state."""
    model_id = _resolve_model_id(model)
    return _payload_for_read_status(model_id, _read_status())


def start_bootstrap(model: str) -> tuple[dict[str, str], int]:
    """Launch native installation and wait briefly for its durable initial state."""
    if not resolve_local_endpoint().is_bundled:
        raise LocalBootstrapUnavailableError("BYO local endpoint is active")
    model_id = _resolve_model_id(model)
    readiness = mlx_install.inspect_readiness(model_id) if _is_mlx_backend() else local_install.inspect_readiness(model_id)
    if readiness.ready:
        return {"install_state": "installed"}, 200
    if readiness.status in {"proof-unavailable", "host-ineligible"}:
        raise LocalBootstrapUnavailableError(readiness.reason_code)
    availability = get_availability_payload(model_id)
    installed = bool(availability["binary_present"] and availability["model_present"])
    fingerprint = mlx_install.target_fingerprint(model_id) if _is_mlx_backend() else local_install.target_fingerprint(model_id)
    from solstone.think.providers.install_state import canonical_fingerprint, fingerprint_sha256
    target_sha = fingerprint_sha256(canonical_fingerprint(fingerprint))
    if not probe_install_lease_free(local_install.LOCAL_PROVIDER_NAME):
        status = _read_status()
        if status["install_state"] in IN_FLIGHT_STATES and status["target_fingerprint_sha256"] == target_sha:
            return {"install_state": status["install_state"]}, 200
        return {"install_state": status["install_state"], "reason_code": "install_busy"}, 409
    with _INSTALL_LOCK:
        status = _read_status()
        if readiness.ready or (status["install_state"] == "idle" and installed):
            return {"install_state": "installed"}, 200
        if status["install_state"] in IN_FLIGHT_STATES and status["target_fingerprint_sha256"] == target_sha:
            return {"install_state": status["install_state"]}, 200
        report = _fit_report_for_model(model_id)
        blocked_reason = _blocked_reason(report) or _disk_blocked_reason(report)
        if blocked_reason:
            raise LocalBootstrapUnavailableError(blocked_reason)
        try:
            process = mlx_install.spawn_install_local_mlx(model_id, owner={"entry": "thinking_bootstrap"}) if _is_mlx_backend() else local_install.spawn_install_local(model_id, owner={"entry": "thinking_bootstrap"})
        except Exception as exc:
            _mark_native_launch_failure(fingerprint, str(exc))
            raise LocalBootstrapStartError(str(exc)) from exc
    deadline = time.monotonic() + 5.0
    attempt_context: InstallStatus | None = None
    while time.monotonic() < deadline:
        status = _read_status()
        if status["target_fingerprint_sha256"] == target_sha:
            attempt_context = status
        if status["install_state"] in IN_FLIGHT_STATES and status["target_fingerprint_sha256"] == target_sha:
            with _INSTALL_LOCK:
                _INSTALL_PROCESSES[model_id] = process
            threading.Thread(target=_reap_process, args=(model_id, process), name=f"local-provider-reaper-{model_id}", daemon=True).start()
            return {"install_state": status["install_state"]}, 202
        if process.poll() is not None:
            if process.returncode == 75:
                return {"install_state": _read_status()["install_state"], "reason_code": "install_busy"}, 409
            launch_error = getattr(process, "launch_error", None)
            if launch_error is not None:
                _mark_native_launch_failure(fingerprint, str(launch_error))
            raise LocalBootstrapStartError("local bootstrap process exited before starting")
        time.sleep(0.05)
    if _is_mlx_backend() and bool(getattr(process, "pending", False)):
        with _INSTALL_LOCK:
            _INSTALL_PROCESSES[model_id] = process
        threading.Thread(target=_reap_process, args=(model_id, process), name=f"local-provider-reaper-{model_id}", daemon=True).start()
        return {"install_state": "resolving"}, 202
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
    _mark_native_launch_failure(
        fingerprint,
        "local bootstrap process did not publish initial status",
        attempt=attempt_context,
    )
    raise LocalBootstrapStartError("local bootstrap process did not publish initial status")


def _fit_report_for_model(model_id: str) -> FitReport:
    from solstone.think.providers import fit_report

    if _is_mlx_backend():
        return fit_report.build_mlx_fit_report(model_id)
    return fit_report.build_local_fit_report(model_id)


def _blocked_reason(report: FitReport) -> str:
    for check in report.checks:
        if check.name == "disk":
            continue
        if check.severity == "blocked":
            return check.detail
    return ""


def _disk_blocked_reason(report: FitReport) -> str:
    for check in report.checks:
        if check.name == "disk" and check.severity == "blocked":
            return check.detail
    return ""


def _reap_process(model: str, process: Any) -> None:
    process.wait()
    with _INSTALL_LOCK:
        if _INSTALL_PROCESSES.get(model) is process:
            _INSTALL_PROCESSES.pop(model, None)


def _mark_native_launch_failure(
    target: dict[str, Any],
    message: str,
    *,
    attempt: InstallStatus | None = None,
) -> None:
    """Durably fail a missing launch or the exact attempt observed by this poll."""
    lease = acquire_install_lease(local_install.LOCAL_PROVIDER_NAME)
    if lease is None:
        return
    try:
        if attempt is None:
            status = begin_or_replace_install_attempt(
                local_install.LOCAL_PROVIDER_NAME,
                target,
                initial_state="resolving",
                owner={"entry": "thinking_bootstrap", "failure": "native_launch"},
            )
            error_code = "native_launch_failed"
        else:
            status = attempt
            error_code = "native_launch_timeout"
        try:
            _write_status(
                transition_state(
                    status,
                    new_state="failed",
                    error=message,
                    error_code=error_code,
                )
            )
        except Exception:
            if attempt is None:
                raise
            logger.warning("native bootstrap timeout failure was superseded", exc_info=True)
    finally:
        lease.release()
