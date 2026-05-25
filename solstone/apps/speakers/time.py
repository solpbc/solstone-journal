# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Time helpers for speaker segment provenance."""

from __future__ import annotations

from datetime import datetime

from solstone.think.utils import get_owner_timezone, segment_parse


def segment_start_ts_ms(day: str, segment_key: str) -> int:
    """Return the segment start timestamp as epoch milliseconds."""
    start, _ = segment_parse(segment_key)
    if start is None:
        raise ValueError(f"Invalid segment key: {segment_key}")
    if len(day) != 8 or not day.isdigit():
        raise ValueError(f"Invalid day key: {day}")

    dt = datetime(
        int(day[0:4]),
        int(day[4:6]),
        int(day[6:8]),
        start.hour,
        start.minute,
        start.second,
        tzinfo=get_owner_timezone(),
    )
    return int(dt.timestamp() * 1000)
