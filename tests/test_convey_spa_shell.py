# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import pytest

from solstone.apps import AppRegistry
from solstone.apps.thinking.copy import CONFIDENTIAL_LANE_DETAIL, LANES
from solstone.convey import create_app

REPO_ROOT = Path(__file__).resolve().parents[1]
APPS_ROOT = REPO_ROOT / "solstone" / "apps"
CONVEY_STATIC = REPO_ROOT / "solstone" / "convey" / "static"
SHELL_BOOT_MENU_HARNESS = REPO_ROOT / "tests" / "js" / "shell_boot_menu_harness.js"
JINJA_MARKERS = ("{{", "{%", "{#")
BOOT_PATH_FILES = [
    CONVEY_STATIC / "shell.html",
    CONVEY_STATIC / "shell_gate.js",
    CONVEY_STATIC / "shell_boot.js",
    CONVEY_STATIC / "mount-workspace.js",
    CONVEY_STATIC / "date_format.js",
    CONVEY_STATIC / "chat_chrome.js",
    CONVEY_STATIC / "status_pane.js",
    CONVEY_STATIC / "modal_layer.js",
    CONVEY_STATIC / "presentation_mode.js",
    CONVEY_STATIC / "menu_state.js",
]


@pytest.fixture
def convey_app(journal_copy):
    return create_app(journal=str(journal_copy))


@pytest.fixture
def client(convey_app):
    return convey_app.test_client()


def _assert_construct_free(path: Path) -> None:
    text = path.read_text(encoding="utf-8")
    for marker in JINJA_MARKERS:
        assert marker not in text, f"{path} contains {marker}"


def test_api_shell_shape_ordering_label_and_backgrounds(client, journal_copy):
    config_path = journal_copy / "config" / "journal.json"
    config = json.loads(config_path.read_text(encoding="utf-8"))
    config["agent"] = {"name": "Ada", "name_status": "chosen"}
    config_path.write_text(json.dumps(config, indent=2), encoding="utf-8")

    response = client.get("/api/shell")

    assert response.status_code == 200
    payload = response.get_json()
    apps = payload["apps"]
    by_name = {app["name"]: app for app in apps}

    assert apps[0]["name"] == "home"
    assert [app["name"] for app in apps[1:3]] == ["activities", "entities"]
    assert by_name["sol"]["label"] == "Ada"
    assert "search" not in by_name
    assert by_name["network"]["workspace_url"] == "/app/network/workspace"
    assert by_name["timeline"]["background_url"] == "/app/timeline/background"
    assert by_name["support"]["background_url"] == "/app/support/background"
    assert set(payload["chat_bar"]) == {"placeholder", "attention", "sol_request"}
    assert set(payload["settings"]) == {"reporting_enabled"}


def test_api_shell_chat_seed_degrades_to_defaults(client, monkeypatch):
    def raise_chat_events(_day):
        raise RuntimeError("boom")

    monkeypatch.setattr("solstone.think.awareness.get_current", lambda: {})
    monkeypatch.setattr("solstone.think.utils.day_dirs", lambda: [])
    monkeypatch.setattr(
        "solstone.convey.chat_stream.read_chat_events", raise_chat_events
    )

    response = client.get("/api/shell")

    assert response.status_code == 200
    chat_bar = response.get_json()["chat_bar"]
    assert chat_bar == {
        "placeholder": "send a message…",
        "attention": None,
        "sol_request": None,
    }


def test_network_spa_index_and_workspace(client):
    response = client.get("/app/network/")
    assert response.status_code == 200
    assert b'data-solstone-shell="spa"' in response.data

    workspace_response = client.get("/app/network/workspace")
    assert workspace_response.status_code == 200
    assert (
        workspace_response.data
        == (APPS_ROOT / "network" / "workspace.html").read_bytes()
    )


def test_background_routes_are_verbatim(client):
    for app_name in ("timeline", "support"):
        response = client.get(f"/app/{app_name}/background")
        assert response.status_code == 200
        assert response.data == (APPS_ROOT / app_name / "background.html").read_bytes()


