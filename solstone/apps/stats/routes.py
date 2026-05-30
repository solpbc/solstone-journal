# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import logging
import os
from typing import Any

logger = logging.getLogger(__name__)

from flask import Blueprint, jsonify

from solstone.convey import backlog_copy, state
from solstone.think.talent import get_talent_configs

stats_bp = Blueprint(
    "app:stats",
    __name__,
    url_prefix="/app/stats",
    static_folder="static",
    static_url_path="/static",
)


@stats_bp.app_context_processor
def _inject_backlog_copy() -> dict:
    return {"backlog_copy": backlog_copy}


@stats_bp.route("/api/stats")
def stats_data() -> Any:
    """Return statistics from stats.json."""
    response = {
        "stats": {},
    }

    # Load stats.json
    stats_path = os.path.join(state.journal_root, "stats.json")
    if os.path.isfile(stats_path):
        try:
            with open(stats_path, "r", encoding="utf-8") as f:
                response["stats"] = json.load(f)
            response["file_mtime"] = os.path.getmtime(stats_path)
        except Exception:
            logger.exception("Failed to read stats data")
            response["error"] = "Failed to read stats data"

    response["generators"] = get_talent_configs(type="generate")

    return jsonify(response)
