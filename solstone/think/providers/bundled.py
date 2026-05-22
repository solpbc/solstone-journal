# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""On-demand bundled cogitate provider binaries."""

from __future__ import annotations

import importlib.util
import json
import logging
import os
import platform
import shutil
import subprocess
import sys
import sysconfig
import threading
from collections.abc import Callable
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, TypeAlias

from solstone.think.journal_config import read_journal_config, write_journal_config

logger = logging.getLogger(__name__)

STUCK_ENABLING_SECONDS = 300

SUPPORTED_PROVIDERS = {"anthropic", "openai"}
SUPPORTED_RUNTIMES = {"openhands"}
BINARY_STATES = {"installed-no-key", "key-validating", "valid", "invalid-key"}
TERMINAL_INSTALLED_STATES = {"installed-no-key", "valid", "invalid-key"}

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

_LOCKS: dict[str, threading.Lock] = {}
_LOCKS_LOCK = threading.Lock()


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


def _provider_lock(name: str) -> threading.Lock:
    with _LOCKS_LOCK:
        lock = _LOCKS.get(name)
        if lock is None:
            lock = threading.Lock()
            _LOCKS[name] = lock
        return lock


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def _parse_timestamp(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value)
    except ValueError:
        return None


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


def _bundled_config(config: dict[str, Any], name: str) -> dict[str, Any]:
    return config.get("providers", {}).get("bundled", {}).get(name, {}).copy()


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


def _state_from_config(
    config: dict[str, Any],
    name: str,
    bundled_record: dict[str, Any],
) -> str:
    persisted = bundled_record.get("state", "not-enabled")
    if persisted in {
        "not-enabled",
        "enabling",
        "key-validating",
        "install-failed",
        "disabled",
    }:
        return persisted
    if persisted not in TERMINAL_INSTALLED_STATES:
        return "not-enabled"

    if not _key_configured(config, name):
        return "installed-no-key"
    validation = _key_validation(config, name)
    if validation.get("valid") is True:
        return "valid"
    if validation.get("valid") is False:
        return "invalid-key"
    return "installed-no-key"


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


def _runtime_state_from_config(name: str, record: dict[str, Any]) -> str:
    persisted = record.get("state", "not-enabled")
    installed = all(path for path in _runtime_module_paths(name).values())
    if persisted == "disabled":
        return "disabled"
    if installed:
        return "valid"
    if persisted in {"enabling", "install-failed"}:
        return persisted
    return "not-enabled"


def _issues_for_state(
    state: str,
    *,
    name: str,
    env_key: str,
    key_configured: bool,
    key_validation: dict[str, Any],
    install_error: str | None,
) -> list[str]:
    if state == "not-enabled":
        return [f"bundled CLI not installed — run `{_install_hint(name)}`"]
    if state == "install-failed":
        return [install_error or "bundled CLI install failed"]
    if state == "disabled":
        return ["bundled provider disabled"]
    if state == "installed-no-key":
        if not key_configured:
            return [f"{env_key} not set"] if env_key else ["API key not configured"]
        if not key_validation:
            return ["API key not validated"]
    if state == "invalid-key":
        error = key_validation.get("error")
        return [str(error)] if error else ["API key validation failed"]
    return []


def _issues_for_runtime_state(
    state: str,
    *,
    name: str,
    install_error: str | None,
) -> list[str]:
    if state == "not-enabled":
        missing = [
            module
            for module, path in _runtime_module_paths(name).items()
            if path is None
        ]
        detail = f" missing: {', '.join(missing)}" if missing else ""
        return [f"bundled runtime not installed — run `{_install_hint(name)}`{detail}"]
    if state == "install-failed":
        return [install_error or "bundled runtime install failed"]
    if state == "disabled":
        return ["bundled runtime disabled"]
    return []


def _actions_for_state(state: str, *, key_configured: bool, stuck: bool) -> list[str]:
    if state == "not-enabled":
        return ["install"]
    if state == "enabling":
        return ["install"] if stuck else []
    if state == "install-failed":
        return ["install", "uninstall"]
    if state == "disabled":
        return ["enable", "uninstall"]
    if state == "key-validating":
        return []
    actions = ["disable", "uninstall"]
    if key_configured:
        actions.insert(0, "validate-key")
    return actions


def _actions_for_runtime_state(state: str, *, stuck: bool) -> list[str]:
    if state == "not-enabled":
        return ["install"]
    if state == "enabling":
        return ["install"] if stuck else []
    if state == "install-failed":
        return ["install", "uninstall"]
    if state == "disabled":
        return ["enable", "uninstall"]
    return ["disable", "uninstall"]


def _is_record_stuck(record: dict[str, Any]) -> bool:
    if record.get("state") != "enabling":
        return False
    timestamp = _parse_timestamp(record.get("last_transition_at"))
    if timestamp is None:
        return False
    if timestamp.tzinfo is None:
        timestamp = timestamp.replace(tzinfo=timezone.utc)
    return (
        datetime.now(timezone.utc) - timestamp
    ).total_seconds() > STUCK_ENABLING_SECONDS


