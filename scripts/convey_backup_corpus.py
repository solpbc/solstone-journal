#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Conformance oracle for the backup app's owner-facing surface.

This drives the reference Flask application over the backup surface in six
journal states and records **the full response body** of every probe, so a port
is checked against what the reference *answered* and not merely against which
routes it answered.

🔴 **Why six states and not three.** The session gate contributes three
(unestablished / established / corrupt-config). The other three are the whole
reason this corpus exists: `fresh` (backup never configured), `broken` (backup
configured, the last run FAILED, and the last read-back verification FAILED),
and `healthy` (the last run succeeded). A status surface that cannot tell those
three apart is the failure this app cannot be allowed to ship, and a corpus that
records only one of them cannot detect it.

⚠ **`broken` is not a variant of `fresh`.** The reference's engine records an
error with a *fresh wall-clock timestamp*
(`solstone/think/backup/engine.py:330-335` — `status="error", time=int(time.time())`),
so a failing backup and a succeeding one are separated **only** by `status` and
`error_reason`. Both carry a recent `time`.

⛔ Every value here is synthetic. No probe reads a real journal, and the seeded
journal is a fresh temporary directory per phase.

Usage:
    python scripts/convey_backup_corpus.py            # write the corpus
    python scripts/convey_backup_corpus.py --check    # fail if it would change
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

# Pin before importing time or any Solstone module: routes read process-local time.
os.environ["TZ"] = "UTC"
import time  # noqa: E402

if hasattr(time, "tzset"):
    time.tzset()

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "core" / "fixtures" / "convey_backup_corpus.json"
sys.path.insert(0, str(REPO_ROOT))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from corpus_scrub import (  # noqa: E402
    assert_egress_guard_can_see,
    assert_guard_can_see,
    assert_no_egress_attempted,
    assert_publishable,
    forbid_non_loopback_egress,
)

# The two destinations the guard's own positive control provokes. Anything else
# in the attempt log is a reference route reaching out.
CONTROL_DESTINATIONS = ("example.invalid", "198.51.100.7")

# 🔴 Installed BEFORE any Solstone module is imported. Driving a reference
# route is not provably read-only: one app's list endpoint was measured
# registering a real account on a production service while being probed on a
# throwaway journal. The harness makes egress impossible rather than reasoning
# about which routes reach out; loopback stays open for callosum and friends.
forbid_non_loopback_egress()
assert_egress_guard_can_see(__file__)
# ⚠ **The unit of analysis is the BLUEPRINT, not the route.** A drain registered
# on `before_request`/`after_request` runs for every request to that blueprint,
# including refusals, so auditing the routes you intend to probe does not bound
# what probing them reaches. Audited on 2026-08-13 with a positive control:
# `app:support` registers `_drain_pending_acknowledgements_{before,after}_request`
# and the query found both, while `app:import` and `app:backup` register **no**
# blueprint-scoped hooks at all. The four app-wide hooks are convey core —
# identity stamp, request id, the access gate, the loopback-origin guard.


PLACEHOLDER_ROOT = "<JOURNAL_ROOT>"

# A fixed setup instant so the established phases are byte-reproducible.
PINNED_COMPLETED_AT = 1767225600
# Fixed backup-run instants. Deliberately *recent-looking* relative to each
# other but absolute, so nothing in a recorded body depends on the run clock.
PINNED_BACKUP_OK_TIME = 1770000000
PINNED_BACKUP_ERROR_TIME = 1770003600
PINNED_PRUNE_TIME = 1769996400
PINNED_VERIFY_OK_TIME = 1769990000
PINNED_VERIFY_ERROR_TIME = 1770003000
PINNED_SNAPSHOT_ID = "9f2c1ab4"
# 🔴 A recovery key is 64 Crockford-32 characters and the reference VALIDATES it
# on read (`think/backup/keys.py:30-33`). A placeholder string here makes
# `/app/backup/confirm` answer 500, and that 500 would be recorded as if it were
# the contract. The Crockford alphabet twice over is 64 valid characters.
PINNED_RECOVERY_KEY = "0123456789ABCDEFGHJKMNPQRSTVWXYZ" * 2

# (method, path, body-or-None, why this probe is in the corpus)
Probe = tuple[str, str, dict[str, Any] | None, str]

