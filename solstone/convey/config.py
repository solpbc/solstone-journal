# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Convey configuration management and API routes."""

from __future__ import annotations

import json
import logging
from collections.abc import Callable
from pathlib import Path
from typing import Any

from flask import Blueprint, request

from solstone.think.journal_io import LockTimeout, atomic_replace, hold_lock

from . import state
from .reasons import (
    CONVEY_BUSY,
    CONVEY_OPERATION_FAILED,
    INVALID_CONFIG_VALUE,
    INVALID_JSON_REQUEST,
    MISSING_REQUIRED_FIELD,
)
from .utils import error_response, load_json, success_response

logger = logging.getLogger(__name__)

bp = Blueprint("config", __name__, url_prefix="/api/config")

DEFAULT_RAIL_APPS = [
    "home",
    "sol",
    "chat",
    "curation",
    "transcripts",
    "observer",
    "search",
    "import",
]

# Default order for the convey sidebar. Includes the starred rail apps in the
# same order they appear in DEFAULT_RAIL_APPS, then pins reflections + news
# adjacent at the top of the unstarred drawer (sol's proactive journal
# artifacts — they belong together).
DEFAULT_APP_ORDER = list(DEFAULT_RAIL_APPS) + ["news"]


def _get_config_path() -> Path:
    """Get path to config/convey.json in journal root."""
    return Path(state.journal_root) / "config" / "convey.json"


def load_convey_config() -> dict[str, Any]:
    """Load config/convey.json from journal root.

    Returns:
        Config dict with optional fields:
        - facets.order: list of facet names
        - facets.selected: currently selected facet name or null
        - apps.order: list of app names
        Empty dict if file doesn't exist or can't be parsed.
    """
    config_path = _get_config_path()
    data = load_json(config_path)
    return data if isinstance(data, dict) else {}


def reporting_enabled() -> bool:
    """Return whether owner-facing error reporting is enabled."""
    config = load_convey_config()
    return config.get("reporting", {}).get("enabled", True)


def _write_convey_config(config: dict[str, Any]) -> None:
    """Atomically persist convey config. Caller MUST hold the convey.json lock."""
    atomic_replace(
        _get_config_path(),
        json.dumps(config, indent=2, ensure_ascii=False) + "\n",
    )


def locked_modify_convey_config(
    transform: Callable[[dict[str, Any]], dict[str, Any] | None],
) -> dict[str, Any] | None:
    """Apply a locked read-modify-write to config/convey.json.

    Reads the current config under an exclusive lock, applies ``transform``,
    and atomically persists the result. If ``transform`` returns ``None`` the
    write is skipped (no-op). Returns the persisted config, or ``None`` when
    skipped.
    """
    path = _get_config_path()
    with hold_lock(path):
        config = load_convey_config()
        new_config = transform(config)
        if new_config is None:
            return None
        _write_convey_config(new_config)
        return new_config


def seed_default_app_navigation(config: dict[str, Any]) -> bool:
    """Seed default starred apps + rail order into a loaded convey config.

    Seeds ``apps.starred`` and ``apps.order`` independently, each only when its
    key is ABSENT (never when present-and-empty - an empty list is a curated
    owner preference). Mutates ``config`` in place. Returns True iff a key was
    seeded.
    """
    apps_config = config.setdefault("apps", {})
    changed = False
    if "starred" not in apps_config:
        apps_config["starred"] = list(DEFAULT_RAIL_APPS)
        changed = True
    if "order" not in apps_config:
        apps_config["order"] = list(DEFAULT_APP_ORDER)
        changed = True
    return changed


def get_selected_facet() -> str | None:
    """Get selected facet from config.

    Returns:
        Selected facet name, or None if not set
    """
    config = load_convey_config()
    facets_config = config.get("facets", {})
    return facets_config.get("selected")


