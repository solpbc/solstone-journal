# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Root blueprint: access gate and core routes."""

from __future__ import annotations

import logging
import os
import queue
import time
from datetime import date
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _pkg_version
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

from flask import (
    Blueprint,
    Response,
    current_app,
    g,
    jsonify,
    redirect,
    request,
    send_from_directory,
    stream_with_context,
    url_for,
)

from solstone.think.cluster import cluster_segments
from solstone.think.journal_config import (
    JournalConfigMutation,
    ensure_journal_config,
    mutate_journal_config,
)
from solstone.think.journal_io import LockTimeout
from solstone.think.utils import (
    day_dirs,
    get_journal,
    journal_is_active,
)

from . import bridge as convey_bridge
from .config import (
    locked_modify_convey_config,
    seed_default_app_navigation,
)
from .reasons import (
    CONFIG_BUSY,
    CONVEY_OPERATION_FAILED,
    CROSS_ORIGIN_BLOCKED,
    HOST_NOT_ALLOWED,
    IDENTITY_NOT_LOCKED,
    INVALID_CONFIG_VALUE,
    INVALID_OPERATION_FOR_STATE,
    INVALID_REQUEST_VALUE,
    PL_REVOKED,
)
from .secure_listener import get_authorized_clients, start_secure_listener
from .secure_listener.wsgi import CERTLESS_PAIR_ENDPOINTS
from .utils import error_response, error_response_with_reason

logger = logging.getLogger(__name__)


bp = Blueprint(
    "root",
    __name__,
    template_folder="templates",
    static_folder="static",
)


