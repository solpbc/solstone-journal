# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

SCHEMA_VERSION = 5

DAY_FIELDS = (
    "transcript_sessions",
    "transcript_segments",
    "transcript_duration",
    "percept_sessions",
    "percept_frames",
    "percept_duration",
    "pending_segments",
    "segments_pending_think",
    "outputs_processed",
    "outputs_pending",
    "day_bytes",
)

TOTAL_FIELDS = (
    "transcript_sessions",
    "transcript_segments",
    "transcript_duration",
    "percept_sessions",
    "percept_frames",
    "percept_duration",
    "pending_segments",
    "segments_pending_think",
    "outputs_processed",
    "outputs_pending",
    "day_bytes",
    "total_transcript_duration",
    "total_percept_duration",
)

REQUIRED_TOP_LEVEL = (
    "schema_version",
    "generated_at",
    "day_count",
    "days",
    "totals",
    "heatmap",
    "tokens",
    "talents",
    "facets",
)


def validate(data: dict) -> list[str]:
    """Validate stats output against schema v3. Returns list of error strings (empty = valid)."""
    errors = []

    # Check schema_version
    if "schema_version" not in data:
        errors.append("missing 'schema_version'")
    elif data["schema_version"] != SCHEMA_VERSION:
        errors.append(
            f"schema_version is {data['schema_version']}, expected {SCHEMA_VERSION}"
        )

    # Check generated_at
    if "generated_at" not in data:
        errors.append("missing 'generated_at'")
    elif not isinstance(data["generated_at"], str):
        errors.append("'generated_at' must be a string")

    # Check required top-level keys
    for key in REQUIRED_TOP_LEVEL:
        if key not in data:
            errors.append(f"missing required key '{key}'")

    # Spot-check one day entry if days is non-empty
    days = data.get("days", {})
    if isinstance(days, dict) and days:
        first_day = next(iter(days.values()))
        for field in DAY_FIELDS:
            if field not in first_day:
                errors.append(f"day entry missing field '{field}'")

    return errors
