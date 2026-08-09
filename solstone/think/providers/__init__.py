# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""AI provider backends for think.

This package exposes the cogitate provider implementations and settings
validation helpers. Native ``solstone-core generate`` owns all generation.

Available providers:
- google: Google Gemini models
- openai: OpenAI GPT models
- anthropic: Anthropic Claude models
- local: bundled llama-server or configured OpenAI-compatible endpoint
"""

from importlib import import_module
from types import ModuleType
from typing import Any, Dict, List

# ---------------------------------------------------------------------------
# Provider Registry
# ---------------------------------------------------------------------------
# Central registry of supported providers and their module paths.
# All registered provider module targets implement run_cogitate(config, on_event).
# ---------------------------------------------------------------------------

PROVIDER_REGISTRY: Dict[str, str] = {
    "google": "solstone.think.providers.openhands",
    "openai": "solstone.think.providers.openhands",
    "anthropic": "solstone.think.providers.openhands",
    "local": "solstone.think.providers.local",
}

# ---------------------------------------------------------------------------
# Provider Metadata
# ---------------------------------------------------------------------------
# Display labels, environment variable names, and CLI metadata where applicable.
# Used by settings UI, provider status, and agent health checks.
# ---------------------------------------------------------------------------

PROVIDER_METADATA: Dict[str, Dict[str, Any]] = {
    "google": {
        "label": "Google (Gemini)",
        "env_key": "GOOGLE_API_KEY",
    },
    "openai": {
        "label": "OpenAI (GPT)",
        "env_key": "OPENAI_API_KEY",
    },
    "anthropic": {
        "label": "Anthropic (Claude)",
        "env_key": "ANTHROPIC_API_KEY",
    },
    "local": {
        "label": "Local (on-device)",
        "env_key": "",
    },
}


def managed_provider_env_keys() -> set[str]:
    """Return the set of managed provider API-key environment-variable names.

    Derived from ``PROVIDER_METADATA``: every non-empty ``env_key`` across the
    registered providers (currently GOOGLE_API_KEY, OPENAI_API_KEY,
    ANTHROPIC_API_KEY; ``local`` has no key). These are exactly the keys for which
    journal config's ``env`` section is the authoritative and exclusive source — a
    managed key absent from journal config is stripped from ``os.environ`` at CLI
    startup (see :func:`solstone.think.utils.setup_cli`) so a shell-set value is
    never used.
    """
    return {m["env_key"] for m in PROVIDER_METADATA.values() if m.get("env_key")}


def is_cloud_provider(provider: str) -> bool:
    """Return True when a registered provider uses a managed cloud API key."""

    meta = PROVIDER_METADATA.get(provider)
    return bool(meta and meta.get("env_key"))


def get_provider_module(provider: str) -> ModuleType:
    """Get the provider module for the given provider name.

    Parameters
    ----------
    provider
        Provider name (e.g., "google", "openai", "anthropic").

    Returns
    -------
    ModuleType
        The provider module with run_cogitate.

    Raises
    ------
    ValueError
        If the provider is not registered.
    """
    if provider == "none":
        from solstone.think.models import NoBrainConfiguredError

        raise NoBrainConfiguredError()
    if provider not in PROVIDER_REGISTRY:
        valid = ", ".join(sorted(PROVIDER_REGISTRY.keys()))
        raise ValueError(f"Unknown provider: {provider!r}. Valid providers: {valid}")

    return import_module(PROVIDER_REGISTRY[provider])


def get_provider_list() -> List[Dict[str, Any]]:
    """Get list of providers with metadata for UI display.

    Returns
    -------
    List[Dict[str, Any]]
        List of provider info dicts, each containing:
        - name: Provider identifier (e.g., "google")
        - label: Display label (e.g., "Google (Gemini)")
        - env_key: Environment variable for API key
    """
    providers = []
    for name in PROVIDER_REGISTRY:
        meta = PROVIDER_METADATA.get(name, {"label": name, "env_key": ""})
        provider = {
            "name": name,
            "label": meta.get("label", name),
            "env_key": meta.get("env_key", ""),
        }
        providers.append(provider)
    return providers


def build_provider_status(
    providers_list: List[Dict[str, Any]] | None = None,
    *,
    config: dict[str, Any] | None = None,
    local_status: dict[str, Any] | None = None,
) -> Dict[str, Dict[str, Any]]:
    """Build per-provider readiness status.

    Parameters
    ----------
    providers_list
        Output of get_provider_list().
    local_status
        When provided, replaces the ``local`` row and ``local_status_dict()`` is
        not called. ``None`` preserves existing behavior.
    config
        An already-read journal config. When omitted, existing helper reads are
        preserved.
    Returns
    -------
    Dict[str, Dict[str, Any]]
        Keyed by provider name. Each entry has readiness fields and issues.
    """
    if providers_list is None:
        providers_list = get_provider_list()

    status: dict[str, dict[str, Any]] = {}
    for provider in providers_list:
        name = provider["name"]
        env_key = provider.get("env_key", "")
        if name == "local" and local_status is not None:
            status[name] = dict(local_status)
            continue

        from solstone.think.providers import state as provider_state

        if name == "local":
            status[name] = provider_state.local_status_dict(config=config)
            continue
        configured = provider_state.cloud_key_configured(env_key, config=config)
        status[name] = {
            "provider": name,
            "configured": configured,
            "generate_ready": configured,
            "cogitate_ready": configured,
            "issues": [] if configured else [f"{env_key} not set"],
        }
    return status


def validate_key(provider: str, api_key: str) -> dict:
    """Validate an API key for a provider.

    Parameters
    ----------
    provider
        Provider name (e.g., "google", "openai", "anthropic").
    api_key
        The API key string to validate.

    Returns
    -------
    dict
        {"valid": True} or {"valid": False, "error": "..."}.

    Raises
    ------
    ValueError
        If the provider is not registered.
    """
    module = get_provider_module(provider)
    return module.validate_key(provider, api_key)


def validate_model(provider: str, model: str, api_key: str) -> dict:
    """Validate that a provider API key can see a specific model.

    Parameters
    ----------
    provider
        Provider name (e.g., "google", "openai", "anthropic").
    model
        Provider-native model identifier to probe.
    api_key
        The API key string to validate against.

    Returns
    -------
    dict
        Success returns {"valid": True}. Failure returns
        {"valid": False, "error": <str>, "reason_code": <str>}; reason_code is
        "model_not_found" when the provider reports the model id is unknown to
        this key, otherwise a classify_provider_error code such as
        "provider_key_invalid".

    Raises
    ------
    ValueError
        If the provider is not registered.
    """
    module = get_provider_module(provider)
    return module.validate_model(provider, model, api_key)


__all__ = [
    "PROVIDER_REGISTRY",
    "PROVIDER_METADATA",
    "get_provider_module",
    "get_provider_list",
    "build_provider_status",
    "validate_key",
    "validate_model",
    "is_cloud_provider",
    "managed_provider_env_keys",
]
