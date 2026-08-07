# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Provider install status records under journal/health/providers."""

from __future__ import annotations

import hashlib
import json
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Literal, TypedDict, cast, get_args

from solstone.think.journal_config import (
    JournalConfigMutation,
    mutate_journal_config,
    read_journal_config,
)
from solstone.think.journal_io.atomic import atomic_replace
from solstone.think.journal_io.locking import hold_lock
from solstone.think.utils import get_journal

ProviderName = Literal["local", "parakeet"]
InstallState = Literal[
    "idle",
    "resolving",
    "downloading",
    "verifying",
    "installing",
    "installed",
    "failed",
]


class InstallStatus(TypedDict):
    schema_version: int
    provider: ProviderName
    revision: int
    install_state: InstallState
    attempt_id: str | None
    target_fingerprint_json: str | None
    target_fingerprint_sha256: str | None
    started_at: str | None
    last_transition_at: str | None
    last_progress_at: str | None
    completed_at: str | None
    progress_bytes_received: int | None
    progress_bytes_total: int | None
    install_error: str | None
    error_code: str | None
    owner: dict[str, Any] | None


class InstallStateError(RuntimeError):
    """Provider install status is invalid or cannot transition."""


class InstallStatusMalformedError(InstallStateError):
    """Persisted provider install status is malformed."""


class InstallStatusConflictError(InstallStateError):
    """Install status write lost a revision or attempt race."""


SCHEMA_VERSION = 1
PROVIDERS: frozenset[str] = frozenset({"local", "parakeet"})
PROGRESS_COALESCE_SECONDS = 1.0
IN_FLIGHT_STATES: frozenset[InstallState] = frozenset(
    {"resolving", "downloading", "verifying", "installing"}
)
TERMINAL_STATES: frozenset[InstallState] = frozenset({"idle", "installed", "failed"})
_INSTALL_STATES = frozenset(get_args(InstallState))
_STATUS_MODE = 0o600
_LEGACY_OPERATIONAL_KEYS = frozenset(
    {
        "install_state",
        "last_transition_at",
        "last_progress_at",
        "progress_bytes_received",
        "progress_bytes_total",
        "install_error",
        "binary_artifact",
        "binary_sha256",
        "binary_path",
        "model_id",
        "model_path",
        "model_sha256",
        "mmproj_path",
        "mmproj_sha256",
        "mlx_model_id",
        "mlx_revision",
        "mlx_snapshot_dir",
        "mlx_variant_dir",
        "binary_artifact_cpu",
        "binary_sha256_cpu",
        "binary_path_cpu",
        "binary_artifact_vulkan",
        "binary_sha256_vulkan",
        "binary_path_vulkan",
        "model_repo",
        "model_filename",
        "model_revision",
        "model_path",
        "model_sha256",
    }
)
_LAST_PROGRESS_WRITE_MONOTONIC: dict[tuple[str, str], float] = {}


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def canonical_fingerprint(data: dict[str, Any]) -> str:
    """Return canonical JSON text for a provider install target fingerprint."""
    normalized = _normalize_fingerprint_value(data)
    return json.dumps(
        normalized,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=True,
    )


def fingerprint_sha256(fingerprint_json: str) -> str:
    return hashlib.sha256(fingerprint_json.encode("utf-8")).hexdigest()


def provider_status_path(
    provider: str,
    *,
    journal_path: str | Path | None = None,
) -> Path:
    validated = _validate_provider(provider)
    root = Path(journal_path) if journal_path is not None else Path(get_journal())
    return root / "health" / "providers" / f"{validated}.json"


def make_idle_status(name: str) -> InstallStatus:
    provider = _validate_provider(name)
    return {
        "schema_version": SCHEMA_VERSION,
        "provider": provider,
        "revision": 0,
        "install_state": "idle",
        "attempt_id": None,
        "target_fingerprint_json": None,
        "target_fingerprint_sha256": None,
        "started_at": None,
        "last_transition_at": None,
        "last_progress_at": None,
        "completed_at": None,
        "progress_bytes_received": None,
        "progress_bytes_total": None,
        "install_error": None,
        "error_code": None,
        "owner": None,
    }


