# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Root blueprint: authentication and core routes."""

from __future__ import annotations

import json
import logging
import os
import queue
import time
from datetime import date
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _pkg_version
from pathlib import Path
from typing import Any

from flask import (
    Blueprint,
    Response,
    g,
    jsonify,
    redirect,
    render_template,
    request,
    send_from_directory,
    session,
    stream_with_context,
    url_for,
)
from werkzeug.security import check_password_hash, generate_password_hash

from solstone.think.cluster import cluster_segments
from solstone.think.journal_config import write_journal_config
from solstone.think.utils import (
    day_dirs,
    ensure_journal_config,
    get_config,
    get_journal,
)

from . import bridge as convey_bridge
from .config import (
    load_convey_config,
    save_convey_config,
    seed_default_app_navigation,
)
from .copy import LOGIN_NO_PASSWORD_CONFIGURED
from .reasons import INVALID_CONFIG_VALUE, PL_REVOKED
from .secure_listener import get_authorized_clients
from .utils import error_response, error_response_with_reason

logger = logging.getLogger(__name__)


def _get_password_hash() -> str:
    """Get current password hash from config, reloading on each call."""
    try:
        config = get_config()
        convey_config = config.get("convey", {})
        return convey_config.get("password_hash", "")
    except Exception:
        return ""


def _is_setup_complete() -> bool:
    """Check if initial setup has been completed."""
    try:
        config = get_config()
        return bool(config.get("setup", {}).get("completed_at"))
    except Exception:
        return False


def _check_basic_auth() -> bool:
    """Check Basic Auth credentials against stored password hash."""
    auth = request.authorization
    if not auth or auth.type != "basic":
        return False
    password_hash = _get_password_hash()
    if not password_hash:
        return False
    return check_password_hash(password_hash, auth.password or "")


def _save_config_section(section: str, data: dict) -> dict:
    """Merge data into a config section and write back to journal.json."""
    config = get_config()
    config.setdefault(section, {}).update(data)
    config_path = Path(get_journal()) / "config" / "journal.json"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    with open(config_path, "w", encoding="utf-8") as f:
        json.dump(config, f, indent=2, ensure_ascii=False)
        f.write("\n")
    os.chmod(config_path, 0o600)
    return config


bp = Blueprint(
    "root",
    __name__,
    template_folder="templates",
    static_folder="static",
)


@bp.before_app_request
def require_login() -> Any:
    if request.endpoint is None:
        return None

    if request.endpoint in {
        "root.init",
        "root.init_validate_provider",
        "root.init_observers",
        "root.init_finalize",
        "root.login",
        "static",
        "root.static",
        "root.favicon",
        # Observer ingest endpoints use key-based auth, not session
        "app:observer.ingest_upload",
        "app:observer.ingest_event",
        "app:observer.ingest_segments",
        "app:observer.ingest_transfer",
        "app:observer.ingest_manifest",
        "app:observer.ingest_manifest_day",
        # Journal-source manifest and ingest endpoints use key-based auth, not session
        "app:import.journal_source_manifest",
        "app:import.ingest_segments",
        "app:import.ingest_entities",
        "app:import.ingest_facets",
        "app:import.ingest_imports",
        "app:import.ingest_config",
    }:
        return None

    identity = getattr(g, "identity", None)
    if identity is not None and identity.mode in {"pl-direct", "pl-via-spl"}:
        if identity.fingerprint and get_authorized_clients().is_authorized(
            identity.fingerprint
        ):
            return None
        return error_response_with_reason(
            PL_REVOKED,
            detail="paired device revoked",
        )

    # Session cookie
    if session.get("logged_in"):
        return None

    # Basic Auth (per-request, no session creation)
    if _check_basic_auth():
        return None

    # Check setup state
    setup_complete = _is_setup_complete()

    # Opt-in localhost bypass (requires completed setup + trust_localhost flag)
    if setup_complete:
        config = get_config()
        if config.get("convey", {}).get("trust_localhost", True):
            remote_addr = request.remote_addr
            is_localhost = remote_addr in ("127.0.0.1", "::1", "localhost")
            proxy_headers = (
                request.headers.get("X-Forwarded-For")
                or request.headers.get("X-Real-IP")
                or request.headers.get("X-Forwarded-Host")
            )
            if is_localhost and not proxy_headers:
                return None

    # Not authenticated — redirect based on setup state
    if not setup_complete:
        return redirect(url_for("root.init"))
    return redirect(url_for("root.login"))


@bp.route("/sse/events", methods=["GET"], endpoint="callosum_sse")
def callosum_sse() -> Response:
    def generate():
        handle = convey_bridge.register_sse_subscriber("convey-ui")
        disconnect_event = request.environ.get("pl.disconnect_event")

        def disconnected() -> bool:
            is_set = getattr(disconnect_event, "is_set", None)
            return bool(is_set is not None and is_set())

        try:
            yield ": heartbeat\n\n"
            next_heartbeat_at = time.monotonic() + convey_bridge._SSE_HEARTBEAT_SECONDS
            while True:
                if disconnected():
                    break
                timeout = max(0.0, next_heartbeat_at - time.monotonic())
                if disconnect_event is not None:
                    timeout = min(timeout, 0.1)
                try:
                    message = handle.queue.get(timeout=timeout)
                except queue.Empty:
                    if disconnected():
                        break
                    if time.monotonic() < next_heartbeat_at:
                        continue
                    if handle.dropped.is_set():
                        break
                    yield ": heartbeat\n\n"
                    next_heartbeat_at = (
                        time.monotonic() + convey_bridge._SSE_HEARTBEAT_SECONDS
                    )
                    continue
                if handle.dropped.is_set() or disconnected():
                    break
                yield f"data: {message}\n\n"
                next_heartbeat_at = (
                    time.monotonic() + convey_bridge._SSE_HEARTBEAT_SECONDS
                )
        finally:
            convey_bridge.unregister_sse_subscriber(handle)

    return Response(
        stream_with_context(generate()),
        mimetype="text/event-stream",
        headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"},
    )


