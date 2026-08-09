# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Read-only Oura API v2 parse/normalize oracle.

Scope of this module:

- A parse layer that turns Oura-API-v2-shaped JSON documents into
  normalized body rows with stable dedupe keys via the legacy-named
  ``health_schema`` compatibility oracle.
- A ``FileImporter`` whose ``detect``/``preview``/dry-run paths work on
  those documents.

The shipping fetch, OAuth, cursor, bundle publication, and dedupe paths
are Rust-owned. This module deliberately retains no network or write
authority; it remains as the independent differential reader while the
corpus is used to verify the native implementation.

Timezone rule (load-bearing): Oura documents carry their own ``day``
field, already attributed by Oura (a night belongs to the day it ended,
matching the journal's cross-midnight canon; ``enhanced_tag`` spells its
day ``start_day`` — see ``_DOCUMENT_DAY_FIELDS``). The journal day IS
Oura's day field verbatim — never recomputed against local time, and
document datetimes (sleep bedtimes, workout/session intervals, tag
times) are wearer-local offset instants kept verbatim. The instant-only
series (``heartrate``, and ``blood_glucose`` by pinned assumption — see
``_normalize_item``) are the exception: they carry no ``day`` field and
Oura returns UTC instants, so samples are converted to the owner's
journal timezone for ``start_date`` and day/month assignment while the
raw timestamp stays in ``source_record_id`` for stable dedupe.

Endpoint roster: ``SYNC_ENDPOINTS`` mirrors what the native engine polls;
``_PARTNER_GATED_ENDPOINTS`` (blood_glucose) stay fully wired for parse/
normalize/dedupe but are never fetched — Oura grants their scope only to
partner integrations (2026-07 portal finding).

