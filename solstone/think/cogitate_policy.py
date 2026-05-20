# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
from datetime import date, datetime, timedelta
from typing import Any


def _normalize_day(day: date | str) -> str:
    if isinstance(day, date):
        return day.strftime("%Y%m%d")
    if day:
        return str(day)
    return datetime.now().strftime("%Y%m%d")


def _day_value(day: str) -> date:
    return datetime.strptime(day, "%Y%m%d").date()


def _expand_day_placeholders(value: str, day: str) -> str:
    base_day = _day_value(day)

    def replace(match: re.Match[str]) -> str:
        offset = int(match.group("offset") or 0)
        return (base_day - timedelta(days=offset)).strftime("%Y%m%d")

    return re.sub(r"<day(?:-(?P<offset>\d+))?>", replace, value)


def resolve_read_scope(
    talent_config: dict[str, Any],
    day: date | str,
    span: int = 0,
) -> list[str]:
    day_str = _normalize_day(day)
    configured_scope = talent_config.get("read_scope")
    if configured_scope:
        return [
            _expand_day_placeholders(str(scope), day_str) for scope in configured_scope
        ]

    effective_span = int(talent_config.get("read_scope_span", span or 0) or 0)
    if effective_span <= 0:
        return [f"chronicle/{day_str}"]

    base_day = _day_value(day_str)
    return [
        f"chronicle/{(base_day - timedelta(days=offset)).strftime('%Y%m%d')}"
        for offset in range(effective_span, -1, -1)
    ]
