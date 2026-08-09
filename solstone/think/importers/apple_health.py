# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Apple Health export detector and preview parser."""

from __future__ import annotations

import datetime as dt
import logging
import zipfile
from collections import Counter
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from posixpath import dirname
from typing import Any, BinaryIO, Callable, Iterator
from xml.etree import ElementTree

from solstone.think.importers.file_importer import ImportPreview, ImportResult
from solstone.think.importers.health_dedupe import HealthDedupeRecord
from solstone.think.importers.health_schema import (
    SOURCE_APPLE_HEALTH,
    HealthRecordIdentity,
    SleepStagedInterval,
    friendly_type_name,
    health_record_dedupe_key,
    health_value_hash,
    pick_day_sleep,
)

logger = logging.getLogger(__name__)

_BYTE_PROGRESS_LOG_INTERVAL = 100 * 1024 * 1024

_EXPORT_XML_CANDIDATES = (
    "apple_health_export/export.xml",
    "export.xml",
)

_NORMALIZED_SCHEMA = "solstone.health.apple_health.v1"
_DAY_SUMMARY_SEGMENT = "000000_300"
_DAY_SUMMARY_SIGNAL_LIMIT = 6

_MONTH_NAMES = (
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
)


@dataclass(slots=True)
class _PreviewStats:
    records: int = 0
    workouts: int = 0
    routes: int = 0
    export_cda_present: bool = False
    electrocardiograms: int = 0
    glucose_records: int = 0
    earliest_day: str | None = None
    latest_day: str | None = None
    source_names: set[str] = field(default_factory=set)
    record_types: dict[str, int] = field(default_factory=dict)
    days: set[str] = field(default_factory=set)

    @property
    def item_count(self) -> int:
        return self.records + self.workouts + self.routes

    @property
    def entity_count(self) -> int:
        return 0

    @property
    def date_range(self) -> tuple[str, str]:
        if self.earliest_day is None or self.latest_day is None:
            return ("", "")
        return (self.earliest_day, self.latest_day)

    def add_day(self, day: str | None) -> None:
        if day is None:
            return
        self.days.add(day)
        if self.earliest_day is None or day < self.earliest_day:
            self.earliest_day = day
        if self.latest_day is None or day > self.latest_day:
            self.latest_day = day

    def add_source(self, source_name: str | None) -> None:
        if source_name:
            self.source_names.add(source_name)

    def add_record_type(self, record_type: str | None) -> None:
        if not record_type:
            return
        self.record_types[record_type] = self.record_types.get(record_type, 0) + 1


@dataclass(frozen=True, slots=True)
class _DateWindow:
    start_day: str | None = None
    end_day: str | None = None

    @property
    def has_filter(self) -> bool:
        return self.start_day is not None or self.end_day is not None

    def includes(self, day: str | None) -> bool:
        if day is None:
            return not self.has_filter
        if self.start_day is not None and day < self.start_day:
            return False
        if self.end_day is not None and day > self.end_day:
            return False
        return True


@dataclass(slots=True)
class _NormalizedItem:
    row: dict[str, Any]
    dedupe_record: HealthDedupeRecord
    month: str
    day: str


@dataclass(frozen=True, slots=True)
class _NightSleep:
    """The main sleep session shown on a day card, already resolved."""

    start: dt.datetime
    end: dt.datetime
    entry_count: int
    duration_minutes: float
    in_bed_minutes: float | None = None
    has_stage_detail: bool = False


@dataclass(slots=True)
class _DaySummary:
    day: str
    record_count: int = 0
    workout_count: int = 0
    type_counts: Counter[str] = field(default_factory=Counter)
    sources: set[str] = field(default_factory=set)
    workouts: list[str] = field(default_factory=list)
    glucose_values: list[float] = field(default_factory=list)
    glucose_unit: str | None = None
    # Raw sleep intervals attributed to this day (by start time), per
    # source. The rendered session is resolved by _attach_night_sleep,
    # which also folds in the previous day's intervals.
    sleep_intervals: dict[str, list[SleepStagedInterval]] = field(default_factory=dict)
    night_sleep: _NightSleep | None = None

    def add_source(self, source_name: str | None) -> None:
        if source_name:
            self.sources.add(source_name)

    def add_sleep_interval(
        self,
        source: str,
        start_date: str | None,
        end_date: str | None,
        value: object = None,
    ) -> None:
        start = _normalize_sleep_time(start_date)
        if start is None:
            return
        end = _normalize_sleep_time(end_date) or start
        stage = str(value) if value is not None else None
        self.sleep_intervals.setdefault(source, []).append((start, end, stage))


