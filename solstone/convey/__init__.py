# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Web interface for navigating and interacting with journal data."""

from __future__ import annotations

import json
import os
from datetime import timedelta
from pathlib import Path

from flask import Flask, g, request
from jinja2 import ChoiceLoader, FileSystemLoader

from solstone.apps import AppRegistry
from solstone.convey.secure_listener import ConveyIdentity
from solstone.think.utils import ensure_journal_config

from . import state, system
from .apps import register_app_context
from .bridge import emit
from .chat import chat_bp, start_chat_runtime
from .config import bp as config_bp
from .request_id import install_request_id_stamper
from .root import bp as root_bp

__all__ = [
    "create_app",
    "emit",
]


def _get_or_create_secret() -> str:
    """Load convey.secret from journal.json, generating one if absent."""
    config = ensure_journal_config()
    return config["convey"]["secret"]


def _migrate_password_hash() -> None:
    """Migrate plaintext convey.password to hashed password_hash."""
    from werkzeug.security import generate_password_hash

    from solstone.think.utils import get_config, get_journal

    config = get_config()
    convey = config.get("convey", {})

    if "password_hash" in convey or "password" not in convey:
        return

    plaintext = convey.pop("password")
    if plaintext:
        convey["password_hash"] = generate_password_hash(plaintext)

    config["convey"] = convey
    config_path = Path(get_journal()) / "config" / "journal.json"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    with open(config_path, "w", encoding="utf-8") as f:
        json.dump(config, f, indent=2, ensure_ascii=False)
        f.write("\n")
    os.chmod(config_path, 0o600)


def _migrate_setup_completed() -> None:
    """Infer setup.completed_at and set trust_localhost for existing installs.

    Legacy migration: handles journals where password_hash was set via
    'sol password set' CLI before web onboarding existed. Web onboarding
    now writes all config atomically in init_finalize(), so this path is
    only reached for pre-existing journals.
    """
    from solstone.think.utils import get_config, get_journal

    config = get_config()

    if not config.get("convey", {}).get("password_hash"):
        return
    if config.get("setup", {}).get("completed_at"):
        return

    from solstone.think.utils import now_ms

    config.setdefault("setup", {})["completed_at"] = now_ms()
    config.setdefault("convey", {})["trust_localhost"] = True

    config_path = Path(get_journal()) / "config" / "journal.json"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    with open(config_path, "w", encoding="utf-8") as f:
        json.dump(config, f, indent=2, ensure_ascii=False)
        f.write("\n")
    os.chmod(config_path, 0o600)


def install_identity_stamper(app: Flask) -> None:
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


def create_app(journal: str = "") -> Flask:
    """Create and configure the Convey Flask application."""
    from solstone.think.link.runtime import start_link_runtime
    from solstone.think.push.runtime import start_push_runtime
    from solstone.think.voice.runtime import start_voice_runtime

    from .push import push_bp
    from .voice import voice_bp

    app = Flask(
        __name__,
        template_folder=os.path.join(os.path.dirname(__file__), "templates"),
        static_folder=os.path.join(os.path.dirname(__file__), "static"),
    )

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

    app.secret_key = _get_or_create_secret()
    _migrate_password_hash()
    _migrate_setup_completed()
    app.config["PERMANENT_SESSION_LIFETIME"] = timedelta(days=30)
    app.config.setdefault("SECURE_LISTENER_ENABLED", False)
    install_identity_stamper(app)
    install_request_id_stamper(app)

    # Register root blueprint (login, logout, /, favicon)
    app.register_blueprint(root_bp)

    # Register config API blueprint
    app.register_blueprint(config_bp)

    # Register chat API blueprint (universal chat bar)
    app.register_blueprint(chat_bp)

    # Register system health API blueprint
    app.register_blueprint(system.bp)

    # Register voice API blueprint
    app.register_blueprint(voice_bp)

    # Register push API blueprint
    app.register_blueprint(push_bp)

    # Initialize and register app system
    registry = AppRegistry()
    registry.discover()
    registry.register_blueprints(app)

    # Register app system context processors
    register_app_context(app, registry)

    start_voice_runtime(app)
    start_push_runtime(app)
    start_chat_runtime(app)
    start_link_runtime(app)

    if journal:
        state.journal_root = journal
    return app
