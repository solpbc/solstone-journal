# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared evidence vocabulary for media-analysis outputs.

This module is the source of truth for the two forms of output evidence:
``_solstone_processing`` metadata-header records and the JSONL row keys that
prove audio or screen analysis rows exist. The screen (``describe``) and audio
(``transcribe``) handlers stamp processing records into the metadata header of
the JSONL they produce, while row-key detection gives bounded evidence for
legacy or record-less outputs.

``FileSensor.scan_unprocessed``, the native ``solstone-core-describe`` binary, and
``derive_modality_state`` consume this vocabulary so capture re-entry,
describe-side skipping, and downstream state derivation share the same reading
of whether an output is useful, terminal, retryable, or indeterminate.
"""

import json
from datetime import datetime, timezone
from pathlib import Path

SCHEMA = "solstone.processing.v1"
FAILED_ATTEMPT_BOUND = 3
ATTEMPTS_KEY = "attempts"
MAX_FIRST_ROW_BYTES = 64 * 1024
SCREEN_ANALYSIS_ROW_KEY = "timestamp"
AUDIO_TRANSCRIPT_ROW_KEY = "start"

# state values (closed set)
STATE_ANALYZED = "analyzed"
STATE_EMPTY = "empty"
STATE_FAILED = "failed"

# reason_code values (closed set). Audio emits no_decodable_audio for terminal
# preserved-empty outputs.
REASON_OK = "ok"
REASON_NO_DECODABLE_FRAMES = "no_decodable_frames"
REASON_NO_DECODABLE_AUDIO = "no_decodable_audio"
REASON_CORRUPT_INPUT = "corrupt_input"
REASON_ANALYSIS_FAILED = "analysis_failed"

# handler values (closed set)
HANDLER_DESCRIBE = "describe"
HANDLER_TRANSCRIBE = "transcribe"


def is_failure_exhausted(record: dict | None) -> bool:
    """Return whether a failed processing record has reached terminal exhaustion."""
    if not isinstance(record, dict) or record.get("state") != STATE_FAILED:
        return False
    if record.get("reason_code") == REASON_CORRUPT_INPUT:
        return True
    return record_attempts(record) >= FAILED_ATTEMPT_BOUND


def record_attempts(record: dict | None) -> int:
    """Return a record's failure-attempt count; absent or malformed means 0."""
    if not isinstance(record, dict):
        return 0
    attempts = record.get(ATTEMPTS_KEY, 0)
    if isinstance(attempts, bool) or not isinstance(attempts, int):
        return 0
    return attempts


def read_processing_record_header(path: Path) -> dict | None:
    """Read a JSONL header's `_solstone_processing` record within one bounded window."""
    try:
        with path.open("rb") as handle:
            first_window = handle.read(MAX_FIRST_ROW_BYTES)
    except OSError:
        return None

    if b"\n" not in first_window:
        return None

    first_line = first_window.split(b"\n", 1)[0]
    try:
        row = json.loads(first_line.decode("utf-8"))
    except UnicodeDecodeError:
        return None
    except json.JSONDecodeError:
        return None

    if not isinstance(row, dict):
        return None
    record = row.get("_solstone_processing")
    return record if isinstance(record, dict) else None


def jsonl_has_row_with_key(path: Path, row_key: str) -> bool:
    """Return whether an early JSONL object has the row key.

    Key-based membership test on at most the first two nonblank lines.
    """
    try:
        lines = []
        with path.open("r", encoding="utf-8") as handle:
            for line in handle:
                if not line.strip():
                    continue
                lines.append(line)
                if len(lines) == 2:
                    break
    except OSError:
        return False
    for line in lines:
        try:
            parsed = json.loads(line)
        except (json.JSONDecodeError, ValueError):
            continue
        if isinstance(parsed, dict) and row_key in parsed:
            return True
    return False


def should_reenter_analysis_output(
    *,
    record: dict | None,
    output_path: Path,
    handler: str,
) -> bool:
    """Return whether an existing analysis output should be retried.

    an existing analysis output only blocks re-entry when it actually carries
    evidence — analyzed rows or a processing record. An output with neither is
    indeterminate, and indeterminate work re-enters until the record ledger can
    govern it. Decode-determined verdicts terminalize regardless of provider
    availability only when there is no frame-description work left to do; a run
    that still has qualified frames defers instead of discarding them.

    This is the primary, automatic remedy for a record-less output: the handler
    *determines* the verdict on re-entry. ``backfill_processing_records`` is the
    operator bulk tool for the same on-disk shape — CLI-only, and it *stamps a
    guessed* ``state=empty`` rather than determining one, so it is scoped to
    marker-less, chunk-less legacy fleets and declines anything carrying a
    marker or an existing record. Re-entry deliberately covers the marker-
    bearing case the backfill refuses.
    """
    if (
        isinstance(record, dict)
        and record.get("state") == STATE_FAILED
        and record.get("handler") == HANDLER_DESCRIBE
        and not is_failure_exhausted(record)
    ):
        return True
    return (
        record is None
        and handler == HANDLER_DESCRIBE
        and not jsonl_has_row_with_key(output_path, SCREEN_ANALYSIS_ROW_KEY)
    )


def now_iso_utc() -> str:
    """ISO-8601 UTC timestamp with a trailing ``Z`` (e.g. ``2026-06-30T12:00:00Z``)."""
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def build_processing_record(
    *,
    state: str,
    reason_code: str,
    handler: str,
    input_size: int,
    attempted_at: str | None = None,
    source: str | None = None,
    attempts: int | None = None,
) -> dict:
    """Build a `_solstone_processing` header record for a determined outcome.

    `attempted_at` defaults to the current UTC instant; pass an explicit value
    only in tests. The outcome must be the one the handler *determined* while
    running — never a pre-stamped guess. `source` is a provenance tag set only
    by the backfill. `attempts` is present only on failing records; absent means
    0, and callers never stamp `attempts: 0`.
    """
    record = {
        "schema": SCHEMA,
        "state": state,
        "reason_code": reason_code,
        "handler": handler,
        "attempted_at": attempted_at or now_iso_utc(),
        "input_size": input_size,
    }
    if source is not None:
        record["source"] = source
    if attempts is not None:
        record[ATTEMPTS_KEY] = attempts
    return record