class AppleHealthImporter:
    name = "apple_health"
    display_name = "Apple Health"
    file_patterns = ["apple_health_export/", "export.xml", "*.zip"]
    description = "Preview Apple Health export.xml data without writing to the journal"

    def detect(self, path: Path) -> bool:
        if path.is_dir():
            return _find_export_xml_in_directory(path) is not None
        if path.is_file() and path.suffix.lower() == ".zip":
            try:
                with zipfile.ZipFile(path) as archive:
                    return _find_export_xml_in_zip(archive.namelist()) is not None
            except zipfile.BadZipFile:
                return False
        return False

    def preview(
        self,
        path: Path,
        *,
        date_from: str | None = None,
        date_to: str | None = None,
    ) -> ImportPreview:
        stats = _preview_export(
            path, date_window=_parse_date_window(date_from, date_to)
        )
        return ImportPreview(
            date_range=stats.date_range,
            item_count=stats.item_count,
            entity_count=stats.entity_count,
            summary=_summary(stats),
        )

    def process(
        self,
        path: Path,
        journal_root: Path,
        *,
        facet: str | None = None,
        import_id: str | None = None,
        progress_callback: Callable | None = None,
        dry_run: bool = False,
        date_from: str | None = None,
        date_to: str | None = None,
        with_day_summaries: bool = False,
        confirm_health_save: bool = False,
    ) -> ImportResult:
        preview = self.preview(path, date_from=date_from, date_to=date_to)
        if dry_run:
            return ImportResult(
                entries_written=0,
                entities_seeded=0,
                files_created=[],
                errors=[],
                summary=f"Dry run only: {preview.summary}",
                date_range=preview.date_range,
            )
        raise RuntimeError(
            "Apple Health save is owned by native body ingress; use journal importer "
            "with --confirm-body-save"
        )


def _find_export_xml_in_directory(path: Path) -> Path | None:
    for candidate in _EXPORT_XML_CANDIDATES:
        candidate_path = path / candidate
        if candidate_path.is_file():
            return candidate_path
    return None


def _find_export_xml_in_zip(names: list[str]) -> str | None:
    normalized = {name.rstrip("/"): name for name in names}
    for candidate in _EXPORT_XML_CANDIDATES:
        if candidate in normalized:
            return normalized[candidate]

    for name in names:
        clean = name.rstrip("/")
        if clean.endswith("/apple_health_export/export.xml"):
            return name
        if clean.endswith("/export.xml") and "apple_health_export/" in clean:
            return name
    return None


class _ByteProgressReader:
    def __init__(self, handle: BinaryIO, path: Path) -> None:
        self._handle = handle
        self._path = path
        self._bytes_read = 0
        self._next_log_at = _BYTE_PROGRESS_LOG_INTERVAL

    def read(self, size: int = -1) -> bytes:
        chunk = self._handle.read(size)
        self._bytes_read += len(chunk)
        while self._next_log_at > 0 and self._bytes_read >= self._next_log_at:
            logger.info(
                "Parsed %d MB (%d bytes) from Apple Health export.xml at %s",
                self._next_log_at // (1024 * 1024),
                self._next_log_at,
                self._path,
            )
            self._next_log_at += _BYTE_PROGRESS_LOG_INTERVAL
        return chunk


def _count_route_files(path: Path) -> int:
    if path.is_dir():
        export_xml = _find_export_xml_in_directory(path)
        if export_xml is None:
            return 0
        route_root = export_xml.parent / "workout-routes"
        if not route_root.is_dir():
            return 0
        return sum(1 for route in route_root.glob("*.gpx") if route.is_file())

    if path.is_file() and path.suffix.lower() == ".zip":
        with zipfile.ZipFile(path) as archive:
            return sum(
                1
                for name in archive.namelist()
                if "/workout-routes/" in name
                and name.lower().endswith(".gpx")
                and not name.endswith("/")
            )
    return 0


