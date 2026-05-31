# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import fcntl
import fnmatch
import functools
import inspect
import json
import logging
import os
import re
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Dict, List, NamedTuple, Optional, Union

import frontmatter
from jsonschema import Draft202012Validator

from solstone.think.callosum import callosum_send
from solstone.think.utils import get_config, get_journal, now_ms

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Tier constants
# ---------------------------------------------------------------------------

TIER_PRO = 1
TIER_FLASH = 2
TIER_LITE = 3

# ---------------------------------------------------------------------------
# Model constants
#
# IMPORTANT: When updating these models, verify pricing support:
#   1. Run: make test-only TEST=tests/test_models.py::test_all_default_models_have_pricing
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


GEMINI_PRO = "gemini-pro-latest"
GEMINI_FLASH = "gemini-flash-latest"
GEMINI_LITE = "gemini-flash-lite-latest"

GPT_5 = "gpt-5.5"
GPT_5_MINI = "gpt-5.4-mini"
GPT_5_NANO = "gpt-5.4-nano"

CLAUDE_OPUS_4 = "claude-opus-4-7"
CLAUDE_SONNET_4 = "claude-sonnet-4-6"
CLAUDE_HAIKU_4 = "claude-haiku-4-5"

LOCAL_MODEL = "local/qwen3.5-4b"

QWEN_35_9B = "qwen3.5:9b"
GEMMA4_26B_A4B_4BIT = "gemma-4-26b-a4b-it-mlx-4bit"
MLX_PRO = QWEN_35_9B
MLX_FLASH = QWEN_35_9B
MLX_LITE = QWEN_35_9B


# Per-model request parameter capability overrides.
# Anthropic reasoning-model temperature deprecation: Opus 4.7 rejects temperature.
# Canonical error string: 'temperature' is deprecated for this model.
# Missing models/params are treated as supported so providers stay permissive by default.
MODEL_CAPABILITIES: dict[str, dict[str, bool]] = {
    CLAUDE_OPUS_4: {"temperature": False},
}


def model_supports(model: str, param: str) -> bool:
    return MODEL_CAPABILITIES.get(model, {}).get(param) is not False


# ---------------------------------------------------------------------------
# System defaults: provider -> tier -> model
# ---------------------------------------------------------------------------

PROVIDER_DEFAULTS: Dict[str, Dict[int, str]] = {
    "google": {
        TIER_PRO: GEMINI_PRO,
        TIER_FLASH: GEMINI_FLASH,
        TIER_LITE: GEMINI_LITE,
    },
    "openai": {
        TIER_PRO: GPT_5,
        TIER_FLASH: GPT_5_MINI,
        TIER_LITE: GPT_5_NANO,
    },
    "anthropic": {
        TIER_PRO: CLAUDE_OPUS_4,
        TIER_FLASH: CLAUDE_SONNET_4,
        TIER_LITE: CLAUDE_HAIKU_4,
    },
    "local": {
        TIER_PRO: LOCAL_MODEL,
        TIER_FLASH: LOCAL_MODEL,
        TIER_LITE: LOCAL_MODEL,
    },
}

TYPE_DEFAULTS: Dict[str, Dict[str, Any]] = {
    "generate": {"provider": "google", "tier": TIER_FLASH, "backup": "anthropic"},
    "cogitate": {"provider": "google", "tier": TIER_FLASH, "backup": "anthropic"},
}


# ---------------------------------------------------------------------------
# Exceptions
# ---------------------------------------------------------------------------


class IncompleteJSONError(ValueError):
    """Raised when JSON response is truncated due to token limits or other reasons.

    Attributes:
        reason: The finish/stop reason from the API (e.g., "MAX_TOKENS", "length").
        partial_text: The truncated response text, useful for debugging.
    """

    def __init__(self, reason: str, partial_text: str):
        self.reason = reason
        self.partial_text = partial_text
        super().__init__(f"JSON response incomplete (reason: {reason})")


