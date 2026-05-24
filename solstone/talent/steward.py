# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Hooks for the steward daily health talent."""

from __future__ import annotations

import dataclasses
import json
import logging
from datetime import datetime

from solstone.think.steward import (
    acquire_steward_lock,
    build_synthesis_context,
    release_steward_lock,
    run_recipe_pass,
    write_health_md,
)

logger = logging.getLogger(__name__)


def _today_from_config(config: dict) -> str:
    day = config.get("day")
    if isinstance(day, str) and day:
        return day
    return datetime.now().strftime("%Y%m%d")


def _load_json_list(value: str) -> list:
    try:
        parsed = json.loads(value)
    except json.JSONDecodeError:
        return []
    return parsed if isinstance(parsed, list) else []


def pre_process(config: dict) -> dict | None:
    """Run steward recipes and inject synthesis context."""
    fd = acquire_steward_lock()
    if fd is None:
        return {"skip_reason": "steward already in flight"}

    # If cogitate raises before post_process, process exit releases this flock.
    config["_steward_lock_fd"] = fd
    today = _today_from_config(config)
    try:
        recipe_result = run_recipe_pass(today)
        context = build_synthesis_context(today)

        errors = _load_json_list(str(context.get("data_source_errors", "[]")))
        errors.extend(recipe_result.get("data_source_errors", []))
        context["data_source_errors"] = json.dumps(
            errors, indent=2, sort_keys=True, default=str
        )
        context["escalated_targets"] = json.dumps(
            recipe_result.get("escalated_targets", []),
            indent=2,
            sort_keys=True,
            default=str,
        )
        context["recipe_outcomes_this_run"] = json.dumps(
            [dataclasses.asdict(outcome) for outcome in recipe_result.get("fired", [])],
            indent=2,
            sort_keys=True,
            default=str,
        )
        return {"template_vars": context}
    except Exception as exc:
        logger.exception("steward pre-hook failed")
        release_steward_lock(fd)
        config.pop("_steward_lock_fd", None)
        return {"skip_reason": f"steward pre-hook failed: {exc}"}


def post_process(result: str, config: dict) -> str | None:
    """Validate and publish the rendered steward health markdown."""
    fd = config.get("_steward_lock_fd")
    try:
        reason = write_health_md(result)
        if reason is not None:
            logger.error("steward render rejected: %s", reason)
        return result
    except Exception:
        logger.exception("steward post-hook failed")
        return result
    finally:
        if isinstance(fd, int):
            release_steward_lock(fd)
            config.pop("_steward_lock_fd", None)