def _count_supplemental_files(path: Path) -> tuple[bool, int]:
    if path.is_dir():
        export_xml = _find_export_xml_in_directory(path)
        if export_xml is None:
            return (False, 0)

        export_root = export_xml.parent
        export_cda_present = (export_root / "export_cda.xml").is_file()
        electrocardiograms_root = export_root / "electrocardiograms"
        electrocardiograms = 0
        if electrocardiograms_root.is_dir():
            electrocardiograms = sum(
                1 for child in electrocardiograms_root.iterdir() if child.is_file()
            )
        return (export_cda_present, electrocardiograms)

    if path.is_file() and path.suffix.lower() == ".zip":
        with zipfile.ZipFile(path) as archive:
            names = archive.namelist()
            export_xml = _find_export_xml_in_zip(names)
            if export_xml is None:
                return (False, 0)

            export_root = dirname(export_xml.rstrip("/"))
            export_prefix = f"{export_root}/" if export_root else ""
            export_cda_present = f"{export_prefix}export_cda.xml" in {
                name.rstrip("/") for name in names
            }
            electrocardiograms_prefix = f"{export_prefix}electrocardiograms/"
            electrocardiograms = sum(
                1
                for name in names
                if _is_direct_zip_child(name, electrocardiograms_prefix)
            )
            return (export_cda_present, electrocardiograms)

    return (False, 0)


def _is_direct_zip_child(name: str, parent_prefix: str) -> bool:
    if name.endswith("/") or not name.startswith(parent_prefix):
        return False
    remainder = name[len(parent_prefix) :]
    return bool(remainder) and "/" not in remainder


@contextmanager
def _open_export_xml(path: Path) -> Iterator[BinaryIO]:
    if path.is_dir():
        export_xml = _find_export_xml_in_directory(path)
        if export_xml is None:
            raise FileNotFoundError(f"No Apple Health export.xml found under {path}")
        with export_xml.open("rb") as handle:
            yield handle
        return

    if path.is_file() and path.suffix.lower() == ".zip":
        with zipfile.ZipFile(path) as archive:
            member = _find_export_xml_in_zip(archive.namelist())
            if member is None:
                raise FileNotFoundError(f"No Apple Health export.xml found in {path}")
            with archive.open(member) as handle:
                yield handle
        return

    raise FileNotFoundError(f"No Apple Health export.xml found at {path}")


def _parse_date_window(date_from: str | None, date_to: str | None) -> _DateWindow:
    start_day = _parse_cli_day(date_from, "--date-from") if date_from else None
    end_day = _parse_cli_day(date_to, "--date-to") if date_to else None
    if start_day and end_day and start_day > end_day:
        raise ValueError("--date-from must be on or before --date-to")
    return _DateWindow(start_day=start_day, end_day=end_day)


def _parse_cli_day(value: str, flag_name: str) -> str:
    clean = value.strip()
    for fmt in ("%Y-%m-%d", "%Y%m%d"):
        try:
            return dt.datetime.strptime(clean, fmt).strftime("%Y%m%d")
        except ValueError:
            pass
    raise ValueError(f"{flag_name} must be YYYY-MM-DD or YYYYMMDD")


def _preview_export(
    path: Path, *, date_window: _DateWindow | None = None
) -> _PreviewStats:
    window = date_window or _DateWindow()
    export_cda_present, electrocardiograms = _count_supplemental_files(path)
    route_count = 0 if window.has_filter else _count_route_files(path)
    return _scan_export(
        path,
        date_window=window,
        routes=route_count,
        export_cda_present=export_cda_present,
        electrocardiograms=electrocardiograms,
    )