def set_selected_facet(facet: str | None) -> None:
    """Update selected facet in config (best-effort; never raises)."""

    def _transform(config: dict[str, Any]) -> dict[str, Any]:
        config.setdefault("facets", {})["selected"] = facet
        return config

    try:
        locked_modify_convey_config(_transform)
    except (LockTimeout, OSError) as exc:
        logger.warning("Failed to save selected facet %s: %s", facet, exc)


def apply_facet_order(facets: list[dict], config: dict) -> list[dict]:
    """Apply custom ordering from config to facet list.

    Args:
        facets: List of facet dicts with 'name' field
        config: Config dict with optional facets.order field

    Returns:
        Reordered facet list (ordered items first, then alphabetical remainder)
    """
    order = config.get("facets", {}).get("order", [])
    if not order:
        return facets

    # Create lookup by name
    facet_map = {f["name"]: f for f in facets}

    # Ordered items first (if they exist)
    ordered = [facet_map[name] for name in order if name in facet_map]

    # Remaining items alphabetically
    ordered_names = set(order)
    remaining = sorted(
        [f for f in facets if f["name"] not in ordered_names],
        key=lambda f: f["name"],
    )

    return ordered + remaining


def apply_app_order(apps: dict[str, Any], config: dict) -> dict[str, Any]:
    """Apply custom ordering from config to app dict.

    Groups apps by starred status, then applies ordering within each group.
    Starred apps appear first, followed by unstarred apps.

    Args:
        apps: Dict mapping app name to app data
        config: Config dict with optional apps.order and apps.starred fields

    Returns:
        Reordered dict (starred apps first in order, then unstarred apps in order)
    """
    order = config.get("apps", {}).get("order", [])
    starred = set(config.get("apps", {}).get("starred", []))

    # Separate apps into starred and unstarred
    starred_apps = {}
    unstarred_apps = {}

    for name, data in apps.items():
        if name in starred:
            starred_apps[name] = data
        else:
            unstarred_apps[name] = data

    # Helper to order a subset of apps
    def order_apps(app_dict: dict[str, Any], app_order: list[str]) -> dict[str, Any]:
        ordered = {}
        # Ordered items first (if they exist)
        for name in app_order:
            if name in app_dict:
                ordered[name] = app_dict[name]
        # Remaining items alphabetically
        ordered_names = set(app_order)
        for name in sorted(app_dict.keys()):
            if name not in ordered_names:
                ordered[name] = app_dict[name]
        return ordered

    # Order each group
    ordered_starred = order_apps(starred_apps, order) if starred_apps else {}
    ordered_unstarred = order_apps(unstarred_apps, order) if unstarred_apps else {}

    # Combine: starred first, then unstarred
    result = {}
    result.update(ordered_starred)
    result.update(ordered_unstarred)

    return result


def validate_config(config: dict[str, Any]) -> tuple[bool, str | None]:
    """Validate config structure and values.

    Args:
        config: Config dict to validate

    Returns:
        (is_valid, error_message) tuple
    """
    # Check top-level structure
    if not isinstance(config, dict):
        return False, "Config must be a JSON object"

    # Validate facets section if present
    if "facets" in config:
        facets_config = config["facets"]
        if not isinstance(facets_config, dict):
            return False, "facets must be an object"

        # Validate facets.order
        if "order" in facets_config:
            order = facets_config["order"]
            if not isinstance(order, list):
                return False, "facets.order must be an array"
            if not all(isinstance(name, str) for name in order):
                return False, "facets.order must contain only strings"

        # Validate facets.selected
        if "selected" in facets_config:
            selected = facets_config["selected"]
            if selected is not None and not isinstance(selected, str):
                return False, "facets.selected must be a string or null"

    # Validate apps section if present
    if "apps" in config:
        apps_config = config["apps"]
        if not isinstance(apps_config, dict):
            return False, "apps must be an object"

        # Validate apps.order
        if "order" in apps_config:
            order = apps_config["order"]
            if not isinstance(order, list):
                return False, "apps.order must be an array"
            if not all(isinstance(name, str) for name in order):
                return False, "apps.order must contain only strings"

        # Validate apps.starred
        if "starred" in apps_config:
            starred = apps_config["starred"]
            if not isinstance(starred, list):
                return False, "apps.starred must be an array"
            if not all(isinstance(name, str) for name in starred):
                return False, "apps.starred must contain only strings"

    # Validate reporting section if present
    if "reporting" in config:
        reporting_config = config["reporting"]
        if not isinstance(reporting_config, dict):
            return False, "reporting must be an object"

        allowed_reporting_keys = {"enabled"}
        unknown_keys = set(reporting_config) - allowed_reporting_keys
        if unknown_keys:
            key_list = ", ".join(sorted(unknown_keys))
            return False, f"reporting contains unknown key(s): {key_list}"

        if "enabled" in reporting_config and not isinstance(
            reporting_config["enabled"], bool
        ):
            return False, "reporting.enabled must be a boolean"

    return True, None


