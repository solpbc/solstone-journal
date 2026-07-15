# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import copy
import json
import logging
import os
import platform
import re
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from flask import Blueprint, current_app, jsonify, request

from solstone.apps.chat import copy as chat_copy
from solstone.apps.chat.config import (
    THINKING_SURFACES_VALUES,
    load_chat_config,
    save_chat_config,
)
from solstone.apps.settings import copy as settings_copy
from solstone.apps.settings import install_copy, transcribe_resource
from solstone.apps.utils import log_app_action
from solstone.convey import chat_stream, state
from solstone.convey import copy as convey_copy
from solstone.convey.icons import resolve_facet_icon_svg
from solstone.convey.reasons import (
    ACTIVITY_INVALID,
    ACTIVITY_NOT_FOUND,
    ACTIVITY_PROTECTED,
    FACET_ALREADY_EXISTS,
    FACET_NOT_FOUND,
    FILE_READ_FAILED,
    INVALID_CONFIG_VALUE,
    INVALID_REQUEST_VALUE,
    MISSING_REQUEST_BODY,
    MISSING_REQUIRED_FIELD,
    SETTINGS_OPERATION_FAILED,
)
from solstone.convey.sol_initiated import copy as sol_voice_copy
from solstone.convey.sol_initiated.copy import KIND_SOL_CHAT_REQUEST
from solstone.convey.sol_initiated.policy import compute_category_mute_state
from solstone.convey.sol_initiated.settings import (
    SolVoiceSettings,
)
from solstone.convey.sol_initiated.settings import (
    load_settings as load_sol_voice_settings,
)
from solstone.convey.sol_initiated.settings import (
    save_settings as save_sol_voice_settings,
)
from solstone.convey.utils import error_response, respond_collection
from solstone.think import facets
from solstone.think.journal_config import (
    hold_config_lock,
    write_journal_config,
)
from solstone.think.log_retention import load_log_retention_config, prune
from solstone.think.processing import (
    load_processing_settings,
    validate_processing_update,
)
from solstone.think.retention import (
    _human_bytes,
    check_storage_health,
    compute_storage_summary,
    load_retention_config,
    purge,
)
from solstone.think.schedule_config import read_schedules, set_schedule_entries
from solstone.think.streams import list_streams
from solstone.think.utils import (
    CorruptConfigError,
    get_journal,
    now_ms,
)
from solstone.think.utils import get_config as get_journal_config

logger = logging.getLogger(__name__)

settings_bp = Blueprint(
    "app:settings",
    __name__,
    url_prefix="/app/settings",
    static_folder="static",
    static_url_path="/static",
)


GENERIC_SETTINGS_ERROR = (
    "something went wrong — try again, and if it persists, check the health dashboard"
)


def _settings_operation_failed(detail: str = GENERIC_SETTINGS_ERROR) -> Any:
    return error_response(SETTINGS_OPERATION_FAILED, detail=detail)


def _serialize_prune_result(result: Any) -> dict[str, Any]:
    return {
        "enabled": result.enabled,
        "dry_run": result.dry_run,
        "days": result.days,
        "cutoff_day": result.cutoff_day,
        "files_deleted": result.files_deleted,
        "dirs_deleted": result.dirs_deleted,
        "bytes_freed": result.bytes_freed,
        "bytes_freed_human": _human_bytes(result.bytes_freed),
        "by_class": result.by_class,
        "by_day": result.by_day,
        "root_task_log": result.root_task_log,
        "errors": result.errors,
        "audit_written": result.audit_written,
        "partial_error": result.partial_error,
    }


def _public_facet_record(name: str, data: dict[str, object]) -> dict[str, object]:
    return {
        "name": name,
        "title": str(data.get("title") or name),
        "color": str(data.get("color") or ""),
        "emoji": str(data.get("emoji") or ""),
        "icon": str(data.get("icon") or ""),
        "icon_svg": resolve_facet_icon_svg(
            data.get("icon"), str(data.get("emoji") or "")
        ),
        "muted": bool(data.get("muted", False)),
    }


# API keys that can be configured in the env section
# Used for system env checks and allowed env fields validation
API_KEY_ENV_VARS = [
    "REVAI_ACCESS_TOKEN",
    "PLAUD_ACCESS_TOKEN",
]
SERVICE_KEY_VALIDATION_KEYS = frozenset({"revai", "plaud"})


def _compute_runtime_label() -> str:
    os_name = platform.system().lower()
    arch = platform.machine().lower()
    if os_name == "darwin" and arch == "arm64":
        return "macOS CoreML helper"
    if os_name != "linux" or arch != "x86_64":
        return "unsupported"
    return "Linux parakeet.cpp"


def _service_key_validation(config: dict[str, Any]) -> dict[str, Any]:
    key_validation = config.get("service_key_validation", {})
    if not isinstance(key_validation, dict):
        key_validation = {}
    return {
        key: value
        for key, value in key_validation.items()
        if key in SERVICE_KEY_VALIDATION_KEYS
    }


def _project_transcribe_config(
    transcribe_config: Any,
    *,
    include_confidential_audio: bool = False,
) -> dict[str, Any]:
    """Return the public transcribe config for currently supported backends."""
    if not isinstance(transcribe_config, dict):
        return {}

    from solstone.observe.transcribe import BACKEND_METADATA, BACKEND_REGISTRY
    from solstone.observe.transcribe.config import confidential_audio_enabled

    supported_backends = set(BACKEND_REGISTRY)
    projected: dict[str, Any] = {}
    for key, value in transcribe_config.items():
        if key in supported_backends:
            if not isinstance(value, dict):
                continue
            allowed = set(BACKEND_METADATA.get(key, {}).get("settings", []))
            projected[key] = {
                nested_key: copy.deepcopy(nested_value)
                for nested_key, nested_value in value.items()
                if nested_key in allowed
            }
            continue

        # Backend subtrees are dict-shaped. Drop stale removed backend configs
        # such as the former Whisper subtree while preserving scalar settings.
        if isinstance(value, dict):
            continue
        projected[key] = copy.deepcopy(value)

    if projected.get("backend") not in supported_backends:
        projected["backend"] = "parakeet"
    if include_confidential_audio:
        projected["confidential_audio"] = confidential_audio_enabled(transcribe_config)
    return projected


def _project_public_config(config: dict[str, Any]) -> dict[str, Any]:
    projected = copy.deepcopy(config)
    service_validation = _service_key_validation(config)
    if service_validation:
        projected["key_validation"] = service_validation
    projected.pop("service_key_validation", None)
    projected["env"] = {
        k: bool((projected.get("env") or {}).get(k)) for k in API_KEY_ENV_VARS
    }
    projected.pop("providers", None)
    convey_config = projected.setdefault("convey", {})
    convey_config.pop("secret", None)
    convey_config.pop("password_hash", None)
    convey_config.pop("password", None)
    if "transcribe" in projected:
        projected["transcribe"] = _project_transcribe_config(projected["transcribe"])
    projected["runtime_env"] = {k: bool(os.getenv(k)) for k in API_KEY_ENV_VARS}
    return projected


def _uppercase_copy_payload(module: Any) -> dict[str, Any]:
    return {
        name: value
        for name, value in vars(module).items()
        if name.isupper() and not name.startswith("_")
    }


def _settings_state_payload() -> dict[str, Any]:
    return {
        "settings_copy": _uppercase_copy_payload(settings_copy),
        "install_copy": {
            name: getattr(install_copy, name) for name in install_copy.__all__
        },
        "chat_copy": _uppercase_copy_payload(chat_copy),
        "sol_voice_copy": _uppercase_copy_payload(sol_voice_copy),
        "thinking_surfaces": load_chat_config().get("thinking_surfaces"),
    }


@settings_bp.route("/facets/<slug>")
def view_facet_detail(slug: str) -> str:
    return current_app.send_static_file("shell.html")