def begin_install_attempt(
    provider: str,
    fingerprint: dict[str, Any],
    *,
    initial_state: InstallState = "resolving",
    owner: dict[str, Any] | None = None,
    journal_path: str | Path | None = None,
) -> InstallStatus:
    """Start a new install attempt for provider and target fingerprint."""
    if initial_state not in IN_FLIGHT_STATES:
        raise ValueError("initial install attempt state must be in-flight")
    fingerprint_json = canonical_fingerprint(fingerprint)
    status = read_install_status(name=provider, journal_path=journal_path)
    status["target_fingerprint_json"] = fingerprint_json
    status["target_fingerprint_sha256"] = fingerprint_sha256(fingerprint_json)
    status["attempt_id"] = uuid.uuid4().hex
    status["owner"] = owner
    return write_install_status(
        transition_state(status, new_state=initial_state),
        journal_path=journal_path,
    )


def begin_or_replace_install_attempt(
    provider: str,
    fingerprint: dict[str, Any],
    *,
    initial_state: InstallState = "resolving",
    owner: dict[str, Any] | None = None,
    journal_path: str | Path | None = None,
) -> InstallStatus:
    """Begin an attempt after the caller owns the provider lease."""
    current = read_install_status(name=provider, journal_path=journal_path)
    if current["install_state"] in IN_FLIGHT_STATES:
        record_interrupted_install(
            provider,
            attempt_id=str(current["attempt_id"]),
            target_fingerprint_sha256=current["target_fingerprint_sha256"],
            journal_path=journal_path,
        )
    return begin_install_attempt(
        provider,
        fingerprint,
        initial_state=initial_state,
        owner=owner,
        journal_path=journal_path,
    )


def assert_install_attempt_current(
    status: InstallStatus,
    *,
    journal_path: str | Path | None = None,
) -> InstallStatus:
    """Return current status if the same in-flight attempt still owns the slot."""
    attempt = _coerce_status(status)
    current = read_install_status(name=attempt["provider"], journal_path=journal_path)
    if current["install_state"] not in IN_FLIGHT_STATES:
        raise InstallStatusConflictError("install attempt is no longer in-flight")
    if current["attempt_id"] != attempt["attempt_id"]:
        raise InstallStatusConflictError("install attempt id changed")
    if current["target_fingerprint_sha256"] != attempt["target_fingerprint_sha256"]:
        raise InstallStatusConflictError("install target fingerprint changed")
    return current


def observe_install_attempt(
    provider: str,
    *,
    target_fingerprint_sha256: str,
    timeout_s: float,
    poll_interval_s: float = 1.0,
    progress_interval_s: float = 10.0,
    progress: Callable[[InstallStatus], None] | None = None,
    journal_path: str | Path | None = None,
) -> InstallStatus | None:
    """Poll the status record for a same-target in-flight attempt to finish."""
    deadline = time.monotonic() + timeout_s
    last_progress_emit = 0.0
    last_progress_key: tuple[Any, ...] | None = None
    while True:
        status = read_install_status(name=provider, journal_path=journal_path)
        if status["target_fingerprint_sha256"] != target_fingerprint_sha256:
            return status
        if progress is not None:
            progress_key = (
                status["install_state"],
                status["progress_bytes_received"],
                status["progress_bytes_total"],
                status["install_error"],
                status["error_code"],
            )
            now = time.monotonic()
            if (
                last_progress_key is None
                or progress_key != last_progress_key
                or now - last_progress_emit >= progress_interval_s
            ):
                progress(status)
                last_progress_key = progress_key
                last_progress_emit = now
        if status["install_state"] in TERMINAL_STATES:
            return status
        if time.monotonic() >= deadline:
            return None
        time.sleep(poll_interval_s)