# ---------------------------------------------------------------------------
# Prompt context discovery
#
# Context metadata (tier, label, group) is defined in prompt .md files via
# YAML frontmatter. This eliminates duplication between code and config.
#
# NAMING CONVENTION:
#   {module}.{feature}[.{operation}]
#
# Examples:
#   - observe.describe.frame    -> observe module, describe feature, frame operation
#   - observe.enrich            -> observe module, enrich feature (no sub-operation)
#   - talent.system.meetings      -> talent module, system source, meetings config
#   - talent.entities.observer    -> talent module, entities app, observer config
#   - app.chat.title            -> apps module, chat app, title operation
#
# DISCOVERY SOURCES:
#   1. Prompt files listed in PROMPT_PATHS (with context in frontmatter)
#   2. Categories from observe/categories/*.md (tier/label/group in frontmatter)
#   3. Talent configs from talent/*.md and apps/*/talent/*.md
#
# When adding new contexts:
#   1. Create a .md prompt file with YAML frontmatter containing:
#      context, tier, label, group
#   2. Add the path to PROMPT_PATHS
#   3. If not listed, context falls back to the type's default tier
# ---------------------------------------------------------------------------

# Flat list of prompt files that define context metadata in frontmatter.
# Each must have: context, tier, label, group in YAML frontmatter.
PROMPT_PATHS: List[str] = [
    "observe/describe.md",
    "observe/enrich.md",
    "observe/extract.md",
    "observe/transcribe/gemini.md",
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
    - context: The context string (e.g., "observe.enrich")
    - tier: Tier number (1=pro, 2=flash, 3=lite)
    - label: Human-readable name
    - group: Settings UI category

    Returns
    -------
    Dict[str, Dict[str, Any]]
        Mapping of context patterns to {tier, label, group} dicts.
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
                "tier": meta.get("tier", TIER_FLASH),
                "label": meta.get("label", context),
                "group": meta.get("group", "Other"),
            }
        except Exception as e:
            logging.getLogger(__name__).warning(f"Failed to load {path}: {e}")

    return contexts


def _discover_talent_contexts() -> Dict[str, Dict[str, Any]]:
    """Discover talent context defaults from talent/*.md config files.

    Uses get_talent_configs() from solstone.think.talent to load all talent configurations
    and converts them to context patterns with tier/label/group metadata.

    Returns
    -------
    Dict[str, Dict[str, Any]]
        Mapping of context patterns to {tier, label, group, type} dicts.
        Context patterns are: talent.system.{name} or talent.{app}.{name}
    """
    from solstone.think.talent import get_talent_configs, key_to_context

    contexts = {}

    # Load all talent configs (including disabled for completeness)
    all_configs = get_talent_configs(include_disabled=True)

    for key, config in all_configs.items():
        context = key_to_context(key)
        contexts[context] = {
            "tier": config.get("tier", TIER_FLASH),
            "label": config.get("label", config.get("title", key)),
            "group": config.get("group", "Think"),
            "type": config.get("type"),
        }

    return contexts


def _build_context_registry() -> Dict[str, Dict[str, Any]]:
    """Build complete context registry from discovered configs.

    Merges:
    1. Prompt contexts from _discover_prompt_contexts()
    2. Category contexts from observe/describe.py CATEGORIES
    3. Talent contexts from _discover_talent_contexts()

    Returns
    -------
    Dict[str, Dict[str, Any]]
        Complete context registry mapping patterns to {tier, label, group}.
    """
    # Start with prompt contexts (from PROMPT_PATHS)
    registry = _discover_prompt_contexts()

    # Merge category contexts (lazy import to avoid circular dependency)
    try:
        from solstone.observe.describe import CATEGORIES

        for category, metadata in CATEGORIES.items():
            context = metadata.get("context", f"observe.describe.{category}")
            registry[context] = {
                "tier": metadata.get("tier", TIER_FLASH),
                "label": metadata.get("label", category.replace("_", " ").title()),
                "group": metadata.get("group", "Screen Analysis"),
            }
    except ImportError:
        pass  # observe module not available

    # Merge talent contexts (agents + generators)
    talent_contexts = _discover_talent_contexts()
    registry.update(talent_contexts)

    return registry


def get_context_registry() -> Dict[str, Dict[str, Any]]:
    """Get the complete context registry, building it lazily on first use.

    Returns
    -------
    Dict[str, Dict[str, Any]]
        Complete context registry mapping patterns to {tier, label, group}.
    """
    global _context_registry
    if _context_registry is None:
        _context_registry = _build_context_registry()
    return _context_registry


