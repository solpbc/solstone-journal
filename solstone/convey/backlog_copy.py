# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Locked owner-facing copy for the processing-backlog surface, shared by stats + health."""

from __future__ import annotations

BACKLOG_ACTION_PROCESS_NOW = "process now"
BACKLOG_ACTION_REDO_SCRATCH = "redo from scratch"
BACKLOG_CONFIRM_REDO_SCRATCH = (
    "redo this whole day from scratch? this re-does the parts solstone already "
    "finished, so it'll take longer. the day you see now won't change until it's done."
)
BACKLOG_QUEUED_FEEDBACK = "queued — processing now"
BACKLOG_VERDICT_CAUGHT_UP = "your journal's all caught up."
BACKLOG_VERDICT_STUCK_ONLY_PLURAL = "caught up except {stuck_n} days that need a hand."
BACKLOG_VERDICT_STUCK_ONLY_SINGULAR = "caught up except 1 day that needs a hand."
BACKLOG_VERDICT_PENDING_ONLY_PLURAL = "caught up — {pending_n} days still catching up."
BACKLOG_VERDICT_PENDING_ONLY_SINGULAR = "caught up — 1 day still catching up."
BACKLOG_VERDICT_BOTH_PLURAL = (
    "caught up except {stuck_n} days that need a hand — "
    "{pending_n} more still catching up."
)
BACKLOG_VERDICT_CANT_TELL = (
    "still checking — give me a moment to see where your journal stands."
)
BACKLOG_BUCKET_HEADING = "days that need a hand"
BACKLOG_BUCKET_DESCRIPTION = (
    "these days stopped on their own and can't pick back up without you — "
    "here's why, and what to try."
)
BACKLOG_DAY_BADGE = "stuck"
BACKLOG_REASON_CORRUPT_RAW = "original recording is missing or damaged — re-import it"
BACKLOG_REASON_FAILING_STEP = "a processing step keeps failing — try again"
BACKLOG_REASON_MISSING_CONFIG = "a setting's missing — check solstone's setup"
BACKLOG_REASON_PROVIDER_DOWN = "the AI service was unreachable — try again"
BACKLOG_WHY_NEVER_ATTEMPTED = "not looked at yet"
BACKLOG_WHY_FAILED = "couldn't finish — will retry"
BACKLOG_WHY_SENSED_NOT_THOUGHT = "observed, not yet thought through"
BACKLOG_CATCHING_UP_DAY = "catching up"
BACKLOG_CATCHING_UP_AGGREGATE = "{pending_n} day(s) catching up"
BACKLOG_CATCHING_UP_TAIL = (
    "solstone's working through these on its own, freshest day first."
)


__all__ = [
    "BACKLOG_ACTION_PROCESS_NOW",
    "BACKLOG_ACTION_REDO_SCRATCH",
    "BACKLOG_BUCKET_DESCRIPTION",
    "BACKLOG_BUCKET_HEADING",
    "BACKLOG_CATCHING_UP_AGGREGATE",
    "BACKLOG_CATCHING_UP_DAY",
    "BACKLOG_CATCHING_UP_TAIL",
    "BACKLOG_CONFIRM_REDO_SCRATCH",
    "BACKLOG_DAY_BADGE",
    "BACKLOG_QUEUED_FEEDBACK",
    "BACKLOG_REASON_CORRUPT_RAW",
    "BACKLOG_REASON_FAILING_STEP",
    "BACKLOG_REASON_MISSING_CONFIG",
    "BACKLOG_REASON_PROVIDER_DOWN",
    "BACKLOG_VERDICT_BOTH_PLURAL",
    "BACKLOG_VERDICT_CANT_TELL",
    "BACKLOG_VERDICT_CAUGHT_UP",
    "BACKLOG_VERDICT_PENDING_ONLY_PLURAL",
    "BACKLOG_VERDICT_PENDING_ONLY_SINGULAR",
    "BACKLOG_VERDICT_STUCK_ONLY_PLURAL",
    "BACKLOG_VERDICT_STUCK_ONLY_SINGULAR",
    "BACKLOG_WHY_FAILED",
    "BACKLOG_WHY_NEVER_ATTEMPTED",
    "BACKLOG_WHY_SENSED_NOT_THOUGHT",
]
