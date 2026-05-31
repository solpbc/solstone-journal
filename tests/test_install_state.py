# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import get_args

import pytest

from solstone.apps.settings.install_copy import (
    INSTALL_BUTTON_INSTALL,
    INSTALL_BUTTON_INSTALLING,
    INSTALL_BUTTON_RETRY,
    INSTALL_FAILED_FALLBACK,
    INSTALL_FAILED_NO_PROGRESS,
    INSTALL_FAILED_UV_MISSING,
    INSTALL_PHASE_DOWNLOADING,
    INSTALL_PHASE_FAILED_PREFIX,
    INSTALL_PHASE_IDLE,
    INSTALL_PHASE_INSTALLED,
    INSTALL_PHASE_INSTALLING,
    INSTALL_PHASE_RESOLVING,
    INSTALL_PHASE_VERIFYING,
)
from solstone.think.providers.install_state import (
    IN_FLIGHT_STATES,
    TERMINAL_STATES,
    InstallState,
    InstallStatus,
    bump_progress,
    is_stalled,
    make_idle_status,
    read_install_status,
    transition_state,
    write_install_status,
)


@pytest.fixture
def journal_config(tmp_path, monkeypatch):
    def _write(config: dict) -> Path:
        config_path = tmp_path / "config" / "journal.json"
        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
        return config_path

    return _write


def _status(
    *,
    state: InstallState,
    last_progress_at: str | None = None,
    received: int | None = None,
    total: int | None = None,
) -> InstallStatus:
    return {
        "name": "x",
        "install_state": state,
        "last_transition_at": "2026-05-23T00:00:00+00:00",
        "last_progress_at": last_progress_at,
        "progress_bytes_received": received,
        "progress_bytes_total": total,
        "install_error": None,
    }


def test_install_state_literal_membership_and_ordering():
    assert get_args(InstallState) == (
        "idle",
        "resolving",
        "downloading",
        "verifying",
        "installing",
        "installed",
        "failed",
    )


def test_install_state_partitions_cover_literal_without_overlap():
    install_states = set(get_args(InstallState))
    assert IN_FLIGHT_STATES | TERMINAL_STATES == install_states
    assert IN_FLIGHT_STATES & TERMINAL_STATES == set()


def test_make_idle_status_returns_documented_shape():
    assert make_idle_status("x") == {
        "name": "x",
        "install_state": "idle",
        "last_transition_at": None,
        "last_progress_at": None,
        "progress_bytes_received": None,
        "progress_bytes_total": None,
        "install_error": None,
    }


def test_transition_state_covers_chain_retry_and_counter_lifecycle():
    idle = make_idle_status("anthropic")
    resolving = transition_state(idle, new_state="resolving")
    assert idle == make_idle_status("anthropic")
    assert resolving["install_state"] == "resolving"
    assert resolving["last_transition_at"] is not None
    assert resolving["last_progress_at"] == resolving["last_transition_at"]

    with_progress = bump_progress(resolving, received=12, total=100)
    downloading = transition_state(with_progress, new_state="downloading")
    assert downloading["install_state"] == "downloading"
    assert downloading["last_progress_at"] == downloading["last_transition_at"]
    assert downloading["progress_bytes_received"] == 12
    assert downloading["progress_bytes_total"] == 100

    verifying = transition_state(downloading, new_state="verifying")
    assert verifying["install_state"] == "verifying"
    assert verifying["last_progress_at"] == verifying["last_transition_at"]
    assert verifying["progress_bytes_received"] == 12
    assert verifying["progress_bytes_total"] == 100

    installing = transition_state(verifying, new_state="installing")
    assert installing["install_state"] == "installing"
    assert installing["last_progress_at"] == installing["last_transition_at"]
    assert installing["progress_bytes_received"] == 12
    assert installing["progress_bytes_total"] == 100

    installed = transition_state(installing, new_state="installed")
    assert installed["install_state"] == "installed"
    assert installed["last_progress_at"] is None
    assert installed["progress_bytes_received"] is None
    assert installed["progress_bytes_total"] is None

    failed = transition_state(
        installing,
        new_state="failed",
        error="download checksum mismatch",
    )
    assert failed["install_state"] == "failed"
    assert failed["install_error"] == "download checksum mismatch"
    assert failed["last_progress_at"] is None
    assert failed["progress_bytes_received"] is None
    assert failed["progress_bytes_total"] is None

    retry = transition_state(failed, new_state="resolving")
    assert retry["install_state"] == "resolving"
    assert retry["install_error"] is None
    assert retry["last_progress_at"] == retry["last_transition_at"]


