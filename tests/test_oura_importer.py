# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Read-only Python differential tests for Oura parse/normalize parity."""

from __future__ import annotations

import json
import os
import stat
import time
from contextlib import contextmanager
from pathlib import Path
from zoneinfo import ZoneInfo

import pytest

from solstone.apps.body.routes import _iter_normalized_rows
from solstone.think.importers import oura
from solstone.think.importers.file_importer import (
    FILE_IMPORTER_REGISTRY,
    get_file_importer,
)
from solstone.think.importers.health_schema import (
    SOURCE_APPLE_HEALTH,
    SOURCE_OURA_API,
    HealthRecordIdentity,
    health_record_dedupe_key,
)
from solstone.think.importers.pre_save_gate import (
    APPROVAL_SCHEMA,
    CHECKLIST_DESTINATIONS,
    CHECKLIST_VERSION,
    OURA_SYNC_APPROVAL_SCHEMA,
    OURA_SYNC_CHECKLIST_VERSION,
    SENSITIVE_IMPORTERS,
    RawRetentionDecision,
    approval_path_for_journal,
    oura_sync_approval_path_for_journal,
)
from solstone.think.importers.sync import SYNCABLE_REGISTRY, get_syncable_backends

FIXTURE_ROOT = (
    Path(__file__).parent / "fixtures" / "importers" / "health" / "oura_synthetic"
)
REVISION_ROOT = FIXTURE_ROOT / "revisions"
APPLE_FIXTURE_ROOT = (
    Path(__file__).parent
    / "fixtures"
    / "importers"
    / "health"
    / "apple_health_synthetic"
)

# Fixture bundle shape: 29 documents across 14 endpoints; each readiness
# document also splits out a temperature-deviation row -> 31 rows.
_FIXTURE_ROW_COUNT = 31
# Sync runs fetch SYNC_ENDPOINTS only: the partner-gated blood_glucose
# fixture (4 rows) is parse/normalize-only, never polled.
_SYNC_ROW_COUNT = _FIXTURE_ROW_COUNT - 4
_SCHEDULED_VALID_UNTIL = "2099-01-01T00:00:00Z"


