# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Talent and generator orchestration utilities.

This module provides functionality for configuring and orchestrating talents
and generators from talent/*.md and apps/*/talent/*.md.

Key functions:
- get_talent_configs(): Discover all talent configs with filtering
- get_talent(): Load complete talent configuration by name
- Hook loading: load_pre_hook(), load_post_hook()

For simple prompt loading without orchestration (observe/, think/*.md prompts),
use solstone.think.prompts.load_prompt() directly.
"""

from __future__ import annotations

import copy
import importlib.util
import json
import logging
import os
import re
from pathlib import Path
from typing import Any, Callable

import frontmatter
from jsonschema import Draft202012Validator, SchemaError

from solstone.think.facets import get_facets

# Import core prompt utilities from solstone.think.prompts
from solstone.think.prompts import _load_prompt_metadata, load_prompt

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

TALENT_DIR = Path(__file__).parent.parent / "talent"
APPS_DIR = Path(__file__).parent.parent / "apps"
RUNTIME_FACETS_SENTINEL = "__RUNTIME_FACETS__"
SLUG_RE = re.compile(r"^[a-z][a-z0-9_-]*$")
LOG = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Talent Config Discovery
# ---------------------------------------------------------------------------


def _validate_cwd(raw_cwd: Any, talent_type: Any, key: str) -> str | None:
    """Validate and normalize the optional talent cwd setting."""
    if talent_type == "cogitate":
        if raw_cwd is None:
            return "journal"
        if raw_cwd in {"journal", "repo"}:
            return raw_cwd
        raise ValueError(
            f"Prompt '{key}' has invalid 'cwd' value '{raw_cwd}' "
            "(must be 'journal' or 'repo')"
        )

    if talent_type == "generate":
        if raw_cwd is not None:
            raise ValueError(
                f"Prompt '{key}' sets 'cwd' but cwd is only valid for type: cogitate"
            )
        return None

    if raw_cwd is None:
        return None

    raise ValueError(
        f"Prompt '{key}' has invalid 'cwd' value '{raw_cwd}' "
        "(must be 'journal' or 'repo')"
    )


def key_to_context(key: str) -> str:
    """Convert talent config key to context pattern.

    Parameters
    ----------
    key:
        Talent config key in format "name" (system) or "app:name" (app).

    Returns
    -------
    str
        Context pattern: "talent.system.{name}" or "talent.{app}.{name}".

    Examples
    --------
    >>> key_to_context("meetings")
    'talent.system.meetings'
    >>> key_to_context("entities:observer")
    'talent.entities.observer'
    """
    if ":" in key:
        app, name = key.split(":", 1)
        return f"talent.{app}.{name}"
    return f"talent.system.{key}"


def get_output_name(key: str) -> str:
    """Convert talent/generator key to a filesystem-safe filename stem.

    Parameters
    ----------
    key:
        Generator key in format "name" (system) or "app:name" (app).

    Returns
    -------
    str
        Filesystem-safe stem: "name" or "_app_name".

    Examples
    --------
    >>> get_output_name("activity")
    'activity'
    >>> get_output_name("chat:sentiment")
    '_chat_sentiment'
    """
    if ":" in key:
        app, name = key.split(":", 1)
        return f"_{app}_{name}"
    return key


def get_output_path(
    day_dir: "os.PathLike[str]",
    key: str,
    segment: str | None = None,
    output_format: str | None = None,
    facet: str | None = None,
    stream: str | None = None,
) -> Path:
    """Return output path for generator/talent output.

    Shared utility for determining where to write generator results.
    Used by solstone.think.talents and solstone.think.cortex.

    Parameters
    ----------
    day_dir:
        Day directory path (YYYYMMDD).
    key:
        Generator key or talent name (e.g., "activity", "chat:sentiment",
        "entities:observer").
    segment:
        Optional segment key (HHMMSS_LEN) for segment-level output.
    output_format:
        Output format - "json" for JSON, anything else for markdown.
    facet:
        Optional facet name for multi-facet talents. When provided, output is
        written under a talents/{facet}/ subdirectory.
    stream:
        Optional stream name for segment-level output. When provided with
        segment, constructs path as YYYYMMDD/{stream}/{segment}/talents/...

    Returns
    -------
    Path
        Output file path:
        - Segment + no facet: YYYYMMDD/{stream}/{segment}/talents/{name}.{ext}
        - Segment + facet: YYYYMMDD/{stream}/{segment}/talents/{facet}/{name}.{ext}
        - Daily + no facet: YYYYMMDD/talents/{name}.{ext}
        - Daily + facet: YYYYMMDD/talents/{facet}/{name}.{ext}
        Where name is derived from key and ext is "json" or "md".
    """
    day = Path(day_dir)
    name = get_output_name(key)
    ext = "json" if output_format == "json" else "md"
    filename = f"{name}.{ext}"

    if segment:
        if stream:
            seg_dir = day / stream / segment
        else:
            seg_dir = day / segment
        if facet:
            return seg_dir / "talents" / facet / filename
        return seg_dir / "talents" / filename
    if facet:
        return day / "talents" / facet / filename
    return day / "talents" / filename


def get_talent_configs(
    *,
    type: str | None = None,
    schedule: str | None = None,
    include_disabled: bool = False,
) -> dict[str, dict[str, Any]]:
    """Load talent configs from system and app directories.

    Unified function for loading both cogitate agents and generate prompts from
    talent/*.md and apps/*/talent/*.md files. Filters based on explicit type field.

    Args:
        type: If provided, only configs with matching type value
            ("generate" or "cogitate").
        schedule: If provided, only configs where schedule matches this value
            (e.g., "segment", "daily").
        include_disabled: If True, include configs with disabled=True.
            Default False (for processing pipelines).

    Returns:
        Dictionary mapping config keys to their metadata including:
        - path: Path to the .md file
        - source: "system" or "app"
        - app: App name (only for app configs)
        - All fields from frontmatter
    """
    from solstone.think.utils import get_config

    configs: dict[str, dict[str, Any]] = {}

    def matches_filter(info: dict) -> bool:
        """Check if config matches the filter criteria."""
        # Check explicit type filter
        if type is not None and info.get("type") != type:
            return False

        # Check specific schedule value
        if schedule is not None and info.get("schedule") != schedule:
            return False

        # Check disabled status
        if not include_disabled and info.get("disabled", False):
            return False

        return True

    # System configs from talent/
    if TALENT_DIR.is_dir():
        for md_path in sorted(TALENT_DIR.glob("*.md")):
            name = md_path.stem
            info = _load_prompt_metadata(md_path)

            info["source"] = "system"
            configs[name] = info

    # App configs from apps/*/talent/
    apps_dir = APPS_DIR
    if apps_dir.is_dir():
        for app_path in sorted(apps_dir.iterdir()):
            if not app_path.is_dir() or app_path.name.startswith("_"):
                continue
            app_talent_dir = app_path / "talent"
            if not app_talent_dir.is_dir():
                continue
            app_name = app_path.name
            for md_path in sorted(app_talent_dir.glob("*.md")):
                item_name = md_path.stem
                info = _load_prompt_metadata(md_path)

                key = f"{app_name}:{item_name}"
                info["source"] = "app"
                info["app"] = app_name
                configs[key] = info

    # Merge journal config overrides from providers.contexts
    providers_config = get_config().get("providers", {})
    contexts = providers_config.get("contexts", {})

    for key, info in configs.items():
        context_key = key_to_context(key)

        # Check for exact match in contexts
        override = contexts.get(context_key)
        if override and isinstance(override, dict):
            # Merge supported override fields
            if "disabled" in override:
                info["disabled"] = override["disabled"]
            if "extract" in override:
                info["extract"] = override["extract"]
            if "tier" in override:
                info["tier"] = override["tier"]
            if "provider" in override:
                info["provider"] = override["provider"]

    # Validate: scheduled prompts must have explicit priority
    for key, info in configs.items():
        if info.get("schedule") and "priority" not in info:
            raise ValueError(
                f"Scheduled prompt '{key}' is missing required 'priority' field. "
                f"All prompts with 'schedule' must declare an explicit priority."
            )

    # Validate: prompts with output must have consistent explicit type
    valid_types = {"generate", "cogitate"}
    for key, info in configs.items():
        output_present = "output" in info
        config_type = info.get("type")

        if config_type is not None and config_type not in valid_types:
            raise ValueError(
                f"Prompt '{key}' has invalid type {config_type!r}. "
                "Expected 'generate' or 'cogitate'."
            )

        if not output_present and config_type is None:
            continue

        if config_type is None:
            raise ValueError(
                f"Prompt '{key}' has output but is missing required 'type' field."
            )

        if config_type == "generate" and not output_present:
            raise ValueError(
                f"Prompt '{key}' has type='generate' but is missing required 'output' field."
            )

    # Validate: activity-scheduled prompts must have 'activities' list
    for key, info in configs.items():
        if info.get("schedule") == "activity":
            activities_field = info.get("activities")
            if not activities_field or not isinstance(activities_field, list):
                raise ValueError(
                    f"Activity-scheduled prompt '{key}' must have a non-empty 'activities' list "
                    f'(activity types to match, or ["*"] for all types).'
                )

    # Validate: cwd is only valid for cogitate prompts and defaults there
    for key, info in configs.items():
        normalized_cwd = _validate_cwd(info.get("cwd"), info.get("type"), key)
        if normalized_cwd is None:
            info.pop("cwd", None)
        else:
            info["cwd"] = normalized_cwd

    return {key: info for key, info in configs.items() if matches_filter(info)}


