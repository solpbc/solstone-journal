# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import copy
import json
import logging
import os
import platform
import re
import subprocess
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from flask import Blueprint, abort, jsonify, render_template, request

from solstone.apps.chat import copy as chat_copy
from solstone.apps.chat.config import (
    THINKING_SURFACES_VALUES,
    load_chat_config,
    save_chat_config,
)
from solstone.apps.settings import copy as settings_copy
from solstone.apps.settings import install_copy, local_bootstrap, mlx_bootstrap
from solstone.apps.settings.copy import (
    CONVEY_REFUSE_NO_PASSWORD_NETWORK,
    CONVEY_REFUSE_NO_PASSWORD_TRUST,
)
from solstone.apps.utils import log_app_action
from solstone.convey import chat_stream, state
from solstone.convey import copy as convey_copy
from solstone.convey.network_access import (
    NetworkAccessPasswordRequired,
    NetworkAccessPasswordTooShort,
    set_network_access,
)
from solstone.convey.reasons import (
    ACTIVITY_INVALID,
    ACTIVITY_NOT_FOUND,
    ACTIVITY_PROTECTED,
    FACET_ALREADY_EXISTS,
    FACET_NOT_FOUND,
    FILE_READ_FAILED,
    INVALID_CONFIG_VALUE,
    INVALID_JSON_REQUEST,
    INVALID_REQUEST_VALUE,
    MISSING_REQUEST_BODY,
    MISSING_REQUIRED_FIELD,
    NETWORK_SECURITY_REQUIRES_PASSWORD,
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
from solstone.convey.utils import error_response
from solstone.think.models import LOCAL_MODEL, QWEN_35_9B
from solstone.think.providers.google import validate_vertex_credentials
from solstone.think.providers.local import LOCAL_MODEL_SPECS
from solstone.think.providers.mlx import _MLX_MODEL_REGISTRY
from solstone.think.retention import (
    _human_bytes,
    check_storage_health,
    compute_storage_summary,
    load_retention_config,
    purge,
)
from solstone.think.streams import list_streams
from solstone.think.utils import get_config as get_journal_config
from solstone.think.utils import get_journal, get_project_root, now_ms

logger = logging.getLogger(__name__)

settings_bp = Blueprint(
    "app:settings",
    __name__,
    url_prefix="/app/settings",
)


GENERIC_SETTINGS_ERROR = (
    "something went wrong — try again, and if it persists, check the health dashboard"
)


def _settings_operation_failed(detail: str = GENERIC_SETTINGS_ERROR) -> Any:
    return error_response(SETTINGS_OPERATION_FAILED, detail=detail)


def _public_facet_record(name: str, data: dict[str, object]) -> dict[str, object]:
    return {
        "name": name,
        "title": str(data.get("title") or name),
        "color": str(data.get("color") or ""),
        "emoji": str(data.get("emoji") or ""),
        "muted": bool(data.get("muted", False)),
    }


# API keys that can be configured in the env section
# Used for system env checks and allowed env fields validation
API_KEY_ENV_VARS = [
    "GOOGLE_API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "REVAI_ACCESS_TOKEN",
    "PLAUD_ACCESS_TOKEN",
]


def _compute_runtime_label() -> str:
    os_name = platform.system().lower()
    arch = platform.machine().lower()
    if os_name == "darwin" and arch == "arm64":
        return "macOS CoreML helper"
    if os_name != "linux" or arch != "x86_64":
        return "unsupported"
    try:
        import onnxruntime

        return (
            "Linux ONNX (CUDA fp32)"
            if "CUDAExecutionProvider" in onnxruntime.get_available_providers()
            else "Linux ONNX (CPU fp32)"
        )
    except Exception:
        return "unsupported"


def _convey_password_is_set(config: dict[str, Any]) -> bool:
    password_hash = config.get("convey", {}).get("password_hash", "")
    return bool(str(password_hash or "").strip())


def _project_public_config(config: dict[str, Any]) -> dict[str, Any]:
    projected = copy.deepcopy(config)
    if "env" in projected:
        projected["env"] = {k: bool(v) for k, v in projected["env"].items()}
    convey_config = projected.setdefault("convey", {})
    convey_config.pop("secret", None)
    has_pw = bool(convey_config.pop("password_hash", None))
    convey_config.pop("password", None)
    convey_config["has_password"] = has_pw
    projected["runtime_env"] = {k: bool(os.getenv(k)) for k in API_KEY_ENV_VARS}
    return projected


@settings_bp.app_context_processor
def _inject_settings_copy() -> dict[str, Any]:
    return {
        "convey_copy": convey_copy,
        "install_copy": {
            name: getattr(install_copy, name) for name in install_copy.__all__
        },
        "chat_config": load_chat_config(),
        "chat_copy": chat_copy,
        "settings_copy": settings_copy,
        "sol_voice_copy": sol_voice_copy,
    }


@settings_bp.route("/facets/<slug>")
def view_facet_detail(slug: str) -> str:
    from solstone.think.facets import get_facets

    facets = get_facets()
    facet = facets.get(slug)
    if facet is None:
        abort(404)

    title = str(facet.get("title") or slug)
    color = str(facet.get("color") or "")
    emoji = str(facet.get("emoji") or "")
    return render_template(
        "settings/facet_detail.html",
        app="settings",
        slug=slug,
        title=title,
        color=color,
        emoji=emoji,
        muted=bool(facet.get("muted", False)),
        primary_cta=settings_copy.FACET_DETAIL_PRIMARY_CTA.format(title=title),
        secondary_cta=settings_copy.FACET_DETAIL_SECONDARY_CTA,
        tertiary_cta=settings_copy.FACET_DETAIL_TERTIARY_ESCAPE,
        success_heading=settings_copy.FACET_DETAIL_SUCCESS_HEADING.format(title=title),
        value_framing=settings_copy.FACET_DETAIL_VALUE_FRAMING.format(title=title),
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
    except Exception:
        logger.exception("error loading config")
        return _settings_operation_failed()


@settings_bp.route("/api/config", methods=["PUT"])
def update_config() -> Any:
    """Update the journal configuration.

    Accepts JSON with a 'section' key and per-section config fields to update.
    Supported writes include identity and transcribe settings, convey security
    settings (password, allow_network_access, trust_localhost), and API-key env
    vars.
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
            "transcribe": ["backend", "enrich", "preserve_all", "noise_upgrade"],
            "convey": ["allow_network_access", "password", "trust_localhost"],
            "support": ["enabled", "proactive", "anonymous_feedback", "portal_url"],
            "agent": ["name", "name_status", "named_date", "proposal_count"],
            "env": API_KEY_ENV_VARS,
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

        config_dir = Path(state.journal_root) / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        config_path = config_dir / "journal.json"

        # Load existing config
        old_config = get_journal_config()
        config = get_journal_config()

        # Ensure section exists
        if section not in config:
            config[section] = {}

        # Track changes for logging
        changed_fields = {}
        old_section = old_config.get(section, {})

        if section == "convey" and "allow_network_access" in data:
            try:
                result = set_network_access(
                    enable=bool(data["allow_network_access"]),
                    password=data.get("password"),
                )
            except NetworkAccessPasswordRequired:
                return error_response(
                    NETWORK_SECURITY_REQUIRES_PASSWORD,
                    detail=CONVEY_REFUSE_NO_PASSWORD_NETWORK,
                )
            except NetworkAccessPasswordTooShort:
                return error_response(
                    INVALID_CONFIG_VALUE,
                    detail="Password must be at least 8 characters",
                )
            return jsonify(result)

        if section == "convey" and "password" in data:
            raw_password = data.pop("password") or ""
            if raw_password:
                if len(raw_password) < 8:
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail="Password must be at least 8 characters",
                    )
                from werkzeug.security import generate_password_hash

                config["convey"]["password_hash"] = generate_password_hash(raw_password)
                changed_fields["password"] = {
                    "old": old_section.get("password_hash"),
                    "new": config["convey"]["password_hash"],
                }

        has_password = _convey_password_is_set(config)

        if (
            section == "convey"
            and "trust_localhost" in data
            and not bool(data["trust_localhost"])
            and not has_password
        ):
            return error_response(
                NETWORK_SECURITY_REQUIRES_PASSWORD,
                detail=CONVEY_REFUSE_NO_PASSWORD_TRUST,
            )

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

        # Handle nested backend configs for transcribe section
        if section == "transcribe":
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
            from solstone.think.providers import PROVIDER_METADATA

            # Build reverse map: env_key -> provider name
            env_to_provider = {
                meta["env_key"]: name
                for name, meta in PROVIDER_METADATA.items()
                if "env_key" in meta
            }
            if "providers" not in config:
                config["providers"] = {}

            # Validate changed provider API keys
            if "key_validation" not in config["providers"]:
                config["providers"]["key_validation"] = {}
            for env_var in changed_fields:
                provider = env_to_provider.get(env_var)
                if provider:
                    new_val = data.get(env_var, "")
                    if new_val:
                        from solstone.think.providers import (
                            validate_key as _validate_key,
                        )

                        result = _validate_key(provider, new_val)
                        result["timestamp"] = datetime.now(timezone.utc).isoformat()
                        config["providers"]["key_validation"][provider] = result
                    else:
                        config["providers"]["key_validation"].pop(provider, None)

            # Validate service tokens (Rev.ai, Plaud) — not AI providers,
            # so they use their own validators instead of think.providers.
            SERVICE_TOKEN_VALIDATORS = {
                "REVAI_ACCESS_TOKEN": ("revai", "solstone.observe.transcribe.revai"),
                "PLAUD_ACCESS_TOKEN": ("plaud", "solstone.think.importers.plaud"),
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
                        config["providers"]["key_validation"][val_key] = result
                    else:
                        config["providers"]["key_validation"].pop(val_key, None)

        # Write back to file
        with open(config_path, "w", encoding="utf-8") as f:
            json.dump(config, f, indent=2, ensure_ascii=False)
            f.write("\n")
        os.chmod(config_path, 0o600)

        # Log if something changed (don't log sensitive values)
        if changed_fields:
            log_fields = changed_fields
            if section == "convey" and "password" in log_fields:
                # Don't log actual password values
                log_fields = {"password": {"old": "***", "new": "***"}}
            elif section == "env":
                # Don't log actual API key values
                log_fields = {k: {"old": "***", "new": "***"} for k in changed_fields}

            log_app_action(
                app="settings",
                facet=None,
                action=f"{section}_update",
                params={"changed_fields": log_fields},
            )

        if section in ("agent", "identity") and changed_fields:
            project_root = Path(get_project_root())
            subprocess.run(
                ["make", "skills"],
                cwd=project_root,
                check=False,
                capture_output=True,
            )

        key_validation = config.get("providers", {}).get("key_validation", {})
        return jsonify(
            {
                "config": _project_public_config(config),
                "key_validation": key_validation,
                "success": True,
            }
        )
    except Exception:
        logger.exception("error updating config")
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
    """
    try:
        from solstone.observe.transcribe import get_backend_list

        config = get_journal_config()
        transcribe_config = config.get("transcribe", {})

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

        return jsonify(
            {
                "backends": backends,
                "api_keys": api_keys,
                "config": transcribe_config,
                "runtime_label": runtime_label,
            }
        )
    except Exception:
        logger.exception("error loading transcribe config")
        return _settings_operation_failed()


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
        return jsonify([])

    try:
        rows = _read_sol_voice_throttled_rows(log_path, limit)
        return jsonify(rows)
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
# Providers API
# ---------------------------------------------------------------------------

VALID_TIERS = {1, 2, 3}
MLX_MODEL_LABELS = {
    QWEN_35_9B: "qwen 3.5 — 16 GB Mac",
    "gemma-4-26b-a4b-it-mlx-4bit": "gemma 4 (26B) — 24 GB Mac",
}
LOCAL_MODEL_LABELS = {
    LOCAL_MODEL: "qwen 2.5 coder 7B — 12 GB",
}


def _mlx_model_error(model: str) -> Any:
    return error_response(
        INVALID_REQUEST_VALUE,
        detail=(
            f"Unknown MLX model: {model}. "
            f"Must be one of: {', '.join(_MLX_MODEL_REGISTRY.keys())}"
        ),
    )


def _mlx_model_from_request() -> tuple[str | None, Any | None]:
    model = request.args.get("model") or QWEN_35_9B
    if model not in _MLX_MODEL_REGISTRY:
        return None, _mlx_model_error(model)
    return model, None


def _local_model_error(model: str) -> Any:
    return error_response(
        INVALID_REQUEST_VALUE,
        detail=(
            f"Unknown local model: {model}. "
            f"Must be one of: {', '.join(LOCAL_MODEL_SPECS.keys())}"
        ),
    )


def _local_model_from_request() -> tuple[str | None, Any | None]:
    model = request.args.get("model") or LOCAL_MODEL
    if model not in LOCAL_MODEL_SPECS:
        return None, _local_model_error(model)
    return model, None


@settings_bp.route("/api/mlx/availability")
def get_mlx_availability() -> Any:
    try:
        model, error = _mlx_model_from_request()
        if error is not None:
            return error
        assert model is not None
        return jsonify(mlx_bootstrap.get_availability_payload(model))
    except Exception:
        logger.exception("error loading MLX availability")
        return _settings_operation_failed()


@settings_bp.route("/api/mlx/bootstrap", methods=["POST"])
def start_mlx_bootstrap() -> Any:
    try:
        model, error = _mlx_model_from_request()
        if error is not None:
            return error
        assert model is not None
        payload, status = mlx_bootstrap.start_bootstrap(model)
        return jsonify(payload), status
    except mlx_bootstrap.MlxBootstrapUnavailableError as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    except mlx_bootstrap.MlxBootstrapStartError as exc:
        logger.exception("error starting MLX bootstrap")
        return _settings_operation_failed(str(exc))
    except Exception:
        logger.exception("error starting MLX bootstrap")
        return _settings_operation_failed()


@settings_bp.route("/api/mlx/bootstrap/status")
def get_mlx_bootstrap_status() -> Any:
    try:
        model, error = _mlx_model_from_request()
        if error is not None:
            return error
        assert model is not None
        return jsonify(mlx_bootstrap.get_state(model))
    except Exception:
        logger.exception("error loading MLX bootstrap status")
        return _settings_operation_failed()


@settings_bp.route("/api/mlx/models")
def get_mlx_models() -> Any:
    try:
        return jsonify(
            [
                {
                    "name": name,
                    "label": MLX_MODEL_LABELS[name],
                    "min_ram_gb": spec.min_ram_bytes // 1024**3,
                }
                for name, spec in _MLX_MODEL_REGISTRY.items()
            ]
        )
    except Exception:
        logger.exception("error loading MLX models")
        return _settings_operation_failed()


@settings_bp.route("/api/local/availability")
def get_local_availability() -> Any:
    try:
        model, error = _local_model_from_request()
        if error is not None:
            return error
        assert model is not None
        return jsonify(local_bootstrap.get_availability_payload(model))
    except Exception:
        logger.exception("error loading local provider availability")
        return _settings_operation_failed()


@settings_bp.route("/api/local/bootstrap", methods=["POST"])
def start_local_bootstrap() -> Any:
    try:
        model, error = _local_model_from_request()
        if error is not None:
            return error
        assert model is not None
        payload, status = local_bootstrap.start_bootstrap(model)
        return jsonify(payload), status
    except local_bootstrap.LocalBootstrapUnavailableError as exc:
        return error_response(INVALID_REQUEST_VALUE, detail=str(exc))
    except local_bootstrap.LocalBootstrapStartError as exc:
        logger.exception("error starting local provider bootstrap")
        return _settings_operation_failed(str(exc))
    except Exception:
        logger.exception("error starting local provider bootstrap")
        return _settings_operation_failed()


@settings_bp.route("/api/local/bootstrap/status")
def get_local_bootstrap_status() -> Any:
    try:
        model, error = _local_model_from_request()
        if error is not None:
            return error
        assert model is not None
        return jsonify(local_bootstrap.get_state(model))
    except Exception:
        logger.exception("error loading local provider bootstrap status")
        return _settings_operation_failed()


@settings_bp.route("/api/local/models")
def get_local_models() -> Any:
    try:
        return jsonify(
            [
                {
                    "name": name,
                    "label": LOCAL_MODEL_LABELS[name],
                    "min_ram_gb": spec.min_ram_bytes // 1024**3,
                    "size_bytes": spec.size_bytes,
                }
                for name, spec in LOCAL_MODEL_SPECS.items()
            ]
        )
    except Exception:
        logger.exception("error loading local provider models")
        return _settings_operation_failed()


@settings_bp.route("/api/providers")
def get_providers() -> Any:
    """Return providers configuration with context defaults and API key status.

    Returns:
        - providers: List of available providers with labels
        - generate: Current generate provider, tier, and backup
        - cogitate: Current cogitate provider, tier, and backup
        - contexts: Configured context overrides from journal.json
        - context_defaults: Context registry with labels/groups for UI
          (includes talent configs with type, schedule, and disabled state)
        - api_keys: Boolean status for each provider's API key
    """
    try:
        from solstone.think.models import (
            TYPE_DEFAULTS,
            get_context_registry,
        )
        from solstone.think.providers import (
            build_provider_status,
            get_provider_list,
        )
        from solstone.think.talent import get_talent_configs

        config = get_journal_config()
        providers_config = config.get("providers", {})

        # Build type-specific settings from config with system defaults
        type_settings = {}
        for agent_type in ("generate", "cogitate"):
            defaults = TYPE_DEFAULTS[agent_type]
            type_config = providers_config.get(agent_type, {})
            type_settings[agent_type] = {
                "provider": type_config.get("provider", defaults["provider"]),
                "tier": type_config.get("tier", defaults["tier"]),
                "backup": type_config.get("backup", defaults["backup"]),
            }

        # Get context overrides from config
        contexts = providers_config.get("contexts", {})

        # Build context defaults with metadata for UI (uses dynamic registry)
        context_defaults = {}
        for pattern, ctx_config in get_context_registry().items():
            context_defaults[pattern] = {
                "tier": ctx_config["tier"],
                "label": ctx_config["label"],
                "group": ctx_config["group"],
            }
            # Include type for talent contexts
            if "type" in ctx_config:
                context_defaults[pattern]["type"] = ctx_config["type"]

        # Enhance talent contexts with additional metadata from get_talent_configs
        from solstone.think.talent import key_to_context

        talent_configs = get_talent_configs(include_disabled=True)
        for key, info in talent_configs.items():
            context_key = key_to_context(key)

            if context_key in context_defaults:
                # Add talent-specific fields
                if "schedule" in info:
                    context_defaults[context_key]["schedule"] = info["schedule"]
                context_defaults[context_key]["disabled"] = info.get("disabled", False)

        # Get providers list from registry
        providers_list = get_provider_list()

        # Check API key status for each provider using os.getenv()
        # This reflects runtime availability (loaded from journal.json via setup_cli)
        api_keys = {}
        for p in providers_list:
            env_key = p.get("env_key", "")
            api_keys[p["name"]] = bool(os.getenv(env_key)) if env_key else False

        # Get cached key validation results
        key_validation = providers_config.get("key_validation", {})

        # Vertex SA credentials status (never expose secrets)
        vertex_creds_path = providers_config.get("vertex_credentials")
        vertex_creds_configured = False
        vertex_creds_email = ""
        if vertex_creds_path and Path(vertex_creds_path).exists():
            vertex_creds_configured = True
            try:
                creds_data = json.loads(Path(vertex_creds_path).read_text())
                vertex_creds_email = creds_data.get("client_email", "")
            except Exception:
                pass

        provider_status = build_provider_status(providers_list, vertex_creds_configured)
        local_model_id = request.args.get("local_model") or LOCAL_MODEL
        if local_model_id not in LOCAL_MODEL_SPECS:
            return _local_model_error(local_model_id)
        local_status = local_bootstrap.get_state(local_model_id)

        mlx_config = providers_config.get("mlx", {})
        mlx_active_model = (
            mlx_config.get("active_model") if isinstance(mlx_config, dict) else None
        ) or QWEN_35_9B
        mlx_status = mlx_bootstrap.get_state(mlx_active_model)

        return jsonify(
            {
                "providers": providers_list,
                "provider_status": provider_status,
                "generate": type_settings["generate"],
                "cogitate": type_settings["cogitate"],
                "contexts": contexts,
                "context_defaults": context_defaults,
                "api_keys": api_keys,
                "key_validation": key_validation,
                "local": local_status,
                "mlx": {"active_model": mlx_active_model, **mlx_status},
                "google_backend": providers_config.get("google_backend", "auto"),
                "vertex_credentials_configured": vertex_creds_configured,
                "vertex_credentials_email": vertex_creds_email,
            }
        )
    except Exception:
        logger.exception("error loading providers")
        return _settings_operation_failed()


@settings_bp.route("/api/providers/local/status")
def get_local_provider_status() -> Any:
    """Return local provider readiness status."""

    try:
        from solstone.think.providers import build_provider_status, get_provider_list

        providers_list = get_provider_list()
        local_provider = next(
            provider for provider in providers_list if provider["name"] == "local"
        )
        provider_status = build_provider_status([local_provider], False)
        return jsonify(provider_status["local"])
    except Exception:
        logger.exception("error loading local provider status")
        return _settings_operation_failed()


@settings_bp.route("/api/validate-keys", methods=["POST"])
def validate_all_keys() -> Any:
    """Re-validate all configured provider API keys.

    Reads keys from journal.json config (not environment), validates each
    against the provider API, and stores results in providers.key_validation.
    """
    try:
        from solstone.think.providers import PROVIDER_METADATA
        from solstone.think.providers import validate_key as _validate_key

        config = get_journal_config()
        env_config = config.get("env", {})

        # Build reverse map: env_key -> provider name
        env_to_provider = {
            meta["env_key"]: name
            for name, meta in PROVIDER_METADATA.items()
            if "env_key" in meta
        }

        if "providers" not in config:
            config["providers"] = {}
        key_validation = {}

        for env_var, provider in env_to_provider.items():
            api_key = env_config.get(env_var, "")
            if api_key:
                result = _validate_key(provider, api_key)
                result["timestamp"] = datetime.now(timezone.utc).isoformat()
                key_validation[provider] = result

        # Validate service tokens (Rev.ai, Plaud)
        SERVICE_TOKEN_VALIDATORS = {
            "REVAI_ACCESS_TOKEN": ("revai", "solstone.observe.transcribe.revai"),
            "PLAUD_ACCESS_TOKEN": ("plaud", "solstone.think.importers.plaud"),
        }
        for env_var, (val_key, module_path) in SERVICE_TOKEN_VALIDATORS.items():
            api_key = env_config.get(env_var, "")
            if api_key:
                import importlib

                mod = importlib.import_module(module_path)
                result = mod.validate_token(api_key)
                result["timestamp"] = datetime.now(timezone.utc).isoformat()
                key_validation[val_key] = result

        # Validate vertex credentials if configured
        providers_config = config.get("providers", {})
        if providers_config.get("google_backend") == "vertex" and providers_config.get(
            "vertex_credentials"
        ):
            from solstone.think.providers.google import validate_vertex_credentials

            result = validate_vertex_credentials(
                providers_config["vertex_credentials"],
            )
            result["timestamp"] = datetime.now(timezone.utc).isoformat()
            key_validation["google"] = result

        config["providers"]["key_validation"] = key_validation

        config_dir = Path(state.journal_root) / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        config_path = config_dir / "journal.json"
        with open(config_path, "w", encoding="utf-8") as f:
            json.dump(config, f, indent=2, ensure_ascii=False)
            f.write("\n")
        os.chmod(config_path, 0o600)

        return jsonify({"success": True, "key_validation": key_validation})
    except Exception:
        logger.exception("error validating keys")
        return _settings_operation_failed()


@settings_bp.route("/api/providers", methods=["PUT"])
def update_providers() -> Any:
    """Update providers configuration.

    Accepts JSON with optional keys:
        - generate: {provider?, tier?, backup?} - Set generate defaults
        - cogitate: {provider?, tier?, backup?} - Set cogitate defaults
        - contexts: {pattern: {provider?, tier?, disabled?, extract?} | null}
          Set or clear context overrides

    Setting a context to null removes the override.
    For talent contexts, disabled and extract can also be set.
    """
    try:
        from solstone.think.providers import PROVIDER_REGISTRY

        request_data = request.get_json()
        if not request_data:
            return error_response(MISSING_REQUEST_BODY, detail="No data provided")

        config_dir = Path(state.journal_root) / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        config_path = config_dir / "journal.json"

        # Load existing config
        config = get_journal_config()
        old_providers = copy.deepcopy(config.get("providers", {}))

        # Ensure providers section exists
        if "providers" not in config:
            config["providers"] = {}

        changed_fields = {}

        # Handle type-specific updates (generate, cogitate)
        for agent_type in ("generate", "cogitate"):
            if agent_type not in request_data:
                continue

            type_data = request_data[agent_type]
            if agent_type not in config["providers"]:
                config["providers"][agent_type] = {}

            old_type = old_providers.get(agent_type, {})

            # Validate and update provider
            if "provider" in type_data:
                provider = type_data["provider"]
                if provider not in PROVIDER_REGISTRY:
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail=(
                            f"Invalid provider: {provider}. "
                            f"Must be one of: {', '.join(sorted(PROVIDER_REGISTRY.keys()))}"
                        ),
                    )
                if old_type.get("provider") != provider:
                    changed_fields[f"{agent_type}.provider"] = {
                        "old": old_type.get("provider"),
                        "new": provider,
                    }
                config["providers"][agent_type]["provider"] = provider

            # Validate and update tier
            if "tier" in type_data:
                tier = type_data["tier"]
                if tier not in VALID_TIERS:
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail=f"Invalid tier: {tier}. Must be 1, 2, or 3.",
                    )
                if old_type.get("tier") != tier:
                    changed_fields[f"{agent_type}.tier"] = {
                        "old": old_type.get("tier"),
                        "new": tier,
                    }
                config["providers"][agent_type]["tier"] = tier

            # Validate and update backup
            if "backup" in type_data:
                backup = type_data["backup"]
                if backup not in PROVIDER_REGISTRY:
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail=(
                            f"Invalid backup provider: {backup}. "
                            f"Must be one of: {', '.join(sorted(PROVIDER_REGISTRY.keys()))}"
                        ),
                    )
                if old_type.get("backup") != backup:
                    changed_fields[f"{agent_type}.backup"] = {
                        "old": old_type.get("backup"),
                        "new": backup,
                    }
                config["providers"][agent_type]["backup"] = backup

        # Handle context overrides
        if "contexts" in request_data:
            contexts_data = request_data["contexts"]
            if "contexts" not in config["providers"]:
                config["providers"]["contexts"] = {}

            old_contexts = old_providers.get("contexts", {})

            for pattern, ctx_config in contexts_data.items():
                old_ctx = old_contexts.get(pattern)

                # null means remove the override
                if ctx_config is None:
                    if pattern in config["providers"]["contexts"]:
                        changed_fields[f"contexts.{pattern}"] = {
                            "old": old_ctx,
                            "new": None,
                        }
                        del config["providers"]["contexts"][pattern]
                    continue

                # Validate provider if specified
                if "provider" in ctx_config:
                    provider = ctx_config["provider"]
                    if provider not in PROVIDER_REGISTRY:
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail=f"Invalid provider for {pattern}: {provider}",
                        )

                # Validate tier if specified
                if "tier" in ctx_config:
                    tier = ctx_config["tier"]
                    if tier not in VALID_TIERS:
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail=f"Invalid tier for {pattern}: {tier}",
                        )

                # Validate disabled if specified (must be boolean)
                if "disabled" in ctx_config:
                    if not isinstance(ctx_config["disabled"], bool):
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail=f"disabled for {pattern} must be a boolean",
                        )

                # Validate extract if specified (must be boolean)
                if "extract" in ctx_config:
                    if not isinstance(ctx_config["extract"], bool):
                        return error_response(
                            INVALID_CONFIG_VALUE,
                            detail=f"extract for {pattern} must be a boolean",
                        )

                # Only store if there's something to override
                if ctx_config:
                    if old_ctx != ctx_config:
                        changed_fields[f"contexts.{pattern}"] = {
                            "old": old_ctx,
                            "new": ctx_config,
                        }
                    config["providers"]["contexts"][pattern] = ctx_config

        # Handle MLX model selection
        if "mlx" in request_data:
            mlx_data = request_data["mlx"]
            if not isinstance(mlx_data, dict):
                return error_response(
                    INVALID_CONFIG_VALUE,
                    detail="mlx must be an object",
                )
            unknown_fields = sorted(set(mlx_data) - {"active_model"})
            if unknown_fields:
                return error_response(
                    INVALID_CONFIG_VALUE,
                    detail=f"Invalid mlx field: {unknown_fields[0]}",
                )
            if "active_model" in mlx_data:
                active_model = mlx_data["active_model"]
                if not isinstance(active_model, str):
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail="mlx.active_model must be a string",
                    )
                if active_model not in _MLX_MODEL_REGISTRY:
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail=(
                            f"Invalid MLX model: {active_model}. "
                            f"Must be one of: {', '.join(_MLX_MODEL_REGISTRY.keys())}"
                        ),
                    )
                if "mlx" not in config["providers"]:
                    config["providers"]["mlx"] = {}
                old_mlx = old_providers.get("mlx", {})
                old_model = (
                    old_mlx.get("active_model") if isinstance(old_mlx, dict) else None
                )
                if old_model != active_model:
                    changed_fields["mlx.active_model"] = {
                        "old": old_model,
                        "new": active_model,
                    }
                config["providers"]["mlx"]["active_model"] = active_model

        # Handle Google backend settings
        if "google_backend" in request_data:
            backend = request_data["google_backend"]
            if backend not in ("auto", "aistudio", "vertex"):
                return error_response(
                    INVALID_CONFIG_VALUE,
                    detail=(
                        f"Invalid google_backend: {backend}. "
                        "Must be 'auto', 'aistudio', or 'vertex'."
                    ),
                )
            old_val = old_providers.get("google_backend", "auto")
            if old_val != backend:
                changed_fields["google_backend"] = {"old": old_val, "new": backend}
            config["providers"]["google_backend"] = backend

        # Handle vertex credentials
        if "vertex_credentials" in request_data:
            vertex_creds_value = request_data["vertex_credentials"]

            if vertex_creds_value:
                # Parse and validate JSON structure
                try:
                    creds_data = (
                        json.loads(vertex_creds_value)
                        if isinstance(vertex_creds_value, str)
                        else vertex_creds_value
                    )
                except json.JSONDecodeError:
                    return error_response(
                        INVALID_JSON_REQUEST,
                        detail="Invalid JSON in vertex_credentials",
                    )

                required_fields = (
                    "type",
                    "project_id",
                    "client_email",
                    "private_key",
                )
                missing = [f for f in required_fields if f not in creds_data]
                if missing:
                    return error_response(
                        MISSING_REQUIRED_FIELD,
                        detail=f"Missing required fields: {', '.join(missing)}",
                    )

                # Save credentials file
                creds_dir = Path(state.journal_root) / ".config"
                creds_dir.mkdir(parents=True, exist_ok=True)
                creds_file = creds_dir / "vertex-credentials.json"
                with open(creds_file, "w", encoding="utf-8") as f:
                    json.dump(creds_data, f, indent=2, ensure_ascii=False)
                    f.write("\n")
                os.chmod(creds_file, 0o600)

                # Store path in config
                old_val = old_providers.get("vertex_credentials", "")
                creds_path_str = str(creds_file)
                if old_val != creds_path_str:
                    changed_fields["vertex_credentials"] = {
                        "old": old_val,
                        "new": creds_path_str,
                    }
                config["providers"]["vertex_credentials"] = creds_path_str

                # Validate credentials by attempting to list models
                validation = validate_vertex_credentials(creds_path_str)

                if not validation.get("valid"):
                    # Still save the file, but report the error.
                    # Don't block save - credentials may be valid with the right service account.
                    pass

                # Store validation result
                if "key_validation" not in config["providers"]:
                    config["providers"]["key_validation"] = {}
                config["providers"]["key_validation"]["google_vertex"] = {
                    **validation,
                    "timestamp": datetime.now(timezone.utc).isoformat(),
                }

            else:
                # Remove credentials — only delete the canonical path
                old_path = config["providers"].get("vertex_credentials")
                if old_path:
                    changed_fields["vertex_credentials"] = {
                        "old": old_path,
                        "new": None,
                    }
                    # Only delete the file we created, not arbitrary paths
                    canonical = (
                        Path(state.journal_root) / ".config" / "vertex-credentials.json"
                    )
                    if Path(old_path).resolve() == canonical.resolve():
                        try:
                            canonical.unlink(missing_ok=True)
                        except OSError:
                            pass
                    config["providers"].pop("vertex_credentials", None)
                    # Clear validation
                    kv = config["providers"].get("key_validation", {})
                    kv.pop("google_vertex", None)

        # Write back to file
        with open(config_path, "w", encoding="utf-8") as f:
            json.dump(config, f, indent=2, ensure_ascii=False)
            f.write("\n")
        os.chmod(config_path, 0o600)

        # Log if something changed
        if changed_fields:
            log_app_action(
                app="settings",
                facet=None,
                action="providers_update",
                params={"changed_fields": changed_fields},
            )

        # Return updated providers config
        return get_providers()

    except Exception:
        logger.exception("error saving providers")
        return _settings_operation_failed()