def _scan_export(
    path: Path,
    *,
    date_window: _DateWindow,
    routes: int = 0,
    export_cda_present: bool = False,
    electrocardiograms: int = 0,
    on_item: Callable[[str, dict[str, str], str | None, int], None] | None = None,
) -> _PreviewStats:
    stats = _PreviewStats(
        routes=routes,
        export_cda_present=export_cda_present,
        electrocardiograms=electrocardiograms,
    )
    item_ordinal = 0
    with _open_export_xml(path) as handle:
        root = None
        inside_workout = False
        progress_handle = _ByteProgressReader(handle, path)
        for event, elem in ElementTree.iterparse(
            progress_handle, events=("start", "end")
        ):
            if event == "start":
                if root is None:
                    root = elem
                if elem.tag == "Workout":
                    inside_workout = True
                continue

            if elem.tag == "Record":
                item_ordinal += 1
                attrs = dict(elem.attrib)
                day = _parse_apple_day(attrs.get("startDate"))
                if not date_window.includes(day):
                    elem.clear()
                    if root is not None:
                        root.clear()
                    continue
                stats.records += 1
                record_type = attrs.get("type")
                stats.add_record_type(record_type)
                stats.add_source(attrs.get("sourceName"))
                stats.add_day(day)
                if _is_glucose_record(record_type):
                    stats.glucose_records += 1
                if on_item is not None:
                    on_item("record", attrs, day, item_ordinal)
            elif elem.tag == "Workout":
                item_ordinal += 1
                attrs = dict(elem.attrib)
                day = _parse_apple_day(attrs.get("startDate"))
                if not date_window.includes(day):
                    elem.clear()
                    if root is not None:
                        root.clear()
                    continue
                stats.workouts += 1
                stats.add_source(attrs.get("sourceName"))
                stats.add_day(day)
                if on_item is not None:
                    on_item("workout", attrs, day, item_ordinal)
                inside_workout = False
            elif inside_workout:
                continue
            elem.clear()
            if root is not None:
                root.clear()
    return stats


def _parse_normalized_items(
    path: Path,
    *,
    import_id: str,
    date_window: _DateWindow,
    raw_ref: str | None,
    progress_callback: Callable | None,
) -> list[_NormalizedItem]:
    items: list[_NormalizedItem] = []
    scanned = 0
    with _open_export_xml(path) as handle:
        root = None
        inside_workout = False
        progress_handle = _ByteProgressReader(handle, path)
        for event, elem in ElementTree.iterparse(
            progress_handle, events=("start", "end")
        ):
            if event == "start":
                if root is None:
                    root = elem
                if elem.tag == "Workout":
                    inside_workout = True
                continue

            if elem.tag in {"Record", "Workout"}:
                scanned += 1
                day = _parse_apple_day(elem.attrib.get("startDate"))
                if date_window.includes(day):
                    attrib = dict(elem.attrib)
                    identity_metadata = _metadata_without_core_fields(attrib, elem.tag)
                    metadata = dict(identity_metadata)
                    if elem.tag == "Workout":
                        for key, value in _workout_statistics_metadata(elem).items():
                            metadata.setdefault(key, value)
                    item_raw_ref = (
                        f"{raw_ref}#{elem.tag.lower()}-{scanned}"
                        if raw_ref is not None
                        else None
                    )
                    items.append(
                        _normalize_element(
                            elem.tag,
                            attrib,
                            import_id=import_id,
                            raw_ref=item_raw_ref,
                            day=day or "",
                            metadata=metadata,
                            identity_metadata=identity_metadata,
                        )
                    )
                if progress_callback and scanned % 10_000 == 0:
                    progress_callback(
                        len(items),
                        scanned,
                        stage="importing",
                    )
                if elem.tag == "Workout":
                    inside_workout = False
            elif inside_workout:
                continue

            elem.clear()
            if root is not None:
                root.clear()

    if progress_callback:
        progress_callback(len(items), scanned, stage="importing")
    return items