def get_provider_state(name: str) -> ProviderStateDict:
    """Return the composed bundled provider state."""

    _require_supported(name)
    config = read_journal_config()
    record = _bundled_config(config, name)
    if _is_runtime_key(name):
        return _get_runtime_state(name, record)

    state = _state_from_config(config, name, record)
    env_key = _provider_env_key(name)
    validation = _key_validation(config, name)
    key_configured = _key_configured(config, name)
    binary_path = record.get("binary_path")
    stuck = _is_record_stuck(record)
    install_error = record.get("install_error")
    label = _provider_metadata(name).get("label", name)
    binary_name = _provider_metadata(name).get("cogitate_cli", name)

    return {
        "name": name,
        "label": label,
        "state": state,
        "last_transition_at": record.get("last_transition_at"),
        "stuck_enabling": stuck,
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
        "binary_exists": bool(binary_path and Path(binary_path).exists()),
        "install_error": install_error,
        "issues": _issues_for_state(
            state,
            name=name,
            env_key=env_key,
            key_configured=key_configured,
            key_validation=validation,
            install_error=install_error,
        ),
        "actions": _actions_for_state(
            state, key_configured=key_configured, stuck=stuck
        ),
    }


def _get_runtime_state(name: str, record: dict[str, Any]) -> ProviderStateDict:
    state = _runtime_state_from_config(name, record)
    paths = _runtime_module_paths(name)
    module_path = paths.get("openhands.sdk") or next(
        (path for path in paths.values() if path),
        None,
    )
    installed = all(path for path in paths.values())
    stuck = _is_record_stuck(record)
    install_error = record.get("install_error")
    label = _provider_metadata(name).get("label", name)
    binary_name = _provider_metadata(name).get("cogitate_cli", name)

    return {
        "name": name,
        "label": label,
        "state": state,
        "last_transition_at": record.get("last_transition_at"),
        "stuck_enabling": stuck,
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
        "binary_exists": bool(module_path and Path(module_path).exists()),
        "install_error": install_error,
        "issues": _issues_for_runtime_state(
            state,
            name=name,
            install_error=install_error,
        ),
        "actions": _actions_for_runtime_state(state, stuck=stuck),
    }


def is_stuck_enabling(name: str) -> bool:
    """Return whether the provider has been enabling too long."""

    return bool(get_provider_state(name)["stuck_enabling"])


def _write_bundled_record(
    config: dict[str, Any],
    name: str,
    record: dict[str, Any],
) -> None:
    config.setdefault("providers", {}).setdefault("bundled", {})[name] = record
    write_journal_config(config)


def _transition_state(
    name: str,
    state: str,
    *,
    extra: dict[str, Any] | None = None,
) -> None:
    config = read_journal_config()
    record = _bundled_config(config, name)
    record.update(_pin_record(name))
    record.update(
        {
            "state": state,
            "last_transition_at": _now_iso(),
            "install_error": None,
        }
    )
    if extra:
        record.update(extra)
    _write_bundled_record(config, name, record)


def _installed_state_for_config(config: dict[str, Any], name: str) -> str:
    if _is_runtime_key(name):
        return "valid"
    if not _key_configured(config, name):
        return "installed-no-key"
    validation = _key_validation(config, name)
    if validation.get("valid") is True:
        return "valid"
    if validation.get("valid") is False:
        return "invalid-key"
    return "installed-no-key"


def _start_thread(target: Callable[..., None], args: tuple[Any, ...]) -> None:
    thread = threading.Thread(target=target, args=args, daemon=True)
    thread.start()


def install_provider(name: str) -> ContractState:
    """Start installing a bundled provider and return the current state."""

    _require_supported(name)
    lock = _provider_lock(name)
    with lock:
        current = get_provider_state(name)
        state = current["state"]
        if state == "enabling" and not current["stuck_enabling"]:
            return current
        if state in BINARY_STATES and not current["stuck_enabling"]:
            return current
        _transition_state(name, "enabling", extra=_pin_record(name))
        _start_thread(_install_thread, (name,))
        return get_provider_state(name)


def uninstall_provider(name: str) -> ContractState:
    """Uninstall a bundled provider and return the current state."""

    _require_supported(name)
    lock = _provider_lock(name)
    with lock:
        current = get_provider_state(name)
        if current["state"] == "enabling" and not current["stuck_enabling"]:
            raise CogitateProviderInstallInFlight("install in flight")
        if current["state"] == "not-enabled":
            return current

    try:
        _run_uv_pip_uninstall(_sdk_specs(name))
        if name == "openai":
            _remove_openai_post_install_artifacts()
    finally:
        with lock:
            _transition_state(
                name,
                "not-enabled",
                extra={
                    "binary_path": None,
                    "install_error": None,
                    "codex_artifact": None,
                    "codex_sha256": None,
                },
            )
            return get_provider_state(name)