def transition_state(
    status: InstallStatus,
    *,
    new_state: InstallState,
    error: str | None = None,
    error_code: str | None = None,
) -> InstallStatus:
    if new_state not in _INSTALL_STATES:
        raise ValueError(f"unknown install state: {new_state}")
    current = _coerce_status(status)
    timestamp = now_iso()
    next_attempt_id = current["attempt_id"]
    if current["install_state"] in TERMINAL_STATES and new_state in IN_FLIGHT_STATES:
        next_attempt_id = uuid.uuid4().hex
    elif next_attempt_id is None and new_state != "idle":
        next_attempt_id = uuid.uuid4().hex

    is_terminal = new_state in TERMINAL_STATES
    return {
        "schema_version": SCHEMA_VERSION,
        "provider": current["provider"],
        "revision": current["revision"],
        "install_state": new_state,
        "attempt_id": None if new_state == "idle" else next_attempt_id,
        "target_fingerprint_json": current["target_fingerprint_json"],
        "target_fingerprint_sha256": current["target_fingerprint_sha256"],
        "started_at": (
            timestamp
            if current["install_state"] in TERMINAL_STATES
            and new_state in IN_FLIGHT_STATES
            else current["started_at"]
        ),
        "last_transition_at": timestamp,
        "last_progress_at": timestamp if new_state in IN_FLIGHT_STATES else None,
        "completed_at": timestamp if is_terminal and new_state != "idle" else None,
        "progress_bytes_received": (
            None if is_terminal else current["progress_bytes_received"]
        ),
        "progress_bytes_total": None
        if is_terminal
        else current["progress_bytes_total"],
        "install_error": error if new_state == "failed" else None,
        "error_code": error_code if new_state == "failed" else None,
        "owner": current["owner"],
    }


def bump_progress(
    status: InstallStatus,
    *,
    received: int | None = None,
    total: int | None = None,
) -> InstallStatus:
    current = _coerce_status(status)
    if current["install_state"] not in IN_FLIGHT_STATES:
        raise ValueError("install progress can only be bumped for in-flight states")
    return {
        **current,
        "last_progress_at": now_iso(),
        "progress_bytes_received": (
            _nonnegative_int(received)
            if received is not None
            else current["progress_bytes_received"]
        ),
        "progress_bytes_total": (
            _nonnegative_int(total)
            if total is not None
            else current["progress_bytes_total"]
        ),
    }


def read_install_status(
    *,
    name: str,
    journal_path: str | Path | None = None,
) -> InstallStatus:
    """Read provider install status; absent status is synthetic idle."""
    provider = _validate_provider(name)
    path = provider_status_path(provider, journal_path=journal_path)
    if not path.exists():
        return make_idle_status(provider)
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:
        raise InstallStatusMalformedError(f"malformed install status: {path}") from exc
    if not isinstance(data, dict):
        raise InstallStatusMalformedError(f"install status must be an object: {path}")
    return _coerce_status(data, provider=provider)


def write_install_status(
    status: InstallStatus,
    *,
    journal_path: str | Path | None = None,
) -> InstallStatus:
    """Write provider install status under a sidecar flock."""
    incoming = _coerce_status(status)
    path = provider_status_path(incoming["provider"], journal_path=journal_path)
    with hold_lock(path, mode=_STATUS_MODE):
        current = _read_current_unlocked(path, incoming["provider"])
        accepted = _accept_transition(current, incoming)
        if accepted is current:
            return current
        stored = {**accepted, "revision": current["revision"] + 1}
        atomic_replace(
            path,
            json.dumps(_persistable_status(stored), indent=2, sort_keys=True) + "\n",
            mode=_STATUS_MODE,
        )
        _record_progress_write(stored)
        return stored


def record_interrupted_install(
    provider: str,
    *,
    attempt_id: str,
    target_fingerprint_sha256: str | None,
    reason: str = "install_interrupted",
    journal_path: str | Path | None = None,
) -> InstallStatus:
    """Mark an interrupted in-flight attempt failed after the caller owns the lease."""
    current = read_install_status(name=provider, journal_path=journal_path)
    if current["install_state"] not in IN_FLIGHT_STATES:
        raise InstallStatusConflictError("only in-flight installs can be interrupted")
    if current["attempt_id"] != attempt_id:
        raise InstallStatusConflictError("interrupted attempt id does not match")
    if current["target_fingerprint_sha256"] != target_fingerprint_sha256:
        raise InstallStatusConflictError(
            "interrupted target fingerprint does not match"
        )
    return write_install_status(
        transition_state(
            current,
            new_state="failed",
            error=reason,
            error_code=reason,
        ),
        journal_path=journal_path,
    )


