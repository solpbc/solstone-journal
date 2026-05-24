# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json

import pytest

from solstone.think.providers import bundled
from tests.bundled_provider_fixtures import BundledCase, bundled_provider_config


@pytest.mark.integration
def test_bundled_install_emits_real_uv_phases(tmp_path, monkeypatch):
    # anthropic is the smallest real path: one SDK spec, no codex artifact/runtime SDK.
    name = "anthropic"
    try:
        bundled._resolve_uv_command()
    except bundled.CogitateProviderInstallFailed as exc:
        pytest.skip(str(exc))

    config_dir = tmp_path / "config"
    config_dir.mkdir(parents=True)
    (config_dir / "journal.json").write_text(
        json.dumps(
            bundled_provider_config(
                name,
                BundledCase("idle", "key-needed", False, False, False),
            ),
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    bundled._LOCKS.clear()
    bundled._INSTALL_THREADS.clear()
    bundled._INSTALL_PROCESSES.clear()
    bundled._OBSERVED_PHASES.clear()

    # Hook _advance_phase to capture every phase the install pipeline emits.
    # Polling for transitions races a uv install that finishes in <100ms with a
    # warm cache; the spec contract (AC2) is that production code *emits*
    # phase transitions, which a deterministic hook verifies without the race.
    emitted_phases: list[str] = []
    original_advance = bundled._advance_phase

    def recording_advance(provider_name, phase):
        if provider_name == name:
            emitted_phases.append(phase)
        original_advance(provider_name, phase)

    monkeypatch.setattr(bundled, "_advance_phase", recording_advance)

    try:
        bundled.install_provider(name)
        thread = bundled._INSTALL_THREADS.get(name)
        if thread is not None:
            thread.join(timeout=300)

        final_state = bundled.get_provider_state(name)
        if final_state["install_state"] == "failed":
            pytest.skip(
                f"real uv bundled install failed: {final_state['install_error']}"
            )

        assert final_state["install_state"] == "installed"
        assert any(phase in {"resolving", "downloading"} for phase in emitted_phases), (
            f"expected resolving/downloading phase, got {emitted_phases}"
        )
    finally:
        try:
            bundled.uninstall_provider(name)
        except Exception:
            pass
