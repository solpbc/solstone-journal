# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import functools
import inspect
import json
import logging
import os
import re
import time
from collections.abc import Mapping
from pathlib import Path
from typing import Any, Callable, Dict, List, NamedTuple, Optional, Union

import frontmatter

from solstone.think.providers.local_endpoint import (
    confidential_provenance_block,
    resolve_local_endpoint_from_config,
)
from solstone.think.utils import get_config, get_journal

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Model constants
#
# IMPORTANT: When updating these models, verify pricing support:
#   1. Run: pytest tests/test_models.py::test_all_default_models_have_pricing
#   2. If test fails, update genai-prices: make update-prices
#   3. If still failing, the model may be too new for genai-prices
#
# The genai-prices library provides token cost data. New models may not have
# pricing immediately after release. See: https://pypi.org/project/genai-prices/
# ---------------------------------------------------------------------------

# Valid OpenAI reasoning effort suffixes appended to model names.
# E.g., "gpt-5.2-high" → reasoning_effort="high", "gpt-5.2" → omitted.
OPENAI_EFFORT_SUFFIXES = ("-none", "-low", "-medium", "-high", "-xhigh")


class _Family(NamedTuple):
    key: tuple[str, str | None]
    version: tuple[int, ...]


def _parse_family_openai(model: str) -> _Family | None:
    model = model.lower()
    if model.startswith("ft:") or "-image" in model or not model.startswith("gpt-"):
        return None
    match = re.fullmatch(r"gpt-(\d+)(?:\.(\d+))?(?:-(mini|nano|pro))?", model)
    if match is None:
        return None
    return _Family(
        key=("openai", match.group(3)),
        version=(int(match.group(1)), int(match.group(2) or 0)),
    )


def _parse_family_anthropic(model: str) -> _Family | None:
    model = model.lower()
    match = re.fullmatch(r"claude-(opus|sonnet|haiku)-(\d+)(?:-(\d+))?", model)
    if match is None:
        return None
    return _Family(
        key=("anthropic", match.group(1)),
        version=(int(match.group(2)), int(match.group(3) or 0)),
    )


def _parse_family_gemini(model: str) -> _Family | None:
    model = model.lower()
    # Vendor serves these aliases; owners may hold them, and pro-latest is manual.
    latest_aliases = {
        "gemini-flash-latest": _Family(key=("gemini", "flash"), version=(0, 0)),
        "gemini-pro-latest": _Family(key=("gemini", "pro"), version=(0, 0)),
        "gemini-flash-lite-latest": _Family(
            key=("gemini", "flash-lite"),
            version=(0, 0),
        ),
    }
    if model in latest_aliases:
        return latest_aliases[model]
    if "-image" in model:
        return None
    if model.endswith("-preview"):
        model = model[: -len("-preview")]
    match = re.fullmatch(r"gemini-(\d+)(?:\.(\d+))?-(pro|flash|flash-lite)", model)
    if match is None:
        return None
    return _Family(
        key=("gemini", match.group(3)),
        version=(int(match.group(1)), int(match.group(2) or 0)),
    )


_FAMILY_PARSERS: dict[str, Callable[[str], _Family | None]] = {
    "openai": _parse_family_openai,
    "anthropic": _parse_family_anthropic,
    "google": _parse_family_gemini,
}

_LOGGED_FALLBACKS: set[str] = set()


@functools.lru_cache(maxsize=None)
def _find_pricing_fallback(model: str, provider_id: str) -> str | None:
    parser = _FAMILY_PARSERS.get(provider_id)
    if parser is None:
        return None
    target = parser(model)
    if target is None:
        return None

    from genai_prices.data import providers

    best: tuple[tuple[int, ...], str] | None = None
    for provider in providers:
        if provider.id != provider_id:
            continue
        for snapshot_model in provider.models:
            candidate = parser(snapshot_model.id)
            if candidate is None or candidate.key != target.key:
                continue
            if best is None or candidate.version > best[0]:
                best = (candidate.version, snapshot_model.id)
    return best[1] if best else None


GEMINI_FLASH = "gemini-3.5-flash"

GPT_5 = "gpt-5.5"
GPT_5_MINI = "gpt-5.4-mini"

CLAUDE_OPUS_4 = "claude-opus-4-7"
CLAUDE_SONNET_4 = "claude-sonnet-4-6"

LOCAL_MODEL = "local/qwen3.5-4b"

QWEN_35_9B = "qwen3.5:9b"
GEMMA4_26B_A4B_4BIT = "gemma-4-26b-a4b-it-mlx-4bit"


