# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import datetime as dt
import importlib
import importlib.util
import json
import logging
from collections import Counter
from pathlib import Path

import pytest

from solstone.think.importers import apple_health
from solstone.think.importers.apple_health import (
    AppleHealthImporter,
)
from solstone.think.importers.health_schema import (
    merge_sleep_sessions,
    pick_day_sleep,
    pick_main_session,
)

FIXTURE_ROOT = (
    Path(__file__).parent
    / "fixtures"
    / "importers"
    / "health"
    / "apple_health_synthetic"
)
ZIP_FIXTURE = (
    Path(__file__).parent
    / "fixtures"
    / "importers"
    / "health"
    / "apple_health_synthetic.zip"
)
DTD_FIXTURE_ROOT = (
    Path(__file__).parent
    / "fixtures"
    / "importers"
    / "health"
    / "apple_health_synthetic_dtd"
)
# The fixture's only sleep entry starts Jan 2 at 10:30 PM and ends Jan 3 at
# 6:30 AM. Under the canonical cross-midnight rule that night belongs to the
# day it ended (Jan 3, which has no records and hence no card), so the Jan 2
# card carries no Sleep line — exactly like the body day page for Jan 2.
EXPECTED_FIXTURE_DAY_CARD = """\
# Body · January 2, 2026

Glucose 105 mg/dL, 1 workout.

**Glucose** 105 mg/dL · 1 reading

**Workouts** Running · 1 workout

**Signals**

- Glucose: 1
- Resting heart rate: 1
- Sleep: 1

*Sources: Synthetic Ring Mirror, Synthetic Stelo, Synthetic Watch \
· 4 records summarized · brought in via Apple Health · import 20260103_120000*
"""


def _record_row(
    record_type: str,
    *,
    value: str | None = None,
    unit: str | None = None,
    source: str = "Synthetic Watch",
    start: str | None = None,
    end: str | None = None,
) -> dict:
    row: dict = {"kind": "record", "record_type": record_type, "source_name": source}
    if value is not None:
        row["value"] = value
    if unit is not None:
        row["unit"] = unit
    if start is not None:
        row["start_date"] = start
    if end is not None:
        row["end_date"] = end
    return row


def _workout_row(activity_type: str, *, source: str = "Synthetic Watch") -> dict:
    return {"kind": "workout", "record_type": activity_type, "source_name": source}


def _rich_day_summary() -> apple_health._DaySummary:
    summary = apple_health._DaySummary(day="20260701")
    rows = [
        _record_row(
            "HKCategoryTypeIdentifierSleepAnalysis",
            start="2026-07-01 00:01:00 -0700",
            end="2026-07-01 03:00:00 -0700",
        ),
        _record_row(
            "HKCategoryTypeIdentifierSleepAnalysis",
            source="Synthetic Ring Mirror",
            start="2026-07-01 03:10:00 -0700",
            end="2026-07-01 08:05:00 -0700",
        ),
        _record_row(
            "HKQuantityTypeIdentifierBloodGlucose",
            value="77",
            unit="mg/dL",
            source="Synthetic Stelo",
        ),
        _record_row(
            "HKQuantityTypeIdentifierBloodGlucose",
            value="104",
            unit="mg/dL",
            source="Synthetic Stelo",
        ),
        _record_row(
            "HKQuantityTypeIdentifierBloodGlucose",
            value="84",
            unit="mg/dL",
            source="Synthetic Stelo",
        ),
        _record_row("HKQuantityTypeIdentifierStepCount", value="500", unit="count"),
        _record_row("HKQuantityTypeIdentifierStepCount", value="250", unit="count"),
        _record_row("HKQuantityTypeIdentifierStepCount", value="125", unit="count"),
        _record_row("HKQuantityTypeIdentifierWalkingStepLength", value="0.7", unit="m"),
        _workout_row("HKWorkoutActivityTypeRunning"),
        _workout_row("HKWorkoutActivityTypeRunning"),
        _workout_row("HKWorkoutActivityTypeRunning"),
        _workout_row("HKWorkoutActivityTypeHighIntensityIntervalTraining"),
    ]
    for row in rows:
        apple_health._add_to_day_summary(summary, row)
    apple_health._attach_night_sleep({summary.day: summary})
    return summary


def test_apple_health_registered_as_read_only_differential_importer():
    file_importer = importlib.import_module("solstone.think.importers.file_importer")

    assert "apple_health" in file_importer.FILE_IMPORTER_REGISTRY
    assert file_importer.get_file_importer("apple_health") is not None