@settings_bp.route("/api/state")
def api_state() -> Any:
    try:
        return jsonify(_settings_state_payload())
    except Exception:
        logger.exception("settings state load failed")
        return error_response(
            FILE_READ_FAILED,
            detail="Failed to load settings state.",
        )


@settings_bp.route("/api/config")
def get_config() -> Any:
    """Return the journal configuration.

    The env section is masked for security - returns boolean indicating
    whether each key is configured rather than the actual values.

    Also returns runtime_env with boolean status for keys loaded into
    the process environment (from journal.json via setup_cli).
    """
    try:
        return jsonify(_project_public_config(get_journal_config()))
    except CorruptConfigError:
        raise
    except Exception:
        logger.exception("error loading config")
        return _settings_operation_failed()


@settings_bp.route("/api/config", methods=["PUT", "POST"])
def update_config() -> Any:
    """Update the journal configuration.

    Accepts JSON with a 'section' key and per-section config fields to update.
    Supported writes include identity and transcribe settings, and API-key env vars.
    """
    try:
        request_data = request.get_json()
        if not request_data:
            return error_response(MISSING_REQUEST_BODY, detail="No data provided")

        section = request_data.get("section")
        data = request_data.get("data", {})
        request_key = request_data.get("key")
        if section and request_key is not None and "value" in request_data and not data:
            data = {request_key: request_data.get("value")}

        # Backward compatibility: if no section specified but identity key exists
        if not section and "identity" in request_data:
            section = "identity"
            data = request_data["identity"]

        if not section:
            return error_response(
                MISSING_REQUIRED_FIELD,
                detail="No section specified",
            )

        # Define allowed fields per section
        # For transcribe, we have flat fields plus nested backend configs
        allowed_sections = {
            "identity": [
                "name",
                "preferred",
                "bio",
                "pronouns",
                "aliases",
                "email_addresses",
                "timezone",
            ],
            "journal": ["name"],
            "transcribe": [
                "backend",
                "enrich",
                "preserve_all",
                "noise_upgrade",
                "confidential_audio",
            ],
            "support": ["enabled", "proactive", "anonymous_feedback", "portal_url"],
            "agent": ["name", "name_status", "named_date"],
            "env": API_KEY_ENV_VARS,
            "processing": [],
        }

        # Nested config schemas for transcribe backends - built from BACKEND_METADATA
        from solstone.observe.transcribe import BACKEND_METADATA

        transcribe_nested = {
            name: meta.get("settings", [])
            for name, meta in BACKEND_METADATA.items()
            if meta.get("settings")
        }

        if section not in allowed_sections:
            return error_response(
                INVALID_CONFIG_VALUE,
                detail=f"Unknown section: {section}",
            )

        if section == "journal" and "name" in data:
            name_value = data["name"]
            if isinstance(name_value, str) and not name_value.strip():
                return error_response(
                    INVALID_CONFIG_VALUE,
                    detail="Journal name cannot be empty",
                )

        with hold_config_lock():
            # Load existing config
            config = get_journal_config()
            old_section = copy.deepcopy(config.get(section, {}))

            # Ensure section exists
            if section not in config:
                config[section] = {}

            # Track changes for logging
            changed_fields = {}

            # Update only allowed fields
            for key in allowed_sections[section]:
                if key in data:
                    new_value = data[key]
                    old_value = old_section.get(key)
                    if old_value != new_value:
                        changed_fields[key] = {"old": old_value, "new": new_value}
                    config[section][key] = new_value
                    if section == "env":
                        if new_value:
                            os.environ[key] = new_value
                        else:
                            os.environ.pop(key, None)

            if section == "processing":
                try:
                    validated = validate_processing_update(old_section, data)
                except ValueError as exc:
                    return error_response(INVALID_CONFIG_VALUE, detail=str(exc))
                new_section = validated.to_dict()
                if old_section != new_section:
                    changed_fields["processing"] = {
                        "old": old_section,
                        "new": new_section,
                    }
                config["processing"] = new_section

            # Handle nested backend configs for transcribe section
            if section == "transcribe":
                if "backend" in data:
                    from solstone.observe.transcribe import get_backend_list

                    selectable = {item["name"] for item in get_backend_list()}
                    if data["backend"] not in selectable:
                        valid = ", ".join(sorted(selectable))
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail=(
                                f"Invalid backend: {data['backend']}. "
                                f"Must be one of: {valid}"
                            ),
                        )
                for bool_key in (
                    "enrich",
                    "preserve_all",
                    "noise_upgrade",
                    "confidential_audio",
                ):
                    if bool_key in data and not isinstance(data[bool_key], bool):
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail=f"transcribe.{bool_key} must be a boolean",
                        )
                for backend_key, allowed_keys in transcribe_nested.items():
                    if backend_key in data and isinstance(data[backend_key], dict):
                        # Ensure nested dict exists
                        if backend_key not in config[section]:
                            config[section][backend_key] = {}
                        old_backend = old_section.get(backend_key, {})
                        # Update only allowed nested fields
                        for nested_key in allowed_keys:
                            if nested_key in data[backend_key]:
                                new_value = data[backend_key][nested_key]
                                old_value = old_backend.get(nested_key)
                                if old_value != new_value:
                                    changed_fields[f"{backend_key}.{nested_key}"] = {
                                        "old": old_value,
                                        "new": new_value,
                                    }
                                config[section][backend_key][nested_key] = new_value

            if section == "env" and changed_fields:
                key_validation = config.setdefault("service_key_validation", {})

                # Validate service tokens (Rev.ai, Plaud) — not AI providers,
                # so they use their own validators instead of think.providers.
                SERVICE_TOKEN_VALIDATORS = {
                    "REVAI_ACCESS_TOKEN": (
                        "revai",
                        "solstone.observe.transcribe.revai",
                    ),
                    "PLAUD_ACCESS_TOKEN": (
                        "plaud",
                        "solstone.think.importers.plaud",
                    ),
                }
                for env_var in changed_fields:
                    if env_var in SERVICE_TOKEN_VALIDATORS:
                        val_key, module_path = SERVICE_TOKEN_VALIDATORS[env_var]
                        new_val = data.get(env_var, "")
                        if new_val:
                            import importlib

                            mod = importlib.import_module(module_path)
                            result = mod.validate_token(new_val)
                            result["timestamp"] = datetime.now(timezone.utc).isoformat()
                            key_validation[val_key] = result
                        else:
                            key_validation.pop(val_key, None)

            write_journal_config(config)

        # Log if something changed (don't log sensitive values)
        if changed_fields:
            log_fields = changed_fields
            if section == "env":
                # Don't log actual API key values
                log_fields = {k: {"old": "***", "new": "***"} for k in changed_fields}

            log_app_action(
                app="settings",
                facet=None,
                action=f"{section}_update",
                params={"changed_fields": log_fields},
            )

        return jsonify(
            {
                "config": _project_public_config(config),
                "key_validation": _service_key_validation(config),
                "success": True,
            }
        )
    except CorruptConfigError:
        raise
    except Exception:
        logger.exception("error updating config")
        return _settings_operation_failed()


def _host_url_status_value() -> str:
    from solstone.think.pairing.config import get_host_url

    return get_host_url()