def _resolve_tier(context: str, agent_type: str) -> int:
    """Resolve context to tier number.

    Checks journal config contexts first, then dynamic context registry with glob matching.

    Parameters
    ----------
    context
        Context string (e.g., "talent.system.default", "observe.describe.frame").
    agent_type
        Agent type ("generate" or "cogitate").

    Returns
    -------
    int
        Tier number (1=pro, 2=flash, 3=lite).
    """
    from solstone.think.utils import get_config

    default_tier = TYPE_DEFAULTS[agent_type]["tier"]

    journal_config = get_config()
    providers_config = journal_config.get("providers", {})
    contexts = providers_config.get("contexts", {})

    # Get dynamic context registry (discovered prompts, categories, talent configs)
    registry = get_context_registry()

    # Check journal config contexts first (exact match)
    if context in contexts:
        return contexts[context].get("tier", default_tier)

    # Check context registry (exact match)
    if context in registry:
        return registry[context]["tier"]

    # Check glob patterns in both
    for pattern, ctx_config in contexts.items():
        if fnmatch.fnmatch(context, pattern):
            return ctx_config.get("tier", default_tier)

    for pattern, ctx_default in registry.items():
        if fnmatch.fnmatch(context, pattern):
            return ctx_default["tier"]

    return default_tier


def _resolve_model(provider: str, tier: int, config_models: Dict[str, Any]) -> str:
    """Resolve tier to model string for a given provider.

    Checks config overrides first, then falls back to system defaults.
    If requested tier is unavailable, falls back to more capable tiers
    (3→2→1, i.e., lite→flash→pro).

    Parameters
    ----------
    provider
        Provider name ("google", "openai", "anthropic").
    tier
        Tier number (1=pro, 2=flash, 3=lite).
    config_models
        The "models" section from providers config, mapping provider to tier overrides.

    Returns
    -------
    str
        Model identifier string.
    """
    # Check config overrides first
    provider_overrides = config_models.get(provider, {})

    # Try requested tier, then fall back to more capable tiers (lower numbers)
    for t in [tier, tier - 1, tier - 2] if tier > 1 else [tier]:
        if t < 1:
            continue

        # Check config override (tier as string key in JSON)
        tier_key = str(t)
        if tier_key in provider_overrides:
            return provider_overrides[tier_key]

        # Check system defaults
        provider_defaults = PROVIDER_DEFAULTS.get(provider, {})
        if t in provider_defaults:
            return provider_defaults[t]

    # Ultimate fallback: system default for provider at TIER_FLASH
    provider_defaults = PROVIDER_DEFAULTS.get(provider, PROVIDER_DEFAULTS["google"])
    return provider_defaults.get(TIER_FLASH, GEMINI_FLASH)


def resolve_model_for_provider(
    context: str, provider: str, agent_type: str = "generate"
) -> str:
    """Resolve model for a specific provider based on context tier.

    Use this when provider is overridden from the default - resolves the
    appropriate model for the given provider at the context's tier.

    Parameters
    ----------
    context
        Context string (e.g., "talent.system.default").
    provider
        Provider name ("google", "openai", "anthropic").
    agent_type
        Agent type ("generate" or "cogitate").

    Returns
    -------
    str
        Model identifier string for the provider at the context's tier.
    """
    from solstone.think.utils import get_config

    tier = _resolve_tier(context, agent_type)
    journal_config = get_config()
    providers_config = journal_config.get("providers", {})
    config_models = providers_config.get("models", {})

    return _resolve_model(provider, tier, config_models)


