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
MLX_COGITATE_UNSUPPORTED_TITLE = (
    "MLX provider does not support cogitate in v1 — it is vision/generate-only. "
    "Configure a cloud provider for cogitate agents."
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


def test_workspace_mlx_option_present_in_generate_backup_and_cogitate_lists():
    text = _workspace_text()

    assert "for (const type of ['generate', 'cogitate'])" in text
    assert "for (const suffix of ['provider', 'backup'])" in text
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
        'id="mlxBootstrapProgressShell"'
    )


def test_workspace_mlx_v1_scope_copy_is_generate_gated():
    text = _workspace_text()

    assert MLX_SCOPE_COPY in text
    assert "provider === 'mlx' ? '' : 'none'" in text


def test_workspace_mlx_progress_region_has_progress_bar():
    text = _workspace_text()

    assert 'id="mlxBootstrapRegion"' in text
    assert 'id="mlxBootstrapProgress"' in text
    assert "<progress" in text


def test_workspace_mlx_progress_region_has_byte_readout():
    text = _workspace_text()

    assert 'id="mlxBootstrapBytes"' in text
    assert "formatMlxBytes(received)" in text
    assert "formatMlxBytes(total)" in text


def test_workspace_mlx_progress_region_has_state_text():
    text = _workspace_text()

    assert 'id="mlxBootstrapState"' in text
    assert "downloading: 'Downloading...'" in text
    assert "verifying: 'Verifying...'" in text


def test_workspace_mlx_retry_button_is_failed_only():
    text = _workspace_text()

    assert 'id="mlxBootstrapRetry"' in text
    assert "retry.style.display = state === 'failed' ? '' : 'none'" in text
    assert "retry.disabled = !!status?.bootstrap_disabled" in text
    assert "retry.onclick" in text
    assert "mountMlxProgress()" in text


def test_workspace_mlx_polling_starts_and_stops_on_terminal_states():
    text = _workspace_text()

    assert "let mlxBootstrapPollTimer = null" in text
    assert "let mlxBootstrapPostStarted = false" in text
    assert "api/mlx/bootstrap?model=${model}" in text
    assert "api/mlx/bootstrap/status?model=${model}" in text
    assert "api/mlx/availability?model=${model}" in text
    assert "setInterval(pollMlxBootstrap, 1000)" in text
    assert "clearInterval(mlxBootstrapPollTimer)" in text
    assert "state === 'installed'" in text
    assert "state === 'failed'" in text
    assert "unmountMlxProgress()" in text


def test_workspace_mlx_bootstrap_handlers_do_not_write_generate_provider():
    text = _workspace_text()
    function_names = [
        "syncMlxProgressRegion",
        "mountMlxProgress",
        "unmountMlxProgress",
        "pollMlxBootstrap",
        "handleMlxBootstrapStatus",
    ]

    for name in function_names:
        match = re.search(
            rf"function {name}\b[\s\S]*?(?=\nfunction |\n// ==========|\Z)",
            text,
        )
        assert match, name
        assert not re.search(
            r"(field-generate-provider|#field-generate-provider)[^\n;]*"
            r"(?:\.value|\.selectedIndex)\s*=",
            match.group(0),
        )


def test_workspace_mlx_cogitate_option_disabled_with_runtime_message():
    text = _workspace_text()

    assert MLX_COGITATE_UNSUPPORTED_TITLE in text
    assert "if (type === 'cogitate') return MLX_COGITATE_UNSUPPORTED_TITLE" in text


def test_workspace_ollama_cogitate_status_block_and_copy():
    text = _workspace_text()

    warning_idx = text.index('id="cogitateProviderKeyWarning"')
    status_idx = text.index('id="ollamaCogitateStatus"')
    provider_status_idx = text.index('id="providerStatus"')
    assert warning_idx < status_idx < provider_status_idx
    assert 'id="ollamaCogitateStatus-indicator"' in text
    assert "curl -fsSL https://opencode.ai/install | bash" in text
    assert "tool-using agents" not in text
