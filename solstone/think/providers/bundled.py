# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""On-demand bundled cogitate provider binaries."""

from __future__ import annotations

import errno
import importlib.util
import json
import logging
import os
import platform
import pty
import re
import shutil
import subprocess
import sys
import sysconfig
import threading
from collections import deque
from collections.abc import Callable
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Literal, TypeAlias

from solstone.apps.settings.install_copy import (
    INSTALL_FAILED_NO_PROGRESS,
    INSTALL_FAILED_UV_MISSING,
)
from solstone.think.journal_config import read_journal_config, write_journal_config
from solstone.think.providers.install_state import (
    IN_FLIGHT_STATES,
    TERMINAL_STATES,
    InstallState,
    InstallStatus,
    bump_progress,
    is_stalled,
    read_install_status,
    transition_state,
    write_install_status,
)

logger = logging.getLogger(__name__)

SUPPORTED_PROVIDERS = {"anthropic", "openai"}
SUPPORTED_RUNTIMES = {"openhands"}

PINS: dict[str, dict[str, Any]] = {
    "anthropic": {
        "sdk_spec": "claude-agent-sdk==0.2.82",
    },
    "openai": {
        "sdk_spec": "openai-codex-sdk==0.1.11",
        "codex_version": "rust-v0.131.0",
        "codex_artifacts": {
            "x86_64-unknown-linux-musl": {
                "filename": "codex-x86_64-unknown-linux-musl.tar.gz",
                "sha256": (
                    "f5b26732b76c9543742f7937a7c88f879e00c0a73b673008043a5cee63e8361d"
                ),
            },
            "aarch64-unknown-linux-musl": {
                "filename": "codex-aarch64-unknown-linux-musl.tar.gz",
                "sha256": (
                    "dfef7c98b67bd1cc857ef5c505b6eee78872610e6bcdc19dc174d695a56082b6"
                ),
            },
            "aarch64-apple-darwin": {
                "filename": "codex-aarch64-apple-darwin.tar.gz",
                "sha256": (
                    "5997e22af1a05ec303be6e06a9f8cd950da38da4b909b6819747f1782e66825c"
                ),
            },
            "x86_64-apple-darwin": {
                "filename": "codex-x86_64-apple-darwin.tar.gz",
                "sha256": (
                    "7359093511b8b99c8ed06f4500d2148515a719e4c256d5d115e960d4b8a9630b"
                ),
            },
        },
    },
    "openhands": {
        "sdk_specs": ["openhands-sdk==1.23.*"],
        "runtime": "python",
    },
}

RUNTIME_METADATA: dict[str, dict[str, Any]] = {
    "openhands": {
        "label": "OpenHands SDK",
        "env_key": "",
        "cogitate_cli": "openhands-sdk",
        "modules": ["openhands.sdk", "litellm"],
    }
}
BUNDLED_PROVIDER_METADATA: dict[str, dict[str, Any]] = {
    "anthropic": {"cogitate_cli": "claude"},
    "openai": {"cogitate_cli": "codex"},
}

ProviderStateDict: TypeAlias = dict[str, Any]
ContractState: TypeAlias = ProviderStateDict
KeyStatus: TypeAlias = Literal[
    "key-needed",
    "validating",
    "valid",
    "invalid",
    "not-applicable",
]

_UV_INSTALL_TIMEOUT_SECONDS = 3600
_UV_ERROR_TAIL_LINES = 50
_UV_PHASE_ORDER: dict[InstallState, int] = {
    "resolving": 0,
    "downloading": 1,
    "installing": 2,
}
_ANSI_ESCAPE_RE = re.compile(r"\x1b\[[0-9;?]*[a-zA-Z]")
_LOCKS: dict[str, threading.RLock] = {}
_LOCKS_LOCK = threading.Lock()
_INSTALL_THREADS: dict[str, threading.Thread] = {}
_INSTALL_PROCESSES: dict[str, "subprocess.Popen[str]"] = {}
_OBSERVED_PHASES: dict[str, InstallState] = {}


class BundledProviderError(Exception):
    """Base class for bundled provider errors."""


class UnsupportedBundledProvider(BundledProviderError):
    """Raised when a provider does not support bundled cogitate binaries."""


class CogitateProviderNotInstalled(BundledProviderError):
    """Raised when the bundled cogitate binary is not installed."""


class CogitateProviderDisabled(BundledProviderError):
    """Raised when the bundled cogitate provider is disabled."""


class CogitateProviderInstallInFlight(BundledProviderError):
    """Raised when an install is already in flight."""


class CogitateProviderInstallFailed(BundledProviderError):
    """Raised when an install operation fails."""


class CogitateProviderResolveError(BundledProviderError):
    """Raised when an installed bundled binary cannot be resolved."""


def _provider_lock(name: str) -> threading.RLock:
    with _LOCKS_LOCK:
        lock = _LOCKS.get(name)
        if lock is None:
            lock = threading.RLock()
            _LOCKS[name] = lock
        return lock