Design doc: ``docs/design/oura-import.md`` (Codex outputs, 2026-07-03
check-m-2), amended by the locked morning decisions O-1..O-9 (O-5
amended to C: the AH-mirror overlap endpoints ``heartrate`` and
``daily_activity`` are imported; presentation precedence is a
body-app concern).
"""

from __future__ import annotations

import datetime as dt
import json
import logging
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Final, Iterable, Mapping
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from solstone.think.importers.file_importer import ImportPreview, ImportResult
from solstone.think.importers.health_dedupe import HealthDedupeRecord
from solstone.think.importers.health_schema import (
    SOURCE_OURA_API,
    HealthRecordIdentity,
    health_record_dedupe_key,
    health_value_hash,
)

logger = logging.getLogger(__name__)

NORMALIZED_SCHEMA: Final = "solstone.health.oura.v1"
DESIGN_DOC: Final = "docs/design/oura-import.md"
SOURCE_LABEL: Final = "Oura (API)"
_DAY_SUMMARY_SOURCE_LINE: Final = "brought in via the Oura API"

_MONTH_NAMES: Final = (
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

# Oura API v2 usercollection endpoints this importer understands. Each maps
# to the record types its documents normalize into. Fixture files are named
# ``<endpoint>.json`` and carry the API page shape ``{"data": [...],
# "next_token": ...}``. ``daily_activity`` and ``heartrate`` overlap the
# Apple Health mirror and are imported anyway per decision O-5C.
ENDPOINT_RECORD_TYPES: Final[Mapping[str, tuple[str, ...]]] = {
    "daily_sleep": ("oura.daily_sleep",),
    "daily_readiness": ("oura.daily_readiness", "oura.temperature_deviation"),
    "daily_resilience": ("oura.daily_resilience",),
    "daily_stress": ("oura.daily_stress",),
    "daily_spo2": ("oura.daily_spo2",),
    "sleep": ("oura.sleep",),
    "daily_activity": ("oura.daily_activity",),
    "heartrate": ("oura.heartrate",),
    "daily_cardiovascular_age": ("oura.daily_cardiovascular_age",),
    "blood_glucose": ("oura.blood_glucose",),
    # 2026-07-07 granted-scope expansion, shapes verified against
    # openapi-1.35 AND live probes (workout/session/enhanced_tag returned
    # real rows; vO2_max is documented but empty for this account). The
    # ``vO2_max`` key doubles as the route segment — that casing is
    # exact (lowercase ``vo2_max`` 404s live); the record type uses the
    # clean lowercase spelling.
    "workout": ("oura.workout",),
    "session": ("oura.session",),
    "enhanced_tag": ("oura.enhanced_tag",),
    "vO2_max": ("oura.vo2_max",),
}

# Read-only parity oracle for the endpoints the native sync engine polls.
SYNC_ENDPOINTS: Final[tuple[str, ...]] = (
    "daily_readiness",
    "daily_sleep",
    "daily_stress",
    "daily_resilience",
    "daily_spo2",
    "sleep",
    "daily_activity",
    "heartrate",
    "daily_cardiovascular_age",
    "workout",
    "session",
    "enhanced_tag",
    "vO2_max",
)

# Endpoints whose scope Oura grants only to partner integrations. The
# owner's developer portal (checked 2026-07) shows every grantable scope
# already enabled and NO ``metabolic`` option — blood_glucose is
# partner-gated (Tidepool-class integrations only), so polling it 401s
# on every run forever. It stays out of SYNC_ENDPOINTS so hourly runs
# stop reporting the unauthorized error each cycle, while parse,
# normalization, dedupe-oracle, fixture, and differential coverage stays
# available for a future partner grant or file import.
_PARTNER_GATED_ENDPOINTS: Final[tuple[str, ...]] = ("blood_glucose",)

# Series endpoints that paginate by datetime (start_datetime/end_datetime),
# not by day; their rows carry no document id or day field. blood_glucose
# membership is a pinned assumption (see _SERIES_REQUIRED_FIELDS).
_DATETIME_PAGED_ENDPOINTS: Final = frozenset({"heartrate", "blood_glucose"})

# Document endpoints whose day attribution field is not literally
# ``day``. enhanced_tag is the one exception (openapi-1.35
# EnhancedTagModel, live-confirmed 2026-07-07): rows carry required
# ``start_day`` (plus nullable ``end_day``) instead — the journal day is
# Oura's ``start_day`` verbatim, matching the day-verbatim rule.
_DOCUMENT_DAY_FIELDS: Final[Mapping[str, str]] = {"enhanced_tag": "start_day"}

# Required row fields per instant-series endpoint: (timestamp field,
# value field). heartrate is documented (openapi-1.35 PublicHeartRateRow:
# timestamp + bpm). blood_glucose is ABSENT from the published spec
# (verified 2026-07-07: openapi-1.35 has no blood_glucose path or schema,
# though the live route exists — missing-token 400 vs 404 for bogus
# routes); its shape here is pinned to Oura's series-row convention of a
# UTC ``timestamp`` plus a domain-named value field (heartrate -> bpm,
# ring_battery_level -> level, hence blood_glucose -> glucose, mg/dL per
# Oura's Stelo integration). blood_glucose is partner-gated (see
# _PARTNER_GATED_ENDPOINTS) so no fetch can currently falsify the pin —
# it holds for a future partner grant or file import, and a mismatch
# fails loudly in parse_endpoint_document, naming the missing fields.
_SERIES_REQUIRED_FIELDS: Final[Mapping[str, tuple[str, str]]] = {
    "heartrate": ("timestamp", "bpm"),
    "blood_glucose": ("timestamp", "glucose"),
}


class OuraDocumentError(ValueError):
    """Raised when an Oura-shaped JSON document does not match the API shape."""


@dataclass(frozen=True, slots=True)
class OuraNormalizedItem:
    """One normalized row plus its importer-owned dedupe record."""

    row: dict[str, Any]
    dedupe_record: HealthDedupeRecord
    day: str
    month: str


# ---------------------------------------------------------------------------
# Parse layer (pure reads; no journal access)
# ---------------------------------------------------------------------------


def parse_endpoint_document(
    endpoint: str, document: Mapping[str, Any]
) -> list[dict[str, Any]]:
    """Validate one API-page-shaped document and return its data items.

    The Oura API v2 returns ``{"data": [...], "next_token": ...}`` pages.
    Document-shaped endpoints require ``id`` and their day field
    (``day``, except ``_DOCUMENT_DAY_FIELDS`` overrides — enhanced_tag
    carries ``start_day``) on every item; instant-series endpoints
    (``_SERIES_REQUIRED_FIELDS``) instead require a timestamp plus their
    value field (their rows carry no document id — see
    ``_normalize_item``).
    """

    if endpoint not in ENDPOINT_RECORD_TYPES:
        supported = ", ".join(sorted(ENDPOINT_RECORD_TYPES))
        raise OuraDocumentError(
            f"Unsupported Oura endpoint {endpoint!r}; supported: {supported}"
        )
    data = document.get("data")
    if not isinstance(data, list):
        raise OuraDocumentError(
            f"Oura {endpoint} document must carry a 'data' list, "
            f"got {type(data).__name__}"
        )
    series_fields = _SERIES_REQUIRED_FIELDS.get(endpoint)
    day_field = _DOCUMENT_DAY_FIELDS.get(endpoint, "day")
    items: list[dict[str, Any]] = []
    for index, item in enumerate(data):
        if not isinstance(item, dict):
            raise OuraDocumentError(
                f"Oura {endpoint} data[{index}] must be an object, "
                f"got {type(item).__name__}"
            )
        if series_fields is not None:
            timestamp_field, value_field = series_fields
            if not item.get(timestamp_field) or item.get(value_field) is None:
                raise OuraDocumentError(
                    f"Oura {endpoint} data[{index}] is missing "
                    f"{timestamp_field!r} or {value_field!r}"
                )
        elif not item.get("id") or not item.get(day_field):
            raise OuraDocumentError(
                f"Oura {endpoint} data[{index}] is missing 'id' or {day_field!r}"
            )
        items.append(item)
    return items


def parse_oura_bundle(path: Path) -> dict[str, list[dict[str, Any]]]:
    """Read ``<endpoint>.json`` documents from a directory or single file.

    Returns ``{endpoint: [item, ...]}`` for every supported endpoint file
    present. Unknown filenames are ignored so a bundle directory may carry
    a README; a malformed supported file raises.
    """

    if path.is_file():
        endpoint = path.stem
        document = _load_json_document(path)
        return {endpoint: parse_endpoint_document(endpoint, document)}

    if not path.is_dir():
        raise FileNotFoundError(f"No Oura document bundle at {path}")

    bundle: dict[str, list[dict[str, Any]]] = {}
    for child in sorted(path.iterdir()):
        if not child.is_file() or child.suffix.lower() != ".json":
            continue
        if child.stem not in ENDPOINT_RECORD_TYPES:
            continue
        document = _load_json_document(child)
        bundle[child.stem] = parse_endpoint_document(child.stem, document)
    return bundle


def _load_json_document(path: Path) -> dict[str, Any]:
    try:
        loaded = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise OuraDocumentError(f"Invalid JSON in {path.name}: {exc}") from exc
    if not isinstance(loaded, dict):
        raise OuraDocumentError(
            f"{path.name} must contain a JSON object, got {type(loaded).__name__}"
        )
    return loaded


def parse_oura_day(value: object) -> str | None:
    """Normalize an Oura ``day`` field (``YYYY-MM-DD``) to ``YYYYMMDD``.

    The journal day IS Oura's day field verbatim — Oura attributes each
    sleep document to the day the night ended, which matches the journal's
    cross-midnight canon, so the value passes through with no local-time
    recomputation.
    """

    if not isinstance(value, str):
        return None
    try:
        return dt.datetime.strptime(value.strip(), "%Y-%m-%d").strftime("%Y%m%d")
    except ValueError:
        return None


def _owner_timezone_for_journal(journal_root: Path | None = None) -> ZoneInfo:
    """Resolve the owner timezone for Oura instant-only endpoints.

    Journal config wins when a journal root is available. Pure preview and
    fixture calls fall back through the existing host/system helper.
    """

    if journal_root is not None:
        try:
            from solstone.think.journal_config import read_journal_config

            config = read_journal_config(journal_root)
        except Exception as exc:  # pragma: no cover - defensive fallback
            logger.warning(
                "Could not read journal timezone from %s; falling back to host timezone: %s",
                journal_root,
                exc,
            )
        else:
            configured = str(config.get("identity", {}).get("timezone") or "").strip()
            if configured:
                try:
                    return ZoneInfo(configured)
                except ZoneInfoNotFoundError:
                    logger.warning(
                        "Invalid identity.timezone %r; falling back to host timezone",
                        configured,
                    )

    from solstone.think.utils import get_owner_timezone

    return get_owner_timezone()


def _timezone_label(owner_timezone: dt.tzinfo) -> str:
    key = getattr(owner_timezone, "key", None)
    if isinstance(key, str) and key:
        return key
    return str(owner_timezone)


def _parse_oura_instant(value: str) -> dt.datetime:
    raw = value.strip()
    normalized = raw[:-1] + "+00:00" if raw.endswith("Z") else raw
    parsed = dt.datetime.fromisoformat(normalized)
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=dt.timezone.utc)
    return parsed


def _owner_local_timestamp(value: str, owner_timezone: dt.tzinfo) -> str:
    return _parse_oura_instant(value).astimezone(owner_timezone).isoformat()


# ---------------------------------------------------------------------------
# Normalization
# ---------------------------------------------------------------------------


def normalize_bundle(
    bundle: Mapping[str, Iterable[Mapping[str, Any]]],
    *,
    import_id: str,
    raw_ref_root: str | None,
    owner_timezone: dt.tzinfo | None = None,
) -> list[OuraNormalizedItem]:
    """Normalize parsed endpoint items into rows with stable dedupe keys."""

    resolved_timezone = owner_timezone or _owner_timezone_for_journal()
    items: list[OuraNormalizedItem] = []
    for endpoint in sorted(bundle):
        for index, item in enumerate(bundle[endpoint], start=1):
            raw_ref = (
                f"{raw_ref_root}#{endpoint}-{index}"
                if raw_ref_root is not None
                else None
            )
            items.extend(
                _normalize_item(
                    endpoint,
                    dict(item),
                    import_id=import_id,
                    raw_ref=raw_ref,
                    owner_timezone=resolved_timezone,
                )
            )
    return items


def _normalize_item(
    endpoint: str,
    item: dict[str, Any],
    *,
    import_id: str,
    raw_ref: str | None,
    owner_timezone: dt.tzinfo,
) -> list[OuraNormalizedItem]:
    day = parse_oura_day(item.get(_DOCUMENT_DAY_FIELDS.get(endpoint, "day"))) or ""
    rows: list[OuraNormalizedItem] = []
    if endpoint == "daily_sleep":
        rows.append(
            _build_item(
                record_type="oura.daily_sleep",
                kind="daily_summary",
                source_record_id=str(item["id"]),
                day=day,
                start_time=str(item.get("timestamp") or item["day"]),
                value=item.get("score"),
                unit="score",
                metadata=_pick(item, ("contributors",)),
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
    elif endpoint == "daily_readiness":
        rows.append(
            _build_item(
                record_type="oura.daily_readiness",
                kind="daily_summary",
                source_record_id=str(item["id"]),
                day=day,
                start_time=str(item.get("timestamp") or item["day"]),
                value=item.get("score"),
                unit="score",
                metadata=_pick(item, ("contributors", "temperature_trend_deviation")),
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
        if item.get("temperature_deviation") is not None:
            rows.append(
                _build_item(
                    record_type="oura.temperature_deviation",
                    kind="daily_summary",
                    # The deviation shares the readiness document; a suffix
                    # keeps its identity distinct from the score row.
                    source_record_id=f"{item['id']}/temperature_deviation",
                    day=day,
                    start_time=str(item.get("timestamp") or item["day"]),
                    value=item.get("temperature_deviation"),
                    unit="degC",
                    metadata={},
                    import_id=import_id,
                    raw_ref=raw_ref,
                )
            )
    elif endpoint == "daily_resilience":
        rows.append(
            _build_item(
                record_type="oura.daily_resilience",
                kind="daily_summary",
                source_record_id=str(item["id"]),
                day=day,
                start_time=str(item.get("timestamp") or item["day"]),
                value=item.get("level"),
                unit=None,
                metadata=_pick(item, ("contributors",)),
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
    elif endpoint == "daily_stress":
        rows.append(
            _build_item(
                record_type="oura.daily_stress",
                kind="daily_summary",
                source_record_id=str(item["id"]),
                day=day,
                start_time=str(item.get("timestamp") or item["day"]),
                value=item.get("day_summary"),
                unit=None,
                metadata=_pick(item, ("stress_high", "recovery_high")),
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
    elif endpoint == "daily_spo2":
        spo2 = item.get("spo2_percentage")
        average = spo2.get("average") if isinstance(spo2, dict) else None
        rows.append(
            _build_item(
                record_type="oura.daily_spo2",
                kind="daily_summary",
                source_record_id=str(item["id"]),
                day=day,
                start_time=str(item.get("timestamp") or item["day"]),
                value=average,
                unit="%",
                metadata=_pick(item, ("breathing_disturbance_index",)),
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
    elif endpoint == "sleep":
        rows.append(
            _build_item(
                record_type="oura.sleep",
                kind="sleep_period",
                source_record_id=str(item["id"]),
                day=day,
                start_time=str(item.get("bedtime_start") or item["day"]),
                end_time=(
                    str(item["bedtime_end"]) if item.get("bedtime_end") else None
                ),
                value=item.get("total_sleep_duration"),
                unit="s",
                metadata=_pick(
                    item,
                    (
                        "type",
                        "deep_sleep_duration",
                        "rem_sleep_duration",
                        "light_sleep_duration",
                        "awake_time",
                        "time_in_bed",
                        "efficiency",
                        "latency",
                        "average_heart_rate",
                        "lowest_heart_rate",
                        "average_hrv",
                        "average_breath",
                        "sleep_phase_5_min",
                    ),
                ),
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
    elif endpoint == "daily_activity":
        # AH-mirror overlap endpoint, imported per decision O-5C. Oura's
        # activity score and totals are Oura's numbers; any precedence
        # against mirrored Apple Health steps is presentation-side.
        rows.append(
            _build_item(
                record_type="oura.daily_activity",
                kind="daily_summary",
                source_record_id=str(item["id"]),
                day=day,
                start_time=str(item.get("timestamp") or item["day"]),
                value=item.get("score"),
                unit="score",
                metadata=_pick(
                    item,
                    (
                        "contributors",
                        "steps",
                        "active_calories",
                        "total_calories",
                        "equivalent_walking_distance",
                        "high_activity_time",
                        "medium_activity_time",
                        "low_activity_time",
                        "sedentary_time",
                        "resting_time",
                        "non_wear_time",
                        "average_met_minutes",
                    ),
                ),
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
    elif endpoint == "heartrate":
        # AH-mirror overlap series, imported per decision O-5C. Heartrate
        # rows carry no document id and no day field: identity is
        # synthesized from Oura's raw timestamp plus the sample source,
        # while day/month assignment uses the owner's local timezone.
        timestamp = str(item["timestamp"])
        local_timestamp = _owner_local_timestamp(timestamp, owner_timezone)
        source = str(item.get("source") or "unknown")
        rows.append(
            _build_item(
                record_type="oura.heartrate",
                kind="sample",
                source_record_id=f"heartrate/{timestamp}/{source}",
                day=parse_oura_day(local_timestamp[:10]) or "",
                start_time=local_timestamp,
                value=item.get("bpm"),
                unit="bpm",
                metadata={
                    "source": source,
                    "raw_timestamp": timestamp,
                    "timezone": _timezone_label(owner_timezone),
                },
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
    elif endpoint == "daily_cardiovascular_age":
        # Documented day-granularity endpoint (openapi-1.35
        # PublicDailyCardiovascularAge: id + day required,
        # pulse_wave_velocity m/s and vascular_age years nullable). The
        # journal day is Oura's ``day`` field verbatim, like every other
        # daily document.
        rows.append(
            _build_item(
                record_type="oura.daily_cardiovascular_age",
                kind="daily_summary",
                source_record_id=str(item["id"]),
                day=day,
                start_time=str(item.get("timestamp") or item["day"]),
                value=item.get("vascular_age"),
                unit="years",
                metadata=_pick(item, ("pulse_wave_velocity",)),
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
    elif endpoint == "blood_glucose":
        # PINNED ASSUMPTION (endpoint absent from openapi-1.35; see
        # _SERIES_REQUIRED_FIELDS): blood_glucose is an instant series
        # shaped like heartrate — no document id, no day field, and
        # ``timestamp`` is a UTC instant (the spec types every series-row
        # timestamp as UtcDateTime; heartrate empirically returns UTC-Z,
        # commit e42d67a7). Day/month assignment therefore converts to
        # the owner's journal timezone while the raw timestamp stays in
        # source_record_id for stable dedupe; values are mg/dL (Oura's
        # Stelo integration). The first post-reauthorization sync
        # confirms or falsifies each pin — tests mark the fixture side.
        timestamp = str(item["timestamp"])
        local_timestamp = _owner_local_timestamp(timestamp, owner_timezone)
        glucose_metadata: dict[str, Any] = {
            "raw_timestamp": timestamp,
            "timezone": _timezone_label(owner_timezone),
        }
        if item.get("source") is not None:
            glucose_metadata["source"] = str(item["source"])
        rows.append(
            _build_item(
                record_type="oura.blood_glucose",
                kind="sample",
                # One CGM stream per account: the raw timestamp alone is
                # the stable sample identity (unlike heartrate, whose
                # sources can collide on a timestamp).
                source_record_id=f"blood_glucose/{timestamp}",
                day=parse_oura_day(local_timestamp[:10]) or "",
                start_time=local_timestamp,
                value=item.get("glucose"),
                unit="mg/dL",
                metadata=glucose_metadata,
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
    elif endpoint == "workout":
        # PublicWorkout (openapi-1.35; live-confirmed 2026-07-07):
        # required id/activity/day/start_datetime/end_datetime/intensity/
        # source; calories (kcal), distance (m), and label nullable.
        # Datetimes are LocalizedDateTime — wearer-local offsets (live
        # rows carry -04:00..-07:00, never UTC-Z) — so they pass through
        # verbatim like sleep periods, and the journal day is Oura's
        # ``day`` verbatim (a 23:12 workout stays on its local day even
        # though its UTC instant crosses midnight). An event row, like
        # Apple Health workouts: no scalar value — calories/distance are
        # metadata facts and duration derives from the interval at render
        # time. Same-ring-two-pipes precedence against the AH mirror's
        # HKWorkoutActivityType* rows is presentation-side (O-5C).
        rows.append(
            _build_item(
                record_type="oura.workout",
                kind="workout",
                source_record_id=str(item["id"]),
                day=day,
                start_time=str(item.get("start_datetime") or item["day"]),
                end_time=(
                    str(item["end_datetime"]) if item.get("end_datetime") else None
                ),
                value=None,
                unit=None,
                metadata=_pick(
                    item,
                    (
                        "activity",
                        "intensity",
                        "source",
                        "label",
                        "calories",
                        "distance",
                    ),
                ),
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
    elif endpoint == "session":
        # PublicSession (openapi-1.35; live-confirmed): required id/day/
        # start_datetime/end_datetime/type (breathing|meditation|nap|
        # relaxation|rest|body_status); mood and the heart_rate/
        # heart_rate_variability/motion_count sample blocks nullable.
        # Datetimes are wearer-local offsets, verbatim. The sample blocks
        # ({interval, items[], timestamp}) stay in the raw page — reached
        # through raw_ref — never in normalized metadata: this is an
        # event row, not a series carrier.
        rows.append(
            _build_item(
                record_type="oura.session",
                kind="session",
                source_record_id=str(item["id"]),
                day=day,
                start_time=str(item.get("start_datetime") or item["day"]),
                end_time=(
                    str(item["end_datetime"]) if item.get("end_datetime") else None
                ),
                value=None,
                unit=None,
                metadata=_pick(item, ("type", "mood")),
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
    elif endpoint == "enhanced_tag":
        # EnhancedTagModel (openapi-1.35; live-confirmed): required id/
        # start_time/start_day — the one document endpoint with no
        # ``day`` field (see _DOCUMENT_DAY_FIELDS). The journal day is
        # Oura's ``start_day`` verbatim; start/end times are wearer-local
        # offsets, verbatim. tag_type_code/comment/custom_name are the
        # owner's own note content — metadata facts, never a value.
        rows.append(
            _build_item(
                record_type="oura.enhanced_tag",
                kind="tag",
                source_record_id=str(item["id"]),
                day=day,
                start_time=str(item.get("start_time") or item["start_day"]),
                end_time=str(item["end_time"]) if item.get("end_time") else None,
                value=None,
                unit=None,
                metadata=_pick(
                    item,
                    ("tag_type_code", "comment", "custom_name", "end_day"),
                ),
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
    elif endpoint == "vO2_max":
        # PublicVO2Max (openapi-1.35): required id/day/timestamp/vo2_max
        # (integer). Zero rows on this account today, so the shape is
        # documented-only until data appears; the route casing is exactly
        # ``vO2_max`` (lowercase 404s live — verified 2026-07-07). Day is
        # Oura's verbatim; ``timestamp`` is a wearer-local offset
        # instant, verbatim. VO2 max is mL/kg/min by definition (the spec
        # carries no unit field).
        rows.append(
            _build_item(
                record_type="oura.vo2_max",
                kind="daily_summary",
                source_record_id=str(item["id"]),
                day=day,
                start_time=str(item.get("timestamp") or item["day"]),
                value=item.get("vo2_max"),
                unit="mL/kg/min",
                metadata={},
                import_id=import_id,
                raw_ref=raw_ref,
            )
        )
    else:  # pragma: no cover - parse_endpoint_document rejects these first
        raise OuraDocumentError(f"Unsupported Oura endpoint {endpoint!r}")
    return rows


def _pick(item: Mapping[str, Any], keys: tuple[str, ...]) -> dict[str, Any]:
    return {key: item[key] for key in keys if item.get(key) is not None}


def _build_item(
    *,
    record_type: str,
    kind: str,
    source_record_id: str,
    day: str,
    start_time: str,
    end_time: str | None = None,
    value: Any | None,
    unit: str | None,
    metadata: dict[str, Any],
    import_id: str,
    raw_ref: str | None,
) -> OuraNormalizedItem:
    dedupe_key = health_record_dedupe_key(
        HealthRecordIdentity(
            source_family=SOURCE_OURA_API,
            record_type=record_type,
            start_time=start_time,
            end_time=end_time,
            source_record_id=source_record_id,
            value=value,
            unit=unit,
            metadata=metadata,
        )
    )
    month = f"{day[:4]}-{day[4:6]}" if day else "undated"
    # Document endpoints keep Oura's own offsets. The no-day heartrate
    # endpoint passes owner-local ``start_time`` in from the caller.
    row = {
        "schema": NORMALIZED_SCHEMA,
        "source_family": SOURCE_OURA_API,
        "kind": kind,
        "dedupe_key": dedupe_key,
        "record_type": record_type,
        "day": day,
        "start_date": start_time,
        "end_date": end_time,
        "source_record_id": source_record_id,
        "unit": unit,
        "value": value,
        "metadata": metadata,
        "raw_ref": raw_ref,
    }
    row = {key: val for key, val in row.items() if val is not None}
    return OuraNormalizedItem(
        row=row,
        dedupe_record=HealthDedupeRecord(
            dedupe_key=dedupe_key,
            source_family=SOURCE_OURA_API,
            record_type=record_type,
            start_time=start_time,
            end_time=end_time,
            source_record_id=source_record_id,
            value_hash=health_value_hash(value=value, unit=unit, metadata=metadata),
            first_import_id=import_id,
            last_seen_import_id=import_id,
            normalized_ref=f"normalized/{month}.jsonl#{dedupe_key}",
            raw_ref=raw_ref,
        ),
        day=day,
        month=month,
    )


# ---------------------------------------------------------------------------
# Day-summary rendering oracle (pure; no write authority)
# ---------------------------------------------------------------------------


def render_day_summary(
    day: str, rows: Iterable[Mapping[str, Any]], *, import_id: str
) -> str:
    """Render one day's Oura rows as owner-facing markdown.

    Every line is an attributed fact — the score or label is Oura's, named
    as Oura's, never glossed or interpreted. Deterministic for given rows.
    """

    facts: list[str] = []
    for row in sorted(rows, key=lambda r: str(r.get("record_type") or "")):
        fact = _fact_line(row)
        if fact:
            facts.append(fact)
    lines = [f"# Body · {_pretty_day(day)}", ""]
    if facts:
        lines.extend(facts)
    else:
        lines.append("No Oura entries for this day.")
    lines.extend(
        ["", f"*{_DAY_SUMMARY_SOURCE_LINE} · import {import_id}*"],
    )
    return "\n".join(lines)


def _fact_line(row: Mapping[str, Any]) -> str | None:
    record_type = str(row.get("record_type") or "")
    value = row.get("value")
    if value is None:
        return None
    if record_type == "oura.daily_readiness":
        return f"Readiness {value} · Oura's score"
    if record_type == "oura.daily_sleep":
        return f"Sleep score {value} · Oura's score"
    if record_type == "oura.daily_resilience":
        return f"Resilience {value} · Oura's label"
    if record_type == "oura.daily_stress":
        return f"Day stress summary {value} · Oura's label"
    if record_type == "oura.daily_spo2":
        return f"Nightly blood oxygen {value}% · Oura's average"
    if record_type == "oura.daily_activity":
        return f"Activity score {value} · Oura's score"
    if record_type == "oura.daily_cardiovascular_age":
        return f"Cardiovascular age {value} · Oura's estimate"
    if record_type == "oura.vo2_max":
        return f"VO2 max {value} · Oura's estimate"
    if record_type == "oura.temperature_deviation":
        return f"Temperature deviation {value:+.2f} °C · Oura's measurement"
    if record_type == "oura.sleep":
        duration = _format_duration_seconds(value)
        stages = _stage_phrase(row.get("metadata") or {})
        line = f"Sleep {duration} · Oura's staging"
        if stages:
            line += f" — {stages}"
        return line
    # Heartrate and blood-glucose samples are series, not day facts —
    # never summarized into prose here (no derived aggregates presented
    # as ours). Workouts, sessions, and tags are value-less event rows —
    # they exit at the value-is-None check above; day surfaces render
    # them from their kind, never from prose here.
    return None


def _stage_phrase(metadata: Mapping[str, Any]) -> str:
    parts: list[str] = []
    for key, label in (
        ("deep_sleep_duration", "deep"),
        ("rem_sleep_duration", "REM"),
        ("light_sleep_duration", "light"),
        ("awake_time", "awake"),
    ):
        seconds = metadata.get(key)
        if seconds is not None:
            parts.append(f"{label} {_format_duration_seconds(seconds)}")
    return ", ".join(parts)


def _format_duration_seconds(value: Any) -> str:
    total_minutes = round(float(value) / 60)
    hours, minutes = divmod(total_minutes, 60)
    if hours:
        return f"{hours}h {minutes:02d}m"
    return f"{minutes}m"


def _pretty_day(day: str) -> str:
    try:
        parsed = dt.datetime.strptime(day, "%Y%m%d")
    except ValueError:
        return day
    return f"{_MONTH_NAMES[parsed.month - 1]} {parsed.day}, {parsed.year}"


# ---------------------------------------------------------------------------
# File importer (detect/preview/dry-run active; save is gated + seamed)
# ---------------------------------------------------------------------------


class OuraImporter:
    name = "oura"
    display_name = "Oura"
    file_patterns = ["daily_sleep.json", "daily_readiness.json", "sleep.json"]
    description = (
        "Preview Oura API v2 JSON documents (synthetic fixtures; "
        "save path is a later, gated phase)"
    )

    def detect(self, path: Path) -> bool:
        try:
            bundle = parse_oura_bundle(path)
        except (OuraDocumentError, FileNotFoundError, OSError):
            return False
        return bool(bundle)

    def preview(self, path: Path) -> ImportPreview:
        bundle = parse_oura_bundle(path)
        items = normalize_bundle(bundle, import_id="preview", raw_ref_root="preview")
        days = sorted({item.day for item in items if item.day})
        endpoint_counts = ", ".join(
            f"{endpoint}={len(list(bundle[endpoint]))}" for endpoint in sorted(bundle)
        )
        return ImportPreview(
            date_range=(days[0], days[-1]) if days else ("", ""),
            item_count=len(items),
            entity_count=0,
            summary=(
                f"{endpoint_counts or 'no documents'}, "
                f"rows={len(items)}, days={len(days)}, "
                f"source_family={SOURCE_OURA_API}"
            ),
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
        confirm_health_save: bool = False,
    ) -> ImportResult:
        if dry_run:
            preview = self.preview(path)
            return ImportResult(
                entries_written=0,
                entities_seeded=0,
                files_created=[],
                errors=[],
                summary=f"Dry run only: {preview.summary}",
                date_range=preview.date_range,
            )
        raise RuntimeError(
            "Oura save is owned by native body ingress; use "
            "journal importer --sync oura --save --confirm-body-save"
        )


importer = OuraImporter()