def test_detects_synthetic_export_directory():
    importer = AppleHealthImporter()

    assert importer.detect(FIXTURE_ROOT) is True
    assert importer.detect(FIXTURE_ROOT / "apple_health_export") is True
    assert importer.detect(Path(__file__)) is False


def test_preview_synthetic_export_directory():
    preview = AppleHealthImporter().preview(FIXTURE_ROOT)

    assert preview.date_range == ("20260101", "20260102")
    assert preview.item_count == 7
    assert preview.entity_count == 0
    assert "records=5" in preview.summary
    assert "workouts=1" in preview.summary
    assert "routes=1" in preview.summary
    assert "glucose=1" in preview.summary


def test_preview_filters_synthetic_export_by_inclusive_date_window():
    preview = AppleHealthImporter().preview(
        FIXTURE_ROOT,
        date_from="2026-01-02",
        date_to="2026-01-02",
    )

    assert preview.date_range == ("20260102", "20260102")
    assert preview.item_count == 4
    assert "records=3" in preview.summary
    assert "workouts=1" in preview.summary
    assert "routes=0" in preview.summary
    assert "glucose=1" in preview.summary


def test_preview_parses_synthetic_export_with_internal_dtd_subset():
    preview = AppleHealthImporter().preview(DTD_FIXTURE_ROOT)

    assert preview.date_range == ("20260410", "20260411")
    assert preview.item_count == 3
    assert "records=2" in preview.summary
    assert "workouts=1" in preview.summary
    assert "export_cda=present" in preview.summary
    assert "electrocardiograms=2" in preview.summary


def test_preview_reports_cda_and_ecg_files_by_name_only():
    preview = AppleHealthImporter().preview(DTD_FIXTURE_ROOT)

    assert "export_cda=present" in preview.summary
    assert "electrocardiograms=2" in preview.summary


def test_dry_run_process_returns_preview_without_files(tmp_path: Path):
    result = AppleHealthImporter().process(
        FIXTURE_ROOT,
        tmp_path,
        import_id="20260102_123000",
        dry_run=True,
    )

    assert result.entries_written == 0
    assert result.entities_seeded == 0
    assert result.files_created == []
    assert result.date_range == ("20260101", "20260102")
    assert "Dry run only" in result.summary
    assert not (tmp_path / "imports").exists()


def test_render_day_summary_owner_card_structure():
    # Two sources cover the night: Synthetic Watch 12:01–3:00 AM and
    # Synthetic Ring Mirror 3:10–8:05 AM. The canonical rule never sums
    # sources — the longest-coverage source (Ring Mirror) is the day's
    # sleep, matching what the body day page shows for the same rows.
    rendered = apple_health._render_day_summary(
        _rich_day_summary(), import_id="20260704_090000"
    )

    assert rendered == (
        "# Body · July 1, 2026\n"
        "\n"
        "Slept 3:10 AM – 8:05 AM, glucose 77–104 mg/dL (avg 88.3), 4 workouts.\n"
        "\n"
        "**Sleep** 3:10 AM – 8:05 AM · 4h 55m · 1 sleep entry\n"
        "\n"
        "**Glucose** 77–104 mg/dL · avg 88.3 · 3 readings\n"
        "\n"
        "**Workouts** High intensity interval training, Running · 4 workouts\n"
        "\n"
        "**Signals**\n"
        "\n"
        "- Glucose: 3\n"
        "- Step count: 3\n"
        "- Sleep: 2\n"
        "- Walking step length: 1\n"
        "\n"
        "*Sources: Synthetic Ring Mirror, Synthetic Stelo, Synthetic Watch"
        " · 13 records summarized · brought in via Apple Health"
        " · import 20260704_090000*"
    )


def test_render_day_summary_uses_friendly_names_never_raw_identifiers():
    rendered = apple_health._render_day_summary(
        _rich_day_summary(), import_id="20260704_090000"
    )

    assert "HKQuantityTypeIdentifier" not in rendered
    assert "HKCategoryTypeIdentifier" not in rendered
    assert "HKWorkoutActivityType" not in rendered
    assert "- Glucose: 3" in rendered
    assert "- Step count: 3" in rendered
    assert "- Sleep: 2" in rendered
    assert "- Walking step length: 1" in rendered
    assert "High intensity interval training" in rendered