def _resolve_uv_command() -> list[str]:
    found = shutil.which("uv")
    if found:
        return [str(Path(found).resolve())]

    candidates = [
        Path("~/.local/bin/uv").expanduser(),
        Path("~/.cargo/bin/uv").expanduser(),
        Path("/opt/homebrew/bin/uv"),
        Path("/usr/local/bin/uv"),
    ]
    for candidate in candidates:
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return [str(candidate.resolve())]

    if importlib.util.find_spec("uv") is not None:
        return [sys.executable, "-m", "uv"]

    raise CogitateProviderInstallFailed(INSTALL_FAILED_UV_MISSING)


def _clean_uv_line(raw: str) -> str:
    return _ANSI_ESCAPE_RE.sub("", raw).rstrip()


def _phase_from_uv_line(cleaned: str) -> InstallState | None:
    if cleaned.startswith("Resolved ") or "Resolving dependencies" in cleaned:
        return "resolving"
    if (
        cleaned.startswith("Prepared ")
        or cleaned.startswith("Downloaded ")
        or "Preparing packages" in cleaned
    ):
        return "downloading"
    if cleaned.startswith("Installed ") or "Installing wheels" in cleaned:
        return "installing"
    return None


def _has_live_install_thread_locked(name: str) -> bool:
    thread = _INSTALL_THREADS.get(name)
    return thread is not None and thread.is_alive()


def _kill_process_best_effort(proc: subprocess.Popen[str] | None) -> None:
    if proc is None:
        return
    try:
        proc.kill()
    except (OSError, ProcessLookupError):
        pass


def _advance_phase(name: str, phase: InstallState) -> None:
    with _provider_lock(name):
        status = read_install_status(scope="bundled", name=name)
        if status["install_state"] in TERMINAL_STATES:
            return
        observed = _OBSERVED_PHASES.get(name)
        if observed is None:
            _OBSERVED_PHASES[name] = phase
            write_install_status(
                transition_state(status, new_state=phase),
                scope="bundled",
            )
            return
        if _UV_PHASE_ORDER[phase] > _UV_PHASE_ORDER[observed]:
            _OBSERVED_PHASES[name] = phase
            write_install_status(
                transition_state(status, new_state=phase),
                scope="bundled",
            )
            return
        write_install_status(bump_progress(status), scope="bundled")


def _apply_lazy_stall_locked(name: str) -> None:
    status = read_install_status(scope="bundled", name=name)
    if not is_stalled(status) or _has_live_install_thread_locked(name):
        return
    proc = _INSTALL_PROCESSES.pop(name, None)
    _OBSERVED_PHASES.pop(name, None)
    _INSTALL_THREADS.pop(name, None)
    _kill_process_best_effort(proc)
    write_install_status(
        transition_state(
            status,
            new_state="failed",
            error=INSTALL_FAILED_NO_PROGRESS,
        ),
        scope="bundled",
    )


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _require_supported(name: str) -> None:
    supported = SUPPORTED_PROVIDERS | SUPPORTED_RUNTIMES
    if name not in supported:
        valid = ", ".join(sorted(supported))
        raise UnsupportedBundledProvider(
            f"Unsupported bundled provider: {name!r}. Supported providers: {valid}"
        )


def _is_runtime_key(name: str) -> bool:
    return name in SUPPORTED_RUNTIMES


def _provider_metadata(name: str) -> dict[str, Any]:
    if _is_runtime_key(name):
        return RUNTIME_METADATA[name]

    from solstone.think.providers import PROVIDER_METADATA

    return {**PROVIDER_METADATA[name], **BUNDLED_PROVIDER_METADATA.get(name, {})}


def _provider_env_key(name: str) -> str:
    return str(_provider_metadata(name).get("env_key", ""))


def _read_bundled_record(config: dict[str, Any], name: str) -> dict[str, Any]:
    record = config.get("providers", {}).get("bundled", {}).get(name, {})
    return record if isinstance(record, dict) else {}


def _key_validation(config: dict[str, Any], name: str) -> dict[str, Any]:
    validation = config.get("providers", {}).get("key_validation", {}).get(name, {})
    return validation if isinstance(validation, dict) else {}


def _key_configured(config: dict[str, Any], name: str) -> bool:
    env_key = _provider_env_key(name)
    return bool(config.get("env", {}).get(env_key)) if env_key else False


def _auth_mode(config: dict[str, Any], name: str) -> str:
    if _is_runtime_key(name):
        return "runtime"
    return config.get("providers", {}).get("auth", {}).get(name, "platform")


def _sdk_specs(name: str) -> list[str]:
    pin = PINS[name]
    specs = pin.get("sdk_specs")
    if specs:
        return list(specs)
    return [pin["sdk_spec"]]