# ---------------------------------------------------------------------------
# Generators API (compatibility layer for Settings UI)
# ---------------------------------------------------------------------------


def _build_generator_info(key: str, info: dict) -> dict:
    """Build generator info dict using talent config for Settings UI.

    Transforms talent config metadata into the format expected by the
    Settings UI Insights section.
    """
    return {
        "key": key,
        "title": info.get("title", info.get("label", key)),
        "description": info.get("description", ""),
        "source": info.get("source", "system"),
        "app": info.get("app"),
        "disabled": info.get("disabled", False),
    }


@settings_bp.route("/api/generators")
def get_generators() -> Any:
    """Return generators grouped by schedule for Settings UI.

    This is a compatibility layer that transforms the unified talent config
    into the format expected by the Settings UI Insights section.

    Returns:
        - segment: List of segment-schedule generators
        - daily: List of daily-schedule generators
    """
    try:
        from solstone.think.talent import get_talent_configs

        # Get all generate prompts
        all_generators = get_talent_configs(type="generate", include_disabled=True)

        segment = []
        daily = []

        for key, info in all_generators.items():
            gen_info = _build_generator_info(key, info)
            schedule = info.get("schedule")

            if schedule == "segment":
                segment.append(gen_info)
            elif schedule == "daily":
                daily.append(gen_info)
            # Skip generators without valid schedule

        return jsonify({"segment": segment, "daily": daily})

    except Exception:
        logger.exception("error loading generators")
        return _settings_operation_failed()