def resolve_provider(context: str, agent_type: str) -> tuple[str, str]:
    """Resolve context to provider and model based on configuration.

    Matches context against configured contexts using exact match first,
    then glob patterns (via fnmatch), falling back to type-specific defaults.

    Supports both explicit model strings and tier-based routing:
    - {"provider": "google", "model": "gemini-flash-latest"} - explicit model
    - {"provider": "google", "tier": 2} - tier-based (2=flash)
    - {"tier": 1} - tier only, inherits provider from type default

    The "models" section in providers config allows overriding which model
    is used for each tier per provider.

    Parameters
    ----------
    context
        Context string (e.g., "observe.describe.frame", "talent.system.meetings").
    agent_type
        Agent type ("generate" or "cogitate").

    Returns
    -------
    tuple[str, str]
        (provider_name, model) tuple. Provider is one of "google", "openai",
        "anthropic". Model is the full model identifier string.
    """
    config = get_config()
    providers = config.get("providers", {})
    config_models = providers.get("models", {})

    # Get type-specific defaults from config, falling back to system constants
    type_defaults = TYPE_DEFAULTS[agent_type]
    type_config = providers.get(agent_type, {})
    default_provider = type_config.get("provider", type_defaults["provider"])
    default_tier = type_config.get("tier", type_defaults["tier"])

    # Handle explicit "model" key in type config (overrides tier-based resolution)
    if "model" in type_config and "tier" not in type_config:
        default_model = type_config["model"]
    else:
        default_model = _resolve_model(default_provider, default_tier, config_models)

    contexts = providers.get("contexts", {})

    # Find matching context config
    match_config: Optional[Dict[str, Any]] = None

    if context and contexts:
        # Check for exact match first
        if context in contexts:
            match_config = contexts[context]
        else:
            # Check glob patterns - most specific (longest non-wildcard prefix) wins
            matches = []
            for pattern, ctx_config in contexts.items():
                if fnmatch.fnmatch(context, pattern):
                    specificity = len(pattern.split("*")[0])
                    matches.append((specificity, pattern, ctx_config))

            if matches:
                matches.sort(key=lambda x: x[0], reverse=True)
                _, _, match_config = matches[0]

    # No context match - check dynamic context registry for this context
    if match_config is None:
        # Get dynamic context registry (discovered prompts, categories, talent configs)
        registry = get_context_registry()

        # Check for matching context default (exact match first, then glob)
        context_tier = None
        if context:
            if context in registry:
                context_tier = registry[context]["tier"]
            else:
                # Check glob patterns
                matches = []
                for pattern, ctx_default in registry.items():
                    if fnmatch.fnmatch(context, pattern):
                        specificity = len(pattern.split("*")[0])
                        matches.append((specificity, ctx_default["tier"]))
                if matches:
                    matches.sort(key=lambda x: x[0], reverse=True)
                    context_tier = matches[0][1]

        if context_tier is not None:
            model = _resolve_model(default_provider, context_tier, config_models)
            return (default_provider, model)

        return (default_provider, default_model)

    # Resolve provider (from match or default)
    provider = match_config.get("provider", default_provider)

    # Resolve model: explicit model takes precedence over tier
    if "model" in match_config:
        model = match_config["model"]
    elif "tier" in match_config:
        tier = match_config["tier"]
        # Validate tier
        if not isinstance(tier, int) or tier < 1 or tier > 3:
            logging.getLogger(__name__).warning(
                "Invalid tier %r in context %r, using default", tier, context
            )
            tier = default_tier
        model = _resolve_model(provider, tier, config_models)
    else:
        # No model or tier specified - use default tier
        model = _resolve_model(provider, default_tier, config_models)

    return (provider, model)


def is_local_provider_needed(config: dict[str, Any] | None = None) -> bool:
    """Return True when journal provider config selects local anywhere."""
    journal_config = config if config is not None else get_config()
    providers = journal_config.get("providers", {})
    if not isinstance(providers, dict):
        return False

    for agent_type in ("generate", "cogitate"):
        type_config = providers.get(agent_type, {})
        if isinstance(type_config, dict) and type_config.get("provider") == "local":
            return True

    contexts = providers.get("contexts", {})
    if not isinstance(contexts, dict):
        return False
    return any(
        isinstance(context_config, dict) and context_config.get("provider") == "local"
        for context_config in contexts.values()
    )