def _pin_record(name: str) -> dict[str, Any]:
    pin = PINS[name]
    record: dict[str, Any] = {}
    if "sdk_spec" in pin:
        record["sdk_spec"] = pin["sdk_spec"]
    else:
        record["sdk_specs"] = _sdk_specs(name)
    if "runtime" in pin:
        record["runtime"] = pin["runtime"]
    if name == "openai":
        artifact = _codex_artifact_for_current_platform()
        record["codex_version"] = pin["codex_version"]
        if artifact is not None:
            record["codex_artifact"] = artifact["filename"]
            record["codex_sha256"] = artifact["sha256"]
    return record


def _codex_artifact_key() -> str:
    machine = platform.machine().lower()
    if machine in {"amd64", "x64"}:
        machine = "x86_64"
    elif machine in {"arm64"}:
        machine = "aarch64"

    if sys.platform == "darwin":
        return f"{machine}-apple-darwin"
    if sys.platform.startswith("linux"):
        return f"{machine}-unknown-linux-musl"
    return f"{machine}-{sys.platform}"


def _codex_artifact_for_current_platform() -> dict[str, str] | None:
    artifacts = PINS["openai"]["codex_artifacts"]
    return artifacts.get(_codex_artifact_key())


def _install_hint(name: str) -> str:
    return f"sol call settings providers install {name}"


def _write_bundled_record_fields(name: str, **fields: Any) -> None:
    config = read_journal_config()
    bundled = config.setdefault("providers", {}).setdefault("bundled", {})
    slot = bundled.get(name)
    if not isinstance(slot, dict):
        slot = {}
    bundled[name] = slot
    slot.pop("state", None)
    slot.update(_pin_record(name))
    slot.update(fields)
    write_journal_config(config)


def _migrate_legacy_record_if_needed(
    config: dict[str, Any],
    name: str,
    *,
    runtime: bool,
) -> None:
    record = _read_bundled_record(config, name)
    if "state" not in record:
        return

    legacy_state = str(record["state"])
    if "install_state" in record:
        logger.warning(
            "Dropping legacy state=%r from providers.bundled.%s "
            "(canonical install_state=%r already present)",
            legacy_state,
            name,
            record["install_state"],
        )
        _write_bundled_record_fields(name)
        config.clear()
        config.update(read_journal_config())
        return

    key_validation = _key_validation(config, name)
    key_configured = _key_configured(config, name)
    legacy_installed_no_key = "installed-" + "no-key"
    legacy_key_validating = "key-" + "validating"
    legacy_invalid_key = "invalid-" + "key"
    legacy_install_failed = "install-" + "failed"
    if runtime:
        key_status: KeyStatus = "not-applicable"
    elif legacy_state == legacy_key_validating:
        key_status = "validating"
    elif legacy_state == "valid":
        key_status = "valid"
    elif legacy_state == legacy_invalid_key:
        key_status = "invalid"
    elif legacy_state == "disabled":
        if record.get("install_error"):
            key_status = "key-needed"
        elif key_validation.get("valid") is True:
            key_status = "valid"
        elif key_validation.get("valid") is False:
            key_status = "invalid"
        elif key_configured:
            key_status = "validating"
        else:
            key_status = "key-needed"
    else:
        key_status = "key-needed"

    if legacy_state in {
        legacy_installed_no_key,
        legacy_key_validating,
        "valid",
        legacy_invalid_key,
    }:
        install_state = "installed"
    elif legacy_state == legacy_install_failed:
        install_state = "failed"
    elif legacy_state == "disabled" and record.get("install_error"):
        install_state = "failed"
    elif legacy_state == "disabled" and record.get("binary_path"):
        install_state = "installed"
    else:
        install_state = "idle"
    disabled = legacy_state == "disabled"
    current_status = read_install_status(scope="bundled", name=name)
    next_status = transition_state(
        current_status,
        new_state=install_state,
        error=record.get("install_error") if install_state == "failed" else None,
    )
    write_install_status(next_status, scope="bundled")
    _write_bundled_record_fields(name, key_state=key_status, disabled=disabled)
    config.clear()
    config.update(read_journal_config())


def _runtime_module_path(module_name: str) -> str | None:
    try:
        spec = importlib.util.find_spec(module_name)
    except (ImportError, AttributeError, ValueError):
        return None
    if spec is None:
        return None
    if spec.origin and spec.origin != "namespace":
        return spec.origin
    locations = spec.submodule_search_locations
    if locations:
        return next(iter(locations), None)
    return None


def _runtime_module_paths(name: str) -> dict[str, str | None]:
    modules = _provider_metadata(name).get("modules", [])
    return {str(module): _runtime_module_path(str(module)) for module in modules}


def _compose_install_status(config: dict[str, Any], name: str) -> InstallStatus:
    _migrate_legacy_record_if_needed(config, name, runtime=_is_runtime_key(name))
    with _provider_lock(name):
        _apply_lazy_stall_locked(name)
        return read_install_status(scope="bundled", name=name)