PROBES: tuple[Probe, ...] = (
    ("GET", "/app/backup/", None, "the app index: the SPA shell, byte-identical"),
    ("GET", "/app/backup/workspace", None, "the app fragment bytes"),
    ("GET", "/app/backup/background", None, "the background fragment"),
    ("GET", "/app/backup/static/backup.js", None, "per-app static script"),
    ("GET", "/app/backup/static/backup.css", None, "per-app static stylesheet"),
    (
        "GET",
        "/app/backup/status",
        None,
        "🔴 the honesty probe: the full status view an owner's page renders from",
    ),
    ("GET", "/app/backup/offload/status", None, "media-offload status view"),
    # Refusals. Every one of these must stay non-2xx: a refusal that answers 200
    # tells the page it succeeded, and the page then evaluates refusal JSON as if
    # it were state.
    (
        "POST",
        "/app/backup/retention",
        None,
        "mutation with no body: the missing-body refusal",
    ),
    (
        "POST",
        "/app/backup/retention",
        {"hourly": "not-a-number"},
        "mutation with an uncoercible value: the invalid-value refusal",
    ),
    (
        "POST",
        "/app/backup/confirm",
        {"recovery_key": "wrong-key-entirely"},
        "recovery-key mismatch: the refusal an owner can actually trigger",
    ),
    # 🔴 THE RESTORE PROBES MUST NEVER CARRY A VALID RECOVERY KEY, and the reason
    # is not tidiness. A well-formed key passes the guard and reaches
    # `think/backup/restore.py::restore_journal`, which runs restic against the
    # configured repository and writes over the journal it was pointed at. These
    # two probes are shaped to refuse *before* that call, and they exist to pin
    # the refusals — ⛔ do not "improve coverage" by supplying a real key.
    (
        "POST",
        "/app/backup/restore",
        None,
        "restore with no body: refuses before reaching the restore engine",
    ),
    (
        "POST",
        "/app/backup/restore",
        {"recovery_key": "   "},
        "restore with a blank key: whitespace refuses before reaching the restore engine",
    ),
    (
        "POST",
        "/app/backup/offload/config",
        {"budget_bytes": -1},
        "offload config with a negative budget",
    ),
)


def _backup_section(phase: str) -> dict[str, Any] | None:
    """Return the ``backup`` config section for a phase, or None to omit it."""
    if phase == "fresh":
        return None
    common = {
        "enabled": True,
        "mode": "byo",
        "destination": {
            "repository": "s3:s3.example.invalid/journal-corpus",
            "backend": "s3",
            "credentials": {
                "access_key_id": "CORPUSKEYID",
                "secret_access_key": "corpus-secret",
            },
        },
        "daily_key": "corpus-daily-key",
        "recovery_key": PINNED_RECOVERY_KEY,
        "confirmed_recovery_key": True,
        "retention": {"hourly": 24, "daily": 7, "weekly": 4, "monthly": 12},
        "offload": {"enabled": False, "budget_bytes": None, "floor_bytes": None},
        "schedule": {"every": "daily", "enabled": True},
        "last_prune": {
            "time": PINNED_PRUNE_TIME,
            "status": "ok",
            "error_reason": None,
        },
    }
    if phase == "enabled_never_run":
        # Configured and armed, but nothing has ever run. This is the state the
        # owner-visible words "not yet" are TRUE for.
        common["last_backup"] = {
            "time": None,
            "snapshot_id": None,
            "status": None,
            "error_reason": None,
        }
        common["last_verification"] = {
            "time": None,
            "status": None,
            "reason": None,
            "checked_subset": None,
            "last_ok_time": None,
        }
        common["last_prune"] = {"time": None, "status": None, "error_reason": None}
        return common
    if phase == "broken":
        # 🔴 The phase this corpus exists for. A failed run carries a FRESH
        # timestamp; only `status` and `error_reason` distinguish it from a
        # successful one, and `last_verification` failed after its last ok.
        common["last_backup"] = {
            "time": PINNED_BACKUP_ERROR_TIME,
            "snapshot_id": None,
            "status": "error",
            # ⚠ A REAL value from the engine's vocabulary. `reason_for_returncode`
            # (`think/backup/runner.py:109`) emits `incomplete` · `repo_missing` ·
            # `locked` · `auth_failed` · `timeout` · `failed`, and the engine adds
            # `restic_unavailable` and `rclone_unavailable`. ⛔ An oracle carrying a
            # value the producer cannot emit teaches the port a vocabulary that does
            # not exist, and a label table built from it has a dead entry and eight
            # missing ones.
            "error_reason": "locked",
        }
        common["last_verification"] = {
            "time": PINNED_VERIFY_ERROR_TIME,
            "status": "error",
            "reason": "read_data_mismatch",
            "checked_subset": "5%",
            "last_ok_time": PINNED_VERIFY_OK_TIME,
        }
        return common
    if phase == "healthy":
        common["last_backup"] = {
            "time": PINNED_BACKUP_OK_TIME,
            "snapshot_id": PINNED_SNAPSHOT_ID,
            "status": "ok",
            "error_reason": None,
        }
        common["last_verification"] = {
            "time": PINNED_VERIFY_OK_TIME,
            "status": "ok",
            "reason": None,
            "checked_subset": "5%",
            "last_ok_time": PINNED_VERIFY_OK_TIME,
        }
        return common
    raise AssertionError(f"unknown phase {phase}")