@settings_bp.route("/api/generators", methods=["PUT"])
def update_generators() -> Any:
    """Update generator settings via providers.contexts.

    This is a compatibility layer that accepts the old generators API
    format and stores settings in the unified providers.contexts location.

    Accepts JSON with generator keys mapping to {disabled?, extract?}.
    """
    try:
        request_data = request.get_json()
        if not request_data:
            return error_response(MISSING_REQUEST_BODY, detail="No data provided")

        config_dir = Path(state.journal_root) / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        config_path = config_dir / "journal.json"

        # Load existing config
        config = get_journal_config()
        old_providers = copy.deepcopy(config.get("providers", {}))

        if "providers" not in config:
            config["providers"] = {}
        if "contexts" not in config["providers"]:
            config["providers"]["contexts"] = {}

        old_contexts = old_providers.get("contexts", {})
        changed_fields = {}

        from solstone.think.talent import key_to_context

        for key, updates in request_data.items():
            if not isinstance(updates, dict):
                continue

            context_key = key_to_context(key)

            # Get or create context config
            ctx_config = config["providers"]["contexts"].get(context_key, {})
            old_ctx = old_contexts.get(context_key, {})

            # Apply updates
            if "disabled" in updates:
                if not isinstance(updates["disabled"], bool):
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail=f"disabled must be boolean for {key}",
                    )
                ctx_config["disabled"] = updates["disabled"]

            if "extract" in updates:
                if not isinstance(updates["extract"], bool):
                    return error_response(
                        INVALID_CONFIG_VALUE,
                        detail=f"extract must be boolean for {key}",
                    )
                ctx_config["extract"] = updates["extract"]

            # Only store if there's something to override
            if ctx_config:
                if old_ctx != ctx_config:
                    changed_fields[f"contexts.{context_key}"] = {
                        "old": old_ctx if old_ctx else None,
                        "new": ctx_config,
                    }
                config["providers"]["contexts"][context_key] = ctx_config

        # Write back to file
        with open(config_path, "w", encoding="utf-8") as f:
            json.dump(config, f, indent=2, ensure_ascii=False)
            f.write("\n")
        os.chmod(config_path, 0o600)

        # Log if something changed
        if changed_fields:
            log_app_action(
                app="settings",
                facet=None,
                action="generators_update",
                params={"changed_fields": changed_fields},
            )

        # Return updated generators
        return get_generators()

    except Exception:
        logger.exception("error saving generators")
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

        config_dir = Path(state.journal_root) / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        config_path = config_dir / "journal.json"

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
                changed_fields["max_extractions"] = {"old": old_val, "new": max_ext}
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
                                f"Must be one of: {', '.join(sorted(VALID_IMPORTANCE))}"
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

        # Write back to file
        with open(config_path, "w", encoding="utf-8") as f:
            json.dump(config, f, indent=2, ensure_ascii=False)
            f.write("\n")
        os.chmod(config_path, 0o600)

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