def _normalize_element(
    element_tag: str,
    attrib: dict[str, str],
    *,
    import_id: str,
    raw_ref: str | None,
    day: str,
    metadata: dict[str, str] | None = None,
    identity_metadata: dict[str, str] | None = None,
) -> _NormalizedItem:
    if element_tag == "Record":
        kind = "record"
        record_type = attrib.get("type", "Record")
    else:
        kind = "workout"
        record_type = attrib.get("workoutActivityType", "Workout")

    start_time = attrib.get("startDate", "")
    end_time = attrib.get("endDate")
    value = attrib.get("value")
    unit = attrib.get("unit")
    source_name = attrib.get("sourceName")
    if metadata is None:
        metadata = _metadata_without_core_fields(attrib, element_tag)
    if identity_metadata is None:
        identity_metadata = metadata
    dedupe_key = health_record_dedupe_key(
        HealthRecordIdentity(
            source_family=SOURCE_APPLE_HEALTH,
            record_type=record_type,
            start_time=start_time,
            end_time=end_time,
            source_name=source_name,
            value=value,
            unit=unit,
            metadata=identity_metadata,
        )
    )
    month = f"{day[:4]}-{day[4:6]}" if day else "undated"
    normalized_ref = f"normalized/{month}.jsonl#{dedupe_key}"
    row = {
        "schema": _NORMALIZED_SCHEMA,
        "source_family": SOURCE_APPLE_HEALTH,
        "kind": kind,
        "dedupe_key": dedupe_key,
        "record_type": record_type,
        "day": day,
        "start_date": start_time,
        "end_date": end_time,
        "source_name": source_name,
        "source_version": attrib.get("sourceVersion"),
        "unit": unit,
        "value": value,
        "metadata": metadata,
        "raw_ref": raw_ref,
    }
    row = {key: value for key, value in row.items() if value is not None}
    return _NormalizedItem(
        row=row,
        dedupe_record=HealthDedupeRecord(
            dedupe_key=dedupe_key,
            source_family=SOURCE_APPLE_HEALTH,
            record_type=record_type,
            start_time=start_time,
            end_time=end_time,
            value_hash=health_value_hash(
                value=value,
                unit=unit,
                metadata=identity_metadata,
            ),
            first_import_id=import_id,
            last_seen_import_id=import_id,
            normalized_ref=normalized_ref,
            raw_ref=raw_ref,
        ),
        month=month,
        day=day,
    )


def _metadata_without_core_fields(
    attrib: dict[str, str],
    element_tag: str,
) -> dict[str, str]:
    core = {
        "type",
        "workoutActivityType",
        "sourceName",
        "sourceVersion",
        "creationDate",
        "startDate",
        "endDate",
        "unit",
        "value",
    }
    metadata = {key: value for key, value in attrib.items() if key not in core}
    if element_tag == "Workout":
        for key in (
            "duration",
            "durationUnit",
            "totalDistance",
            "totalDistanceUnit",
            "totalEnergyBurned",
            "totalEnergyBurnedUnit",
        ):
            if key in attrib:
                metadata[key] = attrib[key]
    return metadata


def _workout_statistics_metadata(elem: ElementTree.Element) -> dict[str, str]:
    """Extract modern Apple ``WorkoutStatistics`` totals for workout rows."""

    metadata: dict[str, str] = {}
    for child in elem:
        if child.tag != "WorkoutStatistics":
            continue
        stat_type = child.attrib.get("type", "")
        stat_sum = child.attrib.get("sum")
        if stat_sum is None:
            continue
        unit = child.attrib.get("unit")
        if stat_type == "HKQuantityTypeIdentifierActiveEnergyBurned":
            _set_workout_total(
                metadata,
                "totalEnergyBurned",
                "totalEnergyBurnedUnit",
                "totalEnergyBurnedType",
                stat_sum,
                unit,
                stat_type,
            )
        elif "Distance" in stat_type:
            _set_workout_total(
                metadata,
                "totalDistance",
                "totalDistanceUnit",
                "totalDistanceType",
                stat_sum,
                unit,
                stat_type,
            )
    return metadata


def _set_workout_total(
    metadata: dict[str, str],
    value_key: str,
    unit_key: str,
    type_key: str,
    value: str,
    unit: str | None,
    stat_type: str,
) -> None:
    if value_key in metadata:
        return
    metadata[value_key] = value
    if unit:
        metadata[unit_key] = unit
    if stat_type:
        metadata[type_key] = stat_type