@settings_bp.route("/api/convey/host-url", methods=["GET", "POST"])
def convey_host_url() -> Any:
    """Read or update the host URL advertised to remote devices."""

    from solstone.think.pairing.config import (
        InvalidHostUrl,
        clear_host_url,
        get_host_url,
        set_host_url,
        validate_host_url,
    )

    try:
        if request.method == "GET":
            return jsonify({"host_url": get_host_url()})

        request_data = request.get_json()
        if not isinstance(request_data, dict):
            return error_response(
                INVALID_REQUEST_VALUE,
                detail="Expected JSON object with url or auto",
            )

        has_url = "url" in request_data and request_data.get("url") is not None
        auto = bool(request_data.get("auto", False))
        if sum((has_url, auto)) != 1:
            return error_response(
                INVALID_REQUEST_VALUE,
                detail="Provide exactly one of url or auto",
            )

        if auto:
            clear_host_url()
            return jsonify({"host_url": get_host_url(), "cleared": True})

        raw_url = request_data.get("url")
        if not isinstance(raw_url, str):
            return error_response(INVALID_REQUEST_VALUE, detail="url must be a string")
        try:
            canonical = validate_host_url(raw_url)
        except InvalidHostUrl as exc:
            return error_response(INVALID_CONFIG_VALUE, detail=str(exc))
        set_host_url(canonical)
        return jsonify({"host_url": canonical})
    except Exception:
        logger.exception("error updating convey host url")
        return _settings_operation_failed()


@settings_bp.route("/api/convey/status")
def convey_status() -> Any:
    """Return formatted Convey bind and host URL status."""

    try:
        from solstone.convey.cli import _resolve_bind_host
        from solstone.think.service import DEFAULT_SERVICE_PORT
        from solstone.think.utils import read_service_port

        bind_host = _resolve_bind_host()
        port = read_service_port("convey") or DEFAULT_SERVICE_PORT
        status_text = convey_copy.format_convey_status(
            bind=f"{bind_host}:{port}",
            host_url=_host_url_status_value(),
        )
        return jsonify({"status_text": status_text})
    except Exception:
        logger.exception("error loading convey status")
        return _settings_operation_failed()


# ---------------------------------------------------------------------------
# Transcribe API
# ---------------------------------------------------------------------------


@settings_bp.route("/api/transcribe")
def get_transcribe() -> Any:
    """Return transcribe backend configuration.

    Returns:
        - backends: List of available backends with metadata
        - api_keys: Boolean status for each backend's API key
        - config: Current transcribe config from journal
        - resource: Memory/platform display payload for the unset default
    """
    try:
        from solstone.observe.transcribe import get_backend_list

        config = get_journal_config()
        transcribe_config = _project_transcribe_config(
            config.get("transcribe", {}),
            include_confidential_audio=True,
        )

        # Get backends list from registry
        backends = get_backend_list()
        runtime_label = _compute_runtime_label()

        # Check API key status for each backend
        api_keys = {}
        for backend in backends:
            env_key = backend.get("env_key")
            if env_key:
                api_keys[backend["name"]] = bool(os.getenv(env_key))
            else:
                api_keys[backend["name"]] = True  # Local backends always available
        google_key_present = bool(api_keys.get("gemini"))
        configured_backend = transcribe_config.get("backend")
        confidential_audio = bool(transcribe_config.get("confidential_audio"))
        try:
            from solstone.think.services import spp

            confidential_lane_active = spp.confidential_provenance() is not None
            resource = transcribe_resource.get_transcribe_resource_payload(
                google_key_present=google_key_present,
                configured_backend=configured_backend,
                confidential_lane_active=confidential_lane_active,
                confidential_audio=confidential_audio,
            )
        except Exception:
            logger.exception("error loading transcribe resource payload")
            resource = transcribe_resource.fallback_transcribe_resource_payload()

        return jsonify(
            {
                "backends": backends,
                "api_keys": api_keys,
                "config": transcribe_config,
                "runtime_label": runtime_label,
                "parakeet_uses_cpp": (
                    platform.system().lower() == "linux"
                    and platform.machine().lower() == "x86_64"
                ),
                "resource": resource,
            }
        )
    except Exception:
        logger.exception("error loading transcribe config")
        return _settings_operation_failed()


@settings_bp.route("/api/processing")
def get_processing() -> Any:
    """Return effective deferred-processing settings."""
    try:
        return jsonify(load_processing_settings().to_dict())
    except Exception:
        logger.exception("error loading processing settings")
        return error_response(
            SETTINGS_OPERATION_FAILED,
            detail="unable to load processing settings",
        )


# ---------------------------------------------------------------------------
# Sol Voice API
# ---------------------------------------------------------------------------


@settings_bp.route("/api/sol_voice")
def get_sol_voice() -> Any:
    """Return sol-initiated chat settings."""
    try:
        return jsonify(_sol_voice_response(load_sol_voice_settings()))
    except Exception:
        logger.exception("error loading sol voice settings")
        return error_response(
            SETTINGS_OPERATION_FAILED,
            detail="unable to load sol voice settings",
        )


@settings_bp.route("/api/sol_voice", methods=["PUT"])
def update_sol_voice() -> Any:
    """Persist partial sol-initiated chat settings."""
    try:
        updates = request.get_json()
        if not isinstance(updates, dict):
            return error_response(
                INVALID_CONFIG_VALUE,
                detail="sol_voice update must be an object",
            )
        settings = save_sol_voice_settings(updates)
        return jsonify(_sol_voice_response(settings))
    except ValueError as exc:
        return error_response(INVALID_CONFIG_VALUE, detail=str(exc))
    except Exception:
        logger.exception("error saving sol voice settings")
        return error_response(
            SETTINGS_OPERATION_FAILED,
            detail="unable to save sol voice settings",
        )


# ---------------------------------------------------------------------------
# Chat API
# ---------------------------------------------------------------------------


@settings_bp.route("/api/chat")
def get_chat() -> Any:
    """Return chat display settings."""
    try:
        return jsonify(load_chat_config())
    except Exception:
        logger.exception("error loading chat settings")
        return error_response(
            SETTINGS_OPERATION_FAILED,
            detail="unable to load chat settings",
        )


@settings_bp.route("/api/chat", methods=["PUT"])
def update_chat() -> Any:
    """Persist partial chat display settings."""
    try:
        updates = request.get_json()
        if not isinstance(updates, dict):
            return error_response(
                INVALID_CONFIG_VALUE,
                detail="chat update must be an object",
            )
        thinking_surfaces = updates.get("thinking_surfaces")
        if (
            "thinking_surfaces" in updates
            and thinking_surfaces not in THINKING_SURFACES_VALUES
        ):
            logger.warning(
                "invalid chat thinking_surfaces value: %r", thinking_surfaces
            )
            return error_response(
                INVALID_CONFIG_VALUE,
                detail="invalid thinking_surfaces",
            )
        return jsonify(save_chat_config(updates))
    except Exception:
        logger.exception("error saving chat settings")
        return error_response(
            SETTINGS_OPERATION_FAILED,
            detail="unable to save chat settings",
        )


@settings_bp.route("/api/sol_voice/throttled")
def get_sol_voice_throttled() -> Any:
    """Return recent sol-initiated chat throttle rows."""
    raw_limit = request.args.get("limit", "50")
    try:
        limit = int(raw_limit)
    except (TypeError, ValueError):
        limit = 50
    limit = max(1, min(limit, 200))

    log_path = Path(get_journal()) / "push" / "nudge_log.jsonl"
    if not log_path.exists():
        return respond_collection([])

    try:
        rows = _read_sol_voice_throttled_rows(log_path, limit)
        return respond_collection(rows)
    except Exception:
        logger.exception("error loading sol voice throttled log")
        return error_response(FILE_READ_FAILED, detail="unable to load throttled log")


def _read_sol_voice_throttled_rows(log_path: Path, limit: int) -> list[dict[str, Any]]:
    lines = log_path.read_text(encoding="utf-8").splitlines()
    rows: list[dict[str, Any]] = []
    for line in reversed(lines[-limit * 4 :]):
        if len(rows) >= limit:
            break
        if not line.strip():
            continue
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if payload.get("kind") != KIND_SOL_CHAT_REQUEST:
            continue
        if payload.get("outcome") == "written":
            continue
        rows.append(
            {
                "ts": payload.get("ts"),
                "category": payload.get("category"),
                "dedupe_key": payload.get("dedupe_key"),
                "outcome": payload.get("outcome"),
            }
        )
    return rows


