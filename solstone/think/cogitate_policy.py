# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Any

MAX_TURNS = 30
DEFAULT_READ_CALL_BUDGET = 200

_SOL_INVOCATION_RE = re.compile(r"(^sol\s|\bsol call\b)")
_WRITE_TOOLS = {"write_file", "replace"}
_READ_TOOLS = {"read_file", "glob", "list_directory", "grep_search"}


class MaxTurnsExhausted(RuntimeError):
    """Raised when the SDK tool loop exceeds its turn ceiling."""


class CogitatePolicy:
    """In-process policy gate for cogitate tool calls."""

    def __init__(self, *, write: bool, allowed_roots: list[Path]) -> None:
        self.write = write
        self.allowed_roots = [
            Path(root).expanduser().resolve() for root in allowed_roots
        ]

    def check(self, tool: str, args: dict[str, Any]) -> tuple[bool, str]:
        if self.write:
            return True, "ok"

        if tool in _WRITE_TOOLS:
            return False, f"policy_deny: {tool} not allowed for read-only talents"

        if tool == "run_shell_command":
            command = str(args.get("command", ""))
            if not _SOL_INVOCATION_RE.search(command):
                return (
                    False,
                    "policy_deny: run_shell_command restricted to sol invocations",
                )
            return True, "ok"

        if tool in _READ_TOOLS:
            return True, "ok"

        return True, "ok"


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