def _compose_key_status(
    record: dict[str, Any],
    *,
    key_configured: bool,
    key_validation: dict[str, Any],
    runtime: bool,
) -> KeyStatus:
    if runtime:
        return "not-applicable"
    key_state = record.get("key_state")
    if key_state in {"key-needed", "validating", "valid", "invalid"}:
        return key_state
    if key_validation.get("valid") is True:
        return "valid"
    if key_validation.get("valid") is False:
        return "invalid"
    return "key-needed"


def _issues_for_payload(
    install_status: InstallStatus,
    key_status: KeyStatus,
    disabled: bool,
    *,
    name: str,
    runtime: bool,
    env_key: str,
    key_validation: dict[str, Any],
    install_error: str | None,
    binary_exists: bool,
    key_configured: bool,
) -> list[str]:
    install_state = install_status["install_state"]
    if disabled:
        return (
            ["bundled runtime disabled"] if runtime else ["bundled provider disabled"]
        )
    if install_state == "failed":
        fallback = (
            "bundled runtime install failed"
            if runtime
            else "bundled CLI install failed"
        )
        return [install_error or fallback]
    if install_state == "idle":
        if runtime:
            missing = [
                module
                for module, path in _runtime_module_paths(name).items()
                if path is None
            ]
            detail = f" missing: {', '.join(missing)}" if missing else ""
            return [
                f"bundled runtime not installed — run `{_install_hint(name)}`{detail}"
            ]
        return [f"bundled CLI not installed — run `{_install_hint(name)}`"]
    if install_state in {"resolving", "downloading", "verifying", "installing"}:
        return []
    if runtime:
        return []
    if key_status == "key-needed":
        if not key_configured:
            return [f"{env_key} not set"] if env_key else ["API key not configured"]
        if not key_validation:
            return ["API key not validated"]
        return []
    if key_status == "validating":
        return []
    if key_status == "invalid":
        error = key_validation.get("error")
        return [str(error)] if error else ["API key validation failed"]
    return []


def _actions_for_payload(
    install_status: InstallStatus,
    key_status: KeyStatus,
    disabled: bool,
    *,
    runtime: bool,
) -> list[str]:
    install_state = install_status["install_state"]
    if disabled:
        actions = ["enable"]
        if install_state != "installing":
            actions.append("uninstall")
        return actions
    if install_state == "idle":
        return ["install"]
    if install_state in {"resolving", "downloading", "verifying", "installing"}:
        return []
    if install_state == "failed":
        return ["install", "uninstall"]
    actions: list[str] = []
    if not runtime and key_status != "validating":
        actions.append("validate-key")
    actions.extend(["disable", "uninstall"])
    return actions


def get_provider_state(name: str) -> ProviderStateDict:
    """Return the composed bundled provider state."""

    _require_supported(name)
    config = read_journal_config()
    record = _read_bundled_record(config, name)
    if _is_runtime_key(name):
        return _get_runtime_state(name, record)

    _migrate_legacy_record_if_needed(config, name, runtime=False)
    record = _read_bundled_record(config, name)
    install_status = _compose_install_status(config, name)
    env_key = _provider_env_key(name)
    validation = _key_validation(config, name)
    key_configured = _key_configured(config, name)
    key_status = _compose_key_status(
        record,
        key_configured=key_configured,
        key_validation=validation,
        runtime=False,
    )
    binary_path = str(record["binary_path"]) if record.get("binary_path") else None
    binary_exists = bool(binary_path and Path(binary_path).exists())
    disabled = bool(record.get("disabled", False))
    label = _provider_metadata(name).get("label", name)
    binary_name = _provider_metadata(name).get("cogitate_cli", name)

    return {
        "name": name,
        "label": label,
        "install_state": install_status["install_state"],
        "key_status": key_status,
        "disabled": disabled,
        "last_transition_at": install_status["last_transition_at"],
        "last_progress_at": install_status["last_progress_at"],
        "progress_bytes_received": install_status["progress_bytes_received"],
        "progress_bytes_total": install_status["progress_bytes_total"],
        "install_error": install_status["install_error"],
        "sdk_spec": record.get("sdk_spec", _sdk_specs(name)[0]),
        "sdk_specs": record.get("sdk_specs", _sdk_specs(name)),
        "codex_version": record.get("codex_version"),
        "codex_artifact": record.get("codex_artifact"),
        "codex_sha256": record.get("codex_sha256"),
        "auth_mode": _auth_mode(config, name),
        "env_key": env_key,
        "key_configured": key_configured,
        "key_valid": validation.get("valid") is True,
        "key_validation": validation,
        "binary_name": binary_name,
        "binary_path": binary_path,
        "binary_exists": binary_exists,
        "issues": _issues_for_payload(
            install_status,
            key_status,
            disabled,
            name=name,
            runtime=False,
            env_key=env_key,
            key_validation=validation,
            install_error=install_status["install_error"],
            binary_exists=binary_exists,
            key_configured=key_configured,
        ),
        "actions": _actions_for_payload(
            install_status,
            key_status,
            disabled,
            runtime=False,
        ),
    }