def test_render_day_summary_signals_only_day_uses_entry_count_lede():
    summary = apple_health._DaySummary(day="20260101")
    apple_health._add_to_day_summary(
        summary,
        _record_row("HKQuantityTypeIdentifierStepCount", value="500", unit="count"),
    )
    apple_health._add_to_day_summary(
        summary,
        _record_row("HKQuantityTypeIdentifierHeartRate", value="62", unit="count/min"),
    )

    rendered = apple_health._render_day_summary(summary, import_id="20260103_120000")
    lines = rendered.splitlines()

    assert lines[0] == "# Body · January 1, 2026"
    assert lines[2] == "2 entries across 2 signals."
    assert "**Sleep**" not in rendered
    assert "**Glucose**" not in rendered
    assert "**Workouts**" not in rendered
    assert "- Heart rate: 1" in rendered
    assert "- Step count: 1" in rendered


def test_render_day_summary_trims_signals_to_top_six_with_more_line():
    summary = apple_health._DaySummary(day="20260701")
    summary.record_count = 45
    summary.type_counts = Counter(
        {
            "HKQuantityTypeIdentifierStepCount": 9,
            "HKQuantityTypeIdentifierHeartRate": 8,
            "HKQuantityTypeIdentifierRespiratoryRate": 7,
            "HKQuantityTypeIdentifierOxygenSaturation": 6,
            "HKQuantityTypeIdentifierWalkingStepLength": 5,
            "HKQuantityTypeIdentifierFlightsClimbed": 4,
            "HKQuantityTypeIdentifierBodyMass": 3,
            "HKQuantityTypeIdentifierHeight": 2,
            "HKQuantityTypeIdentifierVO2Max": 1,
        }
    )

    rendered = apple_health._render_day_summary(summary, import_id="20260704_090000")
    bullets = [line for line in rendered.splitlines() if line.startswith("- ")]

    assert bullets == [
        "- Step count: 9",
        "- Heart rate: 8",
        "- Respiratory rate: 7",
        "- Blood oxygen: 6",
        "- Walking step length: 5",
        "- Flights climbed: 4",
    ]
    assert "…and 3 more signals" in rendered
    assert "Body mass" not in rendered
    assert "Height" not in rendered
    assert "VO2 max" not in rendered


def test_render_day_summary_more_line_singular_when_one_signal_hidden():
    summary = apple_health._DaySummary(day="20260701")
    summary.record_count = 28
    summary.type_counts = Counter(
        {
            "HKQuantityTypeIdentifierStepCount": 7,
            "HKQuantityTypeIdentifierHeartRate": 6,
            "HKQuantityTypeIdentifierRespiratoryRate": 5,
            "HKQuantityTypeIdentifierOxygenSaturation": 4,
            "HKQuantityTypeIdentifierWalkingStepLength": 3,
            "HKQuantityTypeIdentifierFlightsClimbed": 2,
            "HKQuantityTypeIdentifierBodyMass": 1,
        }
    )

    rendered = apple_health._render_day_summary(summary, import_id="20260704_090000")

    assert "…and 1 more signal" in rendered
    assert "more signals" not in rendered


def test_render_day_summary_no_more_line_when_six_or_fewer_signals():
    rendered = apple_health._render_day_summary(
        _rich_day_summary(), import_id="20260704_090000"
    )

    # Four signal types on this day — every one lists, no trim line.
    assert "- Walking step length: 1" in rendered
    assert "more signal" not in rendered


def test_render_day_summary_formats_counts_with_thousands_separators():
    summary = apple_health._DaySummary(day="20260701")
    summary.record_count = 1234
    summary.type_counts = Counter({"HKQuantityTypeIdentifierStepCount": 1234})

    rendered = apple_health._render_day_summary(summary, import_id="20260704_090000")

    assert "- Step count: 1,234" in rendered
    assert "1,234 records summarized" in rendered


# --- Canonical sleep: shared helpers, card == day page ------------------------


TZ = dt.timezone(dt.timedelta(hours=-6))


def _at(month: int, day: int, hour: int, minute: int = 0) -> dt.datetime:
    return dt.datetime(2026, month, day, hour, minute, tzinfo=TZ)