def migrate_legacy_provider_artifact_truth(
    *,
    journal_path: str | Path | None = None,
) -> dict[str, Any]:
    """Promote trustworthy legacy provider state into manifests/status records."""
    journal = Path(journal_path) if journal_path is not None else Path(get_journal())
    config = read_journal_config(journal)
    providers = config.get("providers")
    bundled = providers.get("bundled") if isinstance(providers, dict) else None
    if not isinstance(bundled, dict):
        cleanup = _cleanup_legacy_provider_install_config(
            clean_providers=frozenset(), journal_path=journal
        )
        return {"actions": [], "cleanup": cleanup}

    actions: list[dict[str, Any]] = []
    clean_providers: set[str] = set()
    for provider in ("local", "parakeet"):
        legacy = bundled.get(provider)
        if not _legacy_provider_has_operational_state(legacy):
            continue
        action = _migrate_legacy_provider(provider, legacy, journal)
        actions.append(action)
        if action.get("cleanup"):
            clean_providers.add(provider)

    cleanup = _cleanup_legacy_provider_install_config(
        clean_providers=frozenset(clean_providers), journal_path=journal
    )
    return {"actions": actions, "cleanup": cleanup}


def migrate_legacy_provider_install_state(
    *,
    journal_path: str | Path | None = None,
) -> dict[str, int]:
    """Remove legacy provider install operational fields from journal config."""
    return _cleanup_legacy_provider_install_config(
        clean_providers=PROVIDERS, journal_path=journal_path
    )


def _cleanup_legacy_provider_install_config(
    *,
    clean_providers: frozenset[str],
    journal_path: str | Path | None = None,
) -> dict[str, int]:
    """Remove legacy provider install fields after owner proof is established."""

    def apply(config: dict[str, Any]) -> JournalConfigMutation[dict[str, int]]:
        removed = 0
        moved = 0
        bundled = config.get("providers", {}).get("bundled")
        if isinstance(bundled, dict):
            local_record = bundled.get("local")
            if isinstance(local_record, dict) and "vulkan_device_index" in local_record:
                providers = config.setdefault("providers", {})
                owner_config = providers.setdefault("local", {})
                if isinstance(owner_config, dict):
                    value = local_record.pop("vulkan_device_index")
                    if owner_config.get("vulkan_device_index") != value:
                        owner_config["vulkan_device_index"] = value
                        moved += 1
                    removed += 1
            for provider in clean_providers:
                record = bundled.get(provider)
                if not isinstance(record, dict):
                    continue
                for key in _LEGACY_OPERATIONAL_KEYS:
                    if key in record:
                        record.pop(key, None)
                        removed += 1
        return JournalConfigMutation(
            changed=removed > 0 or moved > 0,
            value={"removed": removed, "moved": moved},
        )

    return mutate_journal_config(apply, journal_path=journal_path).value


def _legacy_provider_has_operational_state(value: object) -> bool:
    return isinstance(value, dict) and any(
        key in value for key in _LEGACY_OPERATIONAL_KEYS | {"vulkan_device_index"}
    )


def _migrate_legacy_provider(
    provider: str, legacy: object, journal: Path
) -> dict[str, Any]:
    from solstone.think.providers.install_lease import acquire_install_lease

    lease = acquire_install_lease(provider, journal_path=journal)
    if lease is None:
        return {
            "provider": provider,
            "status": "busy",
            "reason_code": "install_busy",
            "cleanup": False,
            "message": (
                f"{provider} install is in progress; legacy provider state will be "
                "retried on the next start."
            ),
        }
    try:
        if provider == "local":
            return _migrate_legacy_local(legacy, journal)
        return _migrate_legacy_parakeet(legacy, journal)
    finally:
        lease.release()


