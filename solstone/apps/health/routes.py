# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
import logging
import os
import re
import socket
from pathlib import Path

from flask import Blueprint, jsonify, render_template, request

from solstone.apps.health import copy as health_copy
from solstone.convey import backlog_copy, state
from solstone.convey.backlog_view import stuck_rows, verdict
from solstone.convey.reasons import (
    FILE_NOT_FOUND,
    FILE_READ_FAILED,
    INVALID_PATH,
    INVALID_REQUEST_VALUE,
    MISSING_REQUIRED_FIELD,
    OBSERVER_RESTART_FAILED,
)
from solstone.convey.utils import error_response
from solstone.think.callosum import callosum_send
from solstone.think.streams import stream_name

logger = logging.getLogger(__name__)

health_bp = Blueprint("app:health", __name__, url_prefix="/app/health")


@health_bp.app_context_processor
def _inject_health_copy() -> dict:
    return {"health_copy": health_copy}


@health_bp.app_context_processor
def _inject_backlog_copy() -> dict:
    return {"backlog_copy": backlog_copy}


# Supervisor currently registers one observer-facing processing service: "sense".
# Observer rows are per registration key, but reconnect restarts this shared worker.
# Keep this endpoint whitelist local until supervisor exposes a public service list.
OBSERVER_RESTART_SERVICES = {"sense"}


def _load_backlog() -> dict | None:
    stats_path = os.path.join(state.journal_root, "stats.json")
    if not os.path.isfile(stats_path):
        return None
    try:
        with open(stats_path, "r", encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        logger.exception("Failed to read backlog from stats.json")
        return None
    backlog = data.get("backlog")
    return backlog if isinstance(backlog, dict) else None


@health_bp.route("/")
def index():
    backlog = _load_backlog()
    return render_template(
        "app.html",
        health_backlog_verdict=verdict(backlog),
        health_stuck_rows=stuck_rows(backlog),
    )


@health_bp.get("/api/log")
def get_log():
    path = request.args.get("path")
    if not path:
        return error_response(MISSING_REQUIRED_FIELD, detail="Missing path parameter")

    if not re.fullmatch(r"\d{8}/health/[^/]+\.log", path):
        return error_response(INVALID_PATH, detail="Invalid path")

    journal_root = Path(state.journal_root).resolve()
    try:
        file_path = (Path(state.journal_root) / path).resolve()
    except ValueError:
        return error_response(INVALID_PATH, detail="Invalid path")
    try:
        file_path.relative_to(journal_root)
    except ValueError:
        return error_response(INVALID_PATH, detail="Invalid path")

    if not file_path.exists():
        return error_response(FILE_NOT_FOUND, detail="Log file not found")

    try:
        content = file_path.read_text(encoding="utf-8")
    except IOError:
        return error_response(FILE_READ_FAILED, detail="Failed to read log file")

    return jsonify(content=content, path=path)


@health_bp.route("/api/info")
def api_info():
    return jsonify({"hostname": stream_name(host=socket.gethostname())})


@health_bp.post("/api/retry-import")
def retry_import():
    data = request.get_json(silent=True) or {}
    if not data.get("import_id"):
        return error_response(MISSING_REQUIRED_FIELD, detail="Missing import_id")
    stage = data.get("stage")
    message = "Import retry will be available in a future update"
    if stage:
        message = (
            f"Import retry from stage {stage} will be available in a future update"
        )
    return jsonify(
        status="not_implemented",
        message=message,
    ), 501


@health_bp.post("/api/restart-observer")
def restart_observer():
    data = request.get_json(silent=True) or {}
    service = data.get("service")
    if not service:
        return error_response(MISSING_REQUIRED_FIELD, detail="Missing service")
    if service not in OBSERVER_RESTART_SERVICES:
        return error_response(INVALID_REQUEST_VALUE, detail="Unknown observer service")

    ok = callosum_send("supervisor", "restart", service=service)
    if not ok:
        return error_response(
            OBSERVER_RESTART_FAILED,
            detail="Could not reach the supervisor",
        )

    return jsonify(status="restart_requested", service=service)