def log_token_usage(
    model: str,
    usage: Union[Dict[str, Any], Any],
    context: Optional[str] = None,
    segment: Optional[str] = None,
    type: Optional[str] = None,
) -> None:
    """Log token usage to journal with unified schema.

    Providers normalize usage into the unified schema (see USAGE_KEYS in
    shared.py) before returning GenerateResult.  This function passes
    through those known keys, computes total_tokens when missing, and
    handles a few legacy field aliases from CLI backends.

    Parameters
    ----------
    model : str
        Model name (e.g., "gpt-5", "gemini-flash-latest")
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

        # Save to journal/tokens/<YYYYMMDD>.jsonl (one file per day)
        tokens_dir = Path(journal) / "tokens"
        tokens_dir.mkdir(exist_ok=True)

        filename = time.strftime("%Y%m%d.jsonl")
        filepath = tokens_dir / filename

        # Atomic append - safe for parallel writers
        with open(filepath, "a") as f:
            f.write(json.dumps(token_data) + "\n")

    except Exception:
        # Silently fail - logging shouldn't break the main flow
        pass


def get_model_provider(model: str) -> str:
    """Get the provider name from a model identifier.

    Parameters
    ----------
    model : str
        Model name (e.g., "gpt-5", "gemini-flash-latest", "claude-sonnet-4-5")

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
            "model": "gemini-flash-latest",
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
# Unified generate/agenerate with provider routing
# ---------------------------------------------------------------------------


def _validate_json_response(result: Dict[str, Any], json_output: bool) -> None:
    """Validate response for JSON output mode.

    Raises IncompleteJSONError if finish_reason indicates truncation.
    """
    if not json_output:
        return

    finish_reason = result.get("finish_reason")
    if finish_reason and finish_reason != "stop":
        raise IncompleteJSONError(
            reason=finish_reason,
            partial_text=result.get("text", ""),
        )


def _validate_schema(text: str, schema: dict) -> dict:
    """Validate JSON text against a JSON Schema and log any violations."""

    def truncate_repr(value: Any) -> str:
        value_repr = repr(value)
        if len(value_repr) <= 80:
            return value_repr
        return value_repr[:77] + "..."

    def build_pointer(path: Any) -> str:
        segments = list(path)
        if not segments:
            return ""
        escaped_segments = []
        for segment in segments:
            escaped = str(segment).replace("~", "~0").replace("/", "~1")
            escaped_segments.append(escaped)
        return "/" + "/".join(escaped_segments)

    try:
        parsed = json.loads(text)
    except ValueError as exc:
        error = {
            "path": "",
            "constraint": "json_parse",
            "message": str(exc),
        }
        logger.warning(
            "schema_validation: %s: %s: %s (value=%s)",
            "",
            "json_parse",
            str(exc),
            truncate_repr(text),
        )
        return {"valid": False, "errors": [error]}

    errors = []
    try:
        validator = Draft202012Validator(schema)
        validation_errors = list(validator.iter_errors(parsed))
    except Exception as exc:
        error = {
            "path": "",
            "constraint": "schema_validation",
            "message": str(exc),
        }
        logger.warning(
            "schema_validation: %s: %s: %s (value=%s)",
            "",
            "schema_validation",
            str(exc),
            truncate_repr(parsed),
        )
        return {"valid": False, "errors": [error]}

    for error in validation_errors:
        path = build_pointer(error.absolute_path)
        constraint = str(error.validator)
        message = error.message
        errors.append(
            {
                "path": path,
                "constraint": constraint,
                "message": message,
            }
        )
        logger.warning(
            "schema_validation: %s: %s: %s (value=%s)",
            path,
            constraint,
            message,
            truncate_repr(error.instance),
        )

    return {"valid": len(errors) == 0, "errors": errors}


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
    **kwargs: Any,
) -> str:
    """Generate text using the configured provider for the given context.

    Routes the request to the appropriate backend (Google, OpenAI, or Anthropic)
    based on the providers configuration in journal.json.

    Parameters
    ----------
    contents : str or List
        The content to send to the model.
    context : str
        Context string for routing and token logging (e.g., "talent.system.meetings").
        This is required and determines which provider/model to use.
    temperature : float
        Temperature for generation (default: 0.3).
    max_output_tokens : int
        Maximum tokens for the model's response output.
    system_instruction : str, optional
        System instruction for the model.
    json_output : bool
        Whether to request JSON response format.
    json_schema : dict, optional
        JSON Schema to request structured output from the provider. When supplied,
        this forces json_output=True and runs advisory local validation on the
        returned text after truncation checks.
    thinking_budget : int, optional
        Token budget for model thinking (ignored by providers that don't support it).
    timeout_s : float, optional
        Request timeout in seconds.
    **kwargs
        Additional provider-specific options passed through to the backend.

    Returns
    -------
    str
        Response text from the model.

    Raises
    ------
    ValueError
        If the resolved provider is not supported.
    IncompleteJSONError
        If json_output=True and response was truncated.
    """
    from solstone.think.providers import get_provider_module

    if json_schema is not None:
        json_output = True

    # Allow model override via kwargs (used by callers with explicit model selection)
    model_override = kwargs.pop("model", None)

    provider, model = resolve_provider(context, "generate")
    if model_override:
        model = model_override

    # Get provider module via registry (raises ValueError for unknown providers)
    provider_mod = get_provider_module(provider)

    # Call provider's run_generate (returns GenerateResult)
    result = provider_mod.run_generate(
        contents=contents,
        model=model,
        provider=provider,
        temperature=temperature,
        max_output_tokens=max_output_tokens,
        system_instruction=system_instruction,
        json_output=json_output,
        json_schema=json_schema,
        thinking_budget=thinking_budget,
        timeout_s=timeout_s,
        **kwargs,
    )

    # Log token usage centrally (before validation so truncated responses
    # still get their usage recorded)
    if result.get("usage"):
        log_token_usage(
            model=result.get("model") or model,
            usage=result["usage"],
            context=context,
            type="generate",
        )

    # Validate JSON output if requested
    _validate_json_response(result, json_output)

    if json_schema is not None:
        _validate_schema(result["text"], json_schema)

    return result["text"]