# Per-model request parameter capability overrides.
# Anthropic reasoning-model temperature deprecation: Opus 4.7 rejects temperature.
# Canonical error string: 'temperature' is deprecated for this model.
# Missing models/params are treated as supported so providers stay permissive by default.
MODEL_CAPABILITIES: dict[str, dict[str, bool]] = {
    CLAUDE_OPUS_4: {"temperature": False},
}


def model_supports(model: str, param: str) -> bool:
    return MODEL_CAPABILITIES.get(model, {}).get(param) is not False


NO_BRAIN_PROVIDER = "none"
DEFAULT_MODEL_BY_PROVIDER: dict[str, str] = {
    "google": GEMINI_FLASH,
    "openai": GPT_5_MINI,
    "anthropic": CLAUDE_SONNET_4,
    "local": LOCAL_MODEL,
}


# ---------------------------------------------------------------------------
# Exceptions
# ---------------------------------------------------------------------------


_LENGTH_FINISH_REASONS = frozenset({"length", "max_tokens"})


class IncompleteJSONError(ValueError):
    """Raised when JSON response is truncated due to token limits or other reasons.

    Attributes:
        reason: The finish/stop reason from the API (e.g., "MAX_TOKENS", "length").
        partial_text: The truncated response text, useful for debugging.
    """

    def __init__(self, reason: str, partial_text: str):
        self.reason = reason
        self.partial_text = partial_text
        # Safety/content-filter/recitation finishes are refusals, not length
        # truncations; labeling them incomplete_json_length would be dishonest
        # and retrying a refusal would not help.
        if str(reason).strip().lower() in _LENGTH_FINISH_REASONS:
            self.reason_code = "incomplete_json_length"
        super().__init__(f"JSON response incomplete (reason: {reason})")


class IncompleteTextError(ValueError):
    """Raised when a non-JSON response is truncated due to token limits."""

    def __init__(self, reason: str, partial_text: str):
        self.reason = reason
        self.partial_text = partial_text
        self.reason_code = "incomplete_text_length"
        super().__init__(f"Text response incomplete (reason: {reason})")


class ProviderResponseInvalidError(ValueError):
    """Raised when a provider response is unusable or malformed.

    finish_reason is a normalized bounded token or None. token_counts carries
    bounded scalar counts only. Raw responses, prompts, and output text are not
    carried.
    """

    reason_code = "provider_response_invalid"

    def __init__(
        self,
        reason: str,
        *,
        finish_reason: str | None = None,
        model: str | None = None,
        token_counts: dict[str, int] | None = None,
    ):
        self.reason = reason
        self.finish_reason = finish_reason
        self.model = model
        self.token_counts = dict(token_counts or {})
        super().__init__(f"Provider response did not finish cleanly (reason: {reason})")


class SchemaValidationError(ValueError):
    """Raised when JSON response text fails local schema validation.

    Attributes:
        errors: The schema validation errors returned by the native wire.
        text: The full offending response text.
        preview: A short preview of the offending response text for error messages.
    """

    def __init__(self, errors: list[dict], text: str):
        self.errors = errors
        self.text = text
        self.preview = text if len(text) <= 200 else text[:197] + "..."
        super().__init__(
            "JSON response failed schema validation "
            f"({len(errors)} error(s); preview={self.preview!r})"
        )


class NoBrainConfiguredError(RuntimeError):
    """Raised when no thinking engine has been selected for model execution."""

    reason_code = "thinking_engine_not_chosen"
    provider = NO_BRAIN_PROVIDER

    def __init__(self) -> None:
        super().__init__("No thinking engine is chosen yet. Choose one in Thinking.")


class AttestationNotVerifiedError(RuntimeError):
    """Raised when confidential processing has not passed attestation."""

    reason_code = "attestation_not_yet_verified"

    def __init__(self) -> None:
        super().__init__(
            "Confidential lane is verifying; hardware attestation is not yet verified."
        )


class AttestationFailedError(AttestationNotVerifiedError):
    """Raised when confidential hardware attestation fails closed."""

    reason_code = "attestation_failed"

    def __init__(self, detail: str) -> None:
        self.detail = detail
        RuntimeError.__init__(self, f"Confidential attestation failed: {detail}.")


class AttestationStaleError(AttestationNotVerifiedError):
    """Raised when confidential hardware attestation has gone stale."""

    reason_code = "attestation_stale"

    def __init__(self, detail: str) -> None:
        self.detail = detail
        RuntimeError.__init__(self, f"Confidential attestation is stale: {detail}.")