def _sol_voice_response(settings: SolVoiceSettings) -> dict[str, Any]:
    payload = settings.to_dict()
    current_ms = now_ms()
    events_today = chat_stream.read_chat_events(chat_stream._day_for_ts(current_ms))
    payload["category_mute_state"] = {
        category: compute_category_mute_state(
            settings,
            events_today,
            category,
            current_ms,
        )
        for category in sol_voice_copy.CATEGORIES
    }
    return payload


# ---------------------------------------------------------------------------
# Service Token Validation API
# ---------------------------------------------------------------------------


def _compute_key_validation(config: dict[str, Any]) -> dict[str, Any]:
    """Validate configured Rev.ai and Plaud tokens without mutating config."""

    env_config = config.get("env", {})
    key_validation: dict[str, Any] = {}
    service_token_validators = {
        "REVAI_ACCESS_TOKEN": ("revai", "solstone.observe.transcribe.revai"),
        "PLAUD_ACCESS_TOKEN": ("plaud", "solstone.think.importers.plaud"),
    }
    for env_var, (val_key, module_path) in service_token_validators.items():
        api_key = env_config.get(env_var, "")
        if api_key:
            import importlib

            mod = importlib.import_module(module_path)
            result = mod.validate_token(api_key)
            result["timestamp"] = datetime.now(timezone.utc).isoformat()
            key_validation[val_key] = result
    return key_validation


@settings_bp.route("/api/validate-keys", methods=["GET", "POST"])
def validate_all_keys() -> Any:
    """Re-validate configured transcription/import service tokens."""

    try:
        if request.method == "GET":
            config = get_journal_config()
            key_validation = _compute_key_validation(config)
            return jsonify({"key_validation": key_validation})

        with hold_config_lock():
            config = get_journal_config()
            key_validation = _compute_key_validation(config)
            existing = config.setdefault("service_key_validation", {})
            for key in ("revai", "plaud"):
                existing.pop(key, None)
            existing.update(key_validation)
            write_journal_config(config)

        return jsonify({"success": True, "key_validation": key_validation})
    except Exception:
        logger.exception("error validating service tokens")
        return _settings_operation_failed()


# ---------------------------------------------------------------------------
# Vision API
# ---------------------------------------------------------------------------

VALID_IMPORTANCE = {"high", "normal", "low", "ignore"}


@settings_bp.route("/api/vision")
def get_vision() -> Any:
    """Return vision configuration with category defaults.

    Returns:
        - max_extractions: Current max extractions setting (default: 20)
        - redact: List of redaction rules (default: [])
        - categories: Dict of category overrides from config
        - category_defaults: Discovered categories with their defaults
    """
    try:
        from solstone.observe.describe import CATEGORIES
        from solstone.observe.extract import DEFAULT_MAX_EXTRACTIONS

        config = get_journal_config()
        describe_config = config.get("describe", {})

        # Build category defaults from discovered categories
        category_defaults = {}
        for name, meta in CATEGORIES.items():
            category_defaults[name] = {
                "label": meta.get("label", name.replace("_", " ").title()),
                "group": meta.get("group", "Screen Analysis"),
                "extraction": meta.get("extraction", ""),
                "importance": meta.get("importance", "normal"),
            }

        return jsonify(
            {
                "max_extractions": describe_config.get(
                    "max_extractions", DEFAULT_MAX_EXTRACTIONS
                ),
                "redact": describe_config.get("redact", []),
                "categories": describe_config.get("categories", {}),
                "category_defaults": category_defaults,
            }
        )
    except Exception:
        logger.exception("error loading vision config")
        return _settings_operation_failed()


@settings_bp.route("/api/vision", methods=["PUT"])
def update_vision() -> Any:
    """Update vision configuration.

    Accepts JSON with optional keys:
        - max_extractions: int (5-100) - Maximum frames to extract
        - redact: list[str] - Redaction rules (max 50 rules, 200 chars each)
        - categories: {name: {importance?, extraction?} | null} - Category overrides

    Setting a category to null removes its overrides.
    """
    try:
        from solstone.observe.describe import CATEGORIES

        request_data = request.get_json()
        if not request_data:
            return error_response(MISSING_REQUEST_BODY, detail="No data provided")

        with hold_config_lock():
            # Load existing config
            config = get_journal_config()
            old_describe = copy.deepcopy(config.get("describe", {}))

            # Ensure describe section exists
            if "describe" not in config:
                config["describe"] = {}

            changed_fields = {}

            # Handle max_extractions update
            if "max_extractions" in request_data:
                max_ext = request_data["max_extractions"]
                if not isinstance(max_ext, int) or max_ext < 5 or max_ext > 100:
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail="max_extractions must be an integer between 5 and 100",
                    )
                old_val = old_describe.get("max_extractions")
                if old_val != max_ext:
                    changed_fields["max_extractions"] = {
                        "old": old_val,
                        "new": max_ext,
                    }
                config["describe"]["max_extractions"] = max_ext

            # Handle redact rules update
            if "redact" in request_data:
                redact = request_data["redact"]
                if not isinstance(redact, list) or not all(
                    isinstance(r, str) for r in redact
                ):
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail="redact must be a list of strings",
                    )
                if len(redact) > 50:
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail="redact may contain at most 50 rules",
                    )
                if any(len(r) > 200 for r in redact):
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail="each redact rule must be 200 characters or fewer",
                    )
                # Filter out empty strings
                redact = [r for r in redact if r.strip()]
                old_val = old_describe.get("redact")
                if old_val != redact:
                    changed_fields["redact"] = {"old": old_val, "new": redact}
                config["describe"]["redact"] = redact

            # Handle category overrides
            if "categories" in request_data:
                categories_data = request_data["categories"]
                if "categories" not in config["describe"]:
                    config["describe"]["categories"] = {}

                old_categories = old_describe.get("categories", {})

                for name, cat_config in categories_data.items():
                    # Validate category exists
                    if name not in CATEGORIES:
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail=f"Unknown category: {name}",
                        )

                    old_cat = old_categories.get(name)

                    # null means remove the override
                    if cat_config is None:
                        if name in config["describe"]["categories"]:
                            changed_fields[f"categories.{name}"] = {
                                "old": old_cat,
                                "new": None,
                            }
                            del config["describe"]["categories"][name]
                        continue

                    # Validate importance if specified
                    if "importance" in cat_config:
                        importance = cat_config["importance"]
                        if importance not in VALID_IMPORTANCE:
                            return error_response(
                                INVALID_CONFIG_VALUE,
                                detail=(
                                    f"Invalid importance for {name}: {importance}. "
                                    "Must be one of: "
                                    f"{', '.join(sorted(VALID_IMPORTANCE))}"
                                ),
                            )

                    # Validate extraction if specified (must be string)
                    if "extraction" in cat_config:
                        extraction = cat_config["extraction"]
                        if not isinstance(extraction, str):
                            return error_response(
                                INVALID_CONFIG_VALUE,
                                detail=f"extraction for {name} must be a string",
                            )

                    # Only store if there's something to override
                    if cat_config:
                        if old_cat != cat_config:
                            changed_fields[f"categories.{name}"] = {
                                "old": old_cat,
                                "new": cat_config,
                            }
                        config["describe"]["categories"][name] = cat_config

            write_journal_config(config)

        # Log if something changed
        if changed_fields:
            log_app_action(
                app="settings",
                facet=None,
                action="vision_update",
                params={"changed_fields": changed_fields},
            )

        # Return updated vision config
        return get_vision()

    except Exception:
        logger.exception("error saving vision config")
        return _settings_operation_failed()


# ---------------------------------------------------------------------------
# Observe API
# ---------------------------------------------------------------------------

# Default observe configuration - single source of truth for all defaults
OBSERVE_TMUX_DEFAULTS = {
    "enabled": True,
    "capture_interval": 5,
    "capture_interval_min": 1,
    "capture_interval_max": 60,
}