# ---------------------------------------------------------------------------
# Provider Health & Fallback Helpers
# ---------------------------------------------------------------------------


def get_backup_provider(agent_type: str) -> Optional[str]:
    """Get the backup provider for the given agent type.

    Reads from the type-specific section in journal config, falling back
    to TYPE_DEFAULTS.

    Returns None if backup would be the same as the primary provider.
    """
    type_defaults = TYPE_DEFAULTS[agent_type]
    config = get_config()
    providers_config = config.get("providers", {})
    type_config = providers_config.get(agent_type, {})
    primary_provider = type_config.get("provider", type_defaults["provider"])
    backup = type_config.get("backup", type_defaults["backup"])
    if primary_provider == "local":
        return None
    if backup == primary_provider:
        return None
    return backup


def load_health_status() -> Optional[dict]:
    """Load health status from journal/health/talents.json.

    Returns parsed dict or None if file is missing/unreadable.
    """
    try:
        health_path = Path(get_journal()) / "health" / "talents.json"
        with open(health_path) as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError, OSError):
        return None


def is_provider_healthy(provider: str, health_data: Optional[dict]) -> bool:
    """Check if a provider is healthy based on health data.

    Returns True (assume healthy) when:
    - health_data is None (no data available)
    - No results exist for the provider
    - Any result for the provider has ok=True

    Returns False only when all results for the provider have ok=False.
    """
    if health_data is None:
        return True
    results = health_data.get("results", [])
    provider_results = [r for r in results if r.get("provider") == provider]
    if not provider_results:
        return True
    return any(r.get("ok") for r in provider_results)


def is_provider_model_interface_healthy(
    provider: str,
    model: str,
    interface: str,
    health_data: Optional[dict],
) -> bool:
    """Check health for a specific provider/model/interface row."""
    if health_data is None:
        return True
    for row in health_data.get("results", []):
        if (
            row.get("provider") == provider
            and row.get("model") == model
            and row.get("interface") == interface
            and row.get("ok") is False
        ):
            return False
    return True


def _summarize_health_results(results: list[dict[str, Any]]) -> dict[str, int]:
    return {
        "total": len(results),
        "passed": sum(1 for row in results if row.get("status") == "ok"),
        "skipped": sum(1 for row in results if row.get("status") == "skip"),
        "failed": sum(1 for row in results if row.get("ok") is False),
    }


