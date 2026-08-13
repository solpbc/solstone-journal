#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Synthetic journal seeder for the Body Convey reference corpus."""

from __future__ import annotations

import json
import sqlite3
from collections import Counter
from datetime import date, timedelta
from pathlib import Path
from typing import Any

APPLE_HEALTH = "apple_health"
OURA_API = "oura_api"
APPLE_STREAM = "import.apple_health"

IMPORT_APPLE = "20260810_080000"
IMPORT_OURA = "20260810_090000"
IMPORT_CORRECTION = "20260810_100000"
UNREADABLE_IMPORT = "unreadable-manifest"
UNKNOWN_IMPORT = "unknown-source"

HEALTH_DEDUPE_SCHEMA = """
CREATE TABLE health_dedupe (
    dedupe_key TEXT PRIMARY KEY,
    source_family TEXT NOT NULL,
    source_record_id TEXT,
    record_type TEXT NOT NULL,
    start_time TEXT NOT NULL,
    end_time TEXT,
    value_hash TEXT,
    first_import_id TEXT,
    last_seen_import_id TEXT,
    normalized_ref TEXT,
    raw_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_health_dedupe_source_record
ON health_dedupe (source_family, source_record_id);
CREATE INDEX idx_health_dedupe_record_time
ON health_dedupe (record_type, start_time, end_time);
"""


def _iso(day_value: date, clock: str) -> str:
    return f"{day_value.isoformat()}T{clock}+00:00"


def _health_row(
    record_type: str,
    start: str,
    *,
    end: str | None = None,
    value: str | float | int | None = None,
    unit: str | None = None,
    source_family: str = APPLE_HEALTH,
    source_name: str = "Synthetic Watch",
    kind: str = "record",
    metadata: dict[str, Any] | None = None,
    day: str | None = None,
    dedupe_key: str | None = "default",
    source_record_id: str | None = None,
) -> dict[str, Any]:
    """Build one Apple-shaped normalized row without importing test helpers."""

    start_day = start[:10].replace("-", "")
    row_day = day or start_day
    row: dict[str, Any] = {
        "schema": "solstone.health.normalized.v1",
        "source_family": source_family,
        "kind": kind,
        "record_type": record_type,
        "day": row_day,
        "start_date": start,
        "source_name": source_name,
        "month": start[:7],
    }
    if dedupe_key == "default":
        row["dedupe_key"] = f"{source_family}:{record_type}:{source_name}:{start}"
    elif dedupe_key is not None:
        row["dedupe_key"] = dedupe_key
    if end is not None:
        row["end_date"] = end
    if value is not None:
        row["value"] = value
    if unit is not None:
        row["unit"] = unit
    if metadata is not None:
        row["metadata"] = metadata
    if source_record_id is not None:
        row["source_record_id"] = source_record_id
    return row


def _oura_row(
    record_type: str,
    day: str,
    *,
    start: str | None = None,
    end: str | None = None,
    value: str | float | int | None = None,
    unit: str | None = None,
    kind: str = "daily_summary",
    metadata: dict[str, Any] | None = None,
    dedupe_key: str | None = "default",
) -> dict[str, Any]:
    """Build one Oura API normalized row without importing test helpers."""

    iso_day = f"{day[:4]}-{day[4:6]}-{day[6:8]}"
    row_start = start or f"{iso_day}T04:00:00+00:00"
    row: dict[str, Any] = {
        "schema": "solstone.health.oura.v1",
        "source_family": OURA_API,
        "kind": kind,
        "record_type": record_type,
        "day": day,
        "start_date": row_start,
        "source_record_id": f"{record_type}-{day}",
        "month": row_start[:7],
    }
    if dedupe_key == "default":
        row["dedupe_key"] = f"oura-api:{record_type}:{day}:{row_start}"
    elif dedupe_key is not None:
        row["dedupe_key"] = dedupe_key
    if end is not None:
        row["end_date"] = end
    if value is not None:
        row["value"] = value
    if unit is not None:
        row["unit"] = unit
    if metadata is not None:
        row["metadata"] = metadata
    return row