# ---------------------------------------------------------------------------
# Talent Resolution
# ---------------------------------------------------------------------------


def _resolve_talent_path(name: str) -> tuple[Path, str]:
    """Resolve talent name to directory path and filename.

    Parameters
    ----------
    name:
        Talent name - either system talent (e.g., "chat") or
        app-namespaced talent (e.g., "support:support").

    Returns
    -------
    tuple[Path, str]
        (talent_directory, talent_name) tuple.
    """
    if ":" in name:
        # App talent: "support:support" -> apps/support/talent/support
        app, talent_name = name.split(":", 1)
        talent_dir = Path(__file__).parent.parent / "apps" / app / "talent"
    else:
        # System talent: bare name -> talent/{name}
        talent_dir = TALENT_DIR
        talent_name = name
    return talent_dir, talent_name


# Default load configuration - prompts must explicitly opt into source loading
_DEFAULT_LOAD = {
    "transcripts": False,
    "percepts": False,
    "talents": False,
}


# ---------------------------------------------------------------------------
# Source Configuration Helpers
# ---------------------------------------------------------------------------


def source_is_enabled(value: bool | str | dict) -> bool:
    """Check if a source should be loaded based on its config value.

    Sources can be configured as:
    - False: don't load
    - True: load if available
    - "required": load (and generation will fail if none found)
    - dict: for talents source, selective loading (e.g., {"entities": true})

    Both True and "required" mean the source should be loaded.
    A non-empty dict means the source should be loaded (with filtering).

    Args:
        value: The source config value (bool, "required" string, or dict for talents)

    Returns:
        True if the source should be loaded, False otherwise.
    """
    if isinstance(value, dict):
        # Dict means selective loading - enabled if any agent is enabled
        return any(v is True or v == "required" for v in value.values())
    return value is True or value == "required"