def test_bump_progress_updates_in_flight_statuses_partially():
    status = _status(
        state="downloading",
        last_progress_at="2026-05-23T00:00:00+00:00",
    )
    bumped = bump_progress(status, received=10, total=20)
    assert bumped is not status
    assert status["progress_bytes_received"] is None
    assert bumped["last_progress_at"] != status["last_progress_at"]
    assert bumped["progress_bytes_received"] == 10
    assert bumped["progress_bytes_total"] == 20

    received_only = bump_progress(bumped, received=15)
    assert received_only["progress_bytes_received"] == 15
    assert received_only["progress_bytes_total"] == 20

    total_only = bump_progress(received_only, total=30)
    assert total_only["progress_bytes_received"] == 15
    assert total_only["progress_bytes_total"] == 30

    neither = bump_progress(total_only)
    assert neither["progress_bytes_received"] == 15
    assert neither["progress_bytes_total"] == 30

    for state in TERMINAL_STATES:
        with pytest.raises(ValueError):
            bump_progress(_status(state=state))


def test_is_stalled_obeys_state_timestamp_and_strict_boundary():
    now = datetime(2026, 5, 23, 12, 0, 0, tzinfo=timezone.utc)
    stale = (now - timedelta(seconds=70)).isoformat()
    recent = (now - timedelta(seconds=30)).isoformat()

    for state in TERMINAL_STATES:
        assert (
            is_stalled(_status(state=state, last_progress_at=stale), now=now) is False
        )

    for state in IN_FLIGHT_STATES:
        assert is_stalled(_status(state=state, last_progress_at=None), now=now) is False
        assert (
            is_stalled(_status(state=state, last_progress_at=recent), now=now) is False
        )
        assert is_stalled(_status(state=state, last_progress_at=stale), now=now) is True

    exactly_threshold = (now - timedelta(seconds=60.000)).isoformat()
    just_past_threshold = (now - timedelta(seconds=60.001)).isoformat()
    assert (
        is_stalled(
            _status(state="downloading", last_progress_at=exactly_threshold), now=now
        )
        is False
    )
    assert (
        is_stalled(
            _status(state="downloading", last_progress_at=just_past_threshold),
            now=now,
        )
        is True
    )


def test_read_install_status_returns_idle_for_empty_legacy_and_invalid_slots(
    journal_config,
):
    journal_config({"providers": {"bundled": {}}})
    assert read_install_status(scope="bundled", name="anthropic") == make_idle_status(
        "anthropic"
    )

    journal_config(
        {
            "providers": {
                "bundled": {
                    "anthropic": {
                        "state": "enabling",
                        "binary_path": "/foo",
                    }
                }
            }
        }
    )
    assert read_install_status(scope="bundled", name="anthropic") == make_idle_status(
        "anthropic"
    )

    journal_config(
        {"providers": {"bundled": {"anthropic": {"install_state": "enabling"}}}}
    )
    assert read_install_status(scope="bundled", name="anthropic") == make_idle_status(
        "anthropic"
    )


def test_write_install_status_preserves_existing_non_contract_fields(journal_config):
    config_path = journal_config(
        {
            "providers": {
                "bundled": {
                    "anthropic": {
                        "binary_path": "/foo",
                        "sdk_spec": {"package": "openai-codex-sdk"},
                    }
                }
            }
        }
    )
    status = transition_state(make_idle_status("anthropic"), new_state="installed")
    write_install_status(status, scope="bundled")

    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    slot = persisted["providers"]["bundled"]["anthropic"]
    assert slot["binary_path"] == "/foo"
    assert slot["sdk_spec"] == {"package": "openai-codex-sdk"}
    assert "name" not in slot
    assert slot["install_state"] == "installed"
    assert slot["last_transition_at"] == status["last_transition_at"]
    assert slot["last_progress_at"] is None
    assert slot["install_error"] is None