def _write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _write_import_bundle(
    root: Path,
    import_id: str,
    source_type: str,
    rows: list[dict[str, Any]],
    *,
    imported_at: str,
) -> dict[str, Any]:
    bundle = root / "imports" / import_id
    grouped: dict[str, list[dict[str, Any]]] = {}
    days = sorted({str(row["day"]) for row in rows})
    for original in rows:
        row = dict(original)
        row["import_id"] = import_id
        grouped.setdefault(str(row["month"]), []).append(row)

    _write_json(
        bundle / "manifest.json",
        {
            "import_id": import_id,
            "source_type": source_type,
            "source_hash": f"sha256:body-corpus-{import_id}",
            "entry_count": len(rows),
            "days_affected": days,
            "imported_at": imported_at,
            "imported_via": "body-corpus",
        },
    )
    for month, month_rows in sorted(grouped.items()):
        path = bundle / "normalized" / f"{month}.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            "".join(json.dumps(row, sort_keys=True) + "\n" for row in month_rows),
            encoding="utf-8",
        )
    return {
        "import_id": import_id,
        "source_type": source_type,
        "entry_count": len(rows),
        "months": sorted(grouped),
        "days_affected": days,
    }


def _write_dedupe_database(root: Path, bundles: list[tuple[str, list[dict[str, Any]]]]) -> None:
    db_path = root / "imports" / "health-dedupe.sqlite"
    db_path.parent.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(db_path) as conn:
        conn.executescript(HEALTH_DEDUPE_SCHEMA)
        for import_id, rows in bundles:
            for row in rows:
                dedupe_key = row.get("dedupe_key")
                if not isinstance(dedupe_key, str) or not dedupe_key:
                    continue
                conn.execute(
                    """
                    INSERT OR IGNORE INTO health_dedupe (
                        dedupe_key, source_family, source_record_id, record_type,
                        start_time, end_time, value_hash, first_import_id,
                        last_seen_import_id, normalized_ref, raw_ref, created_at,
                        updated_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        dedupe_key,
                        row["source_family"],
                        row.get("source_record_id"),
                        row["record_type"],
                        row["start_date"],
                        row.get("end_date"),
                        None,
                        import_id,
                        import_id,
                        f"{import_id}/normalized/{row['month']}.jsonl",
                        None,
                        "2026-08-10T00:00:00Z",
                        "2026-08-10T00:00:00Z",
                    ),
                )


def _days_inclusive(start: date, end: date) -> list[date]:
    return [start + timedelta(days=offset) for offset in range((end - start).days + 1)]


def seed_populated_body_journal(root: Path, *, anchor: date) -> dict[str, Any]:
    """Create the deterministic populated Body journal used only by the corpus.

    The caller must supply a fresh temporary directory.  Every value is synthetic.
    """

    if anchor != date(2026, 8, 1):
        raise ValueError("The Body corpus anchor is fixed at 2026-08-01")
    root.mkdir(parents=True, exist_ok=True)
    _write_json(
        root / "config" / "journal.json",
        {
            "setup": {"completed_at": 1767225600},
            "body": {
                "freshness": {
                    "quiet_days": {"Synthetic Watch": 14, "Synthetic CGM": 14}
                }
            },
        },
    )

    apple_rows: list[dict[str, Any]] = []
    oura_rows: list[dict[str, Any]] = []
    correction_rows: list[dict[str, Any]] = []
    anchor_day = anchor.strftime("%Y%m%d")

    # The first sleep row starts before the 90-day baseline range.  Its night
    # finishes on 2026-05-03, creating that day's first asleep value.
    first_sleep_day = date(2026, 5, 2)
    oura_rows.append(
        _oura_row(
            "oura.sleep",
            "20260502",
            start=_iso(first_sleep_day, "23:00:00"),
            end=_iso(first_sleep_day + timedelta(days=1), "07:00:00"),
            value=28_800,
            unit="s",
            kind="sleep_period",
            metadata={"lowest_heart_rate": 53, "time_in_bed": 28_800},
        )
    )

    baseline_start = date(2026, 5, 3)
    baseline_end = date(2026, 7, 31)
    for offset, current in enumerate(_days_inclusive(baseline_start, baseline_end)):
        day = current.strftime("%Y%m%d")
        apple_rows.extend(
            [
                _health_row(
                    "HKQuantityTypeIdentifierRestingHeartRate",
                    _iso(current, "07:00:00"),
                    value=str(52 + offset % 5),
                    unit="count/min",
                ),
                _health_row(
                    "HKQuantityTypeIdentifierBodyMass",
                    _iso(current, "08:00:00"),
                    value=170.0 + offset * 0.03,
                    unit="lb",
                    source_name="Synthetic Scale",
                ),
            ]
        )
        oura_rows.extend(
            [
                _oura_row("oura.daily_readiness", day, value=70 + offset % 12, unit="score"),
                _oura_row("oura.daily_sleep", day, value=78 + offset % 9, unit="score"),
                _oura_row(
                    "oura.daily_activity",
                    day,
                    value=80 + offset % 10,
                    unit="score",
                    metadata={"steps": 7000 + offset * 13},
                ),
                _oura_row(
                    "oura.sleep",
                    anchor_day if current == anchor - timedelta(days=1) else day,
                    start=_iso(current, "23:00:00"),
                    end=_iso(current + timedelta(days=1), "07:00:00"),
                    value=28_800,
                    unit="s",
                    kind="sleep_period",
                    metadata={"lowest_heart_rate": 52 + offset % 5, "time_in_bed": 28_800},
                ),
            ]
        )

    collision_key = "oura-api:oura.daily_readiness:20260801"
    oura_rows.extend(
        [
            _oura_row(
                "oura.daily_readiness",
                anchor_day,
                value=76,
                unit="score",
                dedupe_key=collision_key,
                metadata={"contributors": {"activity_balance": 74, "sleep_balance": 81}},
            ),
            _oura_row("oura.daily_sleep", anchor_day, value=84, unit="score"),
            _oura_row(
                "oura.daily_activity",
                anchor_day,
                value=88,
                unit="score",
                metadata={"steps": 11_234},
            ),
            _oura_row(
                "oura.workout",
                anchor_day,
                start=_iso(anchor, "16:00:00"),
                end=_iso(anchor, "16:45:00"),
                kind="workout",
                metadata={
                    "activity": "running",
                    "distance": 7200,
                    "distance_unit": "m",
                    "calories": 510,
                    "calories_unit": "kcal",
                },
            ),
        ]
    )
    correction_rows.append(
        _oura_row(
            "oura.daily_readiness",
            anchor_day,
            value=82,
            unit="score",
            dedupe_key=collision_key,
            metadata={"contributors": {"activity_balance": 78, "sleep_balance": 86}},
        )
    )

    for index in range(12):
        apple_rows.append(
            _health_row(
                "HKQuantityTypeIdentifierHeartRate",
                _iso(anchor, f"08:{index * 5:02d}:00"),
                value=str(60 + index),
                unit="count/min",
            )
        )
    apple_rows.extend(
        [
            _health_row(
                "HKQuantityTypeIdentifierRestingHeartRate",
                _iso(anchor, "07:00:00"),
                value="58",
                unit="count/min",
            ),
            *[
                _health_row(
                    "HKQuantityTypeIdentifierBloodGlucose",
                    _iso(anchor, clock),
                    value=str(value),
                    unit="mg/dL",
                    source_name="Synthetic CGM",
                )
                for clock, value in (
                    ("07:00:00", 91),
                    ("11:00:00", 104),
                    ("15:00:00", 98),
                    ("19:00:00", 112),
                )
            ],
            _health_row(
                "HKQuantityTypeIdentifierBodyMass",
                _iso(anchor, "08:00:00"),
                value=172.4,
                unit="lb",
                source_name="Synthetic Scale",
            ),
            _health_row(
                "HKQuantityTypeIdentifierBodyFatPercentage",
                _iso(anchor, "08:05:00"),
                value=0.18,
                unit="%",
                source_name="Synthetic Scale",
            ),
            _health_row(
                "HKQuantityTypeIdentifierStepCount",
                _iso(anchor, "00:00:00"),
                end=_iso(anchor + timedelta(days=1), "00:00:00"),
                value="11234",
                unit="count",
                source_name="Oura Ring",
            ),
            _health_row(
                "HKWorkoutActivityTypeCycling",
                _iso(anchor, "17:00:00"),
                end=_iso(anchor, "18:00:00"),
                kind="workout",
                metadata={
                    "totalDistance": 18.2,
                    "totalDistanceUnit": "km",
                    "totalEnergyBurned": 620,
                    "totalEnergyBurnedUnit": "kcal",
                },
            ),
            _health_row(
                "HKCategoryTypeIdentifierMindfulSession",
                _iso(anchor, "12:00:00"),
                end=_iso(anchor, "12:10:00"),
            ),
            _health_row(
                "HKCategoryTypeIdentifierMindfulSession",
                _iso(anchor, "20:00:00"),
                end=_iso(anchor, "20:15:00"),
            ),
            _health_row(
                "HKQuantityTypeIdentifierHeadphoneAudioExposure",
                _iso(anchor, "13:00:00"),
                value="71",
                unit="dBASPL",
            ),
            _health_row(
                "HKQuantityTypeIdentifierHeadphoneAudioExposure",
                _iso(anchor, "13:30:00"),
                value="76",
                unit="dBASPL",
            ),
            _health_row(
                "HKQuantityTypeIdentifierAppleSleepingWristTemperature",
                _iso(anchor, "22:00:00"),
                value="36.5",
                unit="degC",
                dedupe_key=None,
            ),
        ]
    )

    bundle_manifest = [
        _write_import_bundle(
            root,
            IMPORT_APPLE,
            APPLE_HEALTH,
            apple_rows,
            imported_at="2026-08-10T08:00:00Z",
        ),
        _write_import_bundle(
            root,
            IMPORT_OURA,
            OURA_API,
            oura_rows,
            imported_at="2026-08-10T09:00:00Z",
        ),
        _write_import_bundle(
            root,
            IMPORT_CORRECTION,
            OURA_API,
            correction_rows,
            imported_at="2026-08-10T10:00:00Z",
        ),
    ]
    unreadable = root / "imports" / UNREADABLE_IMPORT
    unreadable.mkdir(parents=True, exist_ok=True)
    (unreadable / "manifest.json").write_text('{"import_id": "unreadable"', encoding="utf-8")
    _write_json(
        root / "imports" / UNKNOWN_IMPORT / "manifest.json",
        {"import_id": UNKNOWN_IMPORT, "source_type": "unknown_health_source"},
    )

    _write_dedupe_database(
        root,
        [
            (IMPORT_APPLE, apple_rows),
            (IMPORT_OURA, oura_rows),
            (IMPORT_CORRECTION, correction_rows),
        ],
    )
    summary_path = root / "chronicle" / anchor_day / APPLE_STREAM / "000000_300"
    summary_path.mkdir(parents=True, exist_ok=True)
    (summary_path / "day_summary_transcript.md").write_text(
        "# Synthetic Body Summary\n\nA deterministic corpus day.\n", encoding="utf-8"
    )

    all_rows = apple_rows + oura_rows + correction_rows
    raw_counts = Counter(row["start_date"][:10].replace("-", "") for row in all_rows)
    dedupe_counts: Counter[str] = Counter()
    seen_keys: set[str] = set()
    for row in all_rows:
        key = row.get("dedupe_key")
        if not isinstance(key, str) or not key or key in seen_keys:
            continue
        seen_keys.add(key)
        dedupe_counts[row["start_date"][:10].replace("-", "")] += 1

    return {
        "anchor_day": anchor_day,
        "valid_import_ids": [IMPORT_APPLE, IMPORT_OURA, IMPORT_CORRECTION],
        "excluded_import_ids": [UNREADABLE_IMPORT, UNKNOWN_IMPORT],
        "raw_normalized_total": len(all_rows),
        "sqlite_normalized_total": sum(dedupe_counts.values()),
        "raw_row_counts_by_start_date": dict(sorted(raw_counts.items())),
        "dedupe_day_counts_by_start_date": dict(sorted(dedupe_counts.items())),
        "special_rows": {
            "collision": {
                "day": anchor_day,
                "dedupe_key": collision_key,
                "older_import_id": IMPORT_OURA,
                "winner_import_id": IMPORT_CORRECTION,
            },
            "cross_midnight_sleep": {
                "start_date": "2026-07-31T23:00:00+00:00",
                "day": anchor_day,
            },
            "no_dedupe_key": {
                "record_type": "HKQuantityTypeIdentifierAppleSleepingWristTemperature",
                "start_date": "2026-08-01T22:00:00+00:00",
            },
            "step_mirror_overlap_day": anchor_day,
        },
        "import_bundles": bundle_manifest,
    }


__all__ = ["seed_populated_body_journal"]