def _build_journal(root: Path, phase: str) -> None:
    """Create the journal a phase's probes run against."""
    if phase == "unestablished":
        return
    (root / "config").mkdir(parents=True, exist_ok=True)
    target = root / "config" / "journal.json"
    if phase == "corrupt":
        # A config that EXISTS and cannot be parsed. This is a third gate
        # outcome, not a variant of the second: `journal_is_active` raises.
        target.write_text('{"setup": {"completed_at": 17672256')
        return
    config: dict[str, Any] = {"setup": {"completed_at": PINNED_COMPLETED_AT}}
    section = _backup_section(phase)
    if section is not None:
        config["backup"] = section
    target.write_text(json.dumps(config, indent=2, sort_keys=True) + "\n")


# 🔴 Fixed device geometry. `/app/backup/offload/status` reaches
# `think/offload_restore.measure_offload_status`, which calls `device_free_bytes()`
# and `device_total_bytes()` — **ambient host state**. Two captures 30 seconds
# apart on the generating host differed in `free_bytes`, and `total_bytes` feeds
# `suggest_offload_defaults`, so the whole suggestion block moves with it.
#
# ⛔ Do NOT solve this by normalizing the fields away: they are the only thing
# pinning the suggestion arithmetic, and a port returning the wrong defaults would
# then match. ✅ The harness ESTABLISHES the condition the recorded case depends
# on, which is also what keeps a build host's real free-space number out of a
# public repository.
PINNED_DEVICE_TOTAL_BYTES = 1_000_000_000_000
PINNED_DEVICE_FREE_BYTES = 250_000_000_000


def _pin_device_geometry() -> None:
    """Replace ambient device measurements with fixed ones, at the call site."""
    from solstone.think import offload_restore

    assert hasattr(offload_restore, "device_free_bytes"), (
        "offload_restore.device_free_bytes moved; the device pin no longer binds"
    )
    assert hasattr(offload_restore, "device_total_bytes"), (
        "offload_restore.device_total_bytes moved; the device pin no longer binds"
    )
    offload_restore.device_free_bytes = lambda: PINNED_DEVICE_FREE_BYTES
    offload_restore.device_total_bytes = lambda: PINNED_DEVICE_TOTAL_BYTES


def _reset_module_state() -> None:
    """Clear the route module's process-global caches between phases.

    ⚠ `routes.py` keeps a 60-second offload measurement cache and a long-op
    registry at module scope. Without this, phase N's body can carry phase
    N-1's measurement and the corpus records a value no implementation can
    reproduce.
    """
    from solstone.apps.backup import routes as backup_routes

    backup_routes._clear_registry()
    backup_routes._clear_measurement_cache()