def _use_journal(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    journal = tmp_path / "journal"
    journal.mkdir()
    config_path = journal / "config" / "journal.json"
    config_path.parent.mkdir()
    config_path.write_text(
        json.dumps({"identity": {"timezone": "America/Denver"}}),
        encoding="utf-8",
    )
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    return journal


def _raw_retention(decision: str = RawRetentionDecision.RETAIN_PARSED.value) -> dict:
    return {
        "decision": decision,
        "notes": "Synthetic test decision.",
    }


def _scheduled_sync_consent() -> dict:
    return {
        "approved": True,
        "cadence": "every 6 hours",
        "valid_until": _SCHEDULED_VALID_UNTIL,
    }


def _valid_artifact(
    journal: Path,
    importers: list[str],
    *,
    raw_retention_decision: str = RawRetentionDecision.RETAIN_PARSED.value,
) -> dict:
    return {
        "schema": APPROVAL_SCHEMA,
        "checklist_version": CHECKLIST_VERSION,
        "approved_by": "Jack",
        "approved_at": "2026-07-05T00:00:00-06:00",
        "journal_root": str(journal.resolve()),
        "approved_importers": importers,
        "replication_destinations": {
            destination: {
                "decision": "approved" if destination == "time_machine" else "excluded",
                "notes": "Synthetic test decision.",
            }
            for destination in CHECKLIST_DESTINATIONS
        },
        "raw_retention": _raw_retention(raw_retention_decision),
        "requires_per_run_confirmation": True,
        "no_real_health_data_in_artifact": True,
    }


def _write_artifact(journal: Path, payload: dict) -> Path:
    approval_path = approval_path_for_journal(journal)
    approval_path.parent.mkdir(parents=True)
    approval_path.write_text(json.dumps(payload), encoding="utf-8")
    return approval_path


def _sync_artifact(
    journal: Path,
    *,
    raw_retention_decision: str = RawRetentionDecision.RETAIN_PARSED.value,
) -> dict:
    return {
        "schema": OURA_SYNC_APPROVAL_SCHEMA,
        "checklist_version": OURA_SYNC_CHECKLIST_VERSION,
        "approved_by": "Jack",
        "approved_at": "2026-07-06T09:00:00-06:00",
        "journal_root": str(journal.resolve()),
        "replication_destinations": {
            destination: {
                "decision": "approved" if destination == "time_machine" else "excluded",
                "notes": "Synthetic test decision.",
            }
            for destination in CHECKLIST_DESTINATIONS
        },
        "raw_retention": _raw_retention(raw_retention_decision),
        "requires_per_run_confirmation": True,
        "no_real_health_data_in_artifact": True,
    }


def _write_sync_artifact(journal: Path, payload: dict) -> Path:
    approval_path = oura_sync_approval_path_for_journal(journal)
    approval_path.parent.mkdir(parents=True, exist_ok=True)
    approval_path.write_text(json.dumps(payload), encoding="utf-8")
    return approval_path


_APPROVAL_ONLY = [
    "imports/_approvals",
    "imports/_approvals/health_import_preflight.json",
]

_SYNC_APPROVAL_ONLY = [
    "imports/_approvals",
    "imports/_approvals/oura_sync_preflight.json",
]


@contextmanager
def _temporary_umask(mask: int):
    old = os.umask(mask)
    try:
        yield
    finally:
        os.umask(old)


def _mode(path: Path) -> int:
    return stat.S_IMODE(path.stat().st_mode)


def _wait_for_path(path: Path, *, timeout: float = 5.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            return
        time.sleep(0.02)
    raise AssertionError(f"timed out waiting for {path}")


def _subprocess_env() -> dict[str, str]:
    env = os.environ.copy()
    repo_root = Path(__file__).resolve().parents[1]
    existing = env.get("PYTHONPATH")
    env["PYTHONPATH"] = (
        str(repo_root) if not existing else f"{repo_root}{os.pathsep}{existing}"
    )
    return env


def _imports_contents(journal: Path) -> list[str]:
    imports_dir = journal / "imports"
    if not imports_dir.exists():
        return []
    return sorted(
        p.relative_to(journal).as_posix()
        for p in imports_dir.rglob("*")
        # sqlite WAL sidecars appear whenever a connection opens the dedupe
        # ledger (even read-only) and vanish on checkpoint — connection
        # artifacts, not import writes.
        if not p.name.endswith(("-shm", "-wal"))
    )


# Registration and gate membership
# ---------------------------------------------------------------------------


def test_oura_registered_as_file_importer():
    assert "oura" in FILE_IMPORTER_REGISTRY
    assert get_file_importer("oura") is not None


def test_oura_is_a_sensitive_importer():
    assert "oura" in SENSITIVE_IMPORTERS


def test_oura_is_not_a_python_sync_backend():
    assert "oura" not in SYNCABLE_REGISTRY
    assert all(backend.name != "oura" for backend in get_syncable_backends())


# ---------------------------------------------------------------------------
# Parse layer
# ---------------------------------------------------------------------------


def test_parse_bundle_reads_all_supported_endpoint_files():
    bundle = oura.parse_oura_bundle(FIXTURE_ROOT)

    assert set(bundle) == {
        "daily_sleep",
        "daily_readiness",
        "daily_resilience",
        "daily_stress",
        "daily_spo2",
        "sleep",
        "daily_activity",
        "heartrate",
        "daily_cardiovascular_age",
        "blood_glucose",
        "workout",
        "session",
        "enhanced_tag",
        "vO2_max",
    }
    assert len(bundle["daily_sleep"]) == 2
    assert len(bundle["sleep"]) == 2
    assert len(bundle["daily_stress"]) == 1
    assert len(bundle["daily_activity"]) == 2
    assert len(bundle["heartrate"]) == 4
    assert len(bundle["daily_cardiovascular_age"]) == 2
    assert len(bundle["blood_glucose"]) == 4
    assert len(bundle["workout"]) == 2
    assert len(bundle["session"]) == 2
    assert len(bundle["enhanced_tag"]) == 2
    assert len(bundle["vO2_max"]) == 2


def test_parse_single_endpoint_file():
    bundle = oura.parse_oura_bundle(FIXTURE_ROOT / "daily_readiness.json")

    assert set(bundle) == {"daily_readiness"}
    assert bundle["daily_readiness"][0]["id"] == "synthetic-readiness-2026-01-02"


def test_parse_rejects_unknown_endpoint():
    with pytest.raises(oura.OuraDocumentError, match="Unsupported Oura endpoint"):
        oura.parse_endpoint_document("daily_unicorns", {"data": []})


def test_parse_rejects_document_without_data_list():
    with pytest.raises(oura.OuraDocumentError, match="'data' list"):
        oura.parse_endpoint_document("daily_sleep", {"items": []})


def test_parse_rejects_item_missing_id_or_day():
    with pytest.raises(oura.OuraDocumentError, match="missing 'id' or 'day'"):
        oura.parse_endpoint_document(
            "daily_sleep", {"data": [{"id": "x", "score": 10}]}
        )


def test_parse_heartrate_requires_timestamp_and_bpm():
    with pytest.raises(oura.OuraDocumentError, match="missing 'timestamp' or 'bpm'"):
        oura.parse_endpoint_document("heartrate", {"data": [{"bpm": 60}]})
    with pytest.raises(oura.OuraDocumentError, match="missing 'timestamp' or 'bpm'"):
        oura.parse_endpoint_document(
            "heartrate", {"data": [{"timestamp": "2026-01-02T03:15:00-07:00"}]}
        )


def test_parse_blood_glucose_requires_timestamp_and_glucose():
    # Pins the assumed row shape for the undocumented blood_glucose series
    # (see _SERIES_REQUIRED_FIELDS): if the live API names its value field
    # differently, the first post-reauthorization fetch fails loudly here.
    with pytest.raises(
        oura.OuraDocumentError, match="missing 'timestamp' or 'glucose'"
    ):
        oura.parse_endpoint_document("blood_glucose", {"data": [{"glucose": 92}]})
    with pytest.raises(
        oura.OuraDocumentError, match="missing 'timestamp' or 'glucose'"
    ):
        oura.parse_endpoint_document(
            "blood_glucose", {"data": [{"timestamp": "2026-01-02T15:05:00Z"}]}
        )


def test_parse_enhanced_tag_requires_id_and_start_day():
    # enhanced_tag is the one document endpoint with no `day` field
    # (openapi-1.35 EnhancedTagModel): its day attribution field is
    # `start_day`, enforced through _DOCUMENT_DAY_FIELDS.
    with pytest.raises(oura.OuraDocumentError, match="missing 'id' or 'start_day'"):
        oura.parse_endpoint_document(
            "enhanced_tag",
            {"data": [{"id": "tag-1", "start_time": "2026-01-02T14:45:12-07:00"}]},
        )
    with pytest.raises(oura.OuraDocumentError, match="missing 'id' or 'start_day'"):
        oura.parse_endpoint_document(
            "enhanced_tag", {"data": [{"start_day": "2026-01-02"}]}
        )
    # The other granted-scope endpoints validate on the plain id+day rule.
    with pytest.raises(oura.OuraDocumentError, match="missing 'id' or 'day'"):
        oura.parse_endpoint_document("workout", {"data": [{"id": "workout-1"}]})
    with pytest.raises(oura.OuraDocumentError, match="missing 'id' or 'day'"):
        oura.parse_endpoint_document(
            "vO2_max", {"data": [{"id": "vo2-1", "vo2_max": 41}]}
        )


def test_parse_oura_day_normalizes_and_rejects():
    assert oura.parse_oura_day("2026-01-02") == "20260102"
    assert oura.parse_oura_day("2026-13-02") is None
    assert oura.parse_oura_day(20260102) is None


# ---------------------------------------------------------------------------
# Normalization
# ---------------------------------------------------------------------------


def _normalized_items() -> list[oura.OuraNormalizedItem]:
    bundle = oura.parse_oura_bundle(FIXTURE_ROOT)
    return oura.normalize_bundle(
        bundle,
        import_id="20260105_000000",
        raw_ref_root="imports/20260105_000000/raw/oura",
        owner_timezone=ZoneInfo("America/Denver"),
    )


def test_normalize_bundle_rows_carry_schema_family_and_days():
    items = _normalized_items()

    assert len(items) == _FIXTURE_ROW_COUNT
    for item in items:
        assert item.row["schema"] == oura.NORMALIZED_SCHEMA
        assert item.row["source_family"] == SOURCE_OURA_API
        assert item.row["dedupe_key"].startswith("sha256:")
        assert item.row["day"] in {"20260102", "20260103"}
        assert item.month in {"2026-01"}
        assert item.dedupe_record.source_family == SOURCE_OURA_API


def test_normalize_bundle_emits_expected_record_types():
    items = _normalized_items()
    by_type = {}
    for item in items:
        by_type.setdefault(item.row["record_type"], []).append(item.row)

    assert set(by_type) == {
        "oura.daily_sleep",
        "oura.daily_readiness",
        "oura.temperature_deviation",
        "oura.daily_resilience",
        "oura.daily_stress",
        "oura.daily_spo2",
        "oura.sleep",
        "oura.daily_activity",
        "oura.heartrate",
        "oura.daily_cardiovascular_age",
        "oura.blood_glucose",
        "oura.workout",
        "oura.session",
        "oura.enhanced_tag",
        "oura.vo2_max",
    }
    # Temperature deviation splits out of each readiness document.
    assert len(by_type["oura.temperature_deviation"]) == 2
    assert by_type["oura.temperature_deviation"][0]["unit"] == "degC"
    # Scores stay attributed values, not re-derived numbers.
    scores = {row["day"]: row["value"] for row in by_type["oura.daily_sleep"]}
    assert scores == {"20260102": 88, "20260103": 74}


def test_normalize_sleep_period_keeps_stage_durations_in_metadata():
    items = _normalized_items()
    periods = [item.row for item in items if item.row["record_type"] == "oura.sleep"]

    night = next(row for row in periods if row["day"] == "20260102")
    assert night["kind"] == "sleep_period"
    assert night["start_date"] == "2026-01-01T22:41:00-07:00"
    assert night["end_date"] == "2026-01-02T06:32:00-07:00"
    assert night["value"] == 26340
    assert night["unit"] == "s"
    metadata = night["metadata"]
    assert metadata["deep_sleep_duration"] == 5460
    assert metadata["rem_sleep_duration"] == 6480
    assert metadata["light_sleep_duration"] == 14400
    assert metadata["awake_time"] == 1920
    assert metadata["sleep_phase_5_min"].startswith("4444")


def test_normalize_daily_activity_keeps_oura_score_and_totals():
    items = _normalized_items()
    rows = [
        item.row for item in items if item.row["record_type"] == "oura.daily_activity"
    ]

    assert len(rows) == 2
    first = next(row for row in rows if row["day"] == "20260102")
    assert first["kind"] == "daily_summary"
    assert first["value"] == 85
    assert first["unit"] == "score"
    assert first["source_record_id"] == "synthetic-activity-2026-01-02"
    assert first["metadata"]["steps"] == 11423
    assert first["metadata"]["active_calories"] == 512
    assert first["metadata"]["contributors"]["meet_daily_targets"] == 92


def test_normalize_heartrate_synthesizes_identity_and_owner_local_day():
    items = _normalized_items()
    rows = [item.row for item in items if item.row["record_type"] == "oura.heartrate"]

    assert len(rows) == 4
    first = next(
        row for row in rows if row["start_date"] == "2026-01-02T03:15:00-07:00"
    )
    # Heartrate rows carry no Oura day field: the journal day is derived
    # from the timestamp after conversion to the owner's local timezone.
    assert first["day"] == "20260102"
    assert first["kind"] == "sample"
    assert first["value"] == 49
    assert first["unit"] == "bpm"
    assert first["source_record_id"] == "heartrate/2026-01-02T03:15:00-07:00/sleep"
    assert first["metadata"] == {
        "source": "sleep",
        "raw_timestamp": "2026-01-02T03:15:00-07:00",
        "timezone": "America/Denver",
    }


def test_normalize_heartrate_converts_utc_to_owner_local_day_and_hour():
    [item] = oura.normalize_bundle(
        {
            "heartrate": [
                {
                    "timestamp": "2026-07-01T00:02:57.000Z",
                    "bpm": 63,
                    "source": "sleep",
                }
            ]
        },
        import_id="20260706_000000",
        raw_ref_root="imports/20260706_000000/raw/oura",
        owner_timezone=ZoneInfo("America/Denver"),
    )

    row = item.row
    assert row["source_record_id"] == "heartrate/2026-07-01T00:02:57.000Z/sleep"
    assert row["day"] == "20260630"
    assert item.month == "2026-06"
    assert row["start_date"] == "2026-06-30T18:02:57-06:00"
    assert row["metadata"] == {
        "source": "sleep",
        "raw_timestamp": "2026-07-01T00:02:57.000Z",
        "timezone": "America/Denver",
    }


def test_normalize_daily_cardiovascular_age_uses_oura_day_verbatim():
    # Documented endpoint (openapi-1.35 PublicDailyCardiovascularAge):
    # day-granularity documents with id + day required. The journal day is
    # Oura's day field verbatim — no local-time recomputation.
    items = _normalized_items()
    rows = [
        item.row
        for item in items
        if item.row["record_type"] == "oura.daily_cardiovascular_age"
    ]

    assert len(rows) == 2
    first = next(row for row in rows if row["day"] == "20260102")
    assert first["kind"] == "daily_summary"
    assert first["value"] == 34
    assert first["unit"] == "years"
    assert first["source_record_id"] == "synthetic-cardio-age-2026-01-02"
    assert first["metadata"] == {"pulse_wave_velocity": 7.1}
    # The second document carries a null pulse_wave_velocity — it must
    # not appear in metadata (nulls are dropped, like every other field).
    second = next(row for row in rows if row["day"] == "20260103")
    assert second["value"] == 35
    assert second["metadata"] == {}


def test_normalize_blood_glucose_converts_utc_and_synthesizes_identity():
    # PINNED ASSUMPTIONS for the undocumented blood_glucose series (see
    # _normalize_item): heartrate-shaped rows, UTC instants, mg/dL. The
    # first post-reauthorization sync confirms or falsifies these; if the
    # live shape differs, fix the fixture AND this test together.
    items = _normalized_items()
    rows = [
        item.row for item in items if item.row["record_type"] == "oura.blood_glucose"
    ]

    assert len(rows) == 4
    first = next(
        row
        for row in rows
        if row["metadata"]["raw_timestamp"].startswith("2026-01-02T15")
    )
    assert first["kind"] == "sample"
    assert first["value"] == 92
    assert first["unit"] == "mg/dL"
    assert first["source_record_id"] == "blood_glucose/2026-01-02T15:05:00Z"
    assert first["day"] == "20260102"
    assert first["start_date"] == "2026-01-02T08:05:00-07:00"
    assert first["metadata"] == {
        "raw_timestamp": "2026-01-02T15:05:00Z",
        "timezone": "America/Denver",
    }
    # The UTC->owner-local conversion is load-bearing: a 04:20Z sample
    # belongs to the PREVIOUS Denver day.
    cross_midnight = next(
        row
        for row in rows
        if row["metadata"]["raw_timestamp"] == "2026-01-03T04:20:00Z"
    )
    assert cross_midnight["day"] == "20260102"
    assert cross_midnight["start_date"] == "2026-01-02T21:20:00-07:00"


def test_normalize_workout_is_event_row_with_verbatim_local_times():
    # PublicWorkout (openapi-1.35; live-confirmed 2026-07-07). Workouts
    # are event rows like Apple Health workouts: kind="workout", no
    # scalar value, activity/intensity/calories/distance as metadata.
    items = _normalized_items()
    rows = [item.row for item in items if item.row["record_type"] == "oura.workout"]

    assert len(rows) == 2
    first = next(row for row in rows if row["day"] == "20260102")
    assert first["kind"] == "workout"
    assert "value" not in first
    assert "unit" not in first
    assert first["source_record_id"] == "synthetic-workout-2026-01-02"
    # Datetimes are wearer-local offsets and pass through VERBATIM —
    # never UTC-converted, never re-derived.
    assert first["start_date"] == "2026-01-02T08:05:00.000-07:00"
    assert first["end_date"] == "2026-01-02T08:41:00.000-07:00"
    # Null label drops from metadata like every other null field.
    assert first["metadata"] == {
        "activity": "walking",
        "intensity": "moderate",
        "source": "confirmed",
        "calories": 148.5,
        "distance": 2412.9,
    }
    # Timezone pin: the second workout starts at 23:12 local, so its UTC
    # instant is the NEXT calendar day — the journal day must stay Oura's
    # `day` verbatim, with no local-time recomputation.
    second = next(row for row in rows if row["start_date"].startswith("2026-01-03T23"))
    assert second["day"] == "20260103"
    assert second["metadata"]["label"] == "synthetic night ride"


def test_normalize_session_keeps_type_and_mood_never_sample_series():
    # PublicSession (openapi-1.35; live-confirmed). The heart_rate/
    # heart_rate_variability/motion_count sample blocks stay in the raw
    # page only — normalized metadata carries just type and mood.
    items = _normalized_items()
    rows = [item.row for item in items if item.row["record_type"] == "oura.session"]

    assert len(rows) == 2
    first = next(row for row in rows if row["day"] == "20260102")
    assert first["kind"] == "session"
    assert "value" not in first
    assert first["source_record_id"] == "synthetic-session-2026-01-02"
    assert first["start_date"] == "2026-01-02T17:10:00.000-07:00"
    assert first["end_date"] == "2026-01-02T17:30:00.000-07:00"
    assert first["metadata"] == {"type": "meditation", "mood": "good"}
    # Null mood drops; sample blocks never enter metadata.
    second = next(row for row in rows if row["day"] == "20260103")
    assert second["metadata"] == {"type": "rest"}


def test_normalize_enhanced_tag_uses_start_day_verbatim():
    # EnhancedTagModel (openapi-1.35; live-confirmed): no `day` field —
    # the journal day is Oura's `start_day` verbatim, even for a tag
    # whose span ends on a later day.
    items = _normalized_items()
    rows = [
        item.row for item in items if item.row["record_type"] == "oura.enhanced_tag"
    ]

    assert len(rows) == 2
    first = next(row for row in rows if row["day"] == "20260102")
    assert first["kind"] == "tag"
    assert "value" not in first
    assert first["source_record_id"] == "synthetic-tag-2026-01-02"
    assert first["start_date"] == "2026-01-02T14:45:12-07:00"
    assert "end_date" not in first
    assert first["metadata"] == {"tag_type_code": "tag_generic_nocaffeine"}
    # A spanning tag: attributed to its start_day, end fields kept.
    second = next(row for row in rows if row["day"] == "20260103")
    assert second["end_date"] == "2026-01-04T07:10:00-07:00"
    assert second["metadata"] == {
        "comment": "synthetic note text",
        "custom_name": "synthetic custom tag",
        "end_day": "2026-01-04",
    }


def test_normalize_vo2_max_uses_documented_shape():
    # PublicVO2Max (openapi-1.35): {id, day, timestamp, vo2_max}. Zero
    # rows on this account today, so the fixture follows the documented
    # shape; VO2 max is mL/kg/min by definition (the spec has no unit).
    items = _normalized_items()
    rows = [item.row for item in items if item.row["record_type"] == "oura.vo2_max"]

    assert len(rows) == 2
    first = next(row for row in rows if row["day"] == "20260102")
    assert first["kind"] == "daily_summary"
    assert first["value"] == 41
    assert first["unit"] == "mL/kg/min"
    assert first["source_record_id"] == "synthetic-vo2max-2026-01-02"
    assert first["metadata"] == {}


def test_workout_dedupe_key_is_payload_independent():
    # Oura documents revise in place (same id, corrected payload) — a
    # re-fetched workout with corrected calories must upsert, not
    # duplicate, exactly like every other document-id-keyed endpoint.
    workout = {
        "id": "synthetic-workout-2026-01-02",
        "activity": "walking",
        "calories": 148.5,
        "day": "2026-01-02",
        "distance": 2412.9,
        "end_datetime": "2026-01-02T08:41:00.000-07:00",
        "intensity": "moderate",
        "label": None,
        "source": "confirmed",
        "start_datetime": "2026-01-02T08:05:00.000-07:00",
    }
    revised = dict(workout, calories=152.0, intensity="hard")

    def key(item: dict) -> str:
        normalized = oura.normalize_bundle(
            {"workout": [item]}, import_id="x", raw_ref_root="x"
        )
        return normalized[0].row["dedupe_key"]

    assert key(workout) == key(revised)


def test_blood_glucose_dedupe_key_is_value_independent():
    # A re-fetched sample at the same timestamp with a corrected reading
    # must update in place, exactly like heartrate revisions.
    sample = {"timestamp": "2026-01-02T15:05:00Z", "glucose": 92}
    revised = dict(sample, glucose=95)

    def key(item: dict) -> str:
        normalized = oura.normalize_bundle(
            {"blood_glucose": [item]}, import_id="x", raw_ref_root="x"
        )
        return normalized[0].row["dedupe_key"]

    assert key(sample) == key(revised)


def test_heartrate_dedupe_key_is_bpm_independent():
    # A re-fetched sample at the same timestamp+source with a corrected
    # bpm must update in place, exactly like document-id revisions.
    sample = {
        "timestamp": "2026-01-02T03:15:00-07:00",
        "timestamp_unix": 1767348900,
        "bpm": 49,
        "source": "sleep",
    }
    revised = dict(sample, bpm=51)

    def key(item: dict) -> str:
        normalized = oura.normalize_bundle(
            {"heartrate": [item]}, import_id="x", raw_ref_root="x"
        )
        return normalized[0].row["dedupe_key"]

    assert key(sample) == key(revised)


def test_dedupe_keys_are_stable_across_reparses():
    first = {item.row["dedupe_key"] for item in _normalized_items()}
    second = {item.row["dedupe_key"] for item in _normalized_items()}

    assert first == second
    assert len(first) == len(_normalized_items()), "keys must be unique per row"


def test_dedupe_keys_use_source_record_id_identity():
    items = _normalized_items()
    readiness = next(
        item
        for item in items
        if item.row["record_type"] == "oura.daily_readiness"
        and item.row["day"] == "20260102"
    )

    expected = health_record_dedupe_key(
        HealthRecordIdentity(
            source_family=SOURCE_OURA_API,
            record_type="oura.daily_readiness",
            start_time=readiness.row["start_date"],
            source_record_id="synthetic-readiness-2026-01-02",
        )
    )
    assert readiness.row["dedupe_key"] == expected


def test_dedupe_keys_do_not_collide_across_source_families():
    identity = dict(
        record_type="oura.daily_sleep",
        start_time="2026-01-02T00:00:00-07:00",
        source_record_id="synthetic-sleep-2026-01-02",
    )
    oura_key = health_record_dedupe_key(
        HealthRecordIdentity(source_family=SOURCE_OURA_API, **identity)
    )
    apple_key = health_record_dedupe_key(
        HealthRecordIdentity(source_family=SOURCE_APPLE_HEALTH, **identity)
    )

    assert oura_key != apple_key


def test_normalized_rows_round_trip_through_jsonl_encoding():
    items = _normalized_items()
    encoded = [json.dumps(item.row, sort_keys=True) for item in items]
    decoded = [json.loads(line) for line in encoded]

    assert decoded == [json.loads(json.dumps(item.row)) for item in items]
    for row in decoded:
        assert row["schema"] == oura.NORMALIZED_SCHEMA
        assert row["source_family"] == SOURCE_OURA_API


# ---------------------------------------------------------------------------
# Revision / upsert — Oura re-issues documents with corrections (same
# document id, changed payload; scores settle for a day or two). Same id →
# same dedupe key → the upsert UPDATES in place, never duplicates.
# ---------------------------------------------------------------------------

_IMPORT_A = "20260105_000000"  # earlier bundle
_IMPORT_B = "20260112_000000"  # later re-fetch of a trailing window


def _normalize_for_import(path: Path, import_id: str) -> list[oura.OuraNormalizedItem]:
    bundle = oura.parse_oura_bundle(path)
    return oura.normalize_bundle(
        bundle,
        import_id=import_id,
        raw_ref_root=f"imports/{import_id}/raw/oura",
        owner_timezone=ZoneInfo("America/Denver"),
    )


def _items_by_row_identity(
    items: list[oura.OuraNormalizedItem],
) -> dict[tuple[str, str], oura.OuraNormalizedItem]:
    return {
        (item.row["record_type"], item.row["source_record_id"]): item for item in items
    }


def _write_bundle_shard(
    journal: Path, import_id: str, items: list[oura.OuraNormalizedItem]
) -> None:
    """Materialize one bundle's normalized month shards in a temp journal.

    Mirrors the storage the sync engine writes: per-bundle
    ``imports/<id>/normalized/<month>.jsonl`` with ``import_id`` /
    ``month`` / bundle-prefixed ``normalized_ref`` stamped on each row.
    """

    by_month: dict[str, list[str]] = {}
    for item in items:
        lines = by_month.setdefault(item.month, [])
        row = dict(item.row)
        row["import_id"] = import_id
        row["month"] = item.month
        row["normalized_ref"] = (
            f"imports/{import_id}/normalized/{item.month}.jsonl#L{len(lines) + 1}"
        )
        lines.append(json.dumps(row, sort_keys=True))
    for month, lines in by_month.items():
        shard = journal / "imports" / import_id / "normalized" / f"{month}.jsonl"
        shard.parent.mkdir(parents=True, exist_ok=True)
        shard.write_text("\n".join(lines) + "\n", encoding="utf-8")


def test_revision_reissue_keeps_dedupe_keys_payload_independent():
    base = _items_by_row_identity(
        _normalize_for_import(FIXTURE_ROOT / "daily_readiness.json", _IMPORT_A)
    )
    revised = _items_by_row_identity(_normalize_for_import(REVISION_ROOT, _IMPORT_B))

    assert set(base) == set(revised)
    changed_id = ("oura.daily_readiness", "synthetic-readiness-2026-01-02")
    # Same document id → same key, even though the score, both temperature
    # fields, a contributor, and the timestamp all changed. The key must
    # not include the payload or revisions would insert instead of update.
    assert revised[changed_id].row["dedupe_key"] == base[changed_id].row["dedupe_key"]
    assert base[changed_id].row["value"] == 82
    assert revised[changed_id].row["value"] == 79
    assert revised[changed_id].row["start_date"] != base[changed_id].row["start_date"]
    assert (
        revised[changed_id].dedupe_record.value_hash
        != base[changed_id].dedupe_record.value_hash
    )

    # The derived temperature-deviation row revises through the same id.
    temp_id = (
        "oura.temperature_deviation",
        "synthetic-readiness-2026-01-02/temperature_deviation",
    )
    assert revised[temp_id].row["dedupe_key"] == base[temp_id].row["dedupe_key"]
    assert base[temp_id].row["value"] == -0.21
    assert revised[temp_id].row["value"] == -0.05

    # The byte-identical re-issue is a pure duplicate: same key, same hash.
    same_id = ("oura.daily_readiness", "synthetic-readiness-2026-01-03")
    assert revised[same_id].row["dedupe_key"] == base[same_id].row["dedupe_key"]
    assert (
        revised[same_id].dedupe_record.value_hash
        == base[same_id].dedupe_record.value_hash
    )


def test_document_id_shared_across_endpoints_never_collides():
    shared_id = "synthetic-shared-2026-01-02"
    day = "2026-01-02"
    bundle = {
        "daily_sleep": [{"id": shared_id, "day": day, "score": 80}],
        "daily_readiness": [{"id": shared_id, "day": day, "score": 70}],
        "daily_resilience": [{"id": shared_id, "day": day, "level": "solid"}],
        "daily_stress": [{"id": shared_id, "day": day, "day_summary": "normal"}],
        "daily_spo2": [
            {"id": shared_id, "day": day, "spo2_percentage": {"average": 97.0}}
        ],
        "daily_activity": [{"id": shared_id, "day": day, "score": 66}],
        "sleep": [
            {
                "id": shared_id,
                "day": day,
                "bedtime_start": "2026-01-01T22:41:00-07:00",
                "total_sleep_duration": 26340,
            }
        ],
    }
    items = oura.normalize_bundle(
        bundle,
        import_id=_IMPORT_A,
        raw_ref_root=f"imports/{_IMPORT_A}/raw/oura",
    )

    keys = [item.row["dedupe_key"] for item in items]
    assert len(items) == len(bundle)
    assert len(set(keys)) == len(keys)
    by_type = {item.row["record_type"]: item.row["dedupe_key"] for item in items}
    # The most collision-prone pair: one endpoint name prefixes the other.
    assert by_type["oura.daily_sleep"] != by_type["oura.sleep"]


def test_cross_bundle_day_read_surfaces_one_row_per_key(tmp_path: Path):
    journal = tmp_path
    base = _normalize_for_import(FIXTURE_ROOT, _IMPORT_A)
    revised = _normalize_for_import(REVISION_ROOT, _IMPORT_B)
    _write_bundle_shard(journal, _IMPORT_A, base)
    _write_bundle_shard(journal, _IMPORT_B, revised)

    rows = _iter_normalized_rows(journal, month="2026-01")

    keys = [row["dedupe_key"] for row in rows]
    assert len(keys) == len(set(keys)), "day reads must dedupe by key"
    assert len(rows) == len(base)  # revision re-issues add no rows
    readiness = next(
        row
        for row in rows
        if row["record_type"] == "oura.daily_readiness" and row["day"] == "20260102"
    )
    # The surfaced row remembers both bundles for the audit drawer.
    assert readiness["import_ids"] == [_IMPORT_A, _IMPORT_B]


def test_cross_bundle_day_read_surfaces_latest_revision(tmp_path: Path):
    journal = tmp_path
    base = _normalize_for_import(FIXTURE_ROOT, _IMPORT_A)
    revised = _normalize_for_import(REVISION_ROOT, _IMPORT_B)
    _write_bundle_shard(journal, _IMPORT_A, base)
    _write_bundle_shard(journal, _IMPORT_B, revised)

    rows = _iter_normalized_rows(journal, month="2026-01")

    readiness = next(
        row
        for row in rows
        if row["record_type"] == "oura.daily_readiness" and row["day"] == "20260102"
    )
    temperature = next(
        row
        for row in rows
        if row["record_type"] == "oura.temperature_deviation"
        and row["day"] == "20260102"
    )
    assert readiness["value"] == 79, "the corrected readiness score must surface"
    assert temperature["value"] == -0.05


# ---------------------------------------------------------------------------
# File importer surface (detect/preview/dry-run)
# ---------------------------------------------------------------------------


def test_detect_matches_only_oura_shaped_bundles():
    importer = oura.OuraImporter()

    assert importer.detect(FIXTURE_ROOT) is True
    assert importer.detect(FIXTURE_ROOT / "sleep.json") is True
    assert importer.detect(APPLE_FIXTURE_ROOT) is False
    assert importer.detect(Path(__file__)) is False


def test_preview_counts_documents_and_days():
    preview = oura.OuraImporter().preview(FIXTURE_ROOT)

    assert preview.date_range == ("20260102", "20260103")
    assert preview.entity_count == 0
    # 29 documents; readiness docs each add a temperature-deviation row.
    assert preview.item_count == _FIXTURE_ROW_COUNT
    assert "daily_readiness=2" in preview.summary
    assert "sleep=2" in preview.summary
    assert "daily_activity=2" in preview.summary
    assert "heartrate=4" in preview.summary
    assert "daily_cardiovascular_age=2" in preview.summary
    assert "blood_glucose=4" in preview.summary
    assert "workout=2" in preview.summary
    assert "session=2" in preview.summary
    assert "enhanced_tag=2" in preview.summary
    assert "vO2_max=2" in preview.summary
    assert f"source_family={SOURCE_OURA_API}" in preview.summary


def test_dry_run_process_writes_nothing(tmp_path: Path, monkeypatch):
    journal = _use_journal(tmp_path, monkeypatch)

    result = oura.OuraImporter().process(FIXTURE_ROOT, journal, dry_run=True)

    assert result.entries_written == 0
    assert result.files_created == []
    assert result.summary.startswith("Dry run only:")
    assert not (journal / "imports").exists()


# ---------------------------------------------------------------------------
# Pre-save gate enforcement (file-import path)
# ---------------------------------------------------------------------------


def test_render_day_summary_attributes_every_score_to_oura():
    rows = [item.row for item in _normalized_items() if item.row["day"] == "20260102"]

    summary = oura.render_day_summary("20260102", rows, import_id="20260105_000000")

    assert summary.splitlines()[0] == "# Body · January 2, 2026"
    assert "Readiness 82 · Oura's score" in summary
    assert "Sleep score 88 · Oura's score" in summary
    assert "Resilience solid · Oura's label" in summary
    assert "Day stress summary normal · Oura's label" in summary
    assert "Nightly blood oxygen 97.4% · Oura's average" in summary
    assert "Temperature deviation -0.21 °C · Oura's measurement" in summary
    assert "Activity score 85 · Oura's score" in summary
    assert "Cardiovascular age 34 · Oura's estimate" in summary
    assert "VO2 max 41 · Oura's estimate" in summary
    assert "Sleep 7h 19m · Oura's staging" in summary
    assert "deep 1h 31m" in summary
    # Heartrate and blood-glucose samples are series, never summarized
    # into day prose.
    assert "bpm" not in summary
    assert "glucose" not in summary.lower()
    # Workouts, sessions, and tags are event rows — day surfaces render
    # them from their kind, never as day-summary prose.
    assert "walking" not in summary.lower()
    assert "meditation" not in summary.lower()
    assert "nocaffeine" not in summary.lower()
    assert "brought in via the Oura API · import 20260105_000000" in summary


def test_render_day_summary_makes_no_medical_gloss():
    rows = [item.row for item in _normalized_items() if item.row["day"] == "20260103"]

    summary = oura.render_day_summary("20260103", rows, import_id="x").lower()

    for banned in ("recovered well", "should", "healthy", "poor", "good", "bad"):
        assert banned not in summary


def test_render_day_summary_without_rows_is_factual():
    summary = oura.render_day_summary("20260104", [], import_id="x")

    assert "No Oura entries for this day." in summary


# ---------------------------------------------------------------------------
# Fetch layer — canned transport, pagination, backoff, 401 refresh