# Attestation failures are non-retryable. AttestationFailedError is raised by
# solstone.think.services.spp_attest.composite.verify_composite. AttestationStaleError
# remains part of the verifier contract; the process-local egress transport rotates
# stale live sessions inline before forwarding bytes.
_CONFIDENTIAL_ATTESTATION_VERIFIER: Callable[[dict[str, Any]], None] | None = None


def _confidential_attestation_verifier() -> Callable[[dict[str, Any]], None]:
    global _CONFIDENTIAL_ATTESTATION_VERIFIER

    if _CONFIDENTIAL_ATTESTATION_VERIFIER is None:
        from solstone.think.services.spp_transport import (
            verify_confidential_attestation,
        )

        _CONFIDENTIAL_ATTESTATION_VERIFIER = verify_confidential_attestation
    return _CONFIDENTIAL_ATTESTATION_VERIFIER


# ---------------------------------------------------------------------------
# Prompt context discovery
#
# Context metadata (label, group, type) is defined in prompt .md files via
# YAML frontmatter. This eliminates duplication between code and config.
#
# NAMING CONVENTION:
#   {module}.{feature}[.{operation}]
#
# Examples:
#   - observe.describe.frame    -> observe module, describe feature, frame operation
#   - observe.extract           -> observe module, extract feature (no sub-operation)
#   - talent.system.meetings      -> talent module, system source, meetings config
#   - talent.entities.observer    -> talent module, entities app, observer config
#   - app.chat.title            -> apps module, chat app, title operation
#
# DISCOVERY SOURCES:
#   1. Prompt files listed in PROMPT_PATHS (with context in frontmatter)
#   2. Checked-in category registry generated from native describe definitions
#   3. Talent configs from talent/*.md and apps/*/talent/*.md
#
# When adding new contexts:
#   1. Create a .md prompt file with YAML frontmatter containing:
#      context, label, group
#   2. Add the path to PROMPT_PATHS
# ---------------------------------------------------------------------------

# Flat list of prompt files that define context metadata in frontmatter.
# Each must have: context, label, group in YAML frontmatter.
PROMPT_PATHS: List[str] = [
    "observe/describe.md",
    "observe/extract.md",
    "think/detect_created.md",
    "think/detect_transcript_segment.md",
    "think/detect_transcript_json.md",
    "think/planner.md",
]


# ---------------------------------------------------------------------------
# Dynamic context discovery
# ---------------------------------------------------------------------------

# Cached context registry (built lazily on first use)
_context_registry: Optional[Dict[str, Dict[str, Any]]] = None
_LEGACY_CONTEXT_PREFIX = "talent."
_TALENT_CONTEXT_PREFIX = "talent."


def _discover_prompt_contexts() -> Dict[str, Dict[str, Any]]:
    """Load context metadata from prompt files listed in PROMPT_PATHS.

    Each file must have YAML frontmatter with:
    - context: The context string (e.g., "observe.extract")
    - label: Human-readable name
    - group: Settings UI category

    Returns
    -------
    Dict[str, Dict[str, Any]]
        Mapping of context patterns to {label, group} dicts.
    """
    contexts = {}
    base_dir = Path(__file__).parent.parent  # Package root

    for rel_path in PROMPT_PATHS:
        path = base_dir / rel_path
        if not path.exists():
            logging.getLogger(__name__).warning(f"Prompt file not found: {path}")
            continue

        try:
            post = frontmatter.load(path)
            meta = post.metadata or {}

            context = meta.get("context")
            if not context:
                logging.getLogger(__name__).warning(f"No context in {path}")
                continue

            contexts[context] = {
                "label": meta.get("label", context),
                "group": meta.get("group", "Other"),
            }
        except Exception as e:
            logging.getLogger(__name__).warning(f"Failed to load {path}: {e}")

    return contexts


def _discover_talent_contexts() -> Dict[str, Dict[str, Any]]:
    """Discover talent context defaults from talent/*.md config files.

    Uses get_talent_configs() from solstone.think.talent to load all talent
    configurations and converts them to context patterns with label/group/type
    metadata.

    Returns
    -------
    Dict[str, Dict[str, Any]]
        Mapping of context patterns to {label, group, type} dicts.
        Context patterns are: talent.system.{name} or talent.{app}.{name}
    """
    from solstone.think.talent import get_talent_configs, key_to_context

    contexts = {}

    # Load all talent configs (including disabled for completeness)
    all_configs = get_talent_configs(include_disabled=True)

    for key, config in all_configs.items():
        context = key_to_context(key)
        contexts[context] = {
            "label": config.get("label", config.get("title", key)),
            "group": config.get("group", "Think"),
            "type": config.get("type"),
        }

    return contexts