# API Routes


@bp.route("/convey")
def get_config() -> tuple[Any, int]:
    """GET /api/config/convey - Return current convey configuration.

    Returns:
        JSON response with config data
    """
    try:
        config = load_convey_config()
        return success_response({"config": config})
    except Exception as e:
        logger.error(f"Failed to load config: {e}", exc_info=True)
        return error_response(
            CONVEY_OPERATION_FAILED,
            detail="Failed to load configuration",
        )


@bp.route("/convey", methods=["POST"])
def update_config() -> tuple[Any, int]:
    """POST /api/config/convey - Update convey configuration.

    Request body: Full or partial config object
    {
        "facets": {
            "order": ["work", "personal"],
            "selected": "work"
        },
        "apps": {
            "order": ["home", "activities"]
        }
    }

    Returns:
        JSON success/error response
    """
    try:
        # Parse request
        new_config = request.get_json()
        if not new_config:
            return error_response(
                INVALID_JSON_REQUEST,
                detail="Request body must be JSON",
            )

        # Validate structure
        valid, error_msg = validate_config(new_config)
        if not valid:
            return error_response(
                INVALID_CONFIG_VALUE,
                detail=f"Invalid config: {error_msg}",
            )

        def _transform(config: dict[str, Any]) -> dict[str, Any]:
            if "facets" in new_config:
                config.setdefault("facets", {}).update(new_config["facets"])
            if "apps" in new_config:
                config.setdefault("apps", {}).update(new_config["apps"])
            if "reporting" in new_config:
                config.setdefault("reporting", {}).update(new_config["reporting"])
            return config

        persisted = locked_modify_convey_config(_transform)
        return success_response({"config": persisted})

    except LockTimeout:
        return error_response(
            CONVEY_BUSY,
            detail="Interface settings are busy; try again",
        )
    except Exception as e:
        logger.error(f"Failed to update config: {e}", exc_info=True)
        return error_response(
            CONVEY_OPERATION_FAILED,
            detail="Failed to update configuration",
        )


@bp.route("/facets/order", methods=["POST"])
def update_facet_order() -> tuple[Any, int]:
    """POST /api/config/facets/order - Update facet ordering.

    Request body: {"order": ["work", "personal", "research"]}

    Returns:
        JSON success/error response
    """
    try:
        data = request.get_json()
        if not data or "order" not in data:
            return error_response(
                MISSING_REQUIRED_FIELD,
                detail="Request must include 'order' array",
            )

        order = data["order"]
        if not isinstance(order, list):
            return error_response(
                INVALID_CONFIG_VALUE, detail="'order' must be an array"
            )

        if not all(isinstance(name, str) for name in order):
            return error_response(
                INVALID_CONFIG_VALUE,
                detail="'order' must contain only strings",
            )

        def _transform(config: dict[str, Any]) -> dict[str, Any]:
            config.setdefault("facets", {})["order"] = order
            return config

        locked_modify_convey_config(_transform)

        return success_response({"order": order})

    except LockTimeout:
        return error_response(
            CONVEY_BUSY,
            detail="Interface settings are busy; try again",
        )
    except Exception as e:
        logger.error(f"Failed to update facet order: {e}", exc_info=True)
        return error_response(
            CONVEY_OPERATION_FAILED,
            detail="Failed to update facet order",
        )