def _workout_statistics_by_raw_ref(
    path: Path, raw_ref: str
) -> dict[str, dict[str, str]]:
    """Map ``imports/...#workout-N`` raw refs to recovered workout totals.

    The ``N`` ordinal intentionally matches the existing normalized ``raw_ref``
    assignment: records and workouts advance the counter, child statistics do
    not. Callers can update row metadata without changing dedupe keys.
    """

    by_ref: dict[str, dict[str, str]] = {}
    scanned = 0
    with _open_export_xml(path) as handle:
        root = None
        inside_workout = False
        progress_handle = _ByteProgressReader(handle, path)
        for event, elem in ElementTree.iterparse(
            progress_handle, events=("start", "end")
        ):
            if event == "start":
                if root is None:
                    root = elem
                if elem.tag == "Workout":
                    inside_workout = True
                continue

            if elem.tag in {"Record", "Workout"}:
                scanned += 1
                if elem.tag == "Workout":
                    metadata = _workout_statistics_metadata(elem)
                    if metadata:
                        by_ref[f"{raw_ref}#workout-{scanned}"] = metadata
                    inside_workout = False
            elif inside_workout:
                continue

            elem.clear()
            if root is not None:
                root.clear()
    return by_ref


def _add_to_day_summary(summary: _DaySummary, row: dict[str, Any]) -> None:
    summary.add_source(row.get("source_name"))
    kind = row["kind"]
    record_type = row["record_type"]
    if kind == "workout":
        summary.workout_count += 1
        summary.workouts.append(friendly_type_name(record_type))
        return

    summary.record_count += 1
    summary.type_counts[record_type] += 1
    if _is_glucose_record(record_type):
        glucose_value = _parse_float(row.get("value"))
        if glucose_value is not None:
            summary.glucose_values.append(glucose_value)
            if row.get("unit"):
                summary.glucose_unit = str(row["unit"])
    if "SleepAnalysis" in record_type:
        source = str(row.get("source_name") or row.get("source_family") or "unknown")
        summary.add_sleep_interval(
            source,
            row.get("start_date"),
            row.get("end_date"),
            row.get("value"),
        )


def _normalize_sleep_time(value: str | None) -> dt.datetime | None:
    """Parse a record timestamp for sleep math, mirroring the body app:
    wall-clock components keep the record's own local time; naive values
    get UTC attached only to make comparisons possible."""

    parsed = _parse_apple_datetime(value)
    if parsed is None:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=dt.timezone.utc)
    return parsed


def _previous_day(day: str) -> str | None:
    try:
        parsed = dt.datetime.strptime(day, "%Y%m%d").date()
    except ValueError:
        return None
    return (parsed - dt.timedelta(days=1)).strftime("%Y%m%d")


def _resolve_night_sleep(
    summary: _DaySummary, prev_summary: _DaySummary | None
) -> _NightSleep | None:
    """The day's canonical main sleep session, or None.

    Sleep entries are day-attributed by start time, so the night that ends
    this morning mostly lives on the previous day — both days' intervals
    feed the shared merge + main-session rule (the same one the body day
    page applies), keeping the card and the page on one number.
    """

    try:
        target = dt.datetime.strptime(summary.day, "%Y%m%d").date()
    except ValueError:
        return None
    intervals_by_source: dict[str, list[SleepStagedInterval]] = {}
    for source_summary in (prev_summary, summary):
        if source_summary is None:
            continue
        for source, intervals in source_summary.sleep_intervals.items():
            intervals_by_source.setdefault(source, []).extend(intervals)
    if not intervals_by_source:
        return None
    sleep = pick_day_sleep(intervals_by_source, target)
    if sleep is None or sleep.main is None:
        return None
    start, end = sleep.main
    entry_count = sum(
        1 for s, _, _stage in intervals_by_source[sleep.source] if start <= s <= end
    )
    duration_minutes = sleep.asleep_minutes
    if duration_minutes is None:
        duration_minutes = (end - start).total_seconds() / 60
    return _NightSleep(
        start=start,
        end=end,
        entry_count=entry_count,
        duration_minutes=duration_minutes,
        in_bed_minutes=sleep.in_bed_minutes,
        has_stage_detail=sleep.has_stage_detail,
    )


def _attach_night_sleep(summaries: dict[str, _DaySummary]) -> None:
    """Resolve every day's night sleep in place, feeding each day the
    previous day's intervals so cross-midnight sessions land on the day
    they ended."""

    for day, summary in summaries.items():
        prev_key = _previous_day(day)
        prev_summary = summaries.get(prev_key) if prev_key else None
        summary.night_sleep = _resolve_night_sleep(summary, prev_summary)