def test_construct_free_spa_workspaces_backgrounds_and_shell_boot_path():
    registry = AppRegistry()
    registry.discover()
    for app in registry.apps.values():
        _assert_construct_free(APPS_ROOT / app.name / "workspace.html")
    for path in APPS_ROOT.glob("*/background.html"):
        _assert_construct_free(path)
    for path in BOOT_PATH_FILES:
        _assert_construct_free(path)


def test_convey_templates_dir_is_construct_free():
    templates_dir = REPO_ROOT / "solstone" / "convey" / "templates"
    files = sorted(p for p in templates_dir.iterdir() if p.is_file())
    assert files, "convey/templates/ should not be empty"
    for path in files:
        _assert_construct_free(path)


def test_assert_construct_free_flags_jinja(tmp_path):
    bad = tmp_path / "bad.html"
    bad.write_text("<div>{{ x }}</div>", encoding="utf-8")
    with pytest.raises(AssertionError):
        _assert_construct_free(bad)


def test_init_template_construct_free_and_state_matches(client, journal_copy):
    _assert_construct_free(
        REPO_ROOT / "solstone" / "convey" / "templates" / "init.html"
    )

    response = client.get("/init/api/state")

    assert response.status_code == 200
    payload = response.get_json()
    config = json.loads((journal_copy / "config" / "journal.json").read_text())
    assert payload["journal_path"] == str(journal_copy)
    assert payload["identity_name"] == config["identity"]["name"]
    assert payload["identity_preferred"] == config["identity"]["preferred"]
    assert payload["retention_mode"] == config.get("retention", {}).get(
        "raw_media", "keep"
    )
    assert payload["lanes"] == [dict(lane) for lane in LANES]
    assert payload["confidential"] == {"lane_detail": dict(CONFIDENTIAL_LANE_DETAIL)}
    assert set(payload) == {
        "version",
        "journal_path",
        "identity_name",
        "identity_preferred",
        "retention_mode",
        "retention_days",
        "lanes",
        "confidential",
    }


def test_init_state_pre_setup_returns_200(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    config_dir = journal / "config"
    config_dir.mkdir(parents=True)
    (config_dir / "journal.json").write_text("{}", encoding="utf-8")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    app = create_app(journal=str(journal))
    response = app.test_client().get("/init/api/state")

    assert response.status_code == 200


def test_date_format_js_matches_python_literals():
    node = shutil.which("node")
    if not node:
        pytest.skip("node not available")

    script = f"""
global.window = global;
const fs = require('fs');
const vm = require('vm');
vm.runInThisContext(fs.readFileSync({str(CONVEY_STATIC / "date_format.js")!r}, 'utf8'));
const now = new Date(2026, 6, 6);
const cases = {{
  '20260706': 'Today',
  '20260705': 'Yesterday',
  '20260707': 'Tomorrow',
  '20260701': 'Wednesday',
  '20260101': 'Thu Jan 1',
  '20251129': "Sat Nov 29 '25",
  'bad': 'bad'
}};
for (const [input, expected] of Object.entries(cases)) {{
  const actual = window.formatDateShort(input, now);
  if (actual !== expected) {{
    throw new Error(`${{input}} expected ${{expected}} got ${{actual}}`);
  }}
}}
    """
    subprocess.run([node, "-e", script], check=True)


def test_shell_boot_menu_hrefs_are_canonical_app_roots():
    node = shutil.which("node")
    if not node:
        pytest.skip("node not available")

    result = subprocess.run(
        [node, str(SHELL_BOOT_MENU_HARNESS), str(REPO_ROOT)],
        capture_output=True,
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    payload = json.loads(result.stdout)
    assert payload["shellReady"] is True
    assert payload["mounted"] == [{"url": "/app/home/workspace", "appName": "home"}]
    assert payload["hrefs"] == [
        "/app/home/",
        "/app/backup/",
        "/app/odd&lt;&amp;&quot;name/",
    ]