CROSS_MIDNIGHT_EXPORT_RECORDS = """
  <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Synthetic Ring" sourceVersion="1.0" creationDate="2026-07-01 07:10:00 -0600" startDate="2026-06-30 22:58:00 -0600" endDate="2026-07-01 02:00:00 -0600" value="HKCategoryValueSleepAnalysisAsleepCore"/>
  <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Synthetic Ring" sourceVersion="1.0" creationDate="2026-07-01 07:10:00 -0600" startDate="2026-07-01 02:30:00 -0600" endDate="2026-07-01 07:08:00 -0600" value="HKCategoryValueSleepAnalysisAsleepDeep"/>
  <Record type="HKCategoryTypeIdentifierSleepAnalysis" sourceName="Synthetic Wrist" sourceVersion="1.0" creationDate="2026-07-01 06:35:00 -0600" startDate="2026-06-30 23:30:00 -0600" endDate="2026-07-01 06:30:00 -0600" value="HKCategoryValueSleepAnalysisAsleepUnspecified"/>
  <Record type="HKQuantityTypeIdentifierHeartRate" sourceName="Synthetic Watch" sourceVersion="1.0" unit="count/min" creationDate="2026-07-01 09:00:00 -0600" startDate="2026-07-01 09:00:00 -0600" endDate="2026-07-01 09:00:00 -0600" value="61"/>
"""


def _write_cross_midnight_export(root: Path) -> Path:
    export_root = root / "apple_health_export"
    export_root.mkdir(parents=True)
    (export_root / "export.xml").write_text(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        "<!DOCTYPE HealthData>\n"
        f'<HealthData locale="en_US">{CROSS_MIDNIGHT_EXPORT_RECORDS}</HealthData>\n',
        encoding="utf-8",
    )
    return root


def test_merge_sleep_sessions_joins_gaps_up_to_the_limit():
    merged = merge_sleep_sessions(
        [
            (_at(7, 1, 2, 30), _at(7, 1, 7, 8)),  # unsorted on purpose
            (_at(6, 30, 22, 58), _at(7, 1, 2, 0)),  # 30-minute wake gap
            (_at(7, 1, 14, 0), _at(7, 1, 14, 45)),  # afternoon nap, far apart
        ]
    )

    assert merged == [
        (_at(6, 30, 22, 58), _at(7, 1, 7, 8)),
        (_at(7, 1, 14, 0), _at(7, 1, 14, 45)),
    ]


def test_merge_sleep_sessions_keeps_gaps_beyond_the_limit_apart():
    merged = merge_sleep_sessions(
        [
            (_at(7, 1, 22, 0), _at(7, 1, 23, 0)),
            (_at(7, 2, 0, 1), _at(7, 2, 6, 0)),  # 61-minute gap
        ]
    )

    assert merged == [
        (_at(7, 1, 22, 0), _at(7, 1, 23, 0)),
        (_at(7, 2, 0, 1), _at(7, 2, 6, 0)),
    ]


def test_pick_main_session_applies_noon_rule_and_naps():
    day = dt.date(2026, 7, 1)
    night = (_at(6, 30, 22, 58), _at(7, 1, 7, 8))
    nap = (_at(7, 1, 14, 0), _at(7, 1, 14, 45))
    tonight = (_at(7, 1, 23, 0), _at(7, 2, 6, 0))  # ends tomorrow: not today's

    main, naps = pick_main_session([nap, night, tonight], day)

    assert main == night
    assert naps == [nap]


def test_pick_main_session_keeps_unmerged_evening_doze_as_nap():
    day = dt.date(2026, 7, 1)
    night = (_at(6, 30, 23, 0), _at(7, 1, 6, 30))
    doze = (_at(7, 1, 20, 0), _at(7, 1, 20, 40))  # never merged into tonight
    tonight = (_at(7, 1, 22, 30), _at(7, 2, 6, 0))  # tomorrow's main night

    main, naps = pick_main_session([night, doze, tonight], day)

    # A genuinely separate same-evening doze stays a nap; the session that
    # runs past midnight belongs to the next day, never to both.
    assert main == night
    assert naps == [doze]