def _render_day_summary(summary: _DaySummary, *, import_id: str) -> str:
    """Render an owner-facing day-summary card as markdown.

    Deterministic for a given summary: every section derives only from the
    summary's data, sorted where iteration order could vary. Times stay in
    each record's own offset; friendly type names replace raw identifiers.
    """

    lines = [f"# Body · {_pretty_day(summary.day)}", "", _day_summary_lede(summary)]
    for section_line in (
        _sleep_line(summary),
        _glucose_line(summary),
        _workouts_line(summary),
    ):
        if section_line:
            lines.extend(["", section_line])
    if summary.type_counts:
        lines.extend(["", "**Signals**", ""])
        friendly_counts: Counter[str] = Counter()
        for record_type, count in summary.type_counts.items():
            friendly_counts[friendly_type_name(record_type)] += count
        ranked = sorted(friendly_counts.items(), key=lambda item: (-item[1], item[0]))
        for name, count in ranked[:_DAY_SUMMARY_SIGNAL_LIMIT]:
            lines.append(f"- {name}: {count:,}")
        hidden = len(ranked) - _DAY_SUMMARY_SIGNAL_LIMIT
        if hidden > 0:
            lines.extend(["", f"…and {_count_phrase(hidden, 'more signal')}"])
    lines.extend(["", _day_summary_footer(summary, import_id=import_id)])
    return "\n".join(lines)


def _day_summary_lede(summary: _DaySummary) -> str:
    parts: list[str] = []
    if summary.night_sleep is not None:
        parts.append(
            f"Slept {_format_time_12h(summary.night_sleep.start)}"
            f" – {_format_time_12h(summary.night_sleep.end)}"
        )
    if summary.glucose_values:
        glucose = f"glucose {_glucose_range(summary)}"
        average = _glucose_average(summary)
        if average is not None:
            glucose += f" (avg {average})"
        parts.append(glucose)
    if summary.workout_count:
        parts.append(_count_phrase(summary.workout_count, "workout"))
    if not parts:
        parts.append(
            f"{_count_phrase(summary.record_count, 'entry', 'entries')} across "
            f"{_count_phrase(len(summary.type_counts), 'signal')}"
        )
    sentence = ", ".join(parts) + "."
    return sentence[0].upper() + sentence[1:]


def _sleep_line(summary: _DaySummary) -> str | None:
    sleep = summary.night_sleep
    if sleep is None:
        return None
    return " · ".join(
        (
            f"**Sleep** {_format_time_12h(sleep.start)}"
            f" – {_format_time_12h(sleep.end)}",
            _night_sleep_duration_label(sleep),
            _count_phrase(sleep.entry_count, "sleep entry", "sleep entries"),
        )
    )


def _night_sleep_duration_label(sleep: _NightSleep) -> str:
    duration = _format_duration(dt.timedelta(minutes=sleep.duration_minutes))
    if (
        sleep.has_stage_detail
        and sleep.in_bed_minutes is not None
        and round(sleep.in_bed_minutes) != round(sleep.duration_minutes)
    ):
        in_bed = _format_duration(dt.timedelta(minutes=sleep.in_bed_minutes))
        return f"asleep {duration} · in bed {in_bed}"
    return duration


def _glucose_line(summary: _DaySummary) -> str | None:
    if not summary.glucose_values:
        return None
    parts = [f"**Glucose** {_glucose_range(summary)}"]
    average = _glucose_average(summary)
    if average is not None:
        parts.append(f"avg {average}")
    parts.append(_count_phrase(len(summary.glucose_values), "reading"))
    return " · ".join(parts)


def _workouts_line(summary: _DaySummary) -> str | None:
    if not summary.workout_count:
        return None
    names = ", ".join(sorted(set(summary.workouts)))
    return f"**Workouts** {names} · {_count_phrase(summary.workout_count, 'workout')}"


def _day_summary_footer(summary: _DaySummary, *, import_id: str) -> str:
    total = summary.record_count + summary.workout_count
    parts: list[str] = []
    if summary.sources:
        parts.append(f"Sources: {', '.join(sorted(summary.sources))}")
    parts.append(f"{total:,} records summarized")
    parts.append("brought in via Apple Health")
    parts.append(f"import {import_id}")
    return f"*{' · '.join(parts)}*"