@settings_bp.route("/api/observe")
def get_observe() -> Any:
    """Return observe configuration with defaults and validation bounds.

    Returns:
        - tmux: Tmux capture settings
            - enabled: Whether tmux capture is enabled
            - capture_interval: Seconds between terminal captures
        - defaults: Default values and validation bounds for UI
    """
    try:
        config = get_journal_config()
        observe_config = config.get("observe", {})
        tmux_config = observe_config.get("tmux", {})

        # Build result with user config merged over defaults
        result = {
            "tmux": {
                "enabled": tmux_config.get("enabled", OBSERVE_TMUX_DEFAULTS["enabled"]),
                "capture_interval": tmux_config.get(
                    "capture_interval", OBSERVE_TMUX_DEFAULTS["capture_interval"]
                ),
            },
            "defaults": {
                "tmux": OBSERVE_TMUX_DEFAULTS,
            },
        }

        return jsonify(result)

    except Exception:
        logger.exception("error loading observe config")
        return _settings_operation_failed()


@settings_bp.route("/api/observe", methods=["PUT", "POST"])
def update_observe() -> Any:
    """Update observe configuration.

    Accepts JSON with optional keys:
        - tmux: {enabled?: bool, capture_interval?: int}
            - enabled: Whether tmux capture is enabled
            - capture_interval: Seconds between terminal captures (1-60)
    """
    try:
        request_data = request.get_json()
        if not request_data:
            return error_response(MISSING_REQUEST_BODY, detail="No data provided")

        with hold_config_lock():
            # Load existing config
            config = get_journal_config()
            old_observe = copy.deepcopy(config.get("observe", {}))

            # Ensure observe section exists
            if "observe" not in config:
                config["observe"] = {}

            changed_fields = {}

            # Handle tmux settings
            if "tmux" in request_data:
                tmux_data = request_data["tmux"]
                if not isinstance(tmux_data, dict):
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail="tmux must be an object",
                    )

                if "tmux" not in config["observe"]:
                    config["observe"]["tmux"] = {}

                old_tmux = old_observe.get("tmux", {})
                defaults = OBSERVE_TMUX_DEFAULTS

                # Validate and update enabled
                if "enabled" in tmux_data:
                    enabled = tmux_data["enabled"]
                    if not isinstance(enabled, bool):
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail="tmux.enabled must be a boolean",
                        )
                    if enabled != old_tmux.get("enabled", defaults["enabled"]):
                        config["observe"]["tmux"]["enabled"] = enabled
                        changed_fields["tmux.enabled"] = enabled

                # Validate and update capture_interval
                if "capture_interval" in tmux_data:
                    capture_interval = tmux_data["capture_interval"]
                    min_val = defaults["capture_interval_min"]
                    max_val = defaults["capture_interval_max"]
                    if (
                        not isinstance(capture_interval, int)
                        or capture_interval < min_val
                        or capture_interval > max_val
                    ):
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail=(
                                "tmux.capture_interval must be an integer between "
                                f"{min_val} and {max_val}"
                            ),
                        )
                    if capture_interval != old_tmux.get(
                        "capture_interval", defaults["capture_interval"]
                    ):
                        config["observe"]["tmux"]["capture_interval"] = capture_interval
                        changed_fields["tmux.capture_interval"] = capture_interval

            # Save config if changed
            if changed_fields:
                write_journal_config(config)

        if changed_fields:
            log_app_action(
                app="settings",
                facet=None,
                action="observe_update",
                params={"changed_fields": changed_fields},
            )

        return get_observe()

    except Exception:
        logger.exception("error saving observe config")
        return _settings_operation_failed()


@settings_bp.route("/api/facets")
def list_facets() -> Any:
    """List all facets."""
    try:
        from solstone.think.facets import get_facets

        facets = [
            _public_facet_record(name, data)
            for name, data in sorted(
                get_facets().items(),
                key=lambda item: str(item[1].get("title") or item[0]).lower(),
            )
        ]
        return jsonify({"facets": facets})
    except Exception:
        logger.exception("error loading facets")
        return _settings_operation_failed()


@settings_bp.route("/api/facets/muted")
def get_muted_facets() -> Any:
    """List muted facets."""
    try:
        from solstone.think.facets import get_facets

        facets = get_facets()
        muted = [
            _public_facet_record(name, data)
            for name, data in facets.items()
            if data.get("muted", False)
        ]
        return jsonify({"facets": muted})
    except Exception:
        logger.exception("error loading muted facets")
        return _settings_operation_failed()


@settings_bp.route("/api/icons")
def search_icons() -> Any:
    try:
        from solstone.convey.icons import search_lucide_icons

        q = request.args.get("q", "")
        limit = request.args.get("limit", default=80, type=int) or 80
        limit = max(1, min(limit, 200))
        return jsonify({"icons": search_lucide_icons(q, limit=limit)})
    except Exception:
        logger.exception("error searching icons")
        return _settings_operation_failed()


@settings_bp.route("/api/facet", methods=["POST"])
def create_facet() -> Any:
    """Create a new facet.

    Accepts JSON with:
        title: Display title (required)
        emoji: Icon emoji (optional, default: "📦")
        color: Hex color (optional, default: "#667eea")

    The facet name (slug) is auto-generated from the title.
    """
    try:
        data = request.get_json()
        if not data:
            return error_response(MISSING_REQUEST_BODY, detail="No data provided")

        title = data.get("title", "").strip()
        if not title:
            return error_response(MISSING_REQUIRED_FIELD, detail="Title is required")

        # Optional fields with defaults
        emoji = data.get("emoji", "📦")
        color = data.get("color", "#667eea")
        icon = (data.get("icon") or "").strip()

        # Generate slug from title: lowercase, replace spaces/special chars with hyphens
        slug = re.sub(r"[^a-z0-9]+", "-", title.lower())
        slug = slug.strip("-")  # Remove leading/trailing hyphens

        if not slug or not re.fullmatch(r"[a-z][a-z0-9_-]*", slug):
            return error_response(
                INVALID_REQUEST_VALUE,
                detail="Title must start with a letter.",
            )

        # Check for conflicts with existing facets
        existing = facets.get_facets()
        if slug in existing:
            return error_response(
                FACET_ALREADY_EXISTS,
                detail=f"Facet '{slug}' already exists",
            )

        facets.create_facet(title, emoji=emoji, color=color, icon=icon)

        config = {
            "title": title,
            "description": "",
            "color": color,
            "emoji": emoji,
        }
        if icon:
            config["icon"] = icon

        return jsonify({"success": True, "facet": slug, "config": config}), 201

    except ValueError as e:
        return error_response(INVALID_REQUEST_VALUE, detail=str(e))
    except Exception:
        logger.exception("error creating facet")
        return _settings_operation_failed()


@settings_bp.route("/api/facet/<facet_name>")
def get_facet_config(facet_name: str) -> Any:
    """Get configuration for a specific facet."""
    try:
        from solstone.think.facets import get_facets

        facets = get_facets()
        if facet_name not in facets:
            return error_response(FACET_NOT_FOUND, detail="Facet not found")

        return jsonify({"facet": facet_name, "config": facets[facet_name]})
    except Exception:
        logger.exception("error loading facet config")
        return _settings_operation_failed()


@settings_bp.route("/api/facet/<facet_name>", methods=["PUT"])
def update_facet_config(facet_name: str) -> Any:
    """Update configuration for a specific facet."""
    try:
        data = request.get_json()
        if not data:
            return error_response(MISSING_REQUEST_BODY, detail="No data provided")

        if facet_name not in facets.get_facets():
            return error_response(FACET_NOT_FOUND, detail="Facet not found")

        update_fields = {
            key: data[key]
            for key in ("title", "description", "color", "emoji", "icon")
            if key in data
        }
        if update_fields:
            facets.update_facet(facet_name, **update_fields)
        if "muted" in data:
            facets.set_facet_muted(facet_name, bool(data["muted"]))

        config = {
            key: value
            for key, value in facets.get_facets()[facet_name].items()
            if key != "path"
        }
        return jsonify({"success": True, "facet": facet_name, "config": config})
    except FileNotFoundError:
        return error_response(FACET_NOT_FOUND, detail="Facet not found")
    except ValueError as e:
        return error_response(INVALID_REQUEST_VALUE, detail=str(e))
    except Exception:
        logger.exception("error saving facet config")
        return _settings_operation_failed()


