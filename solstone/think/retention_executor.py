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
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any

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
