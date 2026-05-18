# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Structured setup and doctor event helpers."""

from __future__ import annotations

import json
from datetime import datetime, timezone
from typing import IO, Any

EVENT_TYPES: frozenset[str] = frozenset(
    {
        "setup.started",
        "setup.completed",
        "step.started",
        "step.completed",
        "step.failed",
        "step.warning",
        "doctor.started",
        "check.completed",
        "doctor.completed",
    }
)

ERROR_CODES: frozenset[str] = frozenset(
    {
        "doctor_failed",
        "doctor_jsonl_incomplete",
        "doctor_timeout",
        "journal_dir_invalid",
        "journal_existing_blocked",
        "service_up_failed",
        "setup_unhandled_exception",
        "step_subprocess_failed",
        "step_subprocess_timeout",
    }
)

STEP_NAMES: tuple[str, ...] = (
    "doctor",
    "journal",
    "install_models",
    "skills_user",
    "skills_journal",
    "wrapper",
    "service",
)

SKIPPED_REASONS: frozenset[str] = frozenset(
    {
        "--skip-models",
        "--skip-skills",
        "--skip-service",
        "packaged_install",
        "prior_run_ok",
        "resumed_after_restart",
    }
)

STATUS_TRANSLATION: dict[str, str] = {
    "ok": "ok",
    "warn": "warning",
    "fail": "failed",
    "skip": "skipped",
}


def utc_now_iso() -> str:
    return (
        datetime.now(timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z")
    )


class JsonlEmitter:
    def __init__(self, writer: IO[str]) -> None:
        self.writer = writer

    def emit(self, event: str, **fields: Any) -> None:
        if event not in EVENT_TYPES:
            raise ValueError(f"unknown setup event type: {event}")
        payload = {"event": event, "ts": utc_now_iso(), **fields}
        self.writer.write(json.dumps(payload, sort_keys=False) + "\n")
        self.writer.flush()

    def forward_line(self, line: str) -> None:
        self.writer.write(line if line.endswith("\n") else line + "\n")
        self.writer.flush()
