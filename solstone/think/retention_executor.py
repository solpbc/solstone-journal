# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""The seam from Python to the Rust retention executor.

Every removal of the owner's media belongs to one executor, and that executor is
Rust (``solstone-core-retention``). This module is the only way Python reaches it.

Why a subprocess: the native ``sol`` commands are HTTP clients that call *this*
service, so they replace a CLI layer rather than logic, and there is no Python
extension module for the core. Executing a Rust binary is how this repository
already crosses the boundary, so it is how removal crosses it too.

What the executor gives a caller that ``shutil.rmtree`` could not:

* a **tombstone** in the emptied segment, so the owner has evidence a deletion
  happened and a later pass can recognise the same segment restored from a backup;
* **staging** -- the segment is moved aside under a name no iterator returns, emptied
  there, and moved back holding only its tombstone, so a crash never leaves a
  half-removed segment sitting under its real name;
* a **path-keyed index prune** instead of a full re-scan of the journal;
* a **receipt** naming every path actually removed and every path refused, with an
  exit code that distinguishes "all of it" from "some of it".

⛔ The last point is the one this module exists to preserve. A caller that treats a
partial removal as success reports a deletion that did not happen.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from dataclasses import dataclass, field
from datetime import date, datetime, timedelta, timezone
from typing import Any

from solstone.think.utils import get_config

BINARY = "solstone-retention"

#: Absolute path override, for a dev tree where the binary is not yet installed.
BIN_ENV = "SOLSTONE_RETENTION_BIN"

#: Exit codes the executor defines. Anything else is unexpected.
EXIT_OK = 0
EXIT_USAGE = 2
EXIT_REFUSED = 3
EXIT_HALTED = 4

#: The identity recorded in a tombstone when no owner DID is available.
#:
#: ⚠ Matches the convention the pre-existing segment tombstone already used. It is
#: deliberately not a fabricated identifier: the *surface* that performed the removal
#: is recorded separately in the tombstone's executor stamp, so this field means
#: "who authorized it" and we do not yet have an answer for an owner acting in their
#: own journal.
UNKNOWN_DID = "unknown"

#: How long the executor may run before a caller gives up on it. Removal is local
#: filesystem work; a minute is already pathological.
TIMEOUT_SECONDS = 60

PRUNE_LOGS_MAX_RUNTIME = "30m"


class ExecutorUnavailable(RuntimeError):
    """The retention executor could not be found or could not be run."""


@dataclass
class Refused:
    """The executor ran and did not remove everything it was asked to."""

    receipt: dict[str, Any]

    def entries(self) -> list[dict[str, str]]:
        """Every refusal, flattened across targets."""
        refusals: list[dict[str, str]] = []
        for target in self.receipt.get("outcome", {}).get("targets", []):
            refusals.extend(target.get("not_removed", []))
        return refusals

    def summary(self) -> str:
        parts = [
            f"{entry.get('entry', '?')}: {entry.get('reason', '?')}"
            for entry in self.entries()
        ]
        return "; ".join(parts) or "the executor refused without naming an entry"


class RemovalRefused(RuntimeError):
    """Raised when the executor did not remove everything.

    ⛔ An exception rather than a return value on purpose: the deferred-commit path
    that performs an owner's deletion has no reader for a status field, and a silent
    partial removal is the failure this whole conversion exists to prevent.
    """

    def __init__(self, refused: Refused) -> None:
        super().__init__(refused.summary())
        self.refused = refused


@dataclass
class LogRetentionConfig:
    """Operational log/cache retention config."""

    enabled: bool = True
    days: int = 30


@dataclass
class PruneResult:
    """Result of a journal log/cache prune run."""

    enabled: bool
    dry_run: bool
    days: int
    cutoff_day: str
    by_class: dict[str, dict]
    by_day: dict[str, dict]
    files_deleted: int
    dirs_deleted: int
    bytes_freed: int
    errors: list[dict]
    audit_written: bool
    partial_error: bool
    root_task_log: dict = field(default_factory=dict)
    retention_log: dict = field(default_factory=dict)


def load_log_retention_config() -> LogRetentionConfig:
    """Load journal log/cache retention config with per-field defaults."""
    config = get_config()
    retention = config.get("retention") or {}
    journal_logs = retention.get("journal_logs") or {}

    enabled = journal_logs.get("enabled", True)
    days = journal_logs.get("days", 30)
    if not isinstance(enabled, bool):
        raise ValueError("retention.journal_logs.enabled must be a boolean")
    if isinstance(days, bool):
        raise ValueError("retention.journal_logs.days must be a positive integer")
    try:
        parsed_days = int(days)
    except (TypeError, ValueError) as exc:
        raise ValueError(
            "retention.journal_logs.days must be a positive integer"
        ) from exc
    if parsed_days < 1:
        raise ValueError("retention.journal_logs.days must be a positive integer")
    return LogRetentionConfig(enabled=enabled, days=parsed_days)


