# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import pytest

from solstone.think.providers import PROVIDER_METADATA, build_provider_status


def test_google_provider_metadata_has_no_cogitate_cli() -> None:
    assert "cogitate_cli" not in PROVIDER_METADATA["google"]
    assert PROVIDER_METADATA["google"]["cogitate_runtime"] == "openhands"


@pytest.mark.parametrize(
    (
        "api_key",
        "vertex_creds_configured",
        "configured",
        "generate_ready",
        "cogitate_ready",
        "issues",
    ),
    [
        ("", False, False, False, False, ["GOOGLE_API_KEY not set"]),
        ("key", False, True, True, True, []),
        ("", True, True, True, False, ["GOOGLE_API_KEY not set for cogitate"]),
    ],
)
def test_google_provider_status_ignores_gemini_path(
    monkeypatch,
    api_key: str,
    vertex_creds_configured: bool,
    configured: bool,
    generate_ready: bool,
    cogitate_ready: bool,
    issues: list[str],
) -> None:
    if api_key:
        monkeypatch.setenv("GOOGLE_API_KEY", api_key)
    else:
        monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
    monkeypatch.setattr(
        "solstone.think.providers.shutil.which",
        lambda _name: (_ for _ in ()).throw(AssertionError("which should not run")),
    )
    monkeypatch.setattr(
        "solstone.think.providers.bundled.get_provider_state",
        lambda _name: {
            "install_state": "installed",
            "key_status": "not-applicable",
            "disabled": False,
            "issues": [],
        },
    )

    status = build_provider_status(
        [{"name": "google", "env_key": "GOOGLE_API_KEY"}],
        vertex_creds_configured=vertex_creds_configured,
    )["google"]

    assert status == {
        "configured": configured,
        "generate_ready": generate_ready,
        "cogitate_ready": cogitate_ready,
        "cogitate_cli": "openhands-sdk",
        "cogitate_cli_found": True,
        "issues": issues,
    }