@bp.route("/login", methods=["GET", "POST"])
def login() -> Any:
    # Re-check password from config on each request
    password_hash = _get_password_hash()

    # If no password is configured, show error page
    if not password_hash:
        error = LOGIN_NO_PASSWORD_CONFIGURED
        return render_template("login.html", error=error, no_password=True)

    error = None
    if request.method == "POST":
        if check_password_hash(password_hash, request.form.get("password", "")):
            session["logged_in"] = True
            session.permanent = True
            return redirect(url_for("root.index"))
        error = "incorrect password. passwords are case-sensitive. if you've forgotten it, you can reset via sol password set on the command line."
    return render_template("login.html", error=error, no_password=False)


@bp.route("/init")
def init() -> Any:
    if _is_setup_complete():
        return redirect(url_for("root.index"))

    config = ensure_journal_config()
    identity = config.get("identity", {})
    identity_name = identity.get("name", "") or ""
    identity_preferred = identity.get("preferred", "") or ""
    retention = config.get("retention", {})
    retention_mode = retention.get("raw_media") or "keep"
    retention_days = retention.get("raw_media_days")
    try:
        version = _pkg_version("solstone")
    except PackageNotFoundError:
        version = "dev"
    journal_path = str(Path(get_journal()))
    return render_template(
        "init.html",
        version=version,
        journal_path=journal_path,
        identity_name=identity_name,
        identity_preferred=identity_preferred,
        retention_mode=retention_mode,
        retention_days=retention_days,
    )


@bp.route("/init/validate-provider", methods=["POST"])
def init_validate_provider() -> Any:
    data = request.get_json(silent=True) or {}
    key = data.get("key", "")

    from solstone.think.providers import validate_key

    try:
        result = validate_key("google", key)
    except Exception as e:
        result = {"valid": False, "error": str(e)}
    return jsonify(result)


@bp.route("/init/observers")
def init_observers() -> Any:
    from solstone.apps.observer.routes import (
        ACTIVE_THRESHOLD_MS,
        STALE_THRESHOLD_MS,
        _serialize_observer,
    )
    from solstone.apps.observer.utils import list_observers
    from solstone.think.utils import now_ms

    current_now = now_ms()
    observers_list = []
    for observer in list_observers():
        if observer.get("revoked", False):
            continue
        observers_list.append(_serialize_observer(observer, current_now))
    return jsonify(
        {
            "thresholds": {
                "active_ms": ACTIVE_THRESHOLD_MS,
                "stale_ms": STALE_THRESHOLD_MS,
            },
            "observers": observers_list,
        }
    )


@bp.route("/init/finalize", methods=["POST"])
def init_finalize() -> Any:
    data = request.get_json(silent=True) or {}

    password = data.get("password") or ""
    if password and len(password) < 8:
        return error_response(
            INVALID_CONFIG_VALUE,
            detail="Password must be at least 8 characters",
        )

    from solstone.think.utils import now_ms

    config = get_config()
    convey_update = {
        "allow_network_access": False,
        "trust_localhost": True,
    }
    if password:
        convey_update["password_hash"] = generate_password_hash(password)
    config.setdefault("convey", {}).update(convey_update)
    config.setdefault("identity", {}).update(
        {
            k: v
            for k, v in {
                "name": data.get("name"),
                "preferred": data.get("preferred"),
                "timezone": data.get("timezone"),
            }.items()
            if v
        }
    )
    gemini_key = data.get("gemini_key")
    if gemini_key:
        config.setdefault("env", {})["GOOGLE_API_KEY"] = gemini_key
    config.setdefault("setup", {})["completed_at"] = now_ms()
    retention_mode = data.get("retention_mode", "keep")
    retention_days = data.get("retention_days")
    if retention_mode == "days" and (
        not isinstance(retention_days, int) or retention_days < 1
    ):
        return error_response(
            INVALID_CONFIG_VALUE,
            detail="retention_days must be a positive integer",
        )
    config.setdefault("retention", {}).update(
        {
            "raw_media": retention_mode,
            "raw_media_days": retention_days if retention_mode == "days" else None,
        }
    )

    write_journal_config(config)

    config = load_convey_config()
    if seed_default_app_navigation(config) and not save_convey_config(config):
        logger.error("default app navigation seed convey-config PERSIST failed")

    session["logged_in"] = True
    session.permanent = True
    return jsonify({"success": True, "redirect": url_for("root.index")})


@bp.route("/logout")
def logout() -> Any:
    session.pop("logged_in", None)
    return redirect(url_for("root.login"))


@bp.route("/favicon.ico")
def favicon() -> Any:
    """Serve the favicon from the project root."""
    project_root = os.path.dirname(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    )
    return send_from_directory(project_root, "favicon.ico", mimetype="image/x-icon")


@bp.route("/app/today")
def app_today() -> Any:
    """Redirect /app/today to the most recent day with journal data."""
    today = date.today().strftime("%Y%m%d")
    for day in sorted(day_dirs().keys(), reverse=True):
        if cluster_segments(day):
            return redirect(url_for("app:transcripts.transcripts_day", day=day))
    return redirect(url_for("app:transcripts.transcripts_day", day=today))


@bp.route("/")
def index() -> Any:
    """Root redirect — always to home; the app handles new journals there."""
    return redirect(url_for("app:home.index"))