def _build_context_registry() -> Dict[str, Dict[str, Any]]:
    """Build complete context registry from discovered configs.

    Merges:
    1. Prompt contexts from _discover_prompt_contexts()
    2. Category contexts from the native-generated observe category registry
    3. Talent contexts from _discover_talent_contexts()

    Returns
    -------
    Dict[str, Dict[str, Any]]
        Complete context registry mapping patterns to {label, group, type?}.
    """
    # Start with prompt contexts (from PROMPT_PATHS)
    registry = _discover_prompt_contexts()

    # This registry is checked in and generated from native definitions. A load
    # failure is a packaging/configuration error, not an optional enhancement.
    from solstone.observe.category_registry import CATEGORIES

    for category, metadata in CATEGORIES.items():
        context = metadata.get("context", f"observe.describe.{category}")
        registry[context] = {
            "label": metadata.get("label", category.replace("_", " ").title()),
            "group": metadata.get("group", "Screen Analysis"),
        }

    # Merge talent contexts (agents + generators)
    talent_contexts = _discover_talent_contexts()
    registry.update(talent_contexts)

    return registry


def get_context_registry() -> Dict[str, Dict[str, Any]]:
    """Get the complete context registry, building it lazily on first use.

    Returns
    -------
    Dict[str, Dict[str, Any]]
        Complete context registry mapping patterns to {label, group, type?}.
    """
    global _context_registry
    if _context_registry is None:
        _context_registry = _build_context_registry()
    return _context_registry


def default_model_for_provider(provider: str) -> str:
    """Return the single default model for a provider."""
    if provider == NO_BRAIN_PROVIDER:
        return ""
    try:
        return DEFAULT_MODEL_BY_PROVIDER[provider]
    except KeyError as exc:
        raise ValueError(f"Unknown provider: {provider!r}") from exc


def resolve_provider(agent_type: str) -> tuple[str, str]:
    """Resolve the journal's one active thinking provider and model.

    ``agent_type`` identifies the runtime interface for callers and diagnostics;
    both generate and cogitate intentionally use the same active brain.
    """
    if agent_type not in {"generate", "cogitate"}:
        raise ValueError(f"Unknown thinking interface: {agent_type!r}")
    config = get_config()
    providers = config.get("providers", {})
    if not isinstance(providers, dict):
        providers = {}

    active = providers.get("active", {})
    if not isinstance(active, dict):
        active = {}
    provider = active.get("provider")
    if not isinstance(provider, str) or not provider:
        provider = NO_BRAIN_PROVIDER
    if provider == NO_BRAIN_PROVIDER:
        return (NO_BRAIN_PROVIDER, "")

    explicit_model = active.get("model")
    if isinstance(explicit_model, str) and explicit_model.strip():
        return (provider, explicit_model.strip())
    return (provider, default_model_for_provider(provider))


def resolve_effective_route(context: str) -> tuple[str, str, str]:
    """Return (interface, provider, model) for a context's effective route.

    Interface is the talent context's registry ``type`` when it is one of
    generate/cogitate, else "generate" — never pass any other value to
    resolve_provider.
    """
    registry_entry = get_context_registry().get(context)
    interface = (
        registry_entry["type"]
        if registry_entry and registry_entry.get("type") in ("generate", "cogitate")
        else "generate"
    )
    provider, model = resolve_provider(interface)
    return (interface, provider, model)


def is_local_provider_needed(config: dict[str, Any] | None = None) -> bool:
    """Return True when the journal's active thinking provider is local."""
    journal_config = config if config is not None else get_config()
    providers = journal_config.get("providers", {})
    if not isinstance(providers, dict):
        return False
    active = providers.get("active", {})
    return isinstance(active, dict) and active.get("provider") == "local"


def no_thinking_engine_chosen(config: dict[str, Any] | None = None) -> bool:
    """Return True when the journal has no active thinking engine."""
    journal_config = config if config is not None else get_config()
    providers = journal_config.get("providers", {})
    if not isinstance(providers, dict):
        providers = {}
    active = providers.get("active", {})
    return not isinstance(active, dict) or not active.get("provider")


