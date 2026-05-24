# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import threading

import pytest
from werkzeug.serving import make_server

from solstone.apps.settings import install_copy
from solstone.convey import create_app

pytestmark = pytest.mark.integration


@pytest.fixture
def live_settings_server(settings_env):
    journal_path, config = settings_env()
    config["setup"] = {"completed_at": "2026-05-23T00:00:00Z"}
    config.setdefault("convey", {})["trust_localhost"] = True
    (journal_path / "config" / "journal.json").write_text(
        json.dumps(config, indent=2) + "\n",
        encoding="utf-8",
    )
    app = create_app(str(journal_path))
    app.config["TESTING"] = True
    server = make_server("127.0.0.1", 0, app, threaded=True)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        thread.join(timeout=5)


def test_providers_panel_static_render(live_settings_server, page):
    page.goto(f"{live_settings_server}/app/settings/")
    page.locator("#tab-providers").click()
    page.wait_for_selector("#providersPanel", state="visible")
    page.wait_for_function("document.querySelectorAll('.provider-card').length === 5")

    cards = page.locator(".provider-card")
    assert cards.count() == 5
    for provider in ("anthropic", "openai", "openhands", "local", "mlx"):
        assert page.locator(f'.provider-card[data-provider="{provider}"]').count() == 1

    install_copy_values = {getattr(install_copy, name) for name in install_copy.__all__}
    failed_prefix = install_copy.INSTALL_PHASE_FAILED_PREFIX
    badges = page.locator(".provider-card__badge")
    for index in range(badges.count()):
        text = badges.nth(index).inner_text()
        assert text in install_copy_values or text.startswith(failed_prefix)

    assert page.locator("#bundledProviders").count() == 0
    assert page.locator("#mlxBootstrapRegion").count() == 0
    assert page.locator("#localBootstrapRegion").count() == 0
