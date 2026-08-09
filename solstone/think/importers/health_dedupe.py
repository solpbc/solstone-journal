# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Read-only Python value model for the native body dedupe oracle."""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class HealthDedupeRecord:
    """One normalized body identity used by the differential oracle."""

    dedupe_key: str
    source_family: str
    record_type: str
    start_time: str
    end_time: str | None = None
    source_record_id: str | None = None
    value_hash: str | None = None
    first_import_id: str | None = None
    last_seen_import_id: str | None = None
    normalized_ref: str | None = None
    raw_ref: str | None = None