def _glucose_range(summary: _DaySummary) -> str:
    low = _format_glucose_value(min(summary.glucose_values))
    high = _format_glucose_value(max(summary.glucose_values))
    unit = f" {summary.glucose_unit}" if summary.glucose_unit else ""
    if low == high:
        return f"{low}{unit}"
    return f"{low}–{high}{unit}"


def _glucose_average(summary: _DaySummary) -> str | None:
    """Average glucose display value, or None when it adds nothing (flat range)."""

    values = summary.glucose_values
    if min(values) == max(values):
        return None
    text = f"{sum(values) / len(values):,.1f}"
    return text[:-2] if text.endswith(".0") else text


def _count_phrase(count: int, singular: str, plural: str | None = None) -> str:
    label = singular if count == 1 else (plural or f"{singular}s")
    return f"{count:,} {label}"


def _pretty_day(day: str) -> str:
    try:
        parsed = dt.datetime.strptime(day, "%Y%m%d")
    except ValueError:
        return day
    return f"{_MONTH_NAMES[parsed.month - 1]} {parsed.day}, {parsed.year}"


def _format_time_12h(value: dt.datetime) -> str:
    hour = value.hour % 12 or 12
    suffix = "AM" if value.hour < 12 else "PM"
    return f"{hour}:{value.minute:02d} {suffix}"


def _format_duration(duration: dt.timedelta) -> str:
    total_minutes = round(duration.total_seconds() / 60)
    hours, minutes = divmod(total_minutes, 60)
    if hours:
        return f"{hours}h {minutes:02d}m"
    return f"{minutes}m"


def _format_glucose_value(value: float) -> str:
    if value == int(value):
        return f"{int(value):,}"
    return f"{value:g}"

    if not value:
        return None
    value = value.strip()
    for fmt in ("%Y-%m-%d %H:%M:%S %z", "%Y-%m-%d %H:%M:%S"):
        try:
            return dt.datetime.strptime(value, fmt).strftime("%Y%m%d")
        except ValueError:
            pass
    try:
        return dt.datetime.fromisoformat(value.replace("Z", "+00:00")).strftime(
            "%Y%m%d"
        )
    except ValueError:
        return None


def _parse_apple_day(value: str | None) -> str | None:
    if not value:
        return None
    value = value.strip()
    for fmt in ("%Y-%m-%d %H:%M:%S %z", "%Y-%m-%d %H:%M:%S"):
        try:
            return dt.datetime.strptime(value, fmt).strftime("%Y%m%d")
        except ValueError:
            pass
    try:
        return dt.datetime.fromisoformat(value.replace("Z", "+00:00")).strftime(
            "%Y%m%d"
        )
    except ValueError:
        return None


def _parse_apple_datetime(value: str | None) -> dt.datetime | None:
    if not value:
        return None
    value = value.strip()
    for fmt in ("%Y-%m-%d %H:%M:%S %z", "%Y-%m-%d %H:%M:%S"):
        try:
            return dt.datetime.strptime(value, fmt)
        except ValueError:
            pass
    try:
        return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def _is_glucose_record(record_type: str | None) -> bool:
    if not record_type:
        return False
    return "BloodGlucose" in record_type or record_type.endswith("Glucose")


def _parse_float(value: Any | None) -> float | None:
    if value is None:
        return None
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def _summary(stats: _PreviewStats) -> str:
    top_types = sorted(stats.record_types.items(), key=lambda item: (-item[1], item[0]))
    top_type_names = ", ".join(
        name.rsplit("Identifier", 1)[-1] for name, _ in top_types[:3]
    )
    parts = [
        f"records={stats.records}",
        f"workouts={stats.workouts}",
        f"routes={stats.routes}",
        f"glucose={stats.glucose_records}",
        f"sources={len(stats.source_names)}",
        f"export_cda={'present' if stats.export_cda_present else 'absent'}",
        f"electrocardiograms={stats.electrocardiograms}",
    ]
    if top_type_names:
        parts.append(f"top_types={top_type_names}")
    return ", ".join(parts)


importer = AppleHealthImporter()