def record_provider_failure(
    provider: str,
    tier: str,
    model: str,
    interface: str,
    reset_at_ms: int,
) -> None:
    """Record a provider/model/interface quota failure in health status."""
    health_dir = Path(get_journal()) / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    health_path = health_dir / "talents.json"
    lock_path = health_dir / "talents.json.lock"
    tmp_path = health_dir / f".talents.json.{os.getpid()}.{now_ms()}.tmp"
    recorded_at = datetime.now(timezone.utc).isoformat()
    message = f"Quota exhausted; retry after {reset_at_ms}"

    with open(lock_path, "w", encoding="utf-8") as lock_file:
        fcntl.flock(lock_file, fcntl.LOCK_EX)
        try:
            try:
                with open(health_path, encoding="utf-8") as health_file:
                    payload = json.load(health_file)
            except (FileNotFoundError, json.JSONDecodeError, OSError):
                payload = {}

            results = payload.get("results", [])
            if not isinstance(results, list):
                results = []
            failure_row = {
                "provider": provider,
                "tier": tier,
                "model": model,
                "interface": interface,
                "ok": False,
                "status": "quota_exhausted",
                "message": message,
                "elapsed_s": 0.0,
                "reset_at_ms": reset_at_ms,
                "recorded_at": recorded_at,
            }

            for row in results:
                if (
                    row.get("provider") == provider
                    and row.get("model") == model
                    and row.get("interface") == interface
                ):
                    row.update(failure_row)
                    break
            else:
                results.append(failure_row)

            payload["results"] = results
            payload["summary"] = _summarize_health_results(results)
            payload.setdefault("checked_at", recorded_at)

            with open(tmp_path, "w", encoding="utf-8") as tmp_file:
                json.dump(payload, tmp_file, indent=2)
                tmp_file.write("\n")
                tmp_file.flush()
                os.fsync(tmp_file.fileno())
            os.replace(tmp_path, health_path)
        finally:
            try:
                tmp_path.unlink()
            except FileNotFoundError:
                pass


def should_recheck_health(health_data: Optional[dict]) -> bool:
    """Check if health data should be rechecked.

    Returns False when health_data is None or on parse errors.
    """
    if health_data is None:
        return False
    failed_rows = [
        row for row in health_data.get("results", []) if row.get("ok") is False
    ]
    reset_values = [
        int(row["reset_at_ms"])
        for row in failed_rows
        if isinstance(row.get("reset_at_ms"), (int, float))
    ]
    missing_reset = len(reset_values) < len(failed_rows)
    if reset_values and not missing_reset:
        return now_ms() > min(reset_values)

    checked_at = health_data.get("checked_at")
    if not checked_at:
        return False
    try:
        checked_time = datetime.fromisoformat(checked_at)
        if checked_time.tzinfo is None:
            checked_time = checked_time.replace(tzinfo=timezone.utc)
        age = datetime.now(timezone.utc) - checked_time
        return age.total_seconds() > 3600
    except (ValueError, TypeError):
        return False


