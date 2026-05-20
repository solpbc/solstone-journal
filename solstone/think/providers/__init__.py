# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""AI provider backends for think.

This package contains provider-specific implementations for LLM generation
and agent execution. Each provider module exposes:

- run_generate(): Sync text generation, returns GenerateResult
- run_agenerate(): Async text generation, returns GenerateResult
- run_cogitate(): Tool-calling execution with event streaming

GenerateResult is a TypedDict with: text, usage, finish_reason, thinking.
The wrapper functions in think.models handle token logging and JSON validation.

Available providers:
- google: Google Gemini models
- openai: OpenAI GPT models
- anthropic: Anthropic Claude models
- ollama: Ollama local models
- mlx: MLX local Apple Silicon models
"""

import os
import shutil
from importlib import import_module
from types import ModuleType
from typing import Any, Dict, List

# ---------------------------------------------------------------------------
# Provider Registry
# ---------------------------------------------------------------------------
# Central registry of supported providers and their module paths.
# All registered providers must implement:
#   - run_generate(contents, model, ...) -> GenerateResult
#   - run_agenerate(contents, model, ...) -> GenerateResult
#   - run_cogitate(config, on_event) -> str
# ---------------------------------------------------------------------------

PROVIDER_REGISTRY: Dict[str, str] = {
    "google": "solstone.think.providers.google",
    "openai": "solstone.think.providers.openai",
    "anthropic": "solstone.think.providers.anthropic",
    "ollama": "solstone.think.providers.ollama",
    "mlx": "solstone.think.providers.mlx",
}

# ---------------------------------------------------------------------------
# Provider Metadata
# ---------------------------------------------------------------------------
# Display labels, environment variable names, and cogitate CLI binary names
# for each provider. Used by settings UI, provider status, and agent health
# checks.
# ---------------------------------------------------------------------------

PROVIDER_METADATA: Dict[str, Dict[str, Any]] = {
    "google": {
        "label": "Google (Gemini)",
        "env_key": "GOOGLE_API_KEY",
        "cogitate_cli": "gemini",
        "vertex_env_keys": [
            "GOOGLE_GENAI_USE_VERTEXAI",
            "GOOGLE_APPLICATION_CREDENTIALS",
        ],
    },
    "openai": {
        "label": "OpenAI (GPT)",
        "env_key": "OPENAI_API_KEY",
        "cogitate_cli": "codex",
    },
    "anthropic": {
        "label": "Anthropic (Claude)",
        "env_key": "ANTHROPIC_API_KEY",
        "cogitate_cli": "claude",
    },
    "ollama": {
        "label": "Ollama (Local)",
        "env_key": "",
        "cogitate_cli": "opencode",
        "cogitate_cli_install": "curl -fsSL https://opencode.ai/install | bash",
    },
    "mlx": {
        "label": "MLX (Local, Apple Silicon)",
        "env_key": "",
    },
}


def get_provider_module(provider: str) -> ModuleType:
    """Get the provider module for the given provider name.

    Parameters
    ----------
    provider
        Provider name (e.g., "google", "openai", "anthropic").

    Returns
    -------
    ModuleType
        The provider module with run_generate, run_agenerate, and run_cogitate functions.

    Raises
    ------
    ValueError
        If the provider is not registered.
    """
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
        if "vertex_env_keys" in meta:
            provider["vertex_env_keys"] = meta["vertex_env_keys"]
        providers.append(provider)
    return providers


def build_provider_status(
    providers_list: List[Dict[str, Any]],
    vertex_creds_configured: bool = False,
) -> Dict[str, Dict[str, Any]]:
    """Build per-provider readiness status.

    Parameters
    ----------
    providers_list
        Output of get_provider_list().
    vertex_creds_configured
        Whether Vertex AI credentials are configured (for Google).

    Returns
    -------
    Dict[str, Dict[str, Any]]
        Keyed by provider name. Each entry has: configured, generate_ready,
        cogitate_ready, cogitate_cli, cogitate_cli_found, issues.
    """
    status = {}
    bundled_cli_states = {"installed-no-key", "key-validating", "valid", "invalid-key"}
    for provider in providers_list:
        name = provider["name"]
        env_key = provider.get("env_key", "")
        meta = PROVIDER_METADATA.get(name, {})
        cogitate_cli = meta.get("cogitate_cli", "")
        issues: list[str] = []
        bundled_state: dict[str, Any] | None = None

        if name == "ollama":
            try:
                import httpx

                base_url = os.getenv(
                    "OLLAMA_BASE_URL", "http://localhost:11434"
                ).rstrip("/")
                resp = httpx.get(f"{base_url}/api/version", timeout=2)
                resp.raise_for_status()
                configured = True
            except Exception:
                configured = False
                base_url = os.getenv(
                    "OLLAMA_BASE_URL", "http://localhost:11434"
                ).rstrip("/")
                issues.append(f"Ollama not reachable at {base_url}")
        elif name == "google":
            has_key = bool(os.getenv(env_key))
            configured = has_key or vertex_creds_configured
            if not configured:
                issues.append(f"{env_key} not set")
        elif name in {"anthropic", "openai"}:
            from solstone.think.providers import bundled

            bundled_state = bundled.get_provider_state(name)
            configured = bool(bundled_state["key_configured"])
            if not configured and env_key:
                issues.append(f"{env_key} not set")
        else:
            configured = bool(os.getenv(env_key)) if env_key else False
            if not configured and env_key:
                issues.append(f"{env_key} not set")

        if bundled_state is not None:
            bundled_contract_state = bundled_state["state"]
            cogitate_cli_found = bundled_contract_state in bundled_cli_states
            if not cogitate_cli_found:
                issues.append(
                    f"bundled CLI not installed — run `sol call settings providers install {name}`"
                )
            elif bundled_contract_state == "invalid-key":
                issues.extend(bundled_state.get("issues", []))
            cogitate_ready = (
                bundled_contract_state in {"installed-no-key", "valid"} and configured
            )
        else:
            cogitate_cli_found = (
                bool(shutil.which(cogitate_cli)) if cogitate_cli else False
            )
            if cogitate_cli and not cogitate_cli_found:
                install_cmd = meta.get("cogitate_cli_install")
                if install_cmd:
                    issues.append(
                        f"{cogitate_cli} CLI not found on PATH — run: {install_cmd}"
                    )
                else:
                    issues.append(f"{cogitate_cli} CLI not found on PATH")
            cogitate_ready = configured and cogitate_cli_found

        generate_ready = configured

        status[name] = {
            "configured": configured,
            "generate_ready": generate_ready,
            "cogitate_ready": cogitate_ready,
            "cogitate_cli": cogitate_cli,
            "cogitate_cli_found": cogitate_cli_found,
            "issues": issues,
        }
    return status


def get_provider_models(provider: str) -> list[dict]:
    """Get available models for a provider.

    Parameters
    ----------
    provider
        Provider name (e.g., "google", "openai", "anthropic").

    Returns
    -------
    list[dict]
        List of raw model info objects returned by the provider API.

    Raises
    ------
    ValueError
        If the provider is not registered.
    """
    module = get_provider_module(provider)
    return module.list_models()


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
    return module.validate_key(api_key)


__all__ = [
    "PROVIDER_REGISTRY",
    "PROVIDER_METADATA",
    "get_provider_module",
    "get_provider_list",
    "build_provider_status",
    "get_provider_models",
    "validate_key",
]