def _migrate_legacy_local(legacy: object, journal: Path) -> dict[str, Any]:
    from solstone.think.models import LOCAL_MODEL
    from solstone.think.providers import local_cuda, local_install, mlx_install

    readiness = local_install.inspect_readiness(LOCAL_MODEL)
    fingerprint = local_install.target_fingerprint(LOCAL_MODEL)
    if readiness.ready:
        _publish_installed_status("local", fingerprint, journal)
        return _ready_action("local", "already-ready")
    if readiness.status in {"proof-unavailable", "host-ineligible"}:
        return _not_promoted_action("local", readiness.status, readiness.reason_code)
    if not isinstance(legacy, dict) or legacy.get("install_state") != "installed":
        return _not_promoted_action(
            "local",
            "missing-or-mismatched",
            "legacy_status_not_installed",
        )
    if legacy.get("mlx_model_id") or legacy.get("mlx_snapshot_dir"):
        try:
            spec = mlx_install.resolve_model_spec(str(legacy.get("mlx_model_id") or ""))
        except Exception:
            spec = None
        if (
            spec is not None
            and legacy.get("mlx_revision") == spec.revision
            and mlx_install.inspect_readiness(spec.name).ready
        ):
            _publish_installed_status(
                "local", mlx_install.target_fingerprint(spec.name), journal
            )
            return _ready_action("local", "already-ready")
        return _not_promoted_action(
            "local",
            "missing-or-mismatched",
            "manifest_missing",
            message=(
                "Existing local MLX artifacts cannot be trusted because there is "
                "no Solstone manifest for the current pin. A reinstall will rebuild "
                "the proof rather than trusting the old tree."
            ),
        )

    choice = local_cuda.resolve_local_backend(local_install.cuda_server_pin())
    if choice.backend != "vulkan":
        return _not_promoted_action(
            "local", "missing-or-mismatched", "manifest_missing"
        )
    try:
        _verify_legacy_local_llama(legacy)
        local_install.write_vulkan_manifest(
            artifact_key=local_install.llama_server_artifact_key(),
            pin=local_install.pin_for_current_platform(),
            attempt_status=None,
            fingerprint=fingerprint,
        )
        local_install.write_model_manifest(
            model_id=LOCAL_MODEL,
            attempt_status=None,
            fingerprint=fingerprint,
        )
    except OSError:
        return _not_promoted_action("local", "proof-unavailable", "legacy_io_error")
    except Exception as exc:
        return _not_promoted_action(
            "local",
            "missing-or-mismatched",
            getattr(exc, "reason_code", "manifest_missing"),
            message=(
                "Existing local artifacts cannot be trusted because there is no "
                "Solstone manifest for the current pin. A reinstall will rebuild "
                "the proof rather than trusting the old tree."
            ),
        )
    final = local_install.inspect_readiness(LOCAL_MODEL)
    if not final.ready:
        return _not_promoted_action("local", final.status, final.reason_code)
    _publish_installed_status("local", fingerprint, journal)
    return _ready_action("local", "promoted")


def _migrate_legacy_parakeet(legacy: object, journal: Path) -> dict[str, Any]:
    from solstone.think.providers import parakeet_install

    readiness = parakeet_install.inspect_readiness(journal)
    fingerprint = parakeet_install.target_fingerprint(journal_path=journal)
    if readiness.ready:
        _publish_installed_status("parakeet", fingerprint, journal)
        return _ready_action("parakeet", "already-ready")
    if readiness.status in {"proof-unavailable", "host-ineligible"}:
        return _not_promoted_action("parakeet", readiness.status, readiness.reason_code)
    if not isinstance(legacy, dict) or legacy.get("install_state") != "installed":
        return _not_promoted_action(
            "parakeet",
            "missing-or-mismatched",
            "legacy_status_not_installed",
        )
    try:
        _verify_legacy_parakeet(legacy, journal)
        for backend in ("cpu", "vulkan"):
            parakeet_install._write_binary_manifest(
                backend=backend,
                attempt_status=None,
                fingerprint=fingerprint,
                journal_path=journal,
            )
        parakeet_install._write_model_manifest(
            attempt_status=None,
            fingerprint=fingerprint,
            journal_path=journal,
        )
    except OSError:
        return _not_promoted_action("parakeet", "proof-unavailable", "legacy_io_error")
    except Exception as exc:
        return _not_promoted_action(
            "parakeet",
            "missing-or-mismatched",
            getattr(exc, "reason_code", "manifest_missing"),
        )
    final = parakeet_install.inspect_readiness(journal)
    if not final.ready:
        return _not_promoted_action("parakeet", final.status, final.reason_code)
    _publish_installed_status("parakeet", fingerprint, journal)
    return _ready_action("parakeet", "promoted")