def disable_provider(name: str) -> ContractState:
    """Disable a bundled provider without removing artifacts."""

    _require_supported(name)
    with _provider_lock(name):
        _transition_state(name, "disabled")
        return get_provider_state(name)


def enable_provider(name: str) -> ContractState:
    """Enable a disabled provider and return its derived state."""

    _require_supported(name)
    with _provider_lock(name):
        config = read_journal_config()
        record = _bundled_config(config, name)
        if _is_runtime_key(name):
            next_state = (
                _installed_state_for_config(config, name)
                if all(path for path in _runtime_module_paths(name).values())
                else "not-enabled"
            )
        else:
            next_state = (
                _installed_state_for_config(config, name)
                if record.get("binary_path")
                else "not-enabled"
            )
        record.update(_pin_record(name))
        record.update(
            {
                "state": next_state,
                "last_transition_at": _now_iso(),
                "install_error": None,
            }
        )
        _write_bundled_record(config, name, record)
        return get_provider_state(name)


def validate_key(name: str) -> ContractState:
    """Start validating a bundled provider key and return the current state."""

    _require_supported(name)
    if _is_runtime_key(name):
        raise BundledProviderError(f"Bundled runtime {name} has no key to validate")
    with _provider_lock(name):
        current = get_provider_state(name)
        if current["state"] == "enabling" and not current["stuck_enabling"]:
            raise CogitateProviderInstallInFlight("install in flight")
        if current["state"] == "disabled":
            raise CogitateProviderDisabled(
                f"Bundled provider {name} is disabled. Enable it before validating the key."
            )
        if current["state"] not in BINARY_STATES:
            raise CogitateProviderNotInstalled(
                f"Bundled cogitate provider {name} is not installed. Run `{_install_hint(name)}` before validating the key."
            )
        config = read_journal_config()
        if not _key_configured(config, name):
            _transition_state(name, "installed-no-key")
            return get_provider_state(name)
        _transition_state(name, "key-validating")
        _start_thread(_validate_thread, (name,))
        return get_provider_state(name)


def resolve_bundled_binary(name: str) -> Path:
    """Return the installed bundled binary path for a provider."""

    state = get_provider_state(name)
    if state["state"] == "disabled":
        raise CogitateProviderDisabled(
            f"Bundled provider {name} is disabled. Run `{_install_hint(name)}` to reinstall or enable it."
        )
    if state["state"] not in BINARY_STATES:
        raise CogitateProviderNotInstalled(
            f"Bundled cogitate provider {name} is not installed. Run `{_install_hint(name)}`."
        )
    path = state.get("binary_path")
    if not path:
        raise CogitateProviderNotInstalled(
            f"Bundled cogitate provider {name} has no binary path. Run `{_install_hint(name)}`."
        )
    return Path(path)


def _install_thread(name: str) -> None:
    try:
        _run_uv_pip_install(_sdk_specs(name))
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
            config = read_journal_config()
            next_state = _installed_state_for_config(config, name)
            _transition_state(name, next_state, extra=extra)
    except Exception as exc:
        with _provider_lock(name):
            _transition_state(
                name,
                "install-failed",
                extra={"install_error": _install_error_message(exc)},
            )


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
        record = _bundled_config(config, name)
        state = "valid" if result.get("valid") else "invalid-key"
        record.update(_pin_record(name))
        record.update(
            {
                "state": state,
                "last_transition_at": _now_iso(),
                "install_error": None,
            }
        )
        _write_bundled_record(config, name, record)


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


def _run_uv_pip_install(sdk_specs: str | list[str]) -> None:
    """Install a provider SDK into the current Python environment."""

    specs = [sdk_specs] if isinstance(sdk_specs, str) else sdk_specs
    env = os.environ.copy()
    env["UV_HTTP_TIMEOUT"] = "60"
    result = subprocess.run(
        ["uv", "pip", "install", "--python", sys.executable, *specs],
        text=True,
        capture_output=True,
        env=env,
        timeout=300,
        check=False,
    )
    if result.returncode != 0:
        output = "\n".join(part for part in (result.stderr, result.stdout) if part)
        raise CogitateProviderInstallFailed(_categorize_uv_error(specs, output))


def _run_uv_pip_uninstall(sdk_specs: str | list[str]) -> None:
    """Best-effort uninstall of a provider SDK from the current environment."""

    specs = [sdk_specs] if isinstance(sdk_specs, str) else sdk_specs
    packages = [_package_name(spec) for spec in specs]
    result = subprocess.run(
        ["uv", "pip", "uninstall", "--python", sys.executable, *packages],
        text=True,
        capture_output=True,
        timeout=120,
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
    "BINARY_STATES",
    "PINS",
    "STUCK_ENABLING_SECONDS",
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
    "is_stuck_enabling",
    "resolve_bundled_binary",
    "uninstall_provider",
    "validate_key",
]