@bp.route("/apps/order", methods=["POST"])
def update_app_order() -> tuple[Any, int]:
    """POST /api/config/apps/order - Update app ordering.

    Request body: {"order": ["home", "activities", "entities"]}

    Returns:
        JSON success/error response
    """
    try:
        data = request.get_json()
        if not data or "order" not in data:
            return error_response(
                MISSING_REQUIRED_FIELD,
                detail="Request must include 'order' array",
            )

        order = data["order"]
        if not isinstance(order, list):
            return error_response(
                INVALID_CONFIG_VALUE, detail="'order' must be an array"
            )

        if not all(isinstance(name, str) for name in order):
            return error_response(
                INVALID_CONFIG_VALUE,
                detail="'order' must contain only strings",
            )

        def _transform(config: dict[str, Any]) -> dict[str, Any]:
            config.setdefault("apps", {})["order"] = order
            return config

        locked_modify_convey_config(_transform)

        return success_response({"order": order})

    except LockTimeout:
        return error_response(
            CONVEY_BUSY,
            detail="Interface settings are busy; try again",
        )
    except Exception as e:
        logger.error(f"Failed to update app order: {e}", exc_info=True)
        return error_response(
            CONVEY_OPERATION_FAILED,
            detail="Failed to update app order",
        )


@bp.route("/apps/star", methods=["POST"])
def toggle_app_star() -> tuple[Any, int]:
    """POST /api/config/apps/star - Toggle starred status of an app.

    Request body: {"app": "activities", "starred": true}

    Returns:
        JSON success/error response
    """
    try:
        data = request.get_json()
        if not data or "app" not in data or "starred" not in data:
            return error_response(
                MISSING_REQUIRED_FIELD,
                detail="Request must include 'app' and 'starred' fields",
            )

        app_name = data["app"]
        starred = data["starred"]

        if not isinstance(app_name, str):
            return error_response(INVALID_CONFIG_VALUE, detail="'app' must be a string")

        if not isinstance(starred, bool):
            return error_response(
                INVALID_CONFIG_VALUE,
                detail="'starred' must be a boolean",
            )

        def _transform(config: dict[str, Any]) -> dict[str, Any]:
            apps_config = config.setdefault("apps", {})
            starred_apps = set(apps_config.get("starred", []))
            if starred:
                starred_apps.add(app_name)
            else:
                starred_apps.discard(app_name)
            apps_config["starred"] = sorted(starred_apps)
            return config

        locked_modify_convey_config(_transform)

        return success_response({"app": app_name, "starred": starred})

    except LockTimeout:
        return error_response(
            CONVEY_BUSY,
            detail="Interface settings are busy; try again",
        )
    except Exception as e:
        logger.error(f"Failed to toggle app star: {e}", exc_info=True)
        return error_response(
            CONVEY_OPERATION_FAILED,
            detail="Failed to toggle app starred status",
        )


@bp.route("/facets/select", methods=["POST"])
def select_facet() -> tuple[Any, int]:
    """POST /api/config/facets/select - Update selected facet.

    Request body: {"facet": "work"} or {"facet": null}

    Returns:
        JSON success/error response
    """
    try:
        data = request.get_json()
        if data is None:
            return error_response(
                INVALID_JSON_REQUEST,
                detail="Request body must be JSON",
            )

        if "facet" not in data:
            return error_response(
                MISSING_REQUIRED_FIELD,
                detail="Request must include 'facet' field",
            )

        facet = data["facet"]
        if facet is not None and not isinstance(facet, str):
            return error_response(
                INVALID_CONFIG_VALUE,
                detail="'facet' must be a string or null",
            )

        # Update config
        set_selected_facet(facet)

        return success_response({"facet": facet})

    except Exception as e:
        logger.error(f"Failed to update selected facet: {e}", exc_info=True)
        return error_response(
            CONVEY_OPERATION_FAILED,
            detail="Failed to update selected facet",
        )