def _verify_legacy_local_llama(legacy: dict[str, Any]) -> None:
    from solstone.think.models import LOCAL_MODEL
    from solstone.think.providers import local_install
    from solstone.think.providers.local import LOCAL_MODEL_SPECS

    artifact_key = local_install.llama_server_artifact_key()
    pin = local_install.pin_for_current_platform()
    spec = LOCAL_MODEL_SPECS[LOCAL_MODEL]
    expected_binary = local_install.binary_path_for_pin(artifact_key, pin)
    if legacy.get("binary_artifact") != artifact_key:
        raise ValueError("legacy_binary_artifact_mismatch")
    if legacy.get("binary_sha256") != pin["sha256"]:
        raise ValueError("legacy_binary_pin_mismatch")
    if legacy.get("binary_path") != str(expected_binary):
        raise ValueError("legacy_binary_path_mismatch")
    if not expected_binary.is_file() or not (expected_binary.stat().st_mode & 0o111):
        raise ValueError("legacy_binary_missing")
    if legacy.get("model_id") != spec.model_id:
        raise ValueError("legacy_model_id_mismatch")
    if legacy.get("model_path") != str(local_install.model_path(spec.model_id)):
        raise ValueError("legacy_model_path_mismatch")
    local_install.verify_artifact_sha256(local_install.model_path(spec.model_id), spec.sha256)
    if spec.mmproj_sha256:
        projector = local_install.mmproj_path(spec.model_id)
        if projector is None or legacy.get("mmproj_path") != str(projector):
            raise ValueError("legacy_projector_path_mismatch")
        local_install.verify_artifact_sha256(projector, spec.mmproj_sha256)


def _verify_legacy_parakeet(legacy: dict[str, Any], journal: Path) -> None:
    from solstone.think import parakeet_readiness
    from solstone.think.providers import parakeet_install

    artifact_key = parakeet_install.parakeet_server_artifact_key()
    for backend in ("cpu", "vulkan"):
        pin = parakeet_install._pin_for_backend(artifact_key, backend)
        if legacy.get(f"binary_artifact_{backend}") != artifact_key:
            raise ValueError(f"legacy_binary_artifact_{backend}_mismatch")
        if legacy.get(f"binary_sha256_{backend}") != pin["sha256"]:
            raise ValueError(f"legacy_binary_pin_{backend}_mismatch")
        if legacy.get(f"binary_path_{backend}") != str(
            parakeet_install.binary_path(backend, journal)
        ):
            raise ValueError(f"legacy_binary_path_{backend}_mismatch")
    parakeet_readiness.check_parakeet_cpp_files(journal)
    spec = parakeet_install.PARAKEET_MODEL_SPEC
    if legacy.get("model_repo") != spec.repo:
        raise ValueError("legacy_model_repo_mismatch")
    if legacy.get("model_filename") != spec.filename:
        raise ValueError("legacy_model_filename_mismatch")
    if legacy.get("model_revision") != spec.revision:
        raise ValueError("legacy_model_revision_mismatch")
    if legacy.get("model_path") != str(parakeet_install.model_path(journal)):
        raise ValueError("legacy_model_path_mismatch")
    parakeet_install._verify_sha256(parakeet_install.model_path(journal), spec.sha256)


def _publish_installed_status(
    provider: str,
    fingerprint: dict[str, Any],
    journal: Path,
) -> InstallStatus:
    current = read_install_status(name=provider, journal_path=journal)
    if current["install_state"] in IN_FLIGHT_STATES:
        raise InstallStatusConflictError("cannot migrate while install is in flight")
    fingerprint_json = canonical_fingerprint(fingerprint)
    current["target_fingerprint_json"] = fingerprint_json
    current["target_fingerprint_sha256"] = fingerprint_sha256(fingerprint_json)
    current["owner"] = {"entry": "legacy_provider_install_state_migration"}
    return write_install_status(
        transition_state(current, new_state="installed"),
        journal_path=journal,
    )


def _ready_action(provider: str, action: str) -> dict[str, Any]:
    return {
        "provider": provider,
        "status": "ready",
        "reason_code": "ready",
        "action": action,
        "cleanup": True,
        "message": f"{provider} provider install state migrated.",
    }