def request_health_recheck() -> None:
    """Request a health re-check through the supervisor."""
    ok = callosum_send(
        "supervisor",
        "request",
        cmd=["journal", "providers", "check", "--targeted"],
    )
    if not ok:
        logger.warning("request_health_recheck: callosum_send returned false")


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
    **kwargs: Any,
) -> dict:
    """Generate text and return full result with usage data.

    Same as generate() but returns the full GenerateResult dict instead of
    just the text. Used by cortex-managed generators that need usage data
    for event emission.

    Parameters
    ----------
    contents : str or List
        The content to send to the model.
    context : str
        Context string for routing and token logging.
    temperature : float
        Temperature for generation (default: 0.3).
    max_output_tokens : int
        Maximum tokens for the model's response output.
    system_instruction : str, optional
        System instruction for the model.
    json_output : bool
        Whether to request JSON response format.
    json_schema : dict, optional
        JSON Schema to request structured output from the provider. When supplied,
        this forces json_output=True and runs advisory local validation on the
        returned text after truncation checks.
    thinking_budget : int, optional
        Token budget for model thinking (ignored by providers that don't support it).
    timeout_s : float, optional
        Request timeout in seconds.
    **kwargs
        Additional provider-specific options passed through to the backend.

    Returns
    -------
    dict
        GenerateResult with: text, usage, finish_reason, thinking, and
        schema_validation when json_schema is supplied. Validation is advisory
        and runs after truncation checks succeed.
    """
    from solstone.think.providers import get_provider_module

    if json_schema is not None:
        json_output = True

    model_override = kwargs.pop("model", None)
    provider_override = kwargs.pop("provider", None)

    provider, model = resolve_provider(context, "generate")
    if provider_override:
        provider = provider_override
        if not model_override:
            model = resolve_model_for_provider(context, provider, "generate")
    if model_override:
        model = model_override

    provider_mod = get_provider_module(provider)

    result = provider_mod.run_generate(
        contents=contents,
        model=model,
        provider=provider,
        temperature=temperature,
        max_output_tokens=max_output_tokens,
        system_instruction=system_instruction,
        json_output=json_output,
        json_schema=json_schema,
        thinking_budget=thinking_budget,
        timeout_s=timeout_s,
        **kwargs,
    )

    # Log token usage centrally (before validation so truncated responses
    # still get their usage recorded)
    if result.get("usage"):
        log_token_usage(
            model=result.get("model") or model,
            usage=result["usage"],
            context=context,
            type="generate",
        )

    # Validate JSON output if requested
    _validate_json_response(result, json_output)

    if json_schema is not None:
        result["schema_validation"] = _validate_schema(result["text"], json_schema)

    return result


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
    **kwargs: Any,
) -> str:
    """Async generate text using the configured provider for the given context.

    Routes the request to the appropriate backend (Google, OpenAI, or Anthropic)
    based on the providers configuration in journal.json.

    Parameters
    ----------
    contents : str or List
        The content to send to the model.
    context : str
        Context string for routing and token logging (e.g., "talent.system.meetings").
        This is required and determines which provider/model to use.
    temperature : float
        Temperature for generation (default: 0.3).
    max_output_tokens : int
        Maximum tokens for the model's response output.
    system_instruction : str, optional
        System instruction for the model.
    json_output : bool
        Whether to request JSON response format.
    json_schema : dict, optional
        JSON Schema to request structured output from the provider. When supplied,
        this forces json_output=True and runs advisory local validation on the
        returned text after truncation checks.
    thinking_budget : int, optional
        Token budget for model thinking (ignored by providers that don't support it).
    timeout_s : float, optional
        Request timeout in seconds.
    **kwargs
        Additional provider-specific options passed through to the backend.

    Returns
    -------
    str
        Response text from the model.

    Raises
    ------
    ValueError
        If the resolved provider is not supported.
    IncompleteJSONError
        If json_output=True and response was truncated.
    """
    from solstone.think.providers import get_provider_module

    if json_schema is not None:
        json_output = True

    # Allow model override via kwargs (used by Batch for explicit model selection)
    model_override = kwargs.pop("model", None)

    provider, model = resolve_provider(context, "generate")
    if model_override:
        model = model_override

    # Get provider module via registry (raises ValueError for unknown providers)
    provider_mod = get_provider_module(provider)

    # Call provider's run_agenerate (returns GenerateResult)
    result = await provider_mod.run_agenerate(
        contents=contents,
        model=model,
        provider=provider,
        temperature=temperature,
        max_output_tokens=max_output_tokens,
        system_instruction=system_instruction,
        json_output=json_output,
        json_schema=json_schema,
        thinking_budget=thinking_budget,
        timeout_s=timeout_s,
        **kwargs,
    )

    # Log token usage centrally (before validation so truncated responses
    # still get their usage recorded)
    if result.get("usage"):
        log_token_usage(
            model=result.get("model") or model,
            usage=result["usage"],
            context=context,
            type="generate",
        )

    # Validate JSON output if requested
    _validate_json_response(result, json_output)

    if json_schema is not None:
        _validate_schema(result["text"], json_schema)

    return result["text"]


__all__ = [
    # Provider configuration
    "TYPE_DEFAULTS",
    "PROMPT_PATHS",
    "get_context_registry",
    # Model constants (used by provider backends for defaults)
    "GEMINI_FLASH",
    "GPT_5",
    "CLAUDE_SONNET_4",
    "QWEN_35_9B",
    "GEMMA4_26B_A4B_4BIT",
    "LOCAL_MODEL",
    "MLX_FLASH",
    # Model capability helpers
    "model_supports",
    # Unified API
    "generate",
    "generate_with_result",
    "agenerate",
    "resolve_provider",
    "is_local_provider_needed",
    # Utilities
    "log_token_usage",
    "calc_token_cost",
    "calc_agent_cost",
    "get_usage_cost",
    "iter_token_log",
    "get_model_provider",
]
