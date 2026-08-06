# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""The scheduled retention pass, and the confirmation that arms it.

The plate's headline feature had no scheduler, no maintenance routine and no timer, so
an owner's `days` or `processed` setting was owner-settable, UI-rendered and inert.
Arming it is gated on a one-time confirmation of the exact policy, per the founder ruling
of 2026-08-05.

⛔ The property every test here exists for: **an unconfirmed policy deletes nothing, and
a policy the owner has not seen cannot inherit consent given for a different one.**

⚠ These use the REAL executor. A fake would be an oracle this repository controls, and
what needs proving is that the owner's originals are still on disk.
"""

from __future__ import annotations

import json
import os
import shutil
from pathlib import Path

import pytest

from solstone.think import retention_executor as rx

RECORD = {
    "schema": "solstone.processing.v1",
    "state": "analyzed",
    "reason_code": "ok",
    "handler": "transcribe",
    "attempted_at": "2026-01-01T00:00:00Z",
}
RAW = b"the owner's recording"
TODAY = "2026-08-05"
NOW = "2026-08-05T00:00:00Z"


@pytest.fixture(autouse=True)
def _executor(monkeypatch):
    override = os.environ.get("SOLSTONE_RETENTION_BIN")
    if override and os.access(override, os.X_OK):
        monkeypatch.setenv("SOLSTONE_RETENTION_BIN", override)
        return
    found = shutil.which("solstone-retention")
    if found:
        monkeypatch.setenv("SOLSTONE_RETENTION_BIN", found)
        return
    for profile in ("debug", "release"):
        candidate = (
            Path(__file__).resolve().parents[1]
            / "core" / "target" / profile / "solstone-retention"
        )
        if candidate.is_file() and os.access(candidate, os.X_OK):
            monkeypatch.setenv("SOLSTONE_RETENTION_BIN", str(candidate))
            return
    pytest.skip(
        "solstone-retention is not built; the sweep runs through it, so this test has "
        "nothing real to assert against (cargo build -p solstone-core-retention-cli)"
    )


@pytest.fixture
def journal(tmp_path) -> Path:
    """A journal with one old, fully-proven segment."""
    segment = tmp_path / "chronicle" / "20260101" / "field.audio" / "070000_17"
    segment.mkdir(parents=True)
    (segment / "audio.flac").write_bytes(RAW)
    header = {"_solstone_processing": dict(RECORD, input_size=len(RAW))}
    (segment / "audio.jsonl").write_text(
        json.dumps(header) + '\n{"start": 0.0, "text": "hi"}\n'
    )
    (tmp_path / "config").mkdir()
    return tmp_path


def _raw(journal: Path) -> Path:
    return journal / "chronicle/20260101/field.audio/070000_17/audio.flac"


def _derived(journal: Path) -> Path:
    return journal / "chronicle/20260101/field.audio/070000_17/audio.jsonl"


# --- the config-to-policy mapping ------------------------------------------------


def test_keep_is_an_absent_period_not_zero_days():
    """⛔ Zero days means "as soon as the anchor exists", which is not "keep".

    Conflating them is what made `once processing completes` silently never fire.
    """
    assert rx.policy_payload({"raw_media": "keep"})["default_rule"] == {
        "anchor": "captured",
        "period": None,
        "priority": 0,
    }


def test_after_n_days_and_once_processed_both_map():
    assert rx.policy_payload({"raw_media": "days", "raw_media_days": 30})[
        "default_rule"
    ] == {"anchor": "captured", "period": 30, "priority": 0}
    assert rx.policy_payload({"raw_media": "processed"})["default_rule"] == {
        "anchor": "processed",
        "period": 0,
        "priority": 0,
    }


@pytest.mark.parametrize(
    "retention",
    [
        {"raw_media": "days"},
        {"raw_media": "days", "raw_media_days": 0},
        {"raw_media": "days", "raw_media_days": -5},
        {"raw_media": "days", "raw_media_days": "soon"},
        {"raw_media": "days", "raw_media_days": None},
        {"raw_media": "nonsense"},
        {},
    ],
)
def test_every_malformed_policy_keeps(retention):
    """⛔ Falling off the end must KEEP. Not release immediately."""
    payload = rx.policy_payload(retention)
    assert payload["default_rule"]["period"] is None, payload
    assert not rx.policy_would_release(payload)


def test_a_per_stream_rule_reaches_the_payload():
    payload = rx.policy_payload(
        {
            "raw_media": "keep",
            "per_stream": {"field.audio": {"mode": "days", "days": 14}},
        }
    )
    assert payload["per_stream"] == [
        ["field.audio", {"anchor": "captured", "period": 14, "priority": 0}]
    ]
    assert rx.policy_would_release(payload), "a per-stream rule alone can release"


def test_a_minimum_age_reaches_the_payload():
    assert (
        rx.policy_payload(
            {"raw_media": "days", "raw_media_days": 1, "raw_media_minimum_days": 30}
        )["minimum_age"]
        == 30
    )


# --- the confirmation gate ------------------------------------------------------


def test_an_unconfirmed_policy_reports_and_deletes_nothing(journal):
    """🔴 The ruling. Nothing is deleted until the owner confirms THIS policy."""
    with pytest.raises(rx.SweepNotConfirmed) as pending:
        rx.scheduled_sweep(
            str(journal),
            {"raw_media": "days", "raw_media_days": 7},
            today=TODAY,
            now=NOW,
        )
    plan = pending.value.plan["plan"]
    assert plan["candidates"] == 1
    assert plan["bytes"] == len(RAW)
    assert _raw(journal).exists(), "an unconfirmed policy must delete nothing"


def test_the_pending_plan_carries_real_numbers_from_this_journal(journal):
    """The owner is asked about their own data, not an abstraction."""
    with pytest.raises(rx.SweepNotConfirmed) as pending:
        rx.scheduled_sweep(
            str(journal), {"raw_media": "processed"}, today=TODAY, now=NOW
        )
    plan = pending.value.plan["plan"]
    assert plan["files"] == 1
    assert plan["segments"][0]["files"] == [
        "chronicle/20260101/field.audio/070000_17/audio.flac"
    ]


def test_a_confirmed_policy_releases_raw_and_keeps_derived(journal):
    retention = {"raw_media": "days", "raw_media_days": 7}
    fingerprint = rx.policy_fingerprint(retention)
    assert rx.record_confirmation(
        fingerprint, at=NOW, journal_path=str(journal)
    ), "the confirmation is recorded"

    from solstone.think.journal_config import read_journal_config

    stored = read_journal_config(str(journal))["retention"][rx.CONFIRM_KEY]
    assert stored["policy"] == fingerprint

    rx.scheduled_sweep(
        str(journal), dict(retention, **{rx.CONFIRM_KEY: stored}), today=TODAY, now=NOW
    )
    assert not _raw(journal).exists(), "the original is released"
    assert _derived(journal).exists(), "the transcript is what the owner keeps"


def test_a_changed_policy_does_not_inherit_the_old_confirmation(journal):
    """⛔ Consent for a 7-day release is not consent for a 1-day one."""
    retention = {"raw_media": "days", "raw_media_days": 7}
    fingerprint = rx.policy_fingerprint(retention)
    rx.record_confirmation(fingerprint, at=NOW, journal_path=str(journal))
    confirmed = dict(
        retention, **{rx.CONFIRM_KEY: {"policy": fingerprint, "at": NOW}}
    )

    tightened = dict(confirmed, raw_media_days=1)
    with pytest.raises(rx.SweepNotConfirmed):
        rx.scheduled_sweep(str(journal), tightened, today=TODAY, now=NOW)
    assert _raw(journal).exists()

    # ⚠ And a change in the SAFER direction re-asks too. Simpler to reason about than
    # "more destructive", and it fails toward asking rather than toward deleting.
    loosened = dict(confirmed, raw_media_days=90)
    with pytest.raises(rx.SweepNotConfirmed):
        rx.scheduled_sweep(str(journal), loosened, today=TODAY, now=NOW)


def test_a_keep_everything_policy_needs_no_confirmation(journal):
    """Confirming a policy that authorises nothing would train the owner to click."""
    result = rx.scheduled_sweep(
        str(journal), {"raw_media": "keep"}, today=TODAY, now=NOW
    )
    assert result["executed"] is False
    assert result["plan"]["candidates"] == 0
    assert _raw(journal).exists()


def test_the_fingerprint_ignores_unrelated_config_churn():
    """An unrelated setting changing must not invalidate consent."""
    base = {"raw_media": "days", "raw_media_days": 7}
    assert rx.policy_fingerprint(base) == rx.policy_fingerprint(
        dict(base, some_other_setting="changed")
    )


def test_the_fingerprint_covers_every_field_that_changes_behaviour():
    base = {"raw_media": "days", "raw_media_days": 7}
    for changed in (
        {"raw_media": "processed"},
        {"raw_media_days": 8},
        {"raw_media_minimum_days": 30},
        {"per_stream": {"field.audio": {"mode": "days", "days": 1}}},
    ):
        assert rx.policy_fingerprint(base) != rx.policy_fingerprint(
            dict(base, **changed)
        ), changed


def test_recording_the_same_confirmation_twice_changes_nothing(journal):
    fingerprint = rx.policy_fingerprint({"raw_media": "days", "raw_media_days": 7})
    assert rx.record_confirmation(fingerprint, at=NOW, journal_path=str(journal))
    assert not rx.record_confirmation(fingerprint, at=NOW, journal_path=str(journal))


# --- the plan/execute split -----------------------------------------------------


def test_planning_a_sweep_deletes_nothing(journal):
    payload = rx.policy_payload({"raw_media": "days", "raw_media_days": 7})
    result = rx.sweep(str(journal), payload, today=TODAY, now=NOW, execute=False)
    assert result["executed"] is False
    assert result["plan"]["candidates"] == 1
    assert _raw(journal).exists(), "planning must be read-only"


def test_a_young_segment_is_not_swept(journal):
    """The policy's own gate still applies once confirmed."""
    payload = rx.policy_payload({"raw_media": "days", "raw_media_days": 7})
    result = rx.sweep(
        str(journal), payload, today="2026-01-02", now="2026-01-02T00:00:00Z"
    )
    assert result["plan"]["candidates"] == 0
    assert result["plan"]["skipped"] == 1, "reported as held, not dropped"


def test_an_unproven_segment_is_never_swept(tmp_path):
    """⛔ Age cannot overrule missing proof, however old."""
    segment = tmp_path / "chronicle" / "20200101" / "field.audio" / "070000_17"
    segment.mkdir(parents=True)
    (segment / "audio.flac").write_bytes(RAW)  # no sidecar at all

    payload = rx.policy_payload({"raw_media": "days", "raw_media_days": 7})
    result = rx.sweep(str(tmp_path), payload, today=TODAY, now=NOW, execute=True)
    assert result["detail"]["plan"]["candidates"] == 0
    assert (segment / "audio.flac").exists(), "six years old and unproven is held"