@settings_bp.route("/api/observe", methods=["PUT"])
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

        config_dir = Path(state.journal_root) / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        config_path = config_dir / "journal.json"

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
            with open(config_path, "w", encoding="utf-8") as f:
                json.dump(config, f, indent=2, ensure_ascii=False)
                f.write("\n")
            os.chmod(config_path, 0o600)

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

        # Generate slug from title: lowercase, replace spaces/special chars with hyphens
        slug = re.sub(r"[^a-z0-9]+", "-", title.lower())
        slug = slug.strip("-")  # Remove leading/trailing hyphens

        if not slug:
            return error_response(
                INVALID_REQUEST_VALUE,
                detail="Title must contain at least one letter or number",
            )

        # Check for conflicts with existing facets
        from solstone.think.facets import get_facets

        existing = get_facets()
        if slug in existing:
            return error_response(
                FACET_ALREADY_EXISTS,
                detail=f"Facet '{slug}' already exists",
            )

        # Create facet directory and config
        facet_path = Path(state.journal_root) / "facets" / slug
        facet_path.mkdir(parents=True, exist_ok=True)

        config = {
            "title": title,
            "description": "",
            "color": color,
            "emoji": emoji,
        }

        config_file = facet_path / "facet.json"
        with open(config_file, "w", encoding="utf-8") as f:
            json.dump(config, f, indent=2, ensure_ascii=False)
            f.write("\n")

        # Log the creation
        log_app_action(
            app="settings",
            facet=slug,
            action="facet_create",
            params={"title": title, "emoji": emoji, "color": color},
        )

        return jsonify({"success": True, "facet": slug, "config": config}), 201

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

        # Build path to facet config file
        facet_path = Path(state.journal_root) / "facets" / facet_name
        config_file = facet_path / "facet.json"

        if not facet_path.exists():
            return error_response(FACET_NOT_FOUND, detail="Facet not found")

        # Read existing config or create new one
        if config_file.exists():
            with open(config_file, "r", encoding="utf-8") as f:
                config = json.load(f)
        else:
            config = {}

        # Track changes for logging
        changed_fields = {}
        allowed_fields = ["title", "description", "color", "emoji", "muted"]
        for field in allowed_fields:
            if field in data:
                old_value = config.get(field)
                new_value = data[field]
                if old_value != new_value:
                    changed_fields[field] = {"old": old_value, "new": new_value}
                config[field] = new_value

        # Write back to file
        with open(config_file, "w", encoding="utf-8") as f:
            json.dump(config, f, indent=2, ensure_ascii=False)
            f.write("\n")

        # Log only if something actually changed
        if changed_fields:
            log_app_action(
                app="settings",
                facet=facet_name,
                action="facet_update",
                params={"changed_fields": changed_fields},
            )

        return jsonify({"success": True, "facet": facet_name, "config": config})
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

        config_dir = Path(state.journal_root) / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        schedules_path = config_dir / "schedules.json"

        # Load existing schedules
        schedules = {}
        if schedules_path.exists():
            with open(schedules_path, "r", encoding="utf-8") as f:
                schedules = json.load(f)

        changed_fields = {}

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
                            "cmd": ["sol", "import", "--sync", "plaud", "--save"],
                            "every": "hourly",
                        }
                    schedules["sync:plaud"]["enabled"] = enabled
                    changed_fields["plaud.enabled"] = enabled

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
                                "sol",
                                "import",
                                "--sync",
                                "granola",
                                "--save",
                            ],
                            "every": "hourly",
                        }
                    schedules["sync:granola"]["enabled"] = enabled
                    changed_fields["granola.enabled"] = enabled

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
                                "sol",
                                "import",
                                "--sync",
                                "obsidian",
                                "--save",
                            ],
                            "every": "hourly",
                        }
                    schedules["sync:obsidian"]["enabled"] = enabled
                    changed_fields["obsidian.enabled"] = enabled

        if changed_fields:
            with open(schedules_path, "w", encoding="utf-8") as f:
                json.dump(schedules, f, indent=2, ensure_ascii=False)
                f.write("\n")

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
                changed["raw_media"] = {"old": retention.get("raw_media"), "new": mode}
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

        config_dir = Path(state.journal_root) / "config"
        config_dir.mkdir(parents=True, exist_ok=True)
        config_path = config_dir / "journal.json"

        with open(config_path, "w", encoding="utf-8") as f:
            json.dump(config, f, indent=2, ensure_ascii=False)
            f.write("\n")
        os.chmod(config_path, 0o600)

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
    except Exception:
        logger.exception("error running purge")
        return _settings_operation_failed()