def _get_runtime_state(name: str, _record: dict[str, Any]) -> ProviderStateDict:
    config = read_journal_config()
    _migrate_legacy_record_if_needed(config, name, runtime=True)
    record = _read_bundled_record(config, name)
    install_status = _compose_install_status(config, name)
    paths = _runtime_module_paths(name)
    module_path = paths.get("openhands.sdk") or next(
        (path for path in paths.values() if path),
        None,
    )
    installed = all(path for path in paths.values())
    binary_exists = bool(module_path and Path(module_path).exists())
    key_status: KeyStatus = "not-applicable"
    disabled = bool(record.get("disabled", False))
    label = _provider_metadata(name).get("label", name)
    binary_name = _provider_metadata(name).get("cogitate_cli", name)

    return {
        "name": name,
        "label": label,
        "install_state": install_status["install_state"],
        "key_status": key_status,
        "disabled": disabled,
        "last_transition_at": install_status["last_transition_at"],
        "last_progress_at": install_status["last_progress_at"],
        "progress_bytes_received": install_status["progress_bytes_received"],
        "progress_bytes_total": install_status["progress_bytes_total"],
        "install_error": install_status["install_error"],
        "sdk_spec": record.get("sdk_spec", _sdk_specs(name)[0]),
        "sdk_specs": record.get("sdk_specs", _sdk_specs(name)),
        "codex_version": None,
        "codex_artifact": None,
        "codex_sha256": None,
        "runtime": record.get("runtime", PINS[name].get("runtime")),
        "auth_mode": _auth_mode({}, name),
        "env_key": "",
        "key_configured": installed,
        "key_valid": installed,
        "key_validation": {},
        "binary_name": binary_name,
        "binary_path": module_path,
        "binary_exists": binary_exists,
        "issues": _issues_for_payload(
            install_status,
            key_status,
            disabled,
            name=name,
            runtime=True,
            env_key="",
            key_validation={},
            install_error=install_status["install_error"],
            binary_exists=binary_exists,
            key_configured=installed,
        ),
        "actions": _actions_for_payload(
            install_status,
            key_status,
            disabled,
            runtime=True,
        ),
    }


def _start_thread(target: Callable[..., None], args: tuple[Any, ...]) -> None:
    thread = threading.Thread(target=target, args=args, daemon=True)
    thread.start()


def install_provider(name: str, *, wait: bool = False) -> ContractState:
    """Start installing a bundled provider and return the current state."""

    _require_supported(name)
    lock = _provider_lock(name)
    with lock:
        status = read_install_status(scope="bundled", name=name)
        persisted_state = status["install_state"]
        if persisted_state in IN_FLIGHT_STATES and _has_live_install_thread_locked(
            name
        ):
            raise CogitateProviderInstallInFlight("install in flight")
        current = get_provider_state(name)
        install_state = current["install_state"]
        if install_state == "installed":
            return current
        if _has_live_install_thread_locked(name):
            raise CogitateProviderInstallInFlight("install in flight")
        if install_state in {"resolving", "downloading", "verifying", "installing"}:
            raise CogitateProviderInstallInFlight("install in flight")
        status = read_install_status(scope="bundled", name=name)
        thread = threading.Thread(target=_install_thread, args=(name,), daemon=True)
        _OBSERVED_PHASES.pop(name, None)
        write_install_status(
            transition_state(status, new_state="installing"),
            scope="bundled",
        )
        _write_bundled_record_fields(name)
        _INSTALL_THREADS[name] = thread
        try:
            thread.start()
        except Exception as exc:
            if _INSTALL_THREADS.get(name) is thread:
                _INSTALL_THREADS.pop(name, None)
            _OBSERVED_PHASES.pop(name, None)
            status = read_install_status(scope="bundled", name=name)
            write_install_status(
                transition_state(status, new_state="failed", error=str(exc)),
                scope="bundled",
            )
            raise CogitateProviderInstallFailed(str(exc)) from exc
        snapshot = get_provider_state(name)

    # The worker thread needs _provider_lock to write terminal state and clean up.
    if not wait:
        return snapshot
    thread.join(timeout=_UV_INSTALL_TIMEOUT_SECONDS)
    return get_provider_state(name)


def uninstall_provider(name: str) -> ContractState:
    """Uninstall a bundled provider and return the current state."""

    _require_supported(name)
    lock = _provider_lock(name)
    with lock:
        current = get_provider_state(name)
        if current["install_state"] == "idle":
            return current

    try:
        _run_uv_pip_uninstall(_sdk_specs(name))
        if name == "openai":
            _remove_openai_post_install_artifacts()
    finally:
        with lock:
            status = read_install_status(scope="bundled", name=name)
            write_install_status(
                transition_state(status, new_state="idle"),
                scope="bundled",
            )
            _write_bundled_record_fields(
                name,
                key_state="not-applicable" if _is_runtime_key(name) else "key-needed",
                binary_path=None,
                codex_artifact=None,
                codex_sha256=None,
            )
            return get_provider_state(name)