def _get_logs_from_dir(logs_dir: Path, cursor: str | None) -> dict:
    """Load action logs from a directory, one day at a time.

    Args:
        logs_dir: Path to logs directory containing YYYYMMDD.jsonl files
        cursor: Optional YYYYMMDD - load the day before this date

    Returns:
        Dict with {day, entries, next_cursor}
    """
    if not logs_dir.exists():
        return {"day": None, "entries": [], "next_cursor": None}

    # Find all log files sorted newest first
    log_files = sorted(
        [f for f in logs_dir.iterdir() if re.fullmatch(r"\d{8}\.jsonl", f.name)],
        key=lambda f: f.stem,
        reverse=True,
    )

    if not log_files:
        return {"day": None, "entries": [], "next_cursor": None}

    # Apply cursor filter if provided
    if cursor:
        log_files = [f for f in log_files if f.stem < cursor]

    if not log_files:
        return {"day": None, "entries": [], "next_cursor": None}

    # Load the first (newest) day
    target_file = log_files[0]
    day = target_file.stem
    entries = []

    try:
        with open(target_file, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if line:
                    entries.append(json.loads(line))
    except (OSError, json.JSONDecodeError) as exc:
        logger.warning("Failed to read settings log %s: %s", target_file, exc)

    # Reverse to show newest first within the day
    entries.reverse()

    # Determine next cursor
    next_cursor = log_files[1].stem if len(log_files) > 1 else None

    return {"day": day, "entries": entries, "next_cursor": next_cursor}


@settings_bp.route("/api/logs")
def get_journal_logs() -> Any:
    """Get journal-level action logs, one day at a time.

    These are actions not tied to a specific facet, such as settings changes,
    remote observer management, and other journal-wide operations.

    Query params:
        cursor: Optional YYYYMMDD - load the day before this date

    Returns:
        {day, entries, next_cursor} where next_cursor is null if no more days
    """
    logs_dir = Path(state.journal_root) / "config" / "actions"
    cursor = request.args.get("cursor")
    return jsonify(_get_logs_from_dir(logs_dir, cursor))


@settings_bp.route("/api/facet/<facet_name>/logs")
def get_facet_logs(facet_name: str) -> Any:
    """Get action logs for a facet, one day at a time.

    Query params:
        cursor: Optional YYYYMMDD - load the day before this date

    Returns:
        {day, entries, next_cursor} where next_cursor is null if no more days
    """
    logs_dir = Path(state.journal_root) / "facets" / facet_name / "logs"
    cursor = request.args.get("cursor")
    return jsonify(_get_logs_from_dir(logs_dir, cursor))


# ---------------------------------------------------------------------------
# Activities API
# ---------------------------------------------------------------------------


@settings_bp.route("/api/activities/defaults")
def get_default_activities() -> Any:
    """Return the list of predefined default activities.

    These are common activities that users can attach to facets.
    """
    try:
        from solstone.think.activities import get_default_activities as _get_defaults

        return jsonify({"activities": _get_defaults()})
    except Exception:
        logger.exception("error loading default activities")
        return _settings_operation_failed()


@settings_bp.route("/api/facet/<facet_name>/activities")
def get_facet_activities(facet_name: str) -> Any:
    """Get activities attached to a facet.

    Returns:
        - activities: List of attached activities with full metadata
        - defaults: List of predefined activities for reference
    """
    try:
        from solstone.think.activities import get_default_activities as _get_defaults
        from solstone.think.activities import (
            get_facet_activities as _get_facet_activities,
        )
        from solstone.think.facets import get_facets

        # Verify facet exists
        facets = get_facets()
        if facet_name not in facets:
            return error_response(FACET_NOT_FOUND, detail="Facet not found")

        attached = _get_facet_activities(facet_name)
        defaults = _get_defaults()

        return jsonify({"activities": attached, "defaults": defaults})

    except Exception:
        logger.exception("error loading facet activities")
        return _settings_operation_failed()


@settings_bp.route("/api/facet/<facet_name>/activities", methods=["POST"])
def add_facet_activity(facet_name: str) -> Any:
    """Add an activity to a facet.

    For predefined activities, only 'id' is required.
    For custom activities, 'name' and 'description' should be provided.

    Accepts JSON with:
        id: Activity identifier (required for predefined)
        name: Display name (required for custom, optional for predefined)
        description: Activity description (optional)
        priority: "high", "normal", or "low" (optional, default: "normal")
        icon: Emoji icon (optional, for custom activities)
    """
    try:
        from solstone.think.activities import (
            add_activity_to_facet,
            generate_activity_id,
        )
        from solstone.think.activities import get_default_activities as _get_defaults
        from solstone.think.facets import get_facets

        # Verify facet exists
        facets = get_facets()
        if facet_name not in facets:
            return error_response(FACET_NOT_FOUND, detail="Facet not found")

        data = request.get_json()
        if not data:
            return error_response(MISSING_REQUEST_BODY, detail="No data provided")

        # Determine activity ID
        activity_id = data.get("id")
        name = data.get("name")

        if not activity_id:
            if not name:
                return error_response(
                    MISSING_REQUIRED_FIELD,
                    detail="Either 'id' or 'name' is required",
                )
            # Generate ID from name for custom activity
            activity_id = generate_activity_id(name)

        # Validate priority if provided
        priority = data.get("priority", "normal")
        if priority not in ("high", "normal", "low"):
            return error_response(
                ACTIVITY_INVALID,
                detail="priority must be 'high', 'normal', or 'low'",
            )

        # Check if it's a predefined activity
        defaults_by_id = {a["id"]: a for a in _get_defaults()}
        is_predefined = activity_id in defaults_by_id

        # For custom activities, name is required
        if not is_predefined and not name:
            return error_response(
                MISSING_REQUIRED_FIELD,
                detail="'name' is required for custom activities",
            )

        activity = add_activity_to_facet(
            facet_name,
            activity_id,
            name=name,
            description=data.get("description"),
            instructions=data.get("instructions"),
            priority=priority,
            icon=data.get("icon"),
        )

        log_app_action(
            app="settings",
            facet=facet_name,
            action="activity_add",
            params={"activity_id": activity_id},
        )

        return jsonify({"success": True, "activity": activity}), 201

    except Exception:
        logger.exception("error adding activity")
        return _settings_operation_failed()


@settings_bp.route("/api/facet/<facet_name>/activities/<activity_id>", methods=["PUT"])
def update_facet_activity(facet_name: str, activity_id: str) -> Any:
    """Update an activity's configuration in a facet.

    Accepts JSON with optional fields:
        description: New description
        instructions: Detection/level instructions for the LLM
        priority: "high", "normal", or "low"
        name: New name (only for custom activities)
        icon: New icon (only for custom activities)
    """
    try:
        from solstone.think.activities import update_activity_in_facet
        from solstone.think.facets import get_facets

        # Verify facet exists
        facets = get_facets()
        if facet_name not in facets:
            return error_response(FACET_NOT_FOUND, detail="Facet not found")

        data = request.get_json()
        if not data:
            return error_response(MISSING_REQUEST_BODY, detail="No data provided")

        # Validate priority if provided
        priority = data.get("priority")
        if priority is not None and priority not in ("high", "normal", "low"):
            return error_response(
                ACTIVITY_INVALID,
                detail="priority must be 'high', 'normal', or 'low'",
            )

        activity = update_activity_in_facet(
            facet_name,
            activity_id,
            description=data.get("description"),
            instructions=data.get("instructions"),
            priority=priority,
            name=data.get("name"),
            icon=data.get("icon"),
        )

        if activity is None:
            return error_response(
                ACTIVITY_NOT_FOUND,
                detail="Activity not found in facet",
            )

        log_app_action(
            app="settings",
            facet=facet_name,
            action="activity_update",
            params={"activity_id": activity_id, "updates": data},
        )

        return jsonify({"success": True, "activity": activity})

    except Exception:
        logger.exception("error updating activity")
        return _settings_operation_failed()


@settings_bp.route(
    "/api/facet/<facet_name>/activities/<activity_id>", methods=["DELETE"]
)
def remove_facet_activity(facet_name: str, activity_id: str) -> Any:
    """Remove an activity from a facet.

    This detaches the activity from the facet. For predefined activities,
    it can be re-added later. For custom activities, this deletes it.
    """
    try:
        from solstone.think.activities import (
            DEFAULT_ACTIVITIES,
            remove_activity_from_facet,
        )
        from solstone.think.facets import get_facets

        # Verify facet exists
        facets = get_facets()
        if facet_name not in facets:
            return error_response(FACET_NOT_FOUND, detail="Facet not found")

        # Prevent removing always-on activities
        always_on_ids = {a["id"] for a in DEFAULT_ACTIVITIES if a.get("always_on")}
        if activity_id in always_on_ids:
            return error_response(
                ACTIVITY_PROTECTED,
                detail="Cannot remove always-on activity",
            )

        removed = remove_activity_from_facet(facet_name, activity_id)

        if not removed:
            return error_response(
                ACTIVITY_NOT_FOUND,
                detail="Activity not found in facet",
            )

        log_app_action(
            app="settings",
            facet=facet_name,
            action="activity_remove",
            params={"activity_id": activity_id},
        )

        return jsonify({"success": True})

    except Exception:
        logger.exception("error removing activity")
        return _settings_operation_failed()


@settings_bp.route("/api/sync")
def get_sync() -> Any:
    """Return sync configuration (schedule entries + token availability)."""
    try:
        config_dir = Path(state.journal_root) / "config"
        schedules_path = config_dir / "schedules.json"

        # Load schedules
        schedules = {}
        if schedules_path.exists():
            with open(schedules_path, "r", encoding="utf-8") as f:
                schedules = json.load(f)

        plaud_entry = schedules.get("sync:plaud", {})
        granola_entry = schedules.get("sync:granola", {})
        obsidian_entry = schedules.get("sync:obsidian", {})

        # Check token availability from journal config / runtime env
        config = get_journal_config()
        env_keys = config.get("env", {})
        has_token = bool(env_keys.get("PLAUD_ACCESS_TOKEN")) or bool(
            os.getenv("PLAUD_ACCESS_TOKEN")
        )

        return jsonify(
            {
                "plaud": {
                    "available": has_token,
                    "enabled": (
                        plaud_entry.get("enabled", True) if plaud_entry else False
                    ),
                    "configured": bool(plaud_entry),
                },
                "granola": {
                    "enabled": (
                        granola_entry.get("enabled", True) if granola_entry else False
                    ),
                    "configured": bool(granola_entry),
                },
                "obsidian": {
                    "available": True,
                    "enabled": (
                        obsidian_entry.get("enabled", True) if obsidian_entry else False
                    ),
                    "configured": bool(obsidian_entry),
                },
            }
        )

    except Exception:
        logger.exception("error loading sync config")
        return _settings_operation_failed()


@settings_bp.route("/api/sync", methods=["PUT"])
def update_sync() -> Any:
    """Update sync schedule configuration."""
    try:
        request_data = request.get_json()
        if not request_data:
            return error_response(MISSING_REQUEST_BODY, detail="No data provided")

        schedules = read_schedules()
        changed_fields = {}
        changed_entries: dict[str, dict[str, Any]] = {}

        # Handle plaud sync toggle
        if "plaud" in request_data:
            plaud_data = request_data["plaud"]
            if not isinstance(plaud_data, dict):
                return error_response(
                    INVALID_CONFIG_VALUE,
                    detail="plaud must be an object",
                )

            if "enabled" in plaud_data:
                enabled = plaud_data["enabled"]
                if not isinstance(enabled, bool):
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail="plaud.enabled must be a boolean",
                    )

                old_entry = schedules.get("sync:plaud", {})
                old_enabled = old_entry.get("enabled", True) if old_entry else False

                if enabled != old_enabled:
                    # Ensure the entry exists with full config
                    if "sync:plaud" not in schedules:
                        schedules["sync:plaud"] = {
                            "cmd": ["journal", "importer", "--sync", "plaud", "--save"],
                            "every": "hourly",
                        }
                    schedules["sync:plaud"]["enabled"] = enabled
                    changed_fields["plaud.enabled"] = enabled
                    changed_entries["sync:plaud"] = schedules["sync:plaud"]

        # Handle granola sync toggle
        if "granola" in request_data:
            granola_data = request_data["granola"]
            if not isinstance(granola_data, dict):
                return error_response(
                    INVALID_CONFIG_VALUE,
                    detail="granola must be an object",
                )

            if "enabled" in granola_data:
                enabled = granola_data["enabled"]
                if not isinstance(enabled, bool):
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail="granola.enabled must be a boolean",
                    )

                old_entry = schedules.get("sync:granola", {})
                old_enabled = old_entry.get("enabled", True) if old_entry else False

                if enabled != old_enabled:
                    if "sync:granola" not in schedules:
                        schedules["sync:granola"] = {
                            "cmd": [
                                "journal",
                                "importer",
                                "--sync",
                                "granola",
                                "--save",
                            ],
                            "every": "hourly",
                        }
                    schedules["sync:granola"]["enabled"] = enabled
                    changed_fields["granola.enabled"] = enabled
                    changed_entries["sync:granola"] = schedules["sync:granola"]

        # Handle obsidian sync toggle
        if "obsidian" in request_data:
            obsidian_data = request_data["obsidian"]
            if not isinstance(obsidian_data, dict):
                return error_response(
                    INVALID_CONFIG_VALUE,
                    detail="obsidian must be an object",
                )

            if "enabled" in obsidian_data:
                enabled = obsidian_data["enabled"]
                if not isinstance(enabled, bool):
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail="obsidian.enabled must be a boolean",
                    )

                old_entry = schedules.get("sync:obsidian", {})
                old_enabled = old_entry.get("enabled", True) if old_entry else False

                if enabled != old_enabled:
                    if "sync:obsidian" not in schedules:
                        schedules["sync:obsidian"] = {
                            "cmd": [
                                "journal",
                                "importer",
                                "--sync",
                                "obsidian",
                                "--save",
                            ],
                            "every": "hourly",
                        }
                    schedules["sync:obsidian"]["enabled"] = enabled
                    changed_fields["obsidian.enabled"] = enabled
                    changed_entries["sync:obsidian"] = schedules["sync:obsidian"]

        if changed_fields:
            set_schedule_entries(changed_entries)

            log_app_action(
                app="settings",
                facet=None,
                action="sync_update",
                params={"changed_fields": changed_fields},
            )

        return get_sync()

    except Exception:
        logger.exception("error saving sync config")
        return _settings_operation_failed()


