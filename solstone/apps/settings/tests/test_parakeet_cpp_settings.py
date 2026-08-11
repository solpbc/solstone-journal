# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import re
from pathlib import Path

from solstone.convey import create_app

WORKSPACE = Path(__file__).resolve().parents[1] / "workspace.html"
NOTICES = Path(__file__).resolve().parents[3] / "THIRD_PARTY_NOTICES.md"


def _workspace_text() -> str:
    return WORKSPACE.read_text(encoding="utf-8")


def _parakeet_cpp_fieldset() -> str:
    text = _workspace_text()
    match = re.search(
        r'<fieldset id="parakeet-cpp-settings".*?</fieldset>',
        text,
        re.DOTALL,
    )
    assert match is not None
    return match.group(0)


def _client(journal_path: Path):
    app = create_app(str(journal_path))
    app.config["TESTING"] = True
    return app.test_client()


def test_workspace_contains_parakeet_cpp_device_fieldset() -> None:
    fieldset = _parakeet_cpp_fieldset()

    assert 'id="parakeet-cpp-settings"' in fieldset
    assert 'id="field-parakeet-cpp-device"' in fieldset
    options = re.findall(r'<option value="([^"]+)"', fieldset)
    assert options == ["auto", "cpu"]
    assert "cuda" not in options
    assert "vulkan" not in options


def test_workspace_wires_parakeet_cpp_population_switch_and_save() -> None:
    text = _workspace_text()

    assert "let parakeetUsesCpp = false;" in text
    assert "parakeetUsesCpp = Boolean(data.parakeet_uses_cpp)" in text
    assert "const parakeetCpp = transcribe['parakeet-cpp'] || {};" in text
    assert "setValue('field-parakeet-cpp-device', parakeetCpp.device || 'auto')" in text
    assert "document.getElementById('parakeet-cpp-settings').style.display" in text
    assert (
        "backend === 'parakeet-cpp' || (backend === 'parakeet' && parakeetUsesCpp)"
        in text
    )
    assert "backend === 'parakeet' && !parakeetUsesCpp" in text
    assert "data: { 'parakeet-cpp': { device: value } }" in text


def test_transcribe_payload_marks_linux_parakeet_cpp_runtime(
    settings_env, monkeypatch
) -> None:
    from solstone.apps.settings import routes

    journal_path, config = settings_env()
    config["setup"] = {"completed_at": 1700000000000}
    (journal_path / "config" / "journal.json").write_text(
        json.dumps(config, indent=2) + "\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(routes.platform, "system", lambda: "Linux")
    monkeypatch.setattr(routes.platform, "machine", lambda: "x86_64")
    client = _client(journal_path)

    response = client.get("/app/settings/api/transcribe")

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["runtime_label"] == "Linux parakeet.cpp"
    assert payload["parakeet_uses_cpp"] is True


def test_parakeet_cpp_device_round_trips_through_settings_config(settings_env) -> None:
    journal_path, config = settings_env()
    config["setup"] = {"completed_at": 1700000000000}
    (journal_path / "config" / "journal.json").write_text(
        json.dumps(config, indent=2) + "\n",
        encoding="utf-8",
    )
    client = _client(journal_path)

    response = client.put(
        "/app/settings/api/config",
        json={"section": "transcribe", "data": {"parakeet-cpp": {"device": "cpu"}}},
    )

    assert response.status_code == 200
    payload = client.get("/app/settings/api/config").get_json()
    assert payload["transcribe"]["parakeet-cpp"]["device"] == "cpu"

    unknown = client.put(
        "/app/settings/api/config",
        json={"section": "transcribe", "data": {"parakeet-cpp": {"threads": 8}}},
    )

    assert unknown.status_code == 200
    payload = client.get("/app/settings/api/config").get_json()
    assert payload["transcribe"]["parakeet-cpp"] == {"device": "cpu"}


def test_parakeet_cpp_notices() -> None:
    notices = NOTICES.read_text(encoding="utf-8")
    section = notices.split("## runtime-downloaded provider artifacts (parakeet-cpp)")[
        1
    ].split("## WeSpeaker ResNet34 / VoxCeleb")[0]

    assert "mudler/parakeet.cpp" in section
    assert "mudler/parakeet-cpp-gguf" in section
    assert "MIT" in section
    assert "CC-BY-4.0" in section
    assert "Downloaded file: tdt-0.6b-v3-q8_0.gguf" in section
    assert "Bundled file: tdt-0.6b-v3-q8_0.gguf" not in section