def test_write_install_status_preserves_two_providers_in_same_scope(journal_config):
    config_path = journal_config({})
    anthropic = transition_state(make_idle_status("anthropic"), new_state="installed")
    openai = transition_state(
        make_idle_status("openai"),
        new_state="failed",
        error="missing key",
    )

    write_install_status(anthropic, scope="bundled")
    write_install_status(openai, scope="bundled")

    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    bundled = persisted["providers"]["bundled"]
    assert bundled["anthropic"]["install_state"] == "installed"
    assert bundled["openai"]["install_state"] == "failed"
    assert bundled["openai"]["install_error"] == "missing key"


def test_write_install_status_creates_bundled_key_chain(journal_config):
    config_path = journal_config({"providers": {}})
    status = transition_state(make_idle_status("local"), new_state="installing")

    write_install_status(status, scope="bundled")

    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    assert persisted["providers"]["bundled"]["local"]["install_state"] == "installing"


def test_progress_byte_counters_persist_and_round_trip(journal_config):
    config_path = journal_config({})
    status = bump_progress(
        transition_state(make_idle_status("anthropic"), new_state="downloading"),
        received=5,
        total=10,
    )

    write_install_status(status, scope="bundled")

    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    slot = persisted["providers"]["bundled"]["anthropic"]
    assert slot["progress_bytes_received"] == 5
    assert slot["progress_bytes_total"] == 10

    read_back = read_install_status(scope="bundled", name="anthropic")
    assert read_back["progress_bytes_received"] == 5
    assert read_back["progress_bytes_total"] == 10

    partial_status = bump_progress(
        transition_state(make_idle_status("openai"), new_state="downloading"),
        received=7,
    )
    write_install_status(partial_status, scope="bundled")

    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    partial_slot = persisted["providers"]["bundled"]["openai"]
    assert partial_slot["progress_bytes_received"] == 7
    assert partial_slot["progress_bytes_total"] is None

    partial_read_back = read_install_status(scope="bundled", name="openai")
    assert partial_read_back["progress_bytes_received"] == 7
    assert partial_read_back["progress_bytes_total"] is None

    terminal_status = transition_state(partial_status, new_state="installed")
    write_install_status(terminal_status, scope="bundled")

    persisted = json.loads(config_path.read_text(encoding="utf-8"))
    terminal_slot = persisted["providers"]["bundled"]["openai"]
    assert terminal_slot["progress_bytes_received"] is None
    assert terminal_slot["progress_bytes_total"] is None

    terminal_read_back = read_install_status(scope="bundled", name="openai")
    assert terminal_read_back["progress_bytes_received"] is None
    assert terminal_read_back["progress_bytes_total"] is None


def test_install_copy_constants_match_spec_and_typography():
    assert INSTALL_PHASE_IDLE == "Not installed"
    assert INSTALL_PHASE_RESOLVING == "Resolving dependencies…"
    assert INSTALL_PHASE_DOWNLOADING == "Downloading…"
    assert INSTALL_PHASE_VERIFYING == "Verifying…"
    assert INSTALL_PHASE_INSTALLING == "Installing…"
    assert INSTALL_PHASE_INSTALLED == "Installed"
    assert INSTALL_PHASE_FAILED_PREFIX == "Install failed — "
    assert INSTALL_FAILED_FALLBACK == "try again"
    assert INSTALL_FAILED_NO_PROGRESS == "no progress for 60 seconds — try again"
    assert (
        INSTALL_FAILED_UV_MISSING
        == "uv not found — install uv (https://github.com/astral-sh/uv) and retry"
    )
    assert INSTALL_BUTTON_INSTALL == "Install"
    assert INSTALL_BUTTON_INSTALLING == "Installing…"
    assert INSTALL_BUTTON_RETRY == "Try again"

    assert "…" in INSTALL_PHASE_RESOLVING and "..." not in INSTALL_PHASE_RESOLVING
    assert "…" in INSTALL_PHASE_DOWNLOADING and "..." not in INSTALL_PHASE_DOWNLOADING
    assert "…" in INSTALL_PHASE_VERIFYING and "..." not in INSTALL_PHASE_VERIFYING
    assert "…" in INSTALL_PHASE_INSTALLING and "..." not in INSTALL_PHASE_INSTALLING
    assert "…" in INSTALL_BUTTON_INSTALLING and "..." not in INSTALL_BUTTON_INSTALLING
    assert "—" in INSTALL_PHASE_FAILED_PREFIX and "-" not in INSTALL_PHASE_FAILED_PREFIX
    assert "—" in INSTALL_FAILED_NO_PROGRESS
    assert "—" in INSTALL_FAILED_UV_MISSING