def _not_promoted_action(
    provider: str,
    status: str,
    reason_code: str,
    *,
    message: str | None = None,
) -> dict[str, Any]:
    if message is None:
        message = (
            f"{provider} provider legacy install state was not promoted: {reason_code}."
        )
    return {
        "provider": provider,
        "status": status,
        "reason_code": reason_code,
        "action": "not-promoted",
        "cleanup": False,
        "message": message,
    }


def _read_current_unlocked(path: Path, provider: ProviderName) -> InstallStatus:
    if not path.exists():
        return make_idle_status(provider)
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:
        raise InstallStatusMalformedError(f"malformed install status: {path}") from exc
    if not isinstance(data, dict):
        raise InstallStatusMalformedError(f"install status must be an object: {path}")
    return _coerce_status(data, provider=provider)


def _accept_transition(
    current: InstallStatus,
    incoming: InstallStatus,
) -> InstallStatus:
    if incoming["provider"] != current["provider"]:
        raise InstallStatusConflictError("provider mismatch")

    same_attempt = (
        incoming["attempt_id"] is not None
        and incoming["attempt_id"] == current["attempt_id"]
    )
    if current["install_state"] in TERMINAL_STATES and same_attempt:
        return current
    if current["install_state"] in TERMINAL_STATES:
        if incoming["revision"] != current["revision"]:
            raise InstallStatusConflictError("stale install status revision")
        _validate_new_attempt_from_terminal(current, incoming)
        return incoming
    if current["install_state"] in IN_FLIGHT_STATES:
        if not same_attempt:
            raise InstallStatusConflictError(
                "different attempt while install in-flight"
            )
        if incoming["revision"] != current["revision"]:
            raise InstallStatusConflictError("stale install status revision")
        if incoming["install_state"] in IN_FLIGHT_STATES:
            return incoming if _should_write_in_flight(current, incoming) else current
        if incoming["install_state"] in {"installed", "failed"}:
            return incoming
    raise InstallStatusConflictError("illegal install status transition")


def _validate_new_attempt_from_terminal(
    current: InstallStatus,
    incoming: InstallStatus,
) -> None:
    if incoming["install_state"] == "idle":
        return
    if incoming["attempt_id"] is None:
        raise InstallStatusConflictError("non-idle install status requires attempt id")
    if incoming["install_state"] in IN_FLIGHT_STATES:
        if incoming["attempt_id"] == current["attempt_id"]:
            raise InstallStatusConflictError("new in-flight attempt reused attempt id")
        return
    if incoming["install_state"] in {"installed", "failed"}:
        return
    raise InstallStatusConflictError("new attempt must be in-flight or terminal")


def _should_write_in_flight(
    current: InstallStatus,
    incoming: InstallStatus,
) -> bool:
    if current["install_state"] != incoming["install_state"]:
        return True
    if current["progress_bytes_total"] != incoming["progress_bytes_total"]:
        return True
    if (
        current["progress_bytes_received"] == incoming["progress_bytes_received"]
        and current["last_progress_at"] == incoming["last_progress_at"]
    ):
        return False
    attempt_id = incoming["attempt_id"]
    if attempt_id is None:
        return True
    key = (incoming["provider"], attempt_id)
    last = _LAST_PROGRESS_WRITE_MONOTONIC.get(key)
    now = time.monotonic()
    return last is None or now - last >= PROGRESS_COALESCE_SECONDS


def _record_progress_write(status: InstallStatus) -> None:
    attempt_id = status["attempt_id"]
    if attempt_id is None or status["install_state"] not in IN_FLIGHT_STATES:
        return
    _LAST_PROGRESS_WRITE_MONOTONIC[(status["provider"], attempt_id)] = time.monotonic()