def type_default_is_local(
    agent_type: str, config: dict[str, Any] | None = None
) -> bool:
    """Return True when the active brain is local for this runtime interface."""
    if agent_type not in {"generate", "cogitate"}:
        raise ValueError(f"Unknown thinking interface: {agent_type!r}")
    journal_config = config if config is not None else get_config()
    providers = journal_config.get("providers", {})
    if not isinstance(providers, dict):
        return False
    active = providers.get("active", {})
    return isinstance(active, dict) and active.get("provider") == "local"


def log_token_usage(
    model: str,
    usage: Union[Dict[str, Any], Any],
    context: Optional[str] = None,
    segment: Optional[str] = None,
    type: Optional[str] = None,
    non_responsive_output: str | None = None,
    non_responsive_matched_signal: str | None = None,
) -> None:
    """Log token usage to journal with unified schema.

    Providers normalize usage into the unified schema (see USAGE_KEYS in
    the native generate boundary) before returning a result. This function passes
    through those known keys, computes total_tokens when missing, and
    handles a few legacy field aliases from CLI backends.

    Parameters
    ----------
    model : str
        Model name (e.g., "gpt-5", "gemini-3.5-flash")
    usage : dict
        Normalized usage dict with keys from USAGE_KEYS.
    context : str, optional
        Context string (e.g., "module.function:123" or "talent.system.default").
        If None, auto-detects from call stack.
    segment : str, optional
        Segment key (e.g., "143022_300") for attribution.
        If None, falls back to SOL_SEGMENT environment variable.
    type : str, optional
        Token entry type (e.g., "generate", "cogitate").
    non_responsive_output : str, optional
        Capped visible model output when a generate response declines the request.
    non_responsive_matched_signal : str, optional
        Safe classifier signal that identified the non-responsive output.
    """
    from solstone.think.providers.shared import USAGE_KEYS

    try:
        journal = get_journal()

        # Auto-detect calling context if not provided
        if context is None:
            frame = inspect.currentframe()
            caller_frame = frame.f_back if frame else None

            # Skip frames that contain "gemini" in function name
            while caller_frame and "gemini" in caller_frame.f_code.co_name.lower():
                caller_frame = caller_frame.f_back

            if caller_frame:
                module_name = caller_frame.f_globals.get("__name__", "unknown")
                func_name = caller_frame.f_code.co_name
                line_num = caller_frame.f_lineno

                # Clean up module name
                for prefix in ["think.", "observe.", "convey."]:
                    if module_name.startswith(prefix):
                        module_name = module_name[len(prefix) :]
                        break

                context = f"{module_name}.{func_name}:{line_num}"

        # Pass through known keys from the already-normalized usage dict.
        normalized_usage: Dict[str, int] = {}
        for key in USAGE_KEYS:
            val = usage.get(key)
            if val:
                normalized_usage[key] = val

        # Legacy alias: some CLI backends emit cached_input_tokens
        if not normalized_usage.get("cached_tokens") and usage.get(
            "cached_input_tokens"
        ):
            normalized_usage["cached_tokens"] = usage["cached_input_tokens"]

        # Compute total_tokens from parts when missing.
        if not normalized_usage.get("total_tokens"):
            inp = normalized_usage.get("input_tokens", 0)
            out = normalized_usage.get("output_tokens", 0)
            if inp or out:
                normalized_usage["total_tokens"] = inp + out

        # Build token log entry
        token_data = {
            "timestamp": time.time(),
            "model": model,
            "context": context,
            "usage": normalized_usage,
        }

        # Add segment: prefer parameter, fallback to env (set by think/insight, observe handlers)
        segment_key = segment or os.getenv("SOL_SEGMENT")
        if segment_key:
            token_data["segment"] = segment_key
        if type:
            token_data["type"] = type
        if non_responsive_output is not None:
            token_data["non_responsive_output"] = non_responsive_output
        if non_responsive_matched_signal is not None:
            token_data["non_responsive_matched_signal"] = non_responsive_matched_signal

        # Save to journal/tokens/<YYYYMMDD>.jsonl (one file per day)
        tokens_dir = Path(journal) / "tokens"
        tokens_dir.mkdir(exist_ok=True)

        filename = time.strftime("%Y%m%d.jsonl")
        filepath = tokens_dir / filename

        # Atomic append - safe for parallel writers
        with open(filepath, "a") as f:
            f.write(json.dumps(token_data) + "\n")

    except Exception:
        logger.warning("failed to log token usage", exc_info=True)


