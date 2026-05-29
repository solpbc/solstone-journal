# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
from pathlib import Path

WORKSPACE = Path(__file__).resolve().parents[1] / "workspace.html"
MLX_SCOPE_COPY = (
    "vision and text on your device. "
    "tool-calling agents continue to use your configured cogitate provider."
)
MLX_UNAVAILABLE_REASONS = [
    "not running on macOS",
    "not running on Apple Silicon",
    "insufficient RAM (need 16 GB, have 8 GB)",
    "mlx-vlm package not installed",
]


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


def test_workspace_mlx_excluded_from_cogitate_but_present_for_generate():
    text = _workspace_text()

    assert "for (const type of ['generate', 'cogitate'])" in text
    assert "for (const suffix of ['provider', 'backup'])" in text
    assert "if (type === 'cogitate' && p.name === 'mlx') continue;" in text
    assert "opt.value = p.name" in text
    assert "provider.name !== 'mlx'" in text


def test_workspace_mlx_unavailable_option_disabled_title_and_suffix():
    text = _workspace_text()

    assert "opt.disabled = true" in text
    assert "opt.title = disabledReason" in text
    assert "return availability.reason || 'MLX unavailable'" in text
    assert "label += ' — unavailable'" in text
    for reason in MLX_UNAVAILABLE_REASONS:
        assert reason


def test_workspace_mlx_model_identifier_uses_active_model():
    text = _workspace_text()

    assert 'id="mlxModelIdentifier"' in text
    assert 'id="field-mlx-active-model"' in text
    assert "function getSelectedMlxModel()" in text
    assert "providersData?.mlx?.active_model" in text
    assert "model_present" in text
    assert "model: ${getSelectedMlxModel()}" in text


def test_workspace_mlx_model_picker_labels_are_pinned():
    text = _workspace_text()

    assert "qwen 3.5 — 16 GB Mac" in text
    assert "gemma 4 (26B) — 24 GB Mac" in text
    assert text.index('id="field-mlx-active-model"') < text.index(
        'id="field-cogitate-provider"'
    )


def test_workspace_mlx_v1_scope_copy_is_generate_gated():
    text = _workspace_text()

    assert MLX_SCOPE_COPY in text
    assert "provider === 'mlx' ? '' : 'none'" in text


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
    assert "function startMlxBootstrap()" in text
    assert "function startLocalBootstrap()" in text
    assert "function renderProvidersPanel(data)" in text
    assert "function providerCardMeta(state, kind, availability)" in text
    assert "function runProviderAction(providerId, action)" in text
    assert "async function pollProvidersPanel()" in text
    assert "function providerCardOverflow(state, kind)" in text
    assert "const PROVIDER_NAMES = ['anthropic', 'openai', 'local', 'mlx']" in text


def test_workspace_unified_provider_panel_keeps_bootstrap_endpoints_and_polling():
    text = _workspace_text()

    assert "let mlxBootstrapPostStarted = false" in text
    assert "let localBootstrapPostStarted = false" in text
    assert "api/mlx/bootstrap?model=${model}" in text
    assert "api/local/bootstrap?model=${model}" in text
    assert "api/providers?local_model=${model}" in text
    assert "api/mlx/availability?model=${model}" in text
    assert "api/local/availability?model=${model}" in text
    assert "setInterval(pollProvidersPanel, 1000)" in text
    assert "clearInterval(providersPanelPollTimer)" in text
    assert "IN_FLIGHT_INSTALL_STATES.includes(state.install_state)" in text
    assert "providersPanelActionPending" in text


def test_workspace_provider_names_excludes_openhands():
    text = _workspace_text()

    assert "const PROVIDER_NAMES = ['anthropic', 'openai', 'local', 'mlx']" in text
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
    assert "availability.reason === 'model snapshot not present'" in text
    assert "'local runtime is not installed'" in text
    assert "'local model files are not installed'" in text


def test_workspace_mlx_bootstrap_handlers_do_not_write_generate_provider():
    text = _workspace_text()
    function_names = [
        "maybeAutoStartMlxBootstrap",
        "startMlxBootstrap",
        "pollProvidersPanel",
    ]

    for name in function_names:
        match = re.search(
            rf"(?:async )?function {name}\b[\s\S]*?"
            rf"(?=\n(?:async )?function |\n// ==========|\Z)",
            text,
        )
        assert match, name
        assert not re.search(
            r"(field-generate-provider|#field-generate-provider)[^\n;]*"
            r"(?:\.value|\.selectedIndex)\s*=",
            match.group(0),
        )


def test_workspace_mlx_cogitate_unsupported_branch_removed():
    text = _workspace_text()

    assert "MLX_COGITATE_UNSUPPORTED_TITLE" not in text
    assert "if (type === 'cogitate') return MLX_COGITATE_UNSUPPORTED_TITLE" not in text


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


def test_workspace_local_and_mlx_model_rows_are_shared():
    text = _workspace_text()

    assert 'id="mlxModelRow"' in text
    assert 'id="localModelRow"' in text
    assert 'id="field-mlx-active-model"' in text
    assert 'id="field-local-active-model"' in text
    assert "document.getElementById('field-generate-provider')?.value === 'mlx'" in text
    assert "function isLocalProviderSelected()" in text