def _effective_log_days(days: int) -> int:
    if isinstance(days, bool):
        raise ValueError("days must be a positive integer")
    try:
        parsed_days = int(days)
    except (TypeError, ValueError) as exc:
        raise ValueError("days must be a positive integer") from exc
    if parsed_days < 1:
        raise ValueError("days must be a positive integer")
    return parsed_days


def _cutoff_day(days: int) -> str:
    return (date.today() - timedelta(days=days)).strftime("%Y%m%d")


def _prune_plan(receipt: dict[str, Any]) -> dict[str, Any]:
    detail = receipt.get("detail")
    plan = detail.get("plan") if isinstance(detail, dict) else receipt.get("plan")
    if not isinstance(plan, dict):
        raise ExecutorUnavailable("the retention executor receipt had no prune-logs plan")
    return plan


def _count(value: Any) -> int:
    return int(value) if isinstance(value, int) and not isinstance(value, bool) else 0


def _compaction_bytes(stats: dict, *, dry_run: bool) -> int:
    if not dry_run and not bool(stats.get("rewritten", False)):
        return 0
    return _count(stats.get("bytes_before")) - _count(stats.get("bytes_after"))


def _compaction_result(stats: dict, *, dry_run: bool) -> dict:
    return {
        "exists": bool(stats.get("exists", False)),
        "lines_total": _count(stats.get("lines_total")),
        "lines_kept": _count(stats.get("lines_kept")),
        "lines_removed": _count(stats.get("lines_dropped")),
        "unparseable_lines_kept": _count(stats.get("undateable_kept")),
        "bytes_freed": _compaction_bytes(stats, dry_run=dry_run),
        "rewritten": bool(stats.get("rewritten", False)),
        "errors": [
            str(error.get("reason", "unknown error"))
            for error in (stats.get("errors") or [])
            if isinstance(error, dict)
        ],
    }


def prune_result_from_receipt(
    receipt: dict[str, Any], *, dry_run: bool, days: int
) -> PruneResult:
    """Adapt either prune-logs receipt shape to the settings result contract."""
    plan = _prune_plan(receipt)
    prefix = "planned" if dry_run else "removed"
    by_class: dict[str, dict] = {}
    for name, raw_stats in (plan.get("by_class") or {}).items():
        if not isinstance(name, str) or not isinstance(raw_stats, dict):
            continue
        errors = raw_stats.get("errors") or []
        by_class[name] = {
            "files_deleted": _count(raw_stats.get(f"{prefix}_files")),
            "bytes_freed": _count(raw_stats.get(f"{prefix}_bytes")),
            "dirs_deleted": _count(raw_stats.get(f"{prefix}_dirs")),
            "skipped": _count(raw_stats.get("skipped")),
            "errors": [
                str(error.get("reason", "unknown error"))
                for error in errors
                if isinstance(error, dict)
            ],
        }

    by_day: dict[str, dict] = {}
    for day, raw_stats in (plan.get("by_day") or {}).items():
        if not isinstance(day, str) or not isinstance(raw_stats, dict):
            continue
        by_day[day] = {
            "files_deleted": _count(raw_stats.get(f"{prefix}_files")),
            "bytes_freed": _count(raw_stats.get(f"{prefix}_bytes")),
            "dirs_deleted": _count(raw_stats.get(f"{prefix}_dirs")),
        }

    errors = [
        {
            "class": error.get("class"),
            "path": error.get("path"),
            "day": error.get("day"),
            "reason": str(error.get("reason", "unknown error")),
            "message": str(error.get("reason", "unknown error")),
            "hint": error.get("hint"),
        }
        for error in (plan.get("errors") or [])
        if isinstance(error, dict)
    ]
    compactions = plan.get("compactions") or {}
    root = compactions.get("root_task_log") if isinstance(compactions, dict) else {}
    root = root if isinstance(root, dict) else {}
    root_task_log = _compaction_result(root, dry_run=dry_run)
    retention_log = (
        compactions.get("retention_log") if isinstance(compactions, dict) else {}
    )
    retention_log = retention_log if isinstance(retention_log, dict) else {}
    retention_log_result = _compaction_result(retention_log, dry_run=dry_run)
    # 🔴 Unlike the Python writer, Rust also compacts health/retention.log. Its
    # reclaimed bytes now contribute to journal_logs retention accounting.
    compaction_bytes = (
        root_task_log["bytes_freed"] + retention_log_result["bytes_freed"]
    )

    return PruneResult(
        enabled=True,
        dry_run=dry_run,
        days=days,
        cutoff_day=_cutoff_day(days),
        by_class=by_class,
        by_day=by_day,
        files_deleted=sum(stats["files_deleted"] for stats in by_class.values()),
        dirs_deleted=sum(stats["dirs_deleted"] for stats in by_class.values()),
        bytes_freed=sum(stats["bytes_freed"] for stats in by_class.values())
        + compaction_bytes,
        errors=errors,
        audit_written=False,
        partial_error=bool(errors),
        root_task_log=root_task_log,
        retention_log=retention_log_result,
    )


