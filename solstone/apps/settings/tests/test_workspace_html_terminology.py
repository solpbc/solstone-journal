# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
from pathlib import Path

from solstone.apps.settings import install_copy

WORKSPACE = Path(__file__).resolve().parents[1] / "workspace.html"
LEGACY_TERMS = (
    "Downloading...",
    "Verifying...",
    "Installing...",
    "Validating key...",
    "Install may be stuck",
    "Retry?",
    "'enabling'",
    "'key-validating'",
    "'install-failed'",
    "'installed-no-key'",
    "'invalid-key'",
    "'not-enabled'",
    "stuck_enabling",
    "state.state",
)
UNIQUE_INSTALL_COPY_NAMES = (
    "INSTALL_PHASE_RESOLVING",
    "INSTALL_PHASE_DOWNLOADING",
    "INSTALL_PHASE_VERIFYING",
    "INSTALL_PHASE_INSTALLING",
    "INSTALL_PHASE_FAILED_PREFIX",
    "INSTALL_FAILED_NO_PROGRESS",
    "INSTALL_FAILED_UV_MISSING",
    "INSTALL_BUTTON_INSTALLING",
)


def _workspace_text() -> str:
    return WORKSPACE.read_text(encoding="utf-8")


def _without_install_copy_declaration(text: str) -> str:
    return "\n".join(
        line for line in text.splitlines() if "const INSTALL_COPY = " not in line
    )


def test_workspace_has_no_legacy_install_state_terms():
    text = _workspace_text()

    for term in LEGACY_TERMS:
        assert term not in text


def test_workspace_provider_iteration_has_single_source_of_truth():
    text = _workspace_text()
    provider_names = "const PROVIDER_NAMES = ['anthropic', 'openai', 'local']"
    removed_provider = "'" + "mlx" + "'"

    assert provider_names in text
    assert text.count(provider_names) == 1
    assert removed_provider not in text


def test_provider_card_overflow_has_no_hosted_install_actions():
    text = _workspace_text()
    match = re.search(
        r"function providerCardOverflow\(state, kind\) \{(?P<body>.*?)"
        r"function runProviderAction",
        text,
        re.DOTALL,
    )

    assert match is not None
    body = match.group("body")
    assert "'Uninstall'" not in body
    assert "'Disable'" not in body
    assert "'Enable'" not in body


def test_workspace_does_not_duplicate_install_copy_strings():
    text = _without_install_copy_declaration(_workspace_text())

    for name in UNIQUE_INSTALL_COPY_NAMES:
        value = getattr(install_copy, name)
        assert text.count(value) == 0
