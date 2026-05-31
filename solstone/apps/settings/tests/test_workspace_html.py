# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
from pathlib import Path

WORKSPACE = Path(__file__).resolve().parents[1] / "workspace.html"


def _workspace_text() -> str:
    return WORKSPACE.read_text(encoding="utf-8")


def test_apikeys_inputs_are_masked_by_default():
    text = _workspace_text()
    keys = (
        "GOOGLE_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "REVAI_ACCESS_TOKEN",
        "PLAUD_ACCESS_TOKEN",
    )

    for key in keys:
        match = re.search(rf'<input[^>]*\bdata-key="{key}"[^>]*>', text)
        assert match, f"{key} input not found"
        tag = match.group(0)
        assert 'type="password"' in tag, f"{key} input is not type=password"
        assert 'type="text"' not in tag, f"{key} input still has type=text"


def test_workspace_has_diagnostic_reports_toggle():
    text = _workspace_text()

    assert 'id="field-reporting-enabled"' in text
    assert "diagnostic reports" in text


def test_workspace_unified_provider_panel_replaces_install_regions():
    text = _workspace_text()

    assert 'id="providersPanel"' in text
    assert 'id="bundledProviders"' not in text
    assert 'id="mlxBootstrapRegion"' not in text
    assert 'id="localBootstrapRegion"' not in text
    assert "bundled-provider-grid" not in text
    assert "mlx-bootstrap-region" not in text
    assert "local-bootstrap-region" not in text
    assert "mlx-progress-shell" not in text
    assert "local-progress-shell" not in text
    assert "function startLocalBootstrap()" in text
    assert "function renderProvidersPanel(data)" in text
    assert "function providerCardMeta(state, kind, availability)" in text
    assert "function runProviderAction(providerId, action)" in text
    assert "async function pollProvidersPanel()" in text
    assert "function providerCardOverflow(state, kind)" in text
    assert "const PROVIDER_NAMES = ['anthropic', 'openai', 'local']" in text


def test_workspace_unified_provider_panel_keeps_bootstrap_endpoints_and_polling():
    text = _workspace_text()

    assert "let localBootstrapPostStarted = false" in text
    assert "api/local/bootstrap?model=${model}" in text
    assert "api/providers?local_model=${model}" in text
    assert "api/local/availability?model=${model}" in text
    assert "setInterval(pollProvidersPanel, 1000)" in text
    assert "clearInterval(providersPanelPollTimer)" in text
    assert "IN_FLIGHT_INSTALL_STATES.includes(state.install_state)" in text
    assert "providersPanelActionPending" in text


def test_workspace_provider_names_excludes_openhands():
    text = _workspace_text()

    assert "const PROVIDER_NAMES = ['anthropic', 'openai', 'local']" in text
    assert "'openhands'" not in text


def test_workspace_cloud_cards_have_no_install_affordances():
    text = _workspace_text()

    assert "postProviderAction" not in text
    assert "api/providers/${providerId}" not in text
    assert "cloudInstalledMeta" not in text
    assert "CLI: installed at" not in text


def test_workspace_unified_provider_panel_has_byte_and_blocked_state_paths():
    text = _workspace_text()

    assert "formatMlxBytes(receivedBytes)" in text
    assert "formatMlxBytes(totalBytes)" in text
    assert "function localMlxBlockedReason(state, availability)" in text
    assert "'local runtime is not installed'" in text
    assert "'local model files are not installed'" in text


def test_workspace_cogitate_key_guidance_strings_present():
    text = _workspace_text()

    assert (
        "This provider needs an API key to run agents. Get one at "
        "aistudio.google.com, then add it in API keys below."
    ) in text
    assert (
        "This provider needs an API key to run agents. Get one at "
        "console.anthropic.com, then add it in API keys below."
    ) in text
    assert (
        "This provider needs an API key to run agents. Get one at "
        "platform.openai.com, then add it in API keys below."
    ) in text


def test_workspace_cogitate_auth_control_removed():
    text = _workspace_text()

    assert 'id="field-cogitate-auth"' not in text
    assert "platform account" not in text
    assert "document.getElementById('field-cogitate-auth')" not in text


def test_workspace_security_network_mode_ui_removed_and_link_hint_present():
    text = _workspace_text()
    for removed in (
        'id="conveyNetworkButton"',
        'id="conveyNetworkMode"',
        'id="conveyNetworkDesc"',
        'id="conveyNetworkStatus"',
        'id="conveyPasswordDisclosure"',
        'id="conveyDisclosurePassword"',
        'id="conveyDisclosureConfirm"',
        'id="conveyDisclosureSubmit"',
        'id="conveyDisclosureError"',
        "conveyUiText",
        "renderConveyNetworkState",
        "setConveyNetworkStatus",
        "toggleConveyNetworkAccess",
        "showConveyPasswordDisclosure",
        "submitConveyPasswordDisclosure",
    ):
        assert removed not in text, removed

    assert 'id="conveyLanUrlDisplay"' not in text
    assert 'id="field-host-url"' not in text
    assert "function renderConveyHostFields(" in text
    assert 'id="field-password"' in text
    assert 'id="field-trust-localhost"' in text
    assert 'href="/app/link"' in text
    assert "{{ convey_copy.SETTINGS_SECURITY_REACH_HINT }}" in text


def test_workspace_local_cogitate_status_block_and_unified_panel():
    text = _workspace_text()

    warning_idx = text.index('id="cogitateProviderKeyWarning"')
    status_idx = text.index('id="localCogitateStatus"')
    provider_status_idx = text.index('id="providerStatus"')
    assert warning_idx < status_idx < provider_status_idx
    assert 'id="localCogitateStatus-indicator"' in text
    assert 'id="providersPanel"' in text
    assert 'id="localBootstrapRegion"' not in text
    assert "api/providers/local/status" in text
    assert "api/local/bootstrap?model=${model}" in text
    assert "api/local/availability?model=${model}" in text
    assert "tool-using agents" not in text


def test_workspace_local_model_row_uses_shared_local_install_path():
    text = _workspace_text()

    assert 'id="localModelRow"' in text
    assert 'id="field-local-active-model"' in text
    assert 'id="mlxModelRow"' not in text
    assert 'id="field-mlx-active-model"' not in text
    assert "if (providerId === 'local') return 'local-mlx';" in text
    assert "kind === 'local-mlx'" in text
    assert "function isLocalProviderSelected()" in text