@bp.before_app_request
def require_access() -> Any:
    if request.endpoint is None:
        return None

    if request.endpoint in {
        "root.init",
        "root.init_state",
        "root.init_local_capability",
        "root.init_mark",
        "root.init_mark_regenerate",
        "root.init_mark_lock",
        "root.init_observers",
        "root.init_finalize",
        "static",
        "root.static",
        "root.favicon",
        # Observer ingest endpoints use key-based auth, not session
        "app:observer.ingest_upload",
        "app:observer.ingest_event",
        "app:observer.ingest_segments",
        "app:observer.ingest_manifest",
        "app:observer.ingest_manifest_day",
        "app:observer.register",
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
    if (
        request.endpoint in CERTLESS_PAIR_ENDPOINTS
        and identity is not None
        and identity.mode in {"pl-via-spl", "pl-direct"}
        and identity.fingerprint is None
    ):
        # Cert-less pairing stream; structurally confined to /pair by the C2 gate.
        return None

    if identity is not None and identity.mode in {"pl-direct", "pl-via-spl"}:
        if identity.fingerprint and get_authorized_clients().is_authorized(
            identity.fingerprint
        ):
            return None
        return error_response_with_reason(
            PL_REVOKED,
            detail="paired device revoked",
        )

    # Check setup state
    if not journal_is_active(get_journal()):
        return redirect(url_for("root.init"))

    return None


_LOOPBACK_HOSTS = frozenset({"127.0.0.1", "localhost", "::1"})
_SAME_SITE_FETCH = frozenset({"same-origin", "same-site", "none"})
_STATE_CHANGING_METHODS = frozenset({"POST", "PUT", "PATCH", "DELETE"})
_TRUSTED_OBSERVER_EXTENSION_ORIGIN = (
    "chrome-extension://fgfnkcefedeheoeamppkiiloncfekakf"
)


def _hostname(value: str, *, has_scheme: bool) -> str | None:
    """Return the lowercased, port- and bracket-stripped hostname.

    ``has_scheme=False`` handles a ``Host`` header (``127.0.0.1:5015``,
    ``[::1]:5015``, ``localhost``); ``True`` handles an ``Origin`` URL.
    """
    reference = value if has_scheme else f"//{value}"
    try:
        return urlparse(reference).hostname
    except ValueError:
        return None


def _is_trusted_observer_extension_request() -> bool:
    """The pinned solstone-browser extension calling the observer API.

    Chrome's extension service worker sends its ``chrome-extension://``
    Origin (and may send ``Sec-Fetch-Site: cross-site``) on its loopback
    observer POSTs. Grant exactly the pinned extension id, and only on the
    ``/app/observer`` path namespace, an exemption from the browser
    CSRF/Origin checks. Observer registration gates on trusted loopback or an
    authorized device identity; the other observer routes gate on the record's
    device binding where one exists.
    """
    path = request.path
    if not (path == "/app/observer" or path.startswith("/app/observer/")):
        return False
    return request.headers.get("Origin") == _TRUSTED_OBSERVER_EXTENSION_ORIGIN


@bp.before_app_request
def guard_loopback_origin() -> Any:
    """Reject cross-origin / rebound requests to the local (``dl``) surface.

    The loopback bind is the only control on the ``dl`` surface, and it stops
    neither a same-host browser page (CSRF) nor a DNS-rebound hostname (read
    exfil). Paired (PL) devices authenticate by certificate fingerprint and are
    untouched. Gated on identity mode — every legitimate ``dl`` request is
    loopback-Host with no foreign Origin, so key-authed local ingest and the
    setup wizard pass unchanged.
    """
    identity = getattr(g, "identity", None)
    if identity is None or getattr(identity, "mode", None) != "dl":
        return None

    # Host allowlist (all methods) — a non-loopback Host means DNS rebinding.
    host = _hostname(request.headers.get("Host", ""), has_scheme=False)
    if host not in _LOOPBACK_HOSTS:
        return error_response(HOST_NOT_ALLOWED, detail="host not allowed", status=403)

    # Cross-site guard (state-changing methods) — closes same-host CSRF.
    if request.method in _STATE_CHANGING_METHODS:
        # The pinned observer extension is a trusted local caller; its
        # chrome-extension:// Origin / cross-site Sec-Fetch would otherwise trip
        # the browser CSRF guard. The observer route still gates on loopback.
        if _is_trusted_observer_extension_request():
            return None
        sec_fetch = request.headers.get("Sec-Fetch-Site")
        if sec_fetch is not None and sec_fetch not in _SAME_SITE_FETCH:
            return error_response(
                CROSS_ORIGIN_BLOCKED, detail="cross-site request", status=403
            )
        origin = request.headers.get("Origin")
        if origin and _hostname(origin, has_scheme=True) not in _LOOPBACK_HOSTS:
            return error_response(
                CROSS_ORIGIN_BLOCKED, detail="cross-origin request", status=403
            )

    return None


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


def _build_init_state() -> dict[str, Any]:
    from solstone.apps.thinking.copy import CONFIDENTIAL_LANE_DETAIL, LANES

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
    return {
        "version": version,
        "journal_path": journal_path,
        "identity_name": identity_name,
        "identity_preferred": identity_preferred,
        "retention_mode": retention_mode,
        "retention_days": retention_days,
        "lanes": [dict(lane) for lane in LANES],
        "confidential": {"lane_detail": dict(CONFIDENTIAL_LANE_DETAIL)},
    }


@bp.route("/init/api/state")
def init_state() -> Any:
    return jsonify(_build_init_state())


@bp.route("/init/api/local-capability")
def init_local_capability() -> Any:
    try:
        from solstone.think.check import build_check_report

        result = build_check_report()
    except Exception:
        # Host probing must never break the setup wizard; log loudly and let the
        # browser render the honest unknown state.
        logger.exception("local capability probe failed")
        return jsonify({"overall": "unknown", "checks": []})

    return jsonify(
        {
            "overall": result.report.overall,
            "checks": [
                {
                    "name": check.name,
                    "severity": check.severity,
                    "detail": check.detail,
                }
                for check in result.report.checks
            ],
        }
    )


@bp.route("/init")
def init() -> Any:
    if journal_is_active(get_journal()):
        return redirect(url_for("root.index"))

    # Preserve the setup page's historical config materialization on page load;
    # /init/api/state supplies the rendered values to the static page.
    ensure_journal_config()
    return send_from_directory(Path(__file__).parent / "templates", "init.html")


@bp.route("/init/observers")
def init_observers() -> Any:
    from solstone.apps.observer.presentation import (
        ACTIVE_THRESHOLD_MS,
        STALE_THRESHOLD_MS,
        serialize_observer,
    )
    from solstone.apps.observer.utils import list_observers
    from solstone.think.utils import now_ms

    current_now = now_ms()
    observers_list = []
    for observer in list_observers():
        if observer.get("revoked", False):
            continue
        observers_list.append(serialize_observer(observer, current_now))
    return jsonify(
        {
            "thresholds": {
                "active_ms": ACTIVE_THRESHOLD_MS,
                "stale_ms": STALE_THRESHOLD_MS,
            },
            "observers": observers_list,
        }
    )


@bp.route("/init/mark")
def init_mark() -> Any:
    from solstone.think.link import establish

    if establish.is_committed():
        mark = establish.committed_mark()
        locked = True
    else:
        mark = establish.current_candidate_mark()
        locked = False
    return jsonify({"mark": mark.to_render_spec(), "locked": locked})


@bp.route("/init/mark/regenerate", methods=["POST"])
def init_mark_regenerate() -> Any:
    from solstone.think.link import establish

    if establish.is_committed():
        return error_response_with_reason(
            INVALID_OPERATION_FOR_STATE,
            detail="journal id already locked",
        )
    mark = establish.regenerate_candidate()
    return jsonify({"mark": mark.to_render_spec(), "locked": False})


@bp.route("/init/mark/lock", methods=["POST"])
def init_mark_lock() -> Any:
    from solstone.think.link import establish

    mark = establish.lock_in()
    return jsonify({"mark": mark.to_render_spec(), "locked": True})


@bp.route("/init/finalize", methods=["POST"])
def init_finalize() -> Any:
    from solstone.apps.thinking.copy import LANES
    from solstone.think.link import establish

    if not establish.is_committed():
        return error_response_with_reason(
            IDENTITY_NOT_LOCKED,
            detail="journal id must be locked before setup can finish",
        )

    data = request.get_json(silent=True) or {}
    warnings: list[str] = []
    lane = data.get("lane")
    valid_lanes = {item["id"] for item in LANES}
    if lane is None or lane == "":
        redirect_target = url_for("app:thinking.index")
    elif isinstance(lane, str) and lane in valid_lanes:
        lane_target = f"{lane}-setup"
        redirect_target = url_for("app:thinking.index") + f"#{lane_target}"
    else:
        return error_response(
            INVALID_REQUEST_VALUE,
            detail="lane must be one of: " + ", ".join(sorted(valid_lanes)),
        )

    from solstone.think.utils import now_ms

    retention_mode = data.get("retention_mode", "keep")
    retention_days = data.get("retention_days")
    if retention_mode == "days" and (
        not isinstance(retention_days, int) or retention_days < 1
    ):
        return error_response(
            INVALID_CONFIG_VALUE,
            detail="retention_days must be a positive integer",
        )

    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        changed = False
        convey = config.setdefault("convey", {})
        if "allow_network_access" in convey:
            convey.pop("allow_network_access", None)
            changed = True
        identity = config.setdefault("identity", {})
        for key, value in {
            "name": data.get("name"),
            "preferred": data.get("preferred"),
            "timezone": data.get("timezone"),
        }.items():
            if value and identity.get(key) != value:
                identity[key] = value
                changed = True
        completed_at = now_ms()
        setup = config.setdefault("setup", {})
        if setup.get("completed_at") != completed_at:
            setup["completed_at"] = completed_at
            changed = True
        retention = config.setdefault("retention", {})
        next_retention = {
            "raw_media": retention_mode,
            "raw_media_days": retention_days if retention_mode == "days" else None,
        }
        missing = object()
        for key, value in next_retention.items():
            if retention.get(key, missing) != value:
                retention[key] = value
                changed = True
        return JournalConfigMutation(changed=changed, value=None)

    try:
        mutate_journal_config(apply)
    except LockTimeout:
        return error_response(CONFIG_BUSY, detail="settings are busy; try again")

    def _seed(config: dict[str, Any]) -> dict[str, Any] | None:
        return config if seed_default_app_navigation(config) else None

    try:
        locked_modify_convey_config(_seed)
    except Exception:
        logger.exception("default app navigation seed convey-config PERSIST failed")
        return error_response(
            CONVEY_OPERATION_FAILED,
            detail="Setup was saved, but app navigation could not be initialized.",
            extra={
                "committed": True,
                "secondary_effect": "default_app_navigation",
            },
        )

    try:
        start_secure_listener(current_app._get_current_object())
    except Exception:
        logger.exception("secure listener start after finalize FAILED")
        return error_response(
            CONVEY_OPERATION_FAILED,
            detail="Setup was saved, but secure network access did not start.",
            extra={
                "committed": True,
                "secondary_effect": "secure_listener",
            },
        )

    return jsonify(
        {
            "success": True,
            "redirect": redirect_target,
            "warnings": warnings,
        }
    )


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
