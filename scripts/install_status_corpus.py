#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Behavioural oracle for the provider install-status record.

`health/providers/{provider}.json` records what an install attempt is doing:
which target it is installing, how far it has got, and whether the last attempt
finished, failed, or was interrupted. The record is per-provider, so a native
writer of one provider's file and a Python writer of another's do not collide —
which is the only reason the local lane's half can be converted on its own.

⚠ **But that leaves one on-disk format with two implementations**, and the format
is not shape-only. The transitions are guarded: a state may only follow certain
states, a write may only land if it still owns the attempt, and a progress bump
coalesces on a wall-clock interval. None of that is visible in the record's
fields, so a native implementation that reproduces the schema and re-derives the
rules produces a file that validates and a state machine that diverges.

This corpus records what the reference **decides** for each transition — the
resulting status, or the refusal and its type — so the native side is compared
against answers rather than against a restatement of its own logic.

⚠ Determinism: `attempt_id` is a UUID and every timestamp is wall-clock, so both
are **redacted** to stable placeholders. What is recorded is the shape and the
decision, not the identifiers. Progress coalescing is time-dependent and is
recorded as its interval constant rather than as an observed suppression, since
a corpus that slept would not be reproducible.

⚠ Regenerating requires a runnable reference tree. This is a frozen record.
"""

from __future__ import annotations

import json
import tempfile
from pathlib import Path
from typing import Any, get_args

from local_contract_corpus import REFERENCE_SOURCES, reference_commit
from solstone.think.providers import install_state as ist

FIXTURE_VERSION = 1
PROVIDER = "local"
TARGET = {"model": "local/qwen3.5-4b", "server": "b1234", "backend": "vulkan"}
OTHER_TARGET = {"model": "local/qwen3.5-4b", "server": "b9999", "backend": "cuda"}

REDACTED_ID = "<attempt-id>"
REDACTED_TIME = "<timestamp>"
_TIME_FIELDS = (
    "started_at",
    "last_transition_at",
    "last_progress_at",
    "completed_at",
)


def _redact(status: Any) -> Any:
    """Stable placeholders for the two fields that cannot be reproduced."""
    if status is None:
        return None
    out = dict(status)
    if out.get("attempt_id") is not None:
        out["attempt_id"] = REDACTED_ID
    for field in _TIME_FIELDS:
        if out.get(field) is not None:
            out[field] = REDACTED_TIME
    return out


def _case(name: str, note: str, run) -> dict[str, Any]:
    """Run one scripted sequence in its own journal and record the outcome."""
    with tempfile.TemporaryDirectory(prefix="lanej-install-") as root:
        journal = Path(root)
        (journal / "health" / "providers").mkdir(parents=True)
        try:
            result = run(journal)
            outcome: dict[str, Any] = {"refused": None, "status": _redact(result)}
        except Exception as exc:
            # ⚠ A refusal message can name the journal it was refused in, and the
            # journal here is a temporary directory. Redacting the root keeps the
            # message's shape — which is what a reader compares — without pinning
            # a path that changes on every regeneration.
            outcome = {
                "refused": f"{type(exc).__name__}: {exc}".replace(
                    str(journal), "<journal>"
                ),
                "status": None,
            }
        # The file the sequence left behind is the durable half, and it is not
        # always the value the verb returned.
        path = journal / "health" / "providers" / f"{PROVIDER}.json"
        if not path.exists():
            outcome["on_disk"] = None
        else:
            raw = path.read_text()
            try:
                outcome["on_disk"] = _redact(json.loads(raw))
            except json.JSONDecodeError:
                # A case can leave the file deliberately unparseable. That IS the
                # durable state, and recording it as raw text is the point: a
                # native reader must meet the same bytes.
                outcome["on_disk"] = None
                outcome["on_disk_raw"] = raw
        outcome["file_mode"] = format(path.stat().st_mode & 0o777, "04o") if path.exists() else None
    return {"name": name, "note": note, **outcome}


def _begin(journal: Path, *, target: dict[str, Any] = TARGET, initial="resolving"):
    return ist.begin_install_attempt(
        PROVIDER, target, initial_state=initial, journal_path=journal
    )


def _cases() -> list[dict[str, Any]]:
    states = sorted(get_args(ist.InstallState))
    cases: list[dict[str, Any]] = []

    cases.append(
        _case(
            "read_before_any_write",
            "a provider that has never installed reads as idle rather than absent",
            lambda j: ist.read_install_status(name=PROVIDER, journal_path=j),
        )
    )
    cases.append(
        _case(
            "begin_from_idle",
            "the nominal start; records the target fingerprint and its digest",
            lambda j: _begin(j),
        )
    )
    cases.append(
        _case(
            "begin_twice_same_target",
            "a second begin while one is in flight — the guard that stops two "
            "installers racing the same artifact",
            lambda j: (_begin(j), _begin(j))[1],
        )
    )
    cases.append(
        _case(
            "begin_twice_different_target",
            "a second begin for a DIFFERENT target while one is in flight; the "
            "answer decides whether an owner changing their model mid-install "
            "wedges or preempts",
            lambda j: (_begin(j), _begin(j, target=OTHER_TARGET))[1],
        )
    )
    cases.append(
        _case(
            "begin_or_replace_takes_over",
            "the explicit preemption verb, which is a different decision from "
            "the one above",
            lambda j: (
                _begin(j),
                ist.begin_or_replace_install_attempt(
                    PROVIDER, OTHER_TARGET, journal_path=j
                ),
            )[1],
        )
    )

    # Every transition out of every state, legal and illegal alike. The point of
    # the matrix is that the ILLEGAL ones are the contract too — a native
    # implementation that permits one produces a record the next reader believes.
    for source in states:
        for target_state in states:
            def run(j, source=source, target_state=target_state):
                status = _begin(j, initial="resolving")
                if source != "resolving":
                    status = ist.write_install_status(
                        ist.transition_state(status, new_state=source),
                        journal_path=j,
                    )
                return ist.write_install_status(
                    ist.transition_state(status, new_state=target_state),
                    journal_path=j,
                )

            cases.append(
                _case(
                    f"transition_{source}_to_{target_state}",
                    "one cell of the transition matrix",
                    run,
                )
            )

    cases.append(
        _case(
            "transition_to_failed_carries_error",
            "a failure records both the message and the code; a native writer "
            "that drops the code leaves an owner-facing surface with nothing to "
            "present",
            lambda j: ist.write_install_status(
                ist.transition_state(
                    _begin(j),
                    new_state="failed",
                    error="download timed out",
                    error_code="network_unreachable",
                ),
                journal_path=j,
            ),
        )
    )
    cases.append(
        _case(
            "progress_bump",
            "the received/total pair the owner's install progress reads from. ⚠ It takes no journal — it composes a new status and the CALLER persists it, which is a different ownership from every other verb here",
            lambda j: ist.bump_progress(
                ist.write_install_status(
                    ist.transition_state(_begin(j), new_state="downloading"),
                    journal_path=j,
                ),
                received=1024,
                total=4096,
            ),
        )
    )
    cases.append(
        _case(
            "progress_bump_without_total",
            "a server that sends no content length; total stays absent rather "
            "than becoming zero, which would render as 100%",
            lambda j: ist.bump_progress(
                ist.write_install_status(
                    ist.transition_state(_begin(j), new_state="downloading"),
                    journal_path=j,
                ),
                received=1024,
            ),
        )
    )
    cases.append(
        _case(
            "stale_attempt_write_is_refused",
            "🔴 the lost-update guard: a status captured before another attempt "
            "began must not be able to write over it",
            lambda j: (
                stale := _begin(j),
                ist.begin_or_replace_install_attempt(
                    PROVIDER, OTHER_TARGET, journal_path=j
                ),
                ist.write_install_status(
                    ist.transition_state(stale, new_state="installed"),
                    journal_path=j,
                ),
            )[-1],
        )
    )
    cases.append(
        _case(
            "assert_current_after_replacement",
            "the same guard read rather than written",
            lambda j: (
                stale := _begin(j),
                ist.begin_or_replace_install_attempt(
                    PROVIDER, OTHER_TARGET, journal_path=j
                ),
                ist.assert_install_attempt_current(stale, journal_path=j),
            )[-1],
        )
    )
    cases.append(
        _case(
            "record_interrupted",
            "what a killed installer leaves behind, which is the state the next "
            "start has to recognise",
            lambda j: (
                started := _begin(j),
                ist.record_interrupted_install(
                    PROVIDER,
                    attempt_id=str(started["attempt_id"]),
                    target_fingerprint_sha256=started["target_fingerprint_sha256"],
                    journal_path=j,
                ),
            )[-1],
        )
    )
    cases.append(
        _case(
            "record_interrupted_wrong_attempt",
            "an interruption reported against an attempt that is no longer the "
            "current one must not clobber the live attempt",
            lambda j: (
                _begin(j),
                ist.record_interrupted_install(
                    PROVIDER,
                    attempt_id="00000000-0000-0000-0000-000000000000",
                    target_fingerprint_sha256=None,
                    journal_path=j,
                ),
            )[-1],
        )
    )
    cases.append(
        _case(
            "malformed_record_is_refused_not_replaced",
            "🔴 fail closed on read: a status that exists and will not parse "
            "raises rather than being silently reset to idle, which would lose "
            "the evidence of what went wrong",
            lambda j: (
                (j / "health" / "providers" / f"{PROVIDER}.json").write_text("{{"),
                ist.read_install_status(name=PROVIDER, journal_path=j),
            )[-1],
        )
    )
    cases.append(
        _case(
            "unknown_provider_is_refused",
            "the provider vocabulary is closed; a path is never built from an "
            "unvalidated name",
            lambda j: ist.read_install_status(name="not-a-provider", journal_path=j),
        )
    )
    return cases


def build_install_status_fixture() -> dict[str, Any]:
    return {
        "fixture": "solstone-install-status",
        "fixture_version": FIXTURE_VERSION,
        "generated_by": "make core-fixtures",
        "generator": "scripts/install_status_corpus.py",
        "reference_sources": list(REFERENCE_SOURCES)
        + ["solstone/think/providers/install_state.py"],
        "reference_commit": reference_commit(
            ("solstone/think/providers/install_state.py",)
        ),
        "schema_version": ist.SCHEMA_VERSION,
        "providers": sorted(ist.PROVIDERS),
        "states": sorted(get_args(ist.InstallState)),
        "in_flight_states": sorted(ist.IN_FLIGHT_STATES),
        "terminal_states": sorted(ist.TERMINAL_STATES),
        "progress_coalesce_seconds": ist.PROGRESS_COALESCE_SECONDS,
        "path_template": "health/providers/{provider}.json",
        "redacted": {"attempt_id": REDACTED_ID, "timestamps": REDACTED_TIME},
        "targets": {"primary": TARGET, "other": OTHER_TARGET},
        "cases": _cases(),
    }


if __name__ == "__main__":  # pragma: no cover - manual regeneration aid
    print(json.dumps(build_install_status_fixture(), indent=2, ensure_ascii=False))