def disable_provider(name: str) -> ContractState:
    """Disable a bundled provider without removing artifacts."""

    _require_supported(name)
    with _provider_lock(name):
        get_provider_state(name)
        _write_bundled_record_fields(name, disabled=True)
        return get_provider_state(name)


def enable_provider(name: str) -> ContractState:
    """Enable a disabled provider and return its derived state."""

    _require_supported(name)
    with _provider_lock(name):
        get_provider_state(name)
        _write_bundled_record_fields(name, disabled=False)
        return get_provider_state(name)


def validate_key(name: str) -> ContractState:
    """Start validating a bundled provider key and return the current state."""

    _require_supported(name)
    if _is_runtime_key(name):
        raise BundledProviderError(f"Bundled runtime {name} has no key to validate")
    with _provider_lock(name):
        current = get_provider_state(name)
        if current["install_state"] in {
            "resolving",
            "downloading",
            "verifying",
            "installing",
        }:
            raise CogitateProviderInstallInFlight("install in flight")
        if current["disabled"]:
            raise CogitateProviderDisabled(
                f"Bundled provider {name} is disabled. Enable it before validating the key."
            )
        if current["install_state"] != "installed":
            raise CogitateProviderNotInstalled(
                f"Bundled cogitate provider {name} is not installed. Run `{_install_hint(name)}` before validating the key."
            )
        if current["key_status"] == "validating":
            return current
        config = read_journal_config()
        if not _key_configured(config, name):
            _write_bundled_record_fields(name, key_state="key-needed")
            return get_provider_state(name)
        _write_bundled_record_fields(name, key_state="validating")
        _start_thread(_validate_thread, (name,))
        return get_provider_state(name)


def _install_thread(name: str) -> None:
    current_thread = threading.current_thread()
    try:
        _run_uv_pip_install(name, _sdk_specs(name))
        importlib.invalidate_caches()
        if _is_runtime_key(name):
            paths = _runtime_module_paths(name)
            if not all(path for path in paths.values()):
                missing = [module for module, path in paths.items() if path is None]
                raise CogitateProviderResolveError(
                    f"runtime modules not importable: {', '.join(missing)}"
                )
            extra: dict[str, Any] = {"binary_path": paths["openhands.sdk"]}
        elif name == "anthropic":
            binary_path = _resolve_anthropic_binary_via_subprocess()
            extra = {"binary_path": str(binary_path)}
        else:
            artifact = _codex_artifact_for_current_platform()
            filename = artifact["filename"] if artifact else ""
            sha256 = artifact["sha256"] if artifact else ""
            binary_path = _run_codex_install(
                PINS[name]["codex_version"],
                filename,
                sha256,
            )
            extra = {
                "binary_path": str(binary_path),
                "codex_version": PINS[name]["codex_version"],
                "codex_artifact": filename or None,
                "codex_sha256": sha256 or None,
            }
        with _provider_lock(name):
            status = read_install_status(scope="bundled", name=name)
            write_install_status(
                transition_state(status, new_state="installed"),
                scope="bundled",
            )
            _write_bundled_record_fields(
                name,
                key_state="not-applicable" if _is_runtime_key(name) else "key-needed",
                **extra,
            )
    except Exception as exc:
        with _provider_lock(name):
            status = read_install_status(scope="bundled", name=name)
            write_install_status(
                transition_state(
                    status,
                    new_state="failed",
                    error=_install_error_message(exc),
                ),
                scope="bundled",
            )
    finally:
        with _provider_lock(name):
            if _INSTALL_THREADS.get(name) is current_thread:
                _INSTALL_THREADS.pop(name, None)
            _OBSERVED_PHASES.pop(name, None)


def _validate_thread(name: str) -> None:
    try:
        result = _validate_provider_key(name)
    except Exception as exc:
        result = {"valid": False, "error": str(exc)}
    result["timestamp"] = _now_iso()

    with _provider_lock(name):
        config = read_journal_config()
        config.setdefault("providers", {}).setdefault("key_validation", {})[name] = (
            result
        )
        write_journal_config(config)
        _write_bundled_record_fields(
            name,
            key_state="valid" if result.get("valid") else "invalid",
        )


def _install_error_message(exc: Exception) -> str:
    if isinstance(exc, CogitateProviderInstallFailed):
        return str(exc)
    return f"install: {exc}"


def _package_name(sdk_spec: str) -> str:
    return sdk_spec.split("==", 1)[0]


def _categorize_uv_error(sdk_specs: str | list[str], output: str) -> str:
    text = output.strip() or "unknown error"
    lower = text.lower()
    specs = [sdk_specs] if isinstance(sdk_specs, str) else sdk_specs
    packages = ", ".join(_package_name(spec) for spec in specs)
    if any(
        marker in lower
        for marker in (
            "timed out",
            "timeout",
            "connection",
            "network",
            "temporary failure",
            "name resolution",
            "tls",
        )
    ):
        return f"network: {text}"
    if any(
        marker in lower for marker in ("no matching distribution", "not found", "404")
    ):
        return f"pypi: {text}"
    if any(marker in lower for marker in ("conflict", "resolution", "incompatible")):
        return f"dependency conflict: {packages}"
    return f"install: {text}"