def _coerce_status(
    data: dict[str, Any] | InstallStatus,
    *,
    provider: ProviderName | None = None,
) -> InstallStatus:
    raw_provider = provider or data.get("provider")
    validated_provider = _validate_provider(raw_provider)
    state = data.get("install_state")
    if state not in _INSTALL_STATES:
        raise InstallStatusMalformedError(
            f"invalid install_state for {validated_provider}"
        )
    schema_version = data.get("schema_version", SCHEMA_VERSION)
    if schema_version != SCHEMA_VERSION:
        raise InstallStatusMalformedError(
            f"unsupported install status schema_version for {validated_provider}"
        )
    revision = data.get("revision", 0)
    if isinstance(revision, bool) or not isinstance(revision, int) or revision < 0:
        raise InstallStatusMalformedError(f"invalid revision for {validated_provider}")
    attempt_id = data.get("attempt_id")
    if attempt_id is not None and not isinstance(attempt_id, str):
        raise InstallStatusMalformedError(
            f"invalid attempt_id for {validated_provider}"
        )
    fingerprint_json = data.get("target_fingerprint_json")
    fingerprint_sha = data.get("target_fingerprint_sha256")
    if fingerprint_json is not None and not isinstance(fingerprint_json, str):
        raise InstallStatusMalformedError("target_fingerprint_json must be a string")
    if fingerprint_sha is not None and not isinstance(fingerprint_sha, str):
        raise InstallStatusMalformedError("target_fingerprint_sha256 must be a string")
    owner = data.get("owner")
    if owner is not None and not isinstance(owner, dict):
        raise InstallStatusMalformedError("owner must be an object or null")
    return {
        "schema_version": SCHEMA_VERSION,
        "provider": validated_provider,
        "revision": revision,
        "install_state": cast(InstallState, state),
        "attempt_id": attempt_id,
        "target_fingerprint_json": fingerprint_json,
        "target_fingerprint_sha256": fingerprint_sha,
        "started_at": _optional_str(data.get("started_at")),
        "last_transition_at": _optional_str(data.get("last_transition_at")),
        "last_progress_at": _optional_str(data.get("last_progress_at")),
        "completed_at": _optional_str(data.get("completed_at")),
        "progress_bytes_received": _optional_int(data.get("progress_bytes_received")),
        "progress_bytes_total": _optional_int(data.get("progress_bytes_total")),
        "install_error": _optional_str(data.get("install_error")),
        "error_code": _optional_str(data.get("error_code")),
        "owner": owner,
    }


def _persistable_status(status: InstallStatus) -> dict[str, Any]:
    return dict(status)


def _optional_str(value: Any) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str):
        raise InstallStatusMalformedError("expected string or null")
    return value


def _optional_int(value: Any) -> int | None:
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, int):
        raise InstallStatusMalformedError("expected integer or null")
    return value


def _nonnegative_int(value: int | None) -> int | None:
    if value is None:
        return None
    result = int(value)
    return max(0, result)


def _validate_provider(value: object) -> ProviderName:
    if value not in PROVIDERS:
        raise ValueError(f"provider install status must be one of: {sorted(PROVIDERS)}")
    return cast(ProviderName, value)


def _normalize_fingerprint_value(value: Any) -> Any:
    if isinstance(value, dict):
        return {
            str(key): _normalize_fingerprint_value(value[key])
            for key in sorted(value, key=lambda item: str(item))
        }
    if isinstance(value, (list, tuple)):
        normalized_items = [_normalize_fingerprint_value(item) for item in value]
        return sorted(
            normalized_items,
            key=lambda item: json.dumps(
                item,
                sort_keys=True,
                separators=(",", ":"),
                ensure_ascii=True,
            ),
        )
    if isinstance(value, Path):
        return str(value)
    return value


__all__ = [
    "InstallState",
    "InstallStateError",
    "InstallStatus",
    "InstallStatusConflictError",
    "InstallStatusMalformedError",
    "IN_FLIGHT_STATES",
    "PROGRESS_COALESCE_SECONDS",
    "PROVIDERS",
    "ProviderName",
    "SCHEMA_VERSION",
    "TERMINAL_STATES",
    "assert_install_attempt_current",
    "begin_install_attempt",
    "begin_or_replace_install_attempt",
    "bump_progress",
    "canonical_fingerprint",
    "fingerprint_sha256",
    "make_idle_status",
    "migrate_legacy_provider_artifact_truth",
    "migrate_legacy_provider_install_state",
    "now_iso",
    "observe_install_attempt",
    "provider_status_path",
    "read_install_status",
    "record_interrupted_install",
    "transition_state",
    "write_install_status",
]