def prune_logs(
    journal: str, *, days: int | None = None, dry_run: bool = False
) -> PruneResult:
    """Plan or prune operational logs through Rust without writing a Python audit.

    The ``journal_logs`` audit trail ends here: this operation no longer writes
    ``health/pruning-runs`` or per-day task-log entries. Raw-media pruning still uses
    ``pruning_audit.py``; existing run records age out through the Rust log class.
    """
    config = load_log_retention_config()
    effective_days = _effective_log_days(days if days is not None else config.days)
    cutoff_day = _cutoff_day(effective_days)
    if not config.enabled:
        return PruneResult(
            enabled=False,
            dry_run=dry_run,
            days=effective_days,
            cutoff_day=cutoff_day,
            by_class={},
            by_day={},
            files_deleted=0,
            dirs_deleted=0,
            bytes_freed=0,
            errors=[],
            audit_written=False,
            partial_error=False,
            root_task_log={},
            retention_log={},
        )

    today = date.today()
    argv = [
        executor_path(),
        "prune-logs",
        "--journal",
        journal,
        "--today",
        today.isoformat(),
        "--days",
        str(effective_days),
    ]
    if not dry_run:
        argv.extend(["--execute", "true"])
    code, receipt = _run(argv)
    if code == EXIT_OK:
        return prune_result_from_receipt(
            receipt, dry_run=dry_run, days=effective_days
        )
    if code in (EXIT_REFUSED, EXIT_HALTED):
        raise RemovalRefused(Refused(receipt))
    raise ExecutorUnavailable(
        f"the retention log prune was rejected (exit {code}): "
        f"{receipt.get('error', receipt)}"
    )


def executor_path() -> str:
    """Locate the executor binary.

    Raises:
        ExecutorUnavailable: when the binary is neither overridden nor on PATH.
    """
    override = os.environ.get(BIN_ENV)
    if override:
        if not os.path.isfile(override) or not os.access(override, os.X_OK):
            raise ExecutorUnavailable(
                f"{BIN_ENV} points at {override}, which is not an executable file"
            )
        return override
    found = shutil.which(BINARY)
    if found is None:
        raise ExecutorUnavailable(
            f"{BINARY} is not on PATH. Every removal of the owner's media goes "
            f"through it, so nothing is deleted without it. Install the core "
            f"binaries, or set {BIN_ENV} to an absolute path."
        )
    return found


def now_stamp() -> str:
    """The current instant, RFC 3339 in UTC.

    ⚠ The executor takes the instant as an argument and refuses to read the clock
    itself, so that a verdict is reproducible from its receipt. This is the caller
    honouring that: one instant, chosen once, recorded in the tombstone.
    """
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace(
        "+00:00", "Z"
    )


