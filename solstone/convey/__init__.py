# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Web interface for navigating and interacting with journal data."""

from __future__ import annotations

import logging
import os
from datetime import timedelta
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from flask import Flask

logger = logging.getLogger(__name__)

__all__ = [
    "create_app",
    "emit",
    "install_api_error_handlers",
]


def __getattr__(name: str):
    # PEP 562: resolve `emit` lazily so a bare `import solstone.convey.state`
    # does not drag bridge/callosum (and the rest of the web stack) into
    # sys.modules. The AttributeError for every other name is load-bearing:
    # it lets `from solstone.convey import <submodule>` fall through to
    # normal submodule import.
    if name == "emit":
        from .bridge import emit

        return emit
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


def install_identity_stamper(app: Flask) -> None:
    from flask import g, request

    from solstone.convey.secure_listener import ConveyIdentity

    @app.before_request
    def _stamp_identity() -> None:
        stamped = request.environ.get("pl.identity")
        if stamped is not None:
            g.identity = stamped
            return
        g.identity = ConveyIdentity(
            mode="dl",
            fingerprint=None,
            device_label=None,
            paired_at=None,
            session_id=None,
        )


def install_api_error_handlers(app: Flask) -> None:
    """Guarantee JSON error envelopes for every API path."""
    from flask import Response, g, request
    from werkzeug.exceptions import HTTPException, InternalServerError

    from solstone.think.utils import CorruptConfigError

    from .reasons import CORRUPT_CONFIG, HTTP_ERROR, INTERNAL_ERROR
    from .utils import error_response

    def _is_api_request() -> bool:
        return "api" in request.path.strip("/").split("/")

    @app.errorhandler(CorruptConfigError)
    def _handle_corrupt_config(exc: CorruptConfigError):
        if not _is_api_request():
            return Response(str(exc), status=500, mimetype="text/plain")
        return error_response(CORRUPT_CONFIG, detail=str(exc))

    @app.errorhandler(HTTPException)
    def _handle_http_exception(exc: HTTPException):
        if not _is_api_request():
            return exc

        if isinstance(exc, InternalServerError) and exc.original_exception is not None:
            original = exc.original_exception
            logger.error(
                "unhandled API exception request_id=%s path=%s",
                getattr(g, "request_id", ""),
                request.path,
                exc_info=(type(original), original, original.__traceback__),
            )
            return error_response(INTERNAL_ERROR)

        return error_response(HTTP_ERROR, status=exc.code or HTTP_ERROR.status)


def create_app(journal: str = "") -> Flask:
    """Create and configure the Convey Flask application."""
    from flask import Flask
    from jinja2 import ChoiceLoader, FileSystemLoader

    from solstone.apps import AppRegistry
    from solstone.think.contract.journal import build_bundle
    from solstone.think.link.runtime import start_link_runtime
    from solstone.think.push.runtime import start_push_runtime
    from solstone.think.voice.runtime import start_voice_runtime

    from . import state, system
    from .apps import register_app_context
    from .chat import chat_bp, start_chat_runtime
    from .config import bp as config_bp
    from .health import bp as health_bp
    from .ledger import bp as ledger_bp
    from .profile import bp as profile_bp
    from .profile import profiles_bp
    from .push import push_bp
    from .request_id import install_request_id_stamper
    from .root import bp as root_bp
    from .shell_api import create_shell_api_blueprint
    from .voice import voice_bp

    app = Flask(
        __name__,
        template_folder=os.path.join(os.path.dirname(__file__), "templates"),
        static_folder=os.path.join(os.path.dirname(__file__), "static"),
    )

    install_api_error_handlers(app)

    # Add apps directory to template search path so apps can have their templates
    # in apps/{name}/workspace.html instead of needing a templates/ subfolder
    convey_templates = os.path.join(os.path.dirname(__file__), "templates")
    apps_root = os.path.join(os.path.dirname(os.path.dirname(__file__)), "apps")
    app.jinja_loader = ChoiceLoader(
        [
            FileSystemLoader(convey_templates),
            FileSystemLoader(apps_root),
        ]
    )

    app.config["SEND_FILE_MAX_AGE_DEFAULT"] = timedelta(seconds=300)
    app.config.setdefault("SECURE_LISTENER_ENABLED", False)
    # Build once so observer ingest never masks a broken at-rest contract; a
    # missing layout.json previously turned every ingest into a runtime 500.
    try:
        app.config["JOURNAL_CONTRACT_BUNDLE"] = build_bundle()
    except Exception as exc:
        raise RuntimeError(
            f"Journal at-rest contract failed to load at startup: {exc}"
        ) from exc
    install_identity_stamper(app)
    install_request_id_stamper(app)

    # Register root blueprint (/, favicon)
    app.register_blueprint(root_bp)

    # Register config API blueprint
    app.register_blueprint(config_bp)

    # Register chat API blueprint (universal chat bar)
    app.register_blueprint(chat_bp)

    # Register system health API blueprint
    app.register_blueprint(system.bp)

    # Register ledger + profile tool-group API blueprints
    app.register_blueprint(ledger_bp)
    app.register_blueprint(profile_bp)
    app.register_blueprint(profiles_bp)

    # Register data-trust health API blueprint
    app.register_blueprint(health_bp)

    # Register voice API blueprint
    app.register_blueprint(voice_bp)

    # Register push API blueprint
    app.register_blueprint(push_bp)

    # Initialize and register app system
    registry = AppRegistry()
    registry.discover()
    registry.register_blueprints(app)
    app.register_blueprint(create_shell_api_blueprint(registry))
    # One deliberate legacy alias: shipped iOS clients pair against /app/link/*.
    # Serve the SAME view objects at the legacy prefix under endpoint names
    # app:link.* so the cert-less gate's app:link.pair references keep resolving.
    # Single-app special case — NOT a generic per-app alias framework.
    network_bp = registry.apps["network"].blueprint
    app.register_blueprint(network_bp, name="app:link", url_prefix="/app/link")

    # Register app system context processors
    register_app_context(app, registry)

    start_voice_runtime(app)
    if os.environ.get("SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES") == "1":
        app.push_runtime_started = False
        app.chat_runtime_started = False
        app.link_runtime_started = False
    else:
        start_push_runtime(app)
        start_chat_runtime(app)
        start_link_runtime(app)

    if journal:
        state.journal_root = journal
    return app