@settings_bp.route("/api/storage")
def get_storage() -> Any:
    """Return storage summary, retention config, and active streams."""
    try:
        summary = compute_storage_summary()
        config = load_retention_config()
        log_config = load_log_retention_config()
        journal_path = get_journal()
        warnings = check_storage_health(summary, journal_path)
        try:
            streams = list_streams()
        except Exception:
            streams = []

        return jsonify(
            {
                "summary": {
                    "raw_media_bytes": summary.raw_media_bytes,
                    "raw_media_human": summary.raw_media_human,
                    "derived_bytes": summary.derived_bytes,
                    "derived_human": summary.derived_human,
                    "total_segments": summary.total_segments,
                    "segments_with_raw": summary.segments_with_raw,
                    "segments_purged": summary.segments_purged,
                },
                "retention": {
                    "raw_media": config.default.mode,
                    "raw_media_days": config.default.days,
                    "per_stream": {
                        name: {"raw_media": p.mode, "raw_media_days": p.days}
                        for name, p in config.per_stream.items()
                    },
                    "journal_logs": {
                        "enabled": log_config.enabled,
                        "days": log_config.days,
                    },
                },
                "streams": [{"name": s.get("name", "")} for s in streams],
                "warnings": warnings,
            }
        )
    except Exception:
        logger.exception("error loading storage")
        return _settings_operation_failed()


