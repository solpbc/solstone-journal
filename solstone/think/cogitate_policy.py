# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import re
import shlex
from dataclasses import dataclass
from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Any

from solstone.think.cogitate_contract import (
    COGITATE_ACCESS_TIERS,
    COGITATE_JOURNAL_COMMANDS,
    COGITATE_READ_TOOL_NAMES,
    capabilities_for_access_tier,
)

MAX_TURNS = 60
DEFAULT_RUN_COST_CAP_USD = 1.00
COST_WARN_FRAC = 0.70
CONTEXT_WARN_FRAC = 0.70
CONTEXT_FINAL_FRAC = 0.78
TURN_WARN_FRACS = (0.50, 0.75, 0.90)
# Normal tool-turn progression arms at observed turn N-1 and force-stops at
# observed turn N. The SDK iteration counter can also advance on
# message/reasoning-only steps the turn monitor does not count, so this remains
# a true SDK backstop; its MaxIterationsReached path already maps to
# max_turns_exhausted.
MAX_TURNS_HEADROOM = 2
# Conservative fresh-token fallback when the SDK's accumulated_cost is still 0.0
# (no response completed yet, or litellm cost calc failed). Uses Gemini Flash's
# output rate ($2.50 / 1M tokens) for ALL fresh non-cache tokens so the estimate
# errs high and the ceiling trips early rather than late.
_FALLBACK_USD_PER_TOKEN = 0.0000025
DEFAULT_READ_CALL_BUDGET = 200
_SHELL_OPERATOR_CHARS = frozenset("();<>|&")
_WRITE_TOOLS = {"write_file", "replace"}
_READ_TOOLS = frozenset(COGITATE_READ_TOOL_NAMES)
_SUBMIT_TIERS = tuple(
    tier for tier in COGITATE_ACCESS_TIERS if capabilities_for_access_tier(tier).submit
)
_SUPPORT_SEND_VERBS = {
    "create",
    "reply",
    "attach",
    "feedback",
    "close",
    "resolved",
    "still-need-help",
}

SHELL_COMPOSITION_DENY = (
    "policy_deny: shell composition is not available; run one `sol` or approved "
    "`journal` command per call with no pipes, redirects, chaining, or command "
    "substitution"
)
EMPTY_COMMAND_DENY = "policy_deny: empty command"
RESTRICTED_COMMAND_DENY = (
    "policy_deny: run_shell_command restricted to sol or approved journal invocations"
)
HYBRID_JOURNAL_COMMAND_DENY = (
    "policy_deny: `sol call journal {family}` is not a `sol call` verb; "
    "`{family}` is an approved host command family — run it directly as `{repair}`"
)
BARE_JOURNAL_REPAIR_DENY = (
    "policy_deny: `journal {family}` is not a host command; run `{repair}` instead"
)
_BARE_JOURNAL_REPAIR_PREFIXES = {
    "search": ("sol", "call", "journal", "search"),
    "facet": ("sol", "call", "journal", "facet"),
}


@dataclass(frozen=True)
class CommandDecision:
    allowed: bool
    reason: str
    argv: list[str] | None


def _shell_syntax_violation(command: str) -> bool:
    """Return True when command uses shell syntax outside quoted data."""
    if "$(" in command or "`" in command:
        return True
    if "\n" in command or "\r" in command:
        return True
    index = 0
    length = len(command)
    quote: str | None = None
    while index < length:
        char = command[index]
        if quote == "'":
            if char == "'":
                quote = None
        elif quote == '"':
            if char == "\\" and index + 1 < length:
                index += 2
                continue
            if char == '"':
                quote = None
        else:
            if char == "\\" and index + 1 < length:
                index += 2
                continue
            if char in ("'", '"'):
                quote = char
            elif char in _SHELL_OPERATOR_CHARS:
                return True
        index += 1
    return quote is not None


def _hybrid_journal_deny(argv: list[str]) -> str:
    family = argv[3]
    repair = shlex.join(["journal", *argv[3:]])
    return HYBRID_JOURNAL_COMMAND_DENY.format(family=family, repair=repair)


def _bare_journal_repair_deny(argv: list[str]) -> str:
    family = argv[1]
    repair = shlex.join([*_BARE_JOURNAL_REPAIR_PREFIXES[family], *argv[2:]])
    return BARE_JOURNAL_REPAIR_DENY.format(family=family, repair=repair)


class MaxTurnsExhausted(RuntimeError):
    """Raised when the SDK tool loop exceeds its turn ceiling."""


class CogitatePolicy:
    """In-process policy gate for cogitate tool calls."""

    def __init__(
        self,
        *,
        allowed_roots: list[Path],
        access_tier: str,
        outbound_approval: str | None = None,
    ) -> None:
        self.allowed_roots = [
            Path(root).expanduser().resolve() for root in allowed_roots
        ]
        self.access_tier = access_tier
        self.submit_allowed = capabilities_for_access_tier(access_tier).submit
        self.outbound_approval = outbound_approval

    def check(self, tool: str, args: dict[str, Any]) -> tuple[bool, str]:
        if tool in _WRITE_TOOLS:
            return False, f"policy_deny: {tool} not allowed for read-only talents"

        if tool == "run_shell_command":
            decision = self.classify_command(str(args.get("command", "")))
            return decision.allowed, decision.reason

        if tool in _READ_TOOLS:
            return True, "ok"

        return True, "ok"

    def classify_command(self, command: str) -> CommandDecision:
        if _shell_syntax_violation(command):
            return CommandDecision(False, SHELL_COMPOSITION_DENY, None)

        try:
            argv = shlex.split(command, posix=True)
        except ValueError:
            return CommandDecision(False, SHELL_COMPOSITION_DENY, None)

        if not argv:
            return CommandDecision(False, EMPTY_COMMAND_DENY, None)

        if (
            argv[0:3] == ["sol", "call", "journal"]
            and len(argv) >= 4
            and argv[3] in COGITATE_JOURNAL_COMMANDS
        ):
            return CommandDecision(False, _hybrid_journal_deny(argv), None)

        if (
            argv[0] == "journal"
            and len(argv) >= 2
            and argv[1] in _BARE_JOURNAL_REPAIR_PREFIXES
        ):
            return CommandDecision(False, _bare_journal_repair_deny(argv), None)

        if not (
            argv[0] == "sol"
            or (
                argv[0] == "journal"
                and len(argv) >= 2
                and argv[1] in COGITATE_JOURNAL_COMMANDS
            )
        ):
            return CommandDecision(False, RESTRICTED_COMMAND_DENY, None)

        send_verb = _support_send_verb(argv)
        if send_verb and not self.submit_allowed:
            required = " or ".join(_SUBMIT_TIERS)
            return CommandDecision(
                False,
                f"policy_deny: 'sol call support {send_verb}' requires "
                f"access_tier {required!r}; this run is {self.access_tier!r}",
                None,
            )

        if send_verb and not self.outbound_approval:
            return CommandDecision(
                False,
                f"policy_deny: 'sol call support {send_verb}' requires a "
                "per-send owner approval; this run was not launched with one",
                None,
            )

        return CommandDecision(True, "ok", argv)


def _support_send_verb(argv: list[str]) -> str | None:
    for index in range(len(argv) - 3):
        if argv[index : index + 3] != ["sol", "call", "support"]:
            continue
        verb = argv[index + 3]
        if verb in _SUPPORT_SEND_VERBS:
            return verb
    return None


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