def derive_provider_lane(config: Mapping[str, Any], provider: object) -> str:
    provider_name = (
        provider if isinstance(provider, str) and provider else NO_BRAIN_PROVIDER
    )
    config_dict = dict(config)
    if provider_name == NO_BRAIN_PROVIDER:
        return "none"
    if provider_name == "local":
        endpoint = resolve_local_endpoint_from_config(config_dict)
        if endpoint.is_bundled:
            return "local"
        if confidential_provenance_block(config_dict) is not None:
            return "confidential"
        return "byo"
    return "byo"


def _raise_if_confidential_unverified(provider: object) -> None:
    config = get_config()
    if derive_provider_lane(config, provider) == "local":
        return
    block = config.get("services", {}).get("confidential")
    if not isinstance(block, dict):
        return
    _confidential_attestation_verifier()(block)


def get_model_provider(model: str) -> str:
    """Get the provider name from a model identifier.

    Parameters
    ----------
    model : str
        Model name (e.g., "gpt-5", "gemini-3.5-flash", "claude-sonnet-4-5")

    Returns
    -------
    str
        Provider name: "openai", "google", "anthropic", "local", or "unknown"
    """
    model_lower = model.lower()

    if model_lower == GEMMA4_26B_A4B_4BIT.lower():
        return "local"
    elif model_lower == QWEN_35_9B.lower():
        return "local"
    elif model_lower.startswith("local/"):
        return "local"
    elif model_lower.startswith("gpt"):
        return "openai"
    elif model_lower.startswith("gemini"):
        return "google"
    elif model_lower.startswith("claude"):
        return "anthropic"
    else:
        return "unknown"


def calc_token_cost(token_data: Dict[str, Any]) -> Optional[Dict[str, Any]]:
    """Calculate cost for a token usage record.

    Parameters
    ----------
    token_data : dict
        Token usage record from journal logs with structure:
        {
            "model": "gemini-3.5-flash",
            "usage": {
                "input_tokens": 1500,
                "output_tokens": 500,
                "cached_tokens": 800,
                "reasoning_tokens": 200,
                ...
            }
        }

    Returns
    -------
    dict or None
        Cost breakdown:
        {
            "total_cost": 0.00123,
            "input_cost": 0.00075,
            "output_cost": 0.00048,
            "currency": "USD"
        }
        Returns None if pricing unavailable or calculation fails.
    """
    try:
        from genai_prices import Usage, calc_price

        model = token_data.get("model")
        usage_data = token_data.get("usage", {})

        if not model or not usage_data:
            return None

        # Strip OpenAI reasoning effort suffixes for price lookup
        for suffix in OPENAI_EFFORT_SUFFIXES:
            if model.endswith(suffix):
                model = model[: -len(suffix)]
                break

        # Get provider ID before aliasing (alias may change the model family)
        provider_id = get_model_provider(model)
        if provider_id == "unknown":
            return None

        if provider_id == "local":
            return {
                "total_cost": 0.0,
                "input_cost": 0.0,
                "output_cost": 0.0,
                "currency": "USD",
            }

        # Family-fallback below handles unpriced inputs.

        # Map our token fields to genai_prices Usage format
        # Note: Gemini reports reasoning_tokens separately, but they're billed at
        # output token rates. genai-prices doesn't have a separate field for reasoning,
        # so we add them to output_tokens for correct pricing.
        input_tokens = usage_data.get("input_tokens", 0)
        output_tokens = usage_data.get("output_tokens", 0)
        cached_tokens = usage_data.get("cached_tokens", 0)
        reasoning_tokens = usage_data.get("reasoning_tokens", 0)

        # Add reasoning tokens to output for pricing (Gemini bills them as output)
        total_output_tokens = output_tokens + reasoning_tokens

        # Create Usage object
        usage = Usage(
            input_tokens=input_tokens,
            output_tokens=total_output_tokens,
            cache_read_tokens=cached_tokens if cached_tokens > 0 else None,
        )

        # Calculate price
        try:
            result = calc_price(
                usage=usage,
                model_ref=model,
                provider_id=provider_id,
            )
        except LookupError:
            resolved = _find_pricing_fallback(model, provider_id)
            if resolved is None:
                raise
            result = calc_price(
                usage=usage,
                model_ref=resolved,
                provider_id=provider_id,
            )
            if model not in _LOGGED_FALLBACKS:
                _LOGGED_FALLBACKS.add(model)
                logger.info("pricing: family-fallback %s -> %s", model, resolved)

        # Return simplified cost breakdown
        return {
            "total_cost": float(result.total_price),
            "input_cost": float(result.input_price),
            "output_cost": float(result.output_price),
            "currency": "USD",
        }

    except Exception:
        # Silently fail if pricing unavailable
        return None