def source_is_required(value: bool | str | dict) -> bool:
    """Check if a source must have content for generation to proceed.

    Args:
        value: The source config value (bool, "required" string, or dict for talents)

    Returns:
        True if the source is required (generation should skip if no content).
        For dict values, returns True if any agent is marked "required".
    """
    if isinstance(value, dict):
        return any(v == "required" for v in value.values())
    return value == "required"


def get_talent_filter(value: bool | str | dict) -> dict[str, bool | str] | None:
    """Extract talent filter from sources config.

    When talents source is a dict, returns it as filter mapping talent names
    to their enabled/required status. When talents source is bool or "required",
    returns None to indicate all talents should be loaded.

    Args:
        value: The talents source config value

    Returns:
        Dict mapping talent names to bool/"required", or None for all talents.
        Returns empty dict if value is False (no talents).

    Examples:
        >>> get_talent_filter(True)
        None  # All talents
        >>> get_talent_filter(False)
        {}  # No talents
        >>> get_talent_filter({"entities": True, "meetings": "required"})
        {"entities": True, "meetings": "required"}
    """
    if isinstance(value, dict):
        return value
    if value is False:
        return {}  # No talents
    return None  # All talents (True or "required")


def _valid_runtime_facets() -> list[str]:
    """Return sorted list of facet directory names matching SLUG_RE."""
    return sorted(slug for slug in get_facets() if SLUG_RE.fullmatch(slug))