def _run(argv: list[str]) -> tuple[int, dict[str, Any]]:
    """Run the executor and parse its receipt."""
    try:
        completed = subprocess.run(
            argv,
            capture_output=True,
            text=True,
            timeout=TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        raise ExecutorUnavailable(
            f"the retention executor did not finish within {TIMEOUT_SECONDS}s"
        ) from exc
    except OSError as exc:
        raise ExecutorUnavailable(f"the retention executor could not run: {exc}") from exc

    try:
        receipt = json.loads(completed.stdout)
    except (json.JSONDecodeError, ValueError) as exc:
        # ⛔ Never treat unparseable output as success, whatever the exit code.
        raise ExecutorUnavailable(
            "the retention executor produced no readable receipt "
            f"(exit {completed.returncode}): {completed.stderr.strip() or '<no stderr>'}"
        ) from exc
    if not isinstance(receipt, dict):
        raise ExecutorUnavailable("the retention executor's receipt was not an object")
    return completed.returncode, receipt


def remove_segments(
    journal: str,
    segments: list[tuple[str, str, str]],
    *,
    did: str = UNKNOWN_DID,
    at: str | None = None,
    reason: str = "owner",
) -> dict[str, Any]:
    """Remove whole segments, leaving a tombstone in each.

    Args:
        journal: Journal root.
        segments: ``(day, stream, segment_dir)`` triples. ⛔ ``segment_dir`` is the
            directory NAME, never a key parsed out of it -- the two differ whenever a
            name carries a suffix, and a key addresses a different directory or none.
            Name the default stream ``_default``; it contributes no path component.
        did: Identity recorded in the tombstone.
        at: RFC 3339 instant; defaults to now.
        reason: ``owner`` for an owner-directed delete, ``policy`` for the sweep.

    Returns:
        The executor's receipt.

    Raises:
        ExecutorUnavailable: the executor could not be found, run, or understood.
        RemovalRefused: it ran and did not remove everything asked of it.
    """
    if not segments:
        raise ValueError("remove_segments needs at least one segment")
    argv = [
        executor_path(),
        "remove-segments",
        "--journal",
        journal,
        "--at",
        at or now_stamp(),
        "--did",
        did,
        "--reason",
        reason,
    ]
    for day, stream, segment_dir in segments:
        argv.extend(["--segment", f"{day}/{stream}/{segment_dir}"])

    code, receipt = _run(argv)
    if code == EXIT_OK:
        return receipt
    if code in (EXIT_REFUSED, EXIT_HALTED):
        raise RemovalRefused(Refused(receipt))
    raise ExecutorUnavailable(
        f"the retention executor rejected the request (exit {code}): "
        f"{receipt.get('error', receipt)}"
    )


def removed_paths(receipt: dict[str, Any]) -> list[str]:
    """Every journal-relative path a receipt reports as removed."""
    paths: list[str] = []
    for target in receipt.get("outcome", {}).get("targets", []):
        paths.extend(target.get("removed", []))
    return paths


def index_pruned(receipt: dict[str, Any]) -> dict[str, Any]:
    """What the receipt says about the search-index notification.

    ⚠ A failed notification is not a failed removal, and this keeps the two readable
    apart: the files are gone either way, and a stale index row surfaces itself the
    next time something opens it.
    """
    return receipt.get("index", {})


# ---------------------------------------------------------------------------
# Scheduled raw-media policy
# ---------------------------------------------------------------------------

# The scheduled-pass fingerprint/confirmation gate is retired as a route to deletion:
# the founder ruling of 2026-08-06 forbids any routine from converting a one-time owner
# decision into standing deletion authority. Daily maintenance now only lists
# policy-eligible originals via `mark()`/`marks()`; it never removes them.
# `retention.raw_media_release_confirmed` is no longer read or written by any code path
# here — it is harmless if still present in an existing journal's `config/journal.json`.


def policy_payload(retention: dict[str, Any]) -> dict[str, Any]:
    """Translate the journal's retention config into the executor's policy.

    ⚠ `keep` is an **absent** period rather than zero days. Zero days means "as soon as
    the anchor has a value", which for `processed` is the reference's *once processing
    completes* mode; conflating the two is how that mode silently never fires.
    """

    def rule(mode: str, days: Any) -> dict[str, Any]:
        if mode == "days":
            try:
                period = int(days)
            except (TypeError, ValueError):
                period = 0
            # ⛔ A `days` policy with no positive day count keeps. It does not release
            # immediately.
            if period <= 0:
                return {"anchor": "captured", "period": None, "priority": 0}
            return {"anchor": "captured", "period": period, "priority": 0}
        if mode == "processed":
            return {"anchor": "processed", "period": 0, "priority": 0}
        # `keep`, and anything unrecognised. ⛔ Falling off the end must keep.
        return {"anchor": "captured", "period": None, "priority": 0}

    per_stream = []
    for name, stream in (retention.get("per_stream") or {}).items():
        if not isinstance(stream, dict):
            continue
        per_stream.append(
            [name, rule(stream.get("raw_media", "keep"), stream.get("raw_media_days"))]
        )

    minimum = retention.get("raw_media_minimum_days") or 0
    try:
        minimum_age = max(int(minimum), 0)
    except (TypeError, ValueError):
        minimum_age = 0

    return {
        "default_rule": rule(
            retention.get("raw_media", "keep"), retention.get("raw_media_days")
        ),
        "per_stream": per_stream,
        "minimum_age": minimum_age,
        "enabled": True,
    }


def policy_would_release(payload: dict[str, Any]) -> bool:
    """Whether a policy can release anything at all.

    A policy where every rule keeps forever has nothing to mark for removal.
    """
    rules = [payload["default_rule"]] + [rule for _, rule in payload["per_stream"]]
    return any(rule.get("period") is not None for rule in rules)


def sweep(
    journal: str,
    policy: dict[str, Any],
    *,
    today: str,
    now: str,
) -> dict[str, Any]:
    """Plan one scheduled retention pass without removing media."""
    argv = [
        executor_path(),
        "sweep",
        "--journal",
        journal,
        "--today",
        today,
        "--now",
        now,
        "--policy",
        json.dumps(policy),
    ]
    code, receipt = _run(argv)
    if code == EXIT_OK:
        return receipt
    if code in (EXIT_REFUSED, EXIT_HALTED):
        raise RemovalRefused(Refused(receipt))
    raise ExecutorUnavailable(
        f"the retention sweep was rejected (exit {code}): {receipt.get('error', receipt)}"
    )


def mark(journal: str, policy: dict[str, Any], *, today: str, now: str) -> dict[str, Any]:
    """Refresh policy raw-release proposals without removing media."""
    code, receipt = _run([
        executor_path(), "mark", "--journal", journal, "--today", today,
        "--now", now, "--policy", json.dumps(policy),
    ])
    if code == EXIT_OK:
        return receipt
    if code in (EXIT_REFUSED, EXIT_HALTED):
        raise RemovalRefused(Refused(receipt))
    raise ExecutorUnavailable(f"the retention mark was rejected (exit {code}): {receipt.get('error', receipt)}")


def marks(journal: str) -> dict[str, Any]:
    """Read the durable retention removal register."""
    code, receipt = _run([executor_path(), "marks", "--journal", journal])
    if code == EXIT_OK:
        return receipt
    if code in (EXIT_REFUSED, EXIT_HALTED):
        raise RemovalRefused(Refused(receipt))
    raise ExecutorUnavailable(f"the retention marks read was rejected (exit {code}): {receipt.get('error', receipt)}")


def mark_offload(
    journal: str, day: str, segment_dir: str, files: list[str], reason: str, *, now: str, stream: str = "_default"
) -> dict[str, Any]:
    """Record an archive-backed raw-release proposal."""
    argv = [executor_path(), "mark-offload", "--journal", journal, "--day", day,
            "--stream", stream, "--dir", segment_dir, "--reason", reason, "--now", now]
    for name in files:
        argv.extend(["--file", name])
    code, receipt = _run(argv)
    if code == EXIT_OK:
        return receipt
    if code in (EXIT_REFUSED, EXIT_HALTED):
        raise RemovalRefused(Refused(receipt))
    raise ExecutorUnavailable(f"the offload mark was rejected (exit {code}): {receipt.get('error', receipt)}")


def resolve_offload_mark(
    journal: str,
    day: str,
    segment_dir: str,
    files: list[str],
    *,
    stream: str = "_default",
) -> dict[str, Any]:
    """Clear an OffloadRawRelease mark for a segment whose files are confirmed present."""
    argv = [
        executor_path(),
        "resolve-offload",
        "--journal",
        journal,
        "--day",
        day,
        "--stream",
        stream,
        "--dir",
        segment_dir,
    ]
    for name in files:
        argv.extend(["--file", name])
    code, receipt = _run(argv)
    if code == EXIT_OK:
        return receipt
    if code in (EXIT_REFUSED, EXIT_HALTED):
        raise RemovalRefused(Refused(receipt))
    raise ExecutorUnavailable(f"the offload mark resolve was rejected (exit {code}): {receipt.get('error', receipt)}")


def remove_marked(
    journal: str, mark_ids: list[str], policy: dict[str, Any], *, today: str, now: str
) -> dict[str, Any]:
    """Execute only explicit, approval-required retention marks."""
    argv = [executor_path(), "remove-marked", "--journal", journal, "--today", today,
            "--now", now, "--policy", json.dumps(policy)]
    for mark_id in mark_ids:
        argv.extend(["--mark", mark_id])
    code, receipt = _run(argv)
    if code == EXIT_OK:
        return receipt
    if code in (EXIT_REFUSED, EXIT_HALTED):
        raise RemovalRefused(Refused(receipt))
    raise ExecutorUnavailable(f"the marked removal was rejected (exit {code}): {receipt.get('error', receipt)}")