def calc_agent_cost(
    model: Optional[str], usage: Optional[Dict[str, Any]]
) -> Optional[float]:
    """Calculate total cost for an agent run from model and usage data.

    Convenience wrapper around calc_token_cost for agent cost lookups.

    Returns total cost in USD, or None if data is missing or pricing unavailable.
    """
    if not model or not usage:
        return None
    # Token logs store resolved models; this boundary covers cortex start-event aliases.
    resolved_model = usage.get("model_version")
    if resolved_model:
        model = resolved_model
    try:
        cost_data = calc_token_cost({"model": model, "usage": usage})
        if cost_data:
            return cost_data["total_cost"]
    except Exception:
        return None
    return None


def _normalize_legacy_context(ctx: str) -> str:
    """Normalize legacy token-log context strings to the talent namespace."""
    if ctx.startswith(_LEGACY_CONTEXT_PREFIX):
        return _TALENT_CONTEXT_PREFIX + ctx[len(_LEGACY_CONTEXT_PREFIX) :]
    return ctx


def iter_token_log(day: str) -> Any:
    """Iterate over token log entries for a given day.

    Yields parsed JSON entries from the token log file, skipping empty lines
    and invalid JSON. This is a shared utility for code that processes token logs.

    Parameters
    ----------
    day : str
        Day in YYYYMMDD format.

    Yields
    ------
    dict
        Parsed token log entry with fields: timestamp, model, context, usage,
        and optionally segment.
    """
    journal = get_journal()
    log_path = Path(journal) / "tokens" / f"{day}.jsonl"

    if not log_path.exists():
        return

    with open(log_path, "r") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                entry = json.loads(line)
                ctx = entry.get("context")
                if isinstance(ctx, str):
                    entry["context"] = _normalize_legacy_context(ctx)
                yield entry
            except json.JSONDecodeError:
                continue


def get_usage_cost(
    day: str,
    segment: Optional[str] = None,
    context: Optional[str] = None,
) -> Dict[str, Any]:
    """Get aggregated token usage and cost for a day, optionally filtered.

    This is a shared utility for apps that want to display cost information
    for segments, agent runs, or other contexts.

    Parameters
    ----------
    day : str
        Day in YYYYMMDD format.
    segment : str, optional
        Filter to entries with this exact segment key.
    context : str, optional
        Filter to entries where context starts with this prefix.
        For example, "talent.system" matches "talent.system.default".

    Returns
    -------
    dict
        Aggregated usage data:
        {
            "requests": int,
            "tokens": int,
            "cost": float,  # USD
        }
        Returns zeros if no matching entries or day file doesn't exist.
    """
    result = {"requests": 0, "tokens": 0, "cost": 0.0}

    for entry in iter_token_log(day):
        # Apply filters
        if segment is not None and entry.get("segment") != segment:
            continue
        if context is not None:
            entry_context = entry.get("context", "")
            if not entry_context.startswith(context):
                continue

        # Skip unknown providers (can't calculate cost)
        model = entry.get("model", "unknown")
        if get_model_provider(model) == "unknown":
            continue

        # Accumulate
        usage = entry.get("usage", {})
        result["requests"] += 1
        result["tokens"] += usage.get("total_tokens", 0) or 0

        cost_data = calc_token_cost(entry)
        if cost_data:
            result["cost"] += cost_data["total_cost"]

    return result


# ---------------------------------------------------------------------------
# Unified generate/agenerate active-provider dispatch
# ---------------------------------------------------------------------------


def finish_reason_error(
    result: Dict[str, Any],
    *,
    json_output: bool,
) -> Exception | None:
    """Map the native wire's normalized finish reason to an existing exception."""
    finish_reason = result.get("finish_reason")
    if not finish_reason or finish_reason == "stop":
        return None

    text = result.get("text", "")
    partial_text = text if isinstance(text, str) else ""
    if json_output:
        return IncompleteJSONError(
            reason=finish_reason,
            partial_text=partial_text,
        )

    if str(finish_reason).strip().lower() in _LENGTH_FINISH_REASONS:
        return IncompleteTextError(
            reason=finish_reason,
            partial_text=partial_text,
        )
    result_model = result.get("model")
    return ProviderResponseInvalidError(
        reason=str(finish_reason),
        finish_reason=finish_reason if isinstance(finish_reason, str) else None,
        model=result_model if isinstance(result_model, str) and result_model else None,
    )