def _record(client: Any, probe: Probe, root: Path) -> dict[str, Any]:
    method, path, body, why = probe
    # Deliberately uncaught: a reference exception must fail corpus generation
    # rather than be recorded as a case.
    if body is None:
        response = client.open(path, method=method)
    else:
        response = client.open(path, method=method, json=body)
    raw = response.get_data()
    redacted = raw.replace(str(root).encode(), PLACEHOLDER_ROOT.encode())
    content_type = response.headers.get("Content-Type", "")
    case: dict[str, Any] = {
        "method": method,
        "path": path,
        "why": why,
        "status": response.status_code,
        "content_type": content_type,
    }
    if body is not None:
        case["request_json"] = body
    location = response.headers.get("Location")
    if location:
        case["location"] = location
    if redacted != raw:
        case["body_normalized"] = [PLACEHOLDER_ROOT]

    if "json" in content_type:
        # 🔴 The whole body, not a summary. A corpus that records which routes
        # answered cannot see a map served where a list was published.
        case["json"] = json.loads(redacted)
        case["body_sha256"] = hashlib.sha256(
            json.dumps(case["json"], sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        case["body_sha256_basis"] = "canonical-json"
        return case

    case["body_bytes"] = len(redacted)
    case["body_sha256"] = hashlib.sha256(redacted).hexdigest()
    case["body_sha256_basis"] = "raw-body"
    if response.status_code >= 400:
        case["body_text"] = redacted.decode("utf-8", errors="replace")
    return case


PHASES = (
    "unestablished",
    "corrupt",
    "fresh",
    "enabled_never_run",
    "broken",
    "healthy",
)


def build_corpus() -> dict[str, Any]:
    from solstone.convey import create_app

    cases: dict[str, list[dict[str, Any]]] = {}
    for phase in PHASES:
        with tempfile.TemporaryDirectory(prefix=f"convey-backup-{phase}-") as tmp:
            root = Path(tmp)
            _build_journal(root, phase)
            os.environ["SOLSTONE_JOURNAL"] = str(root)
            os.environ["SOLSTONE_DISABLE_CONVEY_SIDE_RUNTIMES"] = "1"
            app = create_app(str(root))
            _pin_device_geometry()
            _reset_module_state()
            client = app.test_client()
            cases[phase] = [_record(client, probe, root) for probe in PROBES]

    return {
        "schema": "solstone-convey-backup-corpus-v1",
        "generator": "scripts/convey_backup_corpus.py",
        "tz": "UTC",
        "pinned": {
            "completed_at": PINNED_COMPLETED_AT,
            "backup_ok_time": PINNED_BACKUP_OK_TIME,
            "backup_error_time": PINNED_BACKUP_ERROR_TIME,
            "prune_time": PINNED_PRUNE_TIME,
            "verify_ok_time": PINNED_VERIFY_OK_TIME,
            "verify_error_time": PINNED_VERIFY_ERROR_TIME,
            "snapshot_id": PINNED_SNAPSHOT_ID,
        },
        "placeholders": {"journal_root": PLACEHOLDER_ROOT},
        # 🔴 WHAT A GREEN REPLAY OF THIS CORPUS IS NOT EVIDENCE ABOUT.
        # Written into the fixture, not only into this generator, so a future
        # reader cannot mistake a green replay for coverage. Every write route in
        # this conversion is outside a corpus's reach by construction: a POST
        # would mutate the sequential per-phase journal underneath later probes.
        "coverage_limits": {
            "note": (
                "A green replay proves the recorded GET bodies and the recorded "
                "rejection bodies. It proves nothing about any route below, and "
                "those routes must be checked by reading the reference."
            ),
            "no_probe_at_all": [
                "POST /app/backup/keys/generate",
                "POST /app/backup/recovery-key/reveal",
                "POST /app/backup/backup-now",
                "POST /app/backup/offload/enable",
                "POST /app/backup/offload/disable",
                "POST /app/backup/enable",
                "POST /app/backup/enable-hosted",
                "POST /app/backup/destination",
                "POST /app/backup/recovery-key/rotate",
                "POST /app/backup/teardown",
                "POST /app/backup/restore-hosted",
                "POST /app/backup/offload/restore",
            ],
            "rejection_paths_only": [
                "POST /app/backup/retention",
                "POST /app/backup/confirm",
                "POST /app/backup/offload/config",
                "POST /app/backup/restore",
            ],
            "no_successful_mutation_is_recorded_anywhere": True,
            "named_hazards_a_replay_cannot_see": [
                "generate_and_store_keys fills daily_key and recovery_key ONLY when "
                "they are None. A second call returns the existing keys. A port "
                "written as an unconditional generate-then-store overwrites the "
                "owner's recovery key and orphans every existing snapshot, and "
                "returns success.",
                "mutate_journal_config returns before taking the lock and before "
                "writing when the computed change is a no-op. A port that always "
                "reports changed writes on every call.",
            ],
        },
        "phases": cases,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit non-zero if the corpus on disk differs from a fresh capture",
    )
    args = parser.parse_args()

    corpus = build_corpus()
    rendered = json.dumps(corpus, indent=2, sort_keys=True) + "\n"

    # 🔴 These fixtures are published. Prove the guard can see a leak, then run it.
    # ⚠ This corpus is exactly why the guard exists: its first capture carried
    # the generating host's real disk geometry, read through
    # `measure_offload_status`.
    assert_no_egress_attempted(
        f"convey {'backup' if 'backup' in __file__ else 'import'} corpus",
        ignore=CONTROL_DESTINATIONS,
    )
    assert_guard_can_see("backup corpus")
    assert_publishable(rendered, label="convey backup corpus")

    if args.check:
        if not CORPUS_PATH.exists():
            print(f"missing corpus: {CORPUS_PATH}", file=sys.stderr)
            return 1
        if CORPUS_PATH.read_text() != rendered:
            print(
                f"convey backup corpus is stale: {CORPUS_PATH}\n"
                "regenerate with: python scripts/convey_backup_corpus.py",
                file=sys.stderr,
            )
            return 1
        print(f"convey backup corpus is current: {CORPUS_PATH}")
        return 0

    CORPUS_PATH.parent.mkdir(parents=True, exist_ok=True)
    CORPUS_PATH.write_text(rendered)
    total = sum(len(phase) for phase in corpus["phases"].values())
    print(f"wrote {CORPUS_PATH} ({total} cases across {len(corpus['phases'])} phases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