def hydrate_runtime_enums(schema: Any) -> Any:
    """Replace runtime sentinels in schema enums with current journal state.

    Walks the schema; wherever an `enum` is exactly [RUNTIME_FACETS_SENTINEL],
    replaces it with the sorted list of valid runtime facet slugs. If zero
    valid facets exist, drops the `enum` key so the node remains a plain
    portable {"type": "string"} with no banned keyword.

    Returns None when given None. Deep-copies non-None input. Idempotent
    for already-hydrated schemas (sentinel is gone after first call).
    """
    if schema is None:
        return None

    hydrated = copy.deepcopy(schema)
    facets = _valid_runtime_facets()

    def _walk(node: Any) -> None:
        if isinstance(node, dict):
            if node.get("enum") == [RUNTIME_FACETS_SENTINEL]:
                if facets:
                    node["enum"] = list(facets)
                else:
                    node.pop("enum", None)
            for value in node.values():
                _walk(value)
        elif isinstance(node, list):
            for item in node:
                _walk(item)

    _walk(hydrated)

    return hydrated


# ---------------------------------------------------------------------------
# Talent Loading
# ---------------------------------------------------------------------------


def _load_talent_schema(
    *,
    name: str,
    md_path: Path,
    raw_schema: Any,
) -> dict[str, Any]:
    """Load and validate a talent JSON Schema from a relative file path."""
    if not isinstance(raw_schema, str):
        raise ValueError(
            f"talent {name}: schema must be a string, got {type(raw_schema).__name__}: "
            f"{raw_schema!r}"
        )

    raw_path = Path(raw_schema)
    if raw_path.is_absolute():
        raise ValueError(f"talent {name}: schema path must be relative: {raw_schema}")
    if ".." in raw_path.parts:
        raise ValueError(
            f"talent {name}: schema path must not contain '..': {raw_schema}"
        )

    talent_dir = md_path.parent.resolve()
    schema_path = (md_path.parent / raw_schema).resolve()
    if not schema_path.is_relative_to(talent_dir):
        raise ValueError(
            f"talent {name}: schema path escapes talent directory: {schema_path}"
        )
    if not schema_path.exists():
        raise FileNotFoundError(f"talent {name}: schema file not found: {schema_path}")

    try:
        with open(schema_path, encoding="utf-8") as f:
            parsed = json.load(f)
    except json.JSONDecodeError as exc:
        raise ValueError(
            f"talent {name}: schema file is not valid JSON: {schema_path}"
        ) from exc

    try:
        Draft202012Validator.check_schema(parsed)
    except SchemaError as exc:
        raise ValueError(
            f"talent {name}: schema file is not a valid JSON Schema: {schema_path}"
        ) from exc

    return parsed