def generate(
    contents: Union[str, List[Any]],
    context: str,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: Optional[str] = None,
    json_output: bool = False,
    *,
    json_schema: dict | None = None,
    thinking_budget: Optional[int] = None,
    timeout_s: Optional[float] = None,
) -> str:
    """Generate text through the native generate boundary."""
    from solstone.think import generate_client

    return generate_client.generate(
        contents,
        context,
        temperature,
        max_output_tokens,
        system_instruction,
        json_output,
        json_schema=json_schema,
        thinking_budget=thinking_budget,
        timeout_s=timeout_s,
    )


def generate_with_result(
    contents: Union[str, List[Any]],
    context: str,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: Optional[str] = None,
    json_output: bool = False,
    *,
    json_schema: dict | None = None,
    thinking_budget: Optional[int] = None,
    timeout_s: Optional[float] = None,
    num_retries: int | None = None,
    inference_retry_index: int = 0,
    local_exclusive_admission: bool = False,
    enforce_responsiveness: bool = True,
) -> dict:
    """Generate a full result through the native generate boundary."""
    from solstone.think import generate_client

    return generate_client.generate_with_result(
        contents,
        context,
        temperature,
        max_output_tokens,
        system_instruction,
        json_output,
        json_schema=json_schema,
        thinking_budget=thinking_budget,
        timeout_s=timeout_s,
        num_retries=num_retries,
        inference_retry_index=inference_retry_index,
        local_exclusive_admission=local_exclusive_admission,
        enforce_responsiveness=enforce_responsiveness,
    )


async def agenerate_with_result(
    contents: Union[str, List[Any]],
    context: str,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: Optional[str] = None,
    json_output: bool = False,
    *,
    json_schema: dict | None = None,
    thinking_budget: Optional[int] = None,
    timeout_s: Optional[float] = None,
    num_retries: int | None = None,
    inference_retry_index: int = 0,
    local_exclusive_admission: bool = False,
    enforce_responsiveness: bool = True,
) -> dict:
    """Asynchronously generate a full result through native core."""
    from solstone.think import generate_client

    return await generate_client.agenerate_with_result(
        contents,
        context,
        temperature,
        max_output_tokens,
        system_instruction,
        json_output,
        json_schema=json_schema,
        thinking_budget=thinking_budget,
        timeout_s=timeout_s,
        num_retries=num_retries,
        inference_retry_index=inference_retry_index,
        local_exclusive_admission=local_exclusive_admission,
        enforce_responsiveness=enforce_responsiveness,
    )


async def agenerate(
    contents: Union[str, List[Any]],
    context: str,
    temperature: float = 0.3,
    max_output_tokens: int = 8192 * 2,
    system_instruction: Optional[str] = None,
    json_output: bool = False,
    *,
    json_schema: dict | None = None,
    thinking_budget: Optional[int] = None,
    timeout_s: Optional[float] = None,
) -> str:
    """Asynchronously generate text through the native generate boundary."""
    from solstone.think import generate_client

    return await generate_client.agenerate(
        contents,
        context,
        temperature,
        max_output_tokens,
        system_instruction,
        json_output,
        json_schema=json_schema,
        thinking_budget=thinking_budget,
        timeout_s=timeout_s,
    )


__all__ = [
    # Provider configuration
    "DEFAULT_MODEL_BY_PROVIDER",
    "NO_BRAIN_PROVIDER",
    "derive_provider_lane",
    "NoBrainConfiguredError",
    "AttestationFailedError",
    "AttestationNotVerifiedError",
    "AttestationStaleError",
    "PROMPT_PATHS",
    "get_context_registry",
    # Model constants (used by provider backends for defaults)
    "GEMINI_FLASH",
    "GPT_5_MINI",
    "CLAUDE_SONNET_4",
    "QWEN_35_9B",
    "GEMMA4_26B_A4B_4BIT",
    "LOCAL_MODEL",
    # Model capability helpers
    "model_supports",
    # Unified API
    "generate",
    "generate_with_result",
    "agenerate",
    "agenerate_with_result",
    "finish_reason_error",
    "IncompleteTextError",
    "ProviderResponseInvalidError",
    "default_model_for_provider",
    "resolve_provider",
    "resolve_effective_route",
    "is_local_provider_needed",
    "no_thinking_engine_chosen",
    "type_default_is_local",
    # Utilities
    "log_token_usage",
    "calc_token_cost",
    "calc_agent_cost",
    "get_usage_cost",
    "iter_token_log",
    "get_model_provider",
]