def _run_uv_pip_install(name: str, specs: list[str]) -> None:
    """Install a provider SDK into the current Python environment."""

    command = [
        *_resolve_uv_command(),
        "pip",
        "install",
        "--python",
        sys.executable,
        *specs,
    ]
    env = os.environ.copy()
    env["UV_HTTP_TIMEOUT"] = "60"
    master_fd: int | None = None
    slave_fd: int | None = None
    proc: subprocess.Popen[str] | None = None
    try:
        master_fd, slave_fd = pty.openpty()
        try:
            proc = subprocess.Popen(
                command,
                stdin=subprocess.DEVNULL,
                stdout=slave_fd,
                stderr=subprocess.STDOUT,
                env=env,
                text=True,
                bufsize=1,
                close_fds=True,
            )
        except OSError as exc:
            for fd in (slave_fd, master_fd):
                if fd is None:
                    continue
                try:
                    os.close(fd)
                except OSError:
                    pass
            slave_fd = None
            master_fd = None
            raise CogitateProviderInstallFailed(INSTALL_FAILED_UV_MISSING) from exc

        try:
            os.close(slave_fd)
        except OSError:
            pass
        slave_fd = None
        with _provider_lock(name):
            _INSTALL_PROCESSES[name] = proc

        tail: deque[str] = deque(maxlen=_UV_ERROR_TAIL_LINES)
        reader = os.fdopen(master_fd, "r", buffering=1, errors="replace")
        master_fd = None
        try:
            while True:
                try:
                    raw = reader.readline()
                except OSError as exc:
                    if exc.errno == errno.EIO:
                        break
                    raise
                if not raw:
                    break
                cleaned = _clean_uv_line(raw)
                if cleaned:
                    tail.append(cleaned)
                phase = _phase_from_uv_line(cleaned)
                if phase is not None:
                    _advance_phase(name, phase)
                elif cleaned:
                    with _provider_lock(name):
                        status = read_install_status(scope="bundled", name=name)
                        if status["install_state"] in IN_FLIGHT_STATES:
                            write_install_status(
                                bump_progress(status),
                                scope="bundled",
                            )
        finally:
            try:
                reader.close()
            except OSError:
                pass

        try:
            proc.wait(timeout=_UV_INSTALL_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired as exc:
            _kill_process_best_effort(proc)
            proc.wait()
            tail.append(f"timed out after {_UV_INSTALL_TIMEOUT_SECONDS} seconds")
            raise CogitateProviderInstallFailed(
                _categorize_uv_error(specs, "\n".join(tail))
            ) from exc

        if proc.returncode == 0:
            return
        if proc.returncode in (126, 127):
            raise CogitateProviderInstallFailed(INSTALL_FAILED_UV_MISSING)
        raise CogitateProviderInstallFailed(
            _categorize_uv_error(specs, "\n".join(tail))
        )
    finally:
        if slave_fd is not None:
            try:
                os.close(slave_fd)
            except OSError:
                pass
        if master_fd is not None:
            try:
                os.close(master_fd)
            except OSError:
                pass
        if proc is not None:
            with _provider_lock(name):
                if _INSTALL_PROCESSES.get(name) is proc:
                    _INSTALL_PROCESSES.pop(name, None)


def _run_uv_pip_uninstall(sdk_specs: str | list[str]) -> None:
    """Best-effort uninstall of a provider SDK from the current environment."""

    specs = [sdk_specs] if isinstance(sdk_specs, str) else sdk_specs
    packages = [_package_name(spec) for spec in specs]
    result = subprocess.run(
        [
            *_resolve_uv_command(),
            "pip",
            "uninstall",
            "--python",
            sys.executable,
            *packages,
        ],
        text=True,
        capture_output=True,
        timeout=300,
        check=False,
    )
    if result.returncode == 0:
        return
    output = "\n".join(part for part in (result.stderr, result.stdout) if part)
    lower = output.lower()
    if "not installed" in lower or "skipping" in lower:
        return
    raise CogitateProviderInstallFailed(f"uninstall: {output.strip()}")


def _remove_openai_post_install_artifacts() -> None:
    """Remove the openai_codex_sdk package directory left behind by Codex.install().

    Best-effort. Scoped to the active venv's site-packages. Idempotent.
    Cleanup failure is non-fatal -- the uninstall state transition still happens.
    """

    purelib = Path(sysconfig.get_paths()["purelib"]).resolve()
    if not purelib.is_dir():
        logger.warning("active site-packages directory does not exist: %s", purelib)
        return

    target = (purelib / "openai_codex_sdk").resolve(strict=False)
    if not target.exists():
        return

    target_resolved = target.resolve()
    if not target_resolved.is_relative_to(purelib):
        logger.warning(
            "refusing to remove openai_codex_sdk outside site-packages: %s "
            "(site-packages: %s)",
            target_resolved,
            purelib,
        )
        return

    try:
        shutil.rmtree(target_resolved)
    except OSError as exc:
        logger.warning(
            "failed to reclaim openai_codex_sdk vendor tree at %s: %s",
            target_resolved,
            exc,
        )
        return

    logger.info("reclaimed openai_codex_sdk vendor tree at %s", target_resolved)


def _smoke_check_binary(path: Path) -> None:
    result = subprocess.run(
        [str(path), "--version"],
        text=True,
        capture_output=True,
        timeout=30,
        check=False,
    )
    if result.returncode != 0:
        output = "\n".join(part for part in (result.stderr, result.stdout) if part)
        raise CogitateProviderResolveError(
            f"binary smoke check failed for {path}: {output.strip()}"
        )


def _resolve_anthropic_binary_via_subprocess() -> Path:
    """Resolve the bundled Claude binary through a fresh Python subprocess."""

    script = (
        "import json, platform\n"
        "from pathlib import Path\n"
        "import claude_agent_sdk\n"
        "name = 'claude.exe' if platform.system() == 'Windows' else 'claude'\n"
        "path = Path(claude_agent_sdk.__file__).parent / '_bundled' / name\n"
        "print(json.dumps({'path': str(path)}))\n"
    )
    result = subprocess.run(
        [sys.executable, "-c", script],
        text=True,
        capture_output=True,
        timeout=30,
        check=False,
    )
    if result.returncode != 0:
        output = "\n".join(part for part in (result.stderr, result.stdout) if part)
        raise CogitateProviderResolveError(
            f"claude binary resolve failed: {output.strip()}"
        )
    try:
        path = Path(json.loads(result.stdout)["path"])
    except (json.JSONDecodeError, KeyError) as exc:
        raise CogitateProviderResolveError(
            "claude binary resolve returned invalid JSON"
        ) from exc
    if not path.is_file():
        raise CogitateProviderResolveError(f"claude binary not found at {path}")
    _smoke_check_binary(path)
    return path


def _categorize_codex_error(output: str) -> str:
    text = output.strip() or "unknown error"
    lower = text.lower()
    if any(
        marker in lower
        for marker in (
            "timed out",
            "timeout",
            "connection",
            "network",
            "temporary failure",
            "name resolution",
            "tls",
        )
    ):
        return "codex binary download: network"
    if "sha256" in lower or "checksum" in lower:
        return "codex binary download: sha256 mismatch"
    if any(marker in lower for marker in ("unsupported", "platform", "triple")):
        return "codex binary download: unsupported platform triple"
    return f"codex binary download: other: {text}"


def _run_codex_install(version: str, filename: str, sha256: str) -> Path:
    """Install the Codex binary through the public Python SDK API."""

    script = (
        "import json, sys\n"
        "from openai_codex_sdk import Codex\n"
        "kwargs = {'version': sys.argv[1], 'overwrite': False}\n"
        "if sys.argv[2]:\n"
        "    kwargs['filename'] = sys.argv[2]\n"
        "if sys.argv[3]:\n"
        "    kwargs['sha256'] = sys.argv[3]\n"
        "result = Codex.install(**kwargs)\n"
        "print(json.dumps({'path': result.codex_path, 'installed': result.installed}))\n"
    )
    result = subprocess.run(
        [sys.executable, "-c", script, version, filename, sha256],
        text=True,
        capture_output=True,
        timeout=300,
        check=False,
    )
    if result.returncode != 0:
        output = "\n".join(part for part in (result.stderr, result.stdout) if part)
        raise CogitateProviderInstallFailed(_categorize_codex_error(output))
    try:
        path = Path(json.loads(result.stdout)["path"])
    except (json.JSONDecodeError, KeyError) as exc:
        raise CogitateProviderResolveError(
            "codex install returned invalid JSON"
        ) from exc
    if not path.is_file():
        raise CogitateProviderResolveError(f"codex binary not found at {path}")
    _smoke_check_binary(path)
    return path


def _validate_provider_key(name: str) -> dict[str, Any]:
    """Validate the configured API key for a bundled provider."""

    config = read_journal_config()
    env_key = _provider_env_key(name)
    api_key = config.get("env", {}).get(env_key, "")
    if not api_key:
        return {"valid": False, "error": f"{env_key} not set"}

    from solstone.think.providers import get_provider_module

    module = get_provider_module(name)
    return module.validate_key(name, api_key)


__all__ = [
    "PINS",
    "BundledProviderError",
    "CogitateProviderDisabled",
    "CogitateProviderInstallFailed",
    "CogitateProviderInstallInFlight",
    "CogitateProviderNotInstalled",
    "CogitateProviderResolveError",
    "UnsupportedBundledProvider",
    "disable_provider",
    "enable_provider",
    "get_provider_state",
    "install_provider",
    "uninstall_provider",
    "validate_key",
]