@settings_bp.route("/api/storage", methods=["PUT"])
def update_storage() -> Any:
    """Update retention configuration."""
    try:
        request_data = request.get_json()
        if not request_data:
            return error_response(MISSING_REQUEST_BODY, detail="No data provided")

        with hold_config_lock():
            config = get_journal_config()
            old_retention = config.get("retention", {})

            retention = config.setdefault("retention", {})

            changed = {}

            # Update global mode
            if "raw_media" in request_data:
                mode = request_data["raw_media"]
                if mode not in ("keep", "days", "processed"):
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail=f"Invalid mode: {mode}",
                    )
                if retention.get("raw_media") != mode:
                    changed["raw_media"] = {
                        "old": retention.get("raw_media"),
                        "new": mode,
                    }
                retention["raw_media"] = mode

            # Update global days
            if "raw_media_days" in request_data:
                days = request_data["raw_media_days"]
                if days is not None:
                    if not isinstance(days, int) or days < 1:
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail="days must be a positive integer",
                        )
                if retention.get("raw_media_days") != days:
                    changed["raw_media_days"] = {
                        "old": retention.get("raw_media_days"),
                        "new": days,
                    }
                retention["raw_media_days"] = days

            # Update per-stream overrides
            if "per_stream" in request_data:
                ps = request_data["per_stream"]
                if not isinstance(ps, dict):
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail="per_stream must be an object",
                    )
                new_per_stream = {}
                for stream_name, stream_cfg in ps.items():
                    if not isinstance(stream_cfg, dict):
                        continue
                    mode = stream_cfg.get("raw_media")
                    if mode is not None and mode not in ("keep", "days", "processed"):
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail=f"Invalid mode for {stream_name}: {mode}",
                        )
                    days = stream_cfg.get("raw_media_days")
                    if days is not None and (not isinstance(days, int) or days < 1):
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail=f"Invalid days for {stream_name}",
                        )
                    new_per_stream[stream_name] = stream_cfg
                if old_retention.get("per_stream") != new_per_stream:
                    changed["per_stream"] = {
                        "old": old_retention.get("per_stream"),
                        "new": new_per_stream,
                    }
                retention["per_stream"] = new_per_stream

            if "journal_logs" in request_data:
                journal_logs = request_data["journal_logs"]
                if not isinstance(journal_logs, dict):
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail="journal_logs must be an object",
                    )

                current_journal_logs = retention.get("journal_logs", {})
                if not isinstance(current_journal_logs, dict):
                    current_journal_logs = {}
                old_journal_logs = {
                    "enabled": current_journal_logs.get("enabled", True),
                    "days": current_journal_logs.get("days", 30),
                }
                new_journal_logs = dict(old_journal_logs)

                if "enabled" in journal_logs:
                    enabled = journal_logs["enabled"]
                    if not isinstance(enabled, bool):
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail="enabled must be a boolean",
                        )
                    new_journal_logs["enabled"] = enabled

                if "days" in journal_logs:
                    days = journal_logs["days"]
                    if not isinstance(days, int) or isinstance(days, bool) or days < 1:
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail="days must be a positive integer",
                        )
                    new_journal_logs["days"] = days

                if old_journal_logs != new_journal_logs:
                    changed["journal_logs"] = {
                        "old": old_journal_logs,
                        "new": new_journal_logs,
                    }
                retention["journal_logs"] = new_journal_logs

            write_journal_config(config)

        if changed:
            log_app_action(
                app="settings",
                facet=None,
                action="retention_update",
                params={"changed_fields": changed},
            )

        return jsonify({"success": True, "retention": retention})
    except Exception:
        logger.exception("error saving retention config")
        return _settings_operation_failed()


@settings_bp.route("/api/storage/purge", methods=["POST"])
def run_purge() -> Any:
    """Run retention purge (dry-run or execute)."""
    try:
        request_data = request.get_json()
        if not request_data:
            return error_response(MISSING_REQUEST_BODY, detail="No data provided")

        older_than_days = request_data.get("older_than_days")
        if older_than_days is None:
            return error_response(
                MISSING_REQUIRED_FIELD,
                detail="older_than_days is required",
            )
        if not isinstance(older_than_days, int) or older_than_days < 1:
            return error_response(
                INVALID_CONFIG_VALUE,
                detail="older_than_days must be a positive integer",
            )

        stream_filter = request_data.get("stream_filter") or None
        dry_run = request_data.get("dry_run", True)

        result = purge(
            older_than_days=older_than_days,
            stream_filter=stream_filter,
            dry_run=dry_run,
        )

        response = {
            "files_deleted": result.files_deleted,
            "bytes_freed": result.bytes_freed,
            "bytes_freed_human": _human_bytes(result.bytes_freed),
            "segments_processed": result.segments_processed,
            "segments_skipped_incomplete": result.segments_skipped_incomplete,
            "segments_skipped_policy": result.segments_skipped_policy,
            "segments_blocked_failed": result.segments_blocked_failed,
            "partial_error": result.partial_error,
            "dry_run": dry_run,
        }

        # On actual purge, also refresh the storage summary
        if not dry_run:
            summary = compute_storage_summary()
            response["summary"] = {
                "raw_media_bytes": summary.raw_media_bytes,
                "raw_media_human": summary.raw_media_human,
                "derived_bytes": summary.derived_bytes,
                "derived_human": summary.derived_human,
                "total_segments": summary.total_segments,
                "segments_with_raw": summary.segments_with_raw,
                "segments_purged": summary.segments_purged,
            }

            log_app_action(
                app="settings",
                facet=None,
                action="retention_purge",
                params={
                    "older_than_days": older_than_days,
                    "stream_filter": stream_filter,
                    "files_deleted": result.files_deleted,
                    "bytes_freed": result.bytes_freed,
                },
            )

        return jsonify(response)
    except CorruptConfigError:
        raise
    except Exception:
        logger.exception("error running purge")
        return _settings_operation_failed()


@settings_bp.route("/api/storage/prune-logs", methods=["POST"])
def run_prune_logs() -> Any:
    """Run operational log/cache pruning (dry-run or execute)."""
    try:
        request_data = request.get_json(silent=True) or {}
        if not isinstance(request_data, dict):
            return error_response(
                INVALID_CONFIG_VALUE,
                detail="request body must be an object",
            )

        dry_run = request_data.get("dry_run", True)
        days = request_data.get("days")
        if days is not None:
            if not isinstance(days, int) or isinstance(days, bool) or days < 1:
                return error_response(
                    INVALID_CONFIG_VALUE,
                    detail="days must be a positive integer",
                )

        result = prune(days=days, dry_run=dry_run)

        if not dry_run and result.enabled:
            log_app_action(
                app="settings",
                facet=None,
                action="prune_logs",
                params={
                    "days": result.days,
                    "files_deleted": result.files_deleted,
                    "dirs_deleted": result.dirs_deleted,
                },
            )

        return jsonify(_serialize_prune_result(result))
    except CorruptConfigError:
        raise
    except Exception:
        logger.exception("error pruning logs")
        return _settings_operation_failed()