def get_talent(
    name: str = "chat",
    facet: str | None = None,
    analysis_day: str | None = None,
) -> dict:
    """Return a complete talent configuration by name.

    Loads configuration from .md file with JSON frontmatter and instruction text.
    Template variables like $facets are resolved during prompt loading.
    Source data config comes from the frontmatter 'load' key.

    Parameters
    ----------
    name:
        Talent name to load. Can be a system talent (e.g., "chat")
        or an app-namespaced talent (e.g., "support:support" for apps/support/talent/support).
    facet:
        Optional facet name to focus on. Controls $facets template variable.
    analysis_day:
        Optional day in YYYYMMDD format. Not used directly — day-based
        template context is applied in prepare_config().

    Returns
    -------
    dict
        Complete talent configuration including:
        - name: Talent name
        - path: Path to the .md file
        - user_instruction: Composed prompt with template vars resolved
        - sources: Source config from 'load' key
        - All frontmatter fields (tools, hook, disabled, thinking_budget, etc.)
    """
    from solstone.think.prompts import _resolve_facets

    # Resolve talent path based on namespace
    talent_dir, talent_name = _resolve_talent_path(name)

    # Verify talent prompt file exists
    md_path = talent_dir / f"{talent_name}.md"
    if not md_path.exists():
        raise FileNotFoundError(f"Talent not found: {name}")

    # Load config from frontmatter - preserve all fields
    post = frontmatter.load(md_path)
    config = dict(post.metadata) if post.metadata else {}
    normalized_cwd = _validate_cwd(config.get("cwd"), config.get("type"), name)
    if normalized_cwd is None:
        config.pop("cwd", None)
    else:
        config["cwd"] = normalized_cwd

    # Store path for later use
    config["path"] = str(md_path)

    if "schema" in config:
        config["json_schema"] = _load_talent_schema(
            name=name,
            md_path=md_path,
            raw_schema=config["schema"],
        )
        del config["schema"]

    # Extract source config from 'load' key (replaces instructions.sources)
    config["sources"] = config.pop("load", _DEFAULT_LOAD.copy())

    # Build template context for $facets resolution
    prompt_context: dict[str, str] = {}
    prompt_context["facets"] = _resolve_facets(facet)

    prompt_obj = load_prompt(talent_name, base_dir=talent_dir, context=prompt_context)
    config["user_instruction"] = prompt_obj.text

    # Set talent name
    config["name"] = name

    return config


# ---------------------------------------------------------------------------
# Hook Loading
# ---------------------------------------------------------------------------


def _resolve_hook_path(hook_name: str) -> Path:
    """Resolve hook name to file path.

    Resolution:
    - Named: "name" -> talent/{name}.py
    - App-qualified: "app:name" -> apps/{app}/talent/{name}.py
    - Explicit path: "path/to/hook.py" -> package-relative path
    """
    if "/" in hook_name or hook_name.endswith(".py"):
        package_root = Path(__file__).parent.parent
        return package_root / hook_name
    elif ":" in hook_name:
        app, name = hook_name.split(":", 1)
        return Path(__file__).parent.parent / "apps" / app / "talent" / f"{name}.py"
    else:
        return TALENT_DIR / f"{hook_name}.py"


def _load_hook_function(config: dict, key: str, func_name: str) -> Callable | None:
    """Load a hook function from config.

    Args:
        config: Agent/generator config dict
        key: Hook key in config ("pre" or "post")
        func_name: Function name to load ("pre_process" or "post_process")

    Returns:
        The hook function, or None if no hook configured.

    Raises:
        ValueError: If hook file doesn't define the required function.
        ImportError: If hook file cannot be loaded.
    """
    hook_config = config.get("hook")
    if not hook_config or not isinstance(hook_config, dict):
        return None

    hook_name = hook_config.get(key)
    if not hook_name:
        return None

    hook_path = _resolve_hook_path(hook_name)

    if not hook_path.exists():
        raise ImportError(f"Hook file not found: {hook_path}")

    spec = importlib.util.spec_from_file_location(
        f"{key}_hook_{hook_path.stem}", hook_path
    )
    if spec is None or spec.loader is None:
        raise ImportError(f"Cannot load hook from {hook_path}")

    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)

    if not hasattr(module, func_name):
        raise ValueError(f"Hook {hook_path} must define a '{func_name}' function")

    process_func = getattr(module, func_name)
    if not callable(process_func):
        raise ValueError(f"Hook {hook_path} '{func_name}' must be callable")

    return process_func


def load_post_hook(config: dict) -> Callable[[str, "HookContext"], str | None] | None:
    """Load post-processing hook from config if defined.

    Hook config format: {"hook": {"post": "name"}}

    Returns:
        Post-processing function or None if no hook configured.
        Function signature: (result: str, context: HookContext) -> str | None
    """
    return _load_hook_function(config, "post", "post_process")


def load_pre_hook(config: dict) -> Callable[["PreHookContext"], dict | None] | None:
    """Load pre-processing hook from config if defined.

    Hook config format: {"hook": {"pre": "name"}}

    Returns:
        Pre-processing function or None if no hook configured.
        Function signature: (context: PreHookContext) -> dict | None
    """
    return _load_hook_function(config, "pre", "pre_process")


# Type aliases for hook context - hooks receive the full config dict
HookContext = dict
PreHookContext = dict