def test_pick_day_sleep_never_attributes_an_interval_to_two_days():
    # Four days shaped like the verified journal days: nightly sleep split
    # into fragments, including a bedtime fragment before midnight that
    # merges into the following night, plus one genuine afternoon nap.
    intervals = {
        "Synthetic Phone": [
            (_at(8, 14, 23, 2), _at(8, 15, 6, 35)),  # night ending the 15th
            (_at(8, 15, 23, 17), _at(8, 15, 23, 58)),  # bedtime fragment
            (_at(8, 16, 0, 20), _at(8, 16, 6, 35)),  # rest of that night
            (_at(8, 16, 14, 0), _at(8, 16, 14, 40)),  # genuine afternoon nap
            (_at(8, 16, 23, 30), _at(8, 17, 7, 0)),  # night ending the 17th
        ]
    }

    sessions_by_day: dict[dt.date, list[tuple[dt.datetime, dt.datetime]]] = {}
    for offset in range(14, 18):
        day = dt.date(2026, 8, offset)
        sleep = pick_day_sleep(intervals, day)
        if sleep is None:
            continue
        sessions_by_day[day] = ([sleep.main] if sleep.main else []) + list(sleep.naps)

    # The bedtime fragment lands exactly once: as the start of the 16th's
    # main night — never doubling as the 15th's nap.
    assert sessions_by_day[dt.date(2026, 8, 15)] == [
        (_at(8, 14, 23, 2), _at(8, 15, 6, 35))
    ]
    assert sessions_by_day[dt.date(2026, 8, 16)] == [
        (_at(8, 15, 23, 17), _at(8, 16, 6, 35)),
        (_at(8, 16, 14, 0), _at(8, 16, 14, 40)),
    ]
    assert sessions_by_day[dt.date(2026, 8, 17)] == [
        (_at(8, 16, 23, 30), _at(8, 17, 7, 0))
    ]

    # Invariant: no interval overlaps between two days' attributed sessions.
    days = sorted(sessions_by_day)
    for i, day_a in enumerate(days):
        for day_b in days[i + 1 :]:
            for start_a, end_a in sessions_by_day[day_a]:
                for start_b, end_b in sessions_by_day[day_b]:
                    assert end_a <= start_b or end_b <= start_a, (
                        f"{day_a} session {start_a}–{end_a} overlaps "
                        f"{day_b} session {start_b}–{end_b}"
                    )


def test_pick_day_sleep_prefers_longest_coverage_source():
    sleep = pick_day_sleep(
        {
            "Synthetic Wrist": [(_at(6, 30, 23, 30), _at(7, 1, 6, 30))],
            "Synthetic Ring": [(_at(6, 30, 22, 58), _at(7, 1, 7, 8))],
        },
        dt.date(2026, 7, 1),
    )

    assert sleep is not None
    assert sleep.source == "Synthetic Ring"
    assert sleep.other_sources == ("Synthetic Wrist",)
    assert sleep.main == (_at(6, 30, 22, 58), _at(7, 1, 7, 8))
    assert sleep.naps == ()


def test_detects_and_previews_synthetic_zip_fixture():
    importer = AppleHealthImporter()

    assert ZIP_FIXTURE.exists()
    assert importer.detect(ZIP_FIXTURE) is True
    assert (
        importer.preview(ZIP_FIXTURE).summary == importer.preview(FIXTURE_ROOT).summary
    )


def test_preview_logs_byte_progress_for_large_xml_reads(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
):
    export_root = tmp_path / "apple_health_export"
    export_root.mkdir()
    records = "\n".join(
        '<Record type="HKQuantityTypeIdentifierStepCount" '
        'sourceName="Synthetic Watch" startDate="2026-05-01 08:00:00 -0700" '
        'endDate="2026-05-01 08:05:00 -0700" unit="count" value="1"/>'
        for _ in range(3)
    )
    (export_root / "export.xml").write_text(
        f'<?xml version="1.0" encoding="UTF-8"?>\n'
        f"<!DOCTYPE HealthData>\n"
        f'<HealthData locale="en_US">{records}</HealthData>\n',
        encoding="utf-8",
    )
    monkeypatch.setattr(apple_health, "_BYTE_PROGRESS_LOG_INTERVAL", 128)
    caplog.set_level(logging.INFO, logger=apple_health.__name__)

    preview = AppleHealthImporter().preview(tmp_path)

    assert preview.item_count == 3
    assert any(
        "from Apple Health export.xml" in record.message for record in caplog.records
    )


def _read_jsonl(path: Path) -> list[dict]:
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]


def test_windowed_source_hash_separates_date_slices(tmp_path: Path):
    # A date-windowed save covers only a slice of the export; a different
    # window over the same file must not be reported as already imported
    # (upstream PR review finding, 2026-07-04). Identical windows still
    # dedupe, and unwindowed imports keep the plain content hash.
    from solstone.think.importers.shared import hash_source, windowed_source_hash

    source = tmp_path / "export.zip"
    source.write_bytes(b"same bytes either way")

    plain = windowed_source_hash(source)
    assert plain == hash_source(source)

    slice_one = windowed_source_hash(source, "2026-06-27", "2026-07-03")
    slice_two = windowed_source_hash(source, "2026-07-04", None)
    full = windowed_source_hash(source, None, None)

    assert slice_one != slice_two
    assert slice_one != plain
    assert full == plain
    assert slice_one == windowed_source_hash(source, "2026-06-27", "2026-07-03")
