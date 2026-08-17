# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import copy
import os
import plistlib
import shlex
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.think import install_guard, service


@pytest.fixture
def doctor():
    from solstone.think import doctor as doctor_module

    return doctor_module


@pytest.fixture
def home_root(monkeypatch, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    return home


def args(doctor):
    return doctor.Args(verbose=False, json=False, jsonl=False, port=5015)


DOCTOR_NOW_MS = 2_000_000_000_000
MINUTE_MS = 60_000
HOUR_MS = 60 * MINUTE_MS
TEST_DEVICE_BINDING = {"device": "sha256:" + ("e" * 64), "kind": "cert"}


def bound_observer(**fields):
    return {"device_binding": dict(TEST_DEVICE_BINDING), **fields}


def make_repo(tmp_path: Path, *, worktree: bool = False) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    if worktree:
        (repo / ".git").write_text("gitdir: /tmp/worktree\n", encoding="utf-8")
    else:
        (repo / ".git").mkdir()
    return repo


def make_alias(home_root: Path, binary: str, target: Path | str) -> Path:
    alias = home_root / ".local" / "bin" / binary
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.symlink_to(target)
    return alias


def make_existing_target(path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("", encoding="utf-8")
    return path


def patch_alias_absent(doctor, monkeypatch):
    monkeypatch.setattr(
        doctor,
        "import_install_guard",
        lambda: (install_guard.AliasState, install_guard.check_alias),
    )


def tree_snapshot(root: Path) -> list[tuple[str, str, str]]:
    snapshot: list[tuple[str, str, str]] = []
    for path in sorted(root.rglob("*")):
        rel = path.relative_to(root).as_posix()
        if path.is_symlink():
            snapshot.append((rel, "symlink", os.readlink(path)))
        elif path.is_file():
            snapshot.append((rel, "file", path.read_text(encoding="utf-8")))
        elif path.is_dir():
            snapshot.append((rel, "dir", ""))
    return snapshot


class _FakeUids:
    def __init__(self, real: int):
        self.real = real


class _FakeProcess:
    def __init__(self, *, pid: int = 99, uid: int = 501, exe: str = "/tmp/other"):
        self.pid = pid
        self._uid = uid
        self._exe = exe

    def uids(self) -> _FakeUids:
        return _FakeUids(self._uid)

    def exe(self) -> str:
        return self._exe


def force_darwin_supervisor_reader(
    doctor,
    monkeypatch,
    *,
    launchctl: subprocess.CompletedProcess,
    processes: list[_FakeProcess] | None = None,
) -> None:
    monkeypatch.setattr(doctor, "platform_tag", lambda: "darwin")
    monkeypatch.setattr(service.sys, "platform", "darwin")
    monkeypatch.setattr(service.os, "getuid", lambda: 501)
    monkeypatch.setattr(service.subprocess, "run", lambda *args, **kwargs: launchctl)
    monkeypatch.setattr(
        service.psutil,
        "process_iter",
        lambda: [] if processes is None else processes,
    )


def write_legacy_plist(home_root: Path, argv: list[str]) -> Path:
    plist_path = home_root / "Library" / "LaunchAgents" / "org.solpbc.solstone.plist"
    plist_path.parent.mkdir(parents=True)
    plist_path.write_bytes(
        plistlib.dumps(
            {
                "Label": "org.solpbc.solstone",
                "ProgramArguments": argv,
            }
        )
    )
    return plist_path


def write_foreign_plist(
    home_root: Path,
    filename: str,
    *,
    label: str,
    keep_alive=True,
    program: str | None = None,
    program_arguments: list[str] | None = None,
) -> Path:
    plist_path = home_root / "Library" / "LaunchAgents" / filename
    plist_path.parent.mkdir(parents=True, exist_ok=True)
    data = {
        "Label": label,
        "KeepAlive": keep_alive,
    }
    if program is not None:
        data["Program"] = program
    if program_arguments is not None:
        data["ProgramArguments"] = program_arguments
    plist_path.write_bytes(plistlib.dumps(data))
    return plist_path


def install_router_skill_links(doctor, journal: Path) -> None:
    sources = doctor._discover_project_sources(doctor.ROOT)
    for rel_dir in [Path(".claude/skills"), Path(".agents/skills")]:
        skills_dir = journal / rel_dir
        skills_dir.mkdir(parents=True)
        for source in sources:
            link = skills_dir / source.name
            link.symlink_to(os.path.relpath(source, skills_dir))


def test_service_running_ok(doctor, monkeypatch):
    monkeypatch.setattr(doctor, "service_is_installed", lambda: True)
    monkeypatch.setattr(doctor, "fetch_supervisor_status", lambda: {"crashed": []})

    result = doctor.service_running_check(args(doctor))

    assert result.status == "ok"
    assert result.detail == "journal service is running"


def test_service_running_stopped_warns(doctor, monkeypatch):
    monkeypatch.setattr(doctor, "service_is_installed", lambda: True)
    monkeypatch.setattr(doctor, "fetch_supervisor_status", lambda: None)
    monkeypatch.setattr(doctor, "service_is_failed", lambda: False)

    result = doctor.service_running_check(args(doctor))

    assert result.status == "warn"
    assert result.detail == "service installed but not running"
    assert result.fix == "run journal service start"


def test_service_running_failed_unit_fails(doctor, monkeypatch):
    monkeypatch.setattr(doctor, "service_is_installed", lambda: True)
    monkeypatch.setattr(doctor, "fetch_supervisor_status", lambda: None)
    monkeypatch.setattr(doctor, "service_is_failed", lambda: True)

    result = doctor.service_running_check(args(doctor))

    assert result.status == "fail"
    assert result.detail == "journal service unit is failed"


def test_observer_ingest_health_warns(doctor, monkeypatch):
    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [
            {
                "name": "fedora",
                "enabled": True,
                "health": {
                    "ingest_rejection": {
                        "version": "0.3.1",
                        "summary": "screen.jsonl:2: value is not of type 'number'",
                        "active_count": 2,
                        "first_ts": 1700000000000,
                    }
                },
            }
        ],
    )

    result = doctor.observer_ingest_health_check(args(doctor))

    assert result.status == "warn"
    assert "fedora" in result.detail
    assert "screen.jsonl:2" in result.detail
    assert "2x" in result.detail
    assert "2023-11-14" in result.detail


def test_observer_ingest_health_ok_and_skip(doctor, monkeypatch):
    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [{"name": "fedora", "enabled": True}],
    )

    result = doctor.observer_ingest_health_check(args(doctor))

    assert result.status == "ok"

    monkeypatch.setattr("solstone.apps.observer.utils.list_observers", lambda: [])

    result = doctor.observer_ingest_health_check(args(doctor))

    assert result.status == "skip"


def test_capture_health_check_maps_every_rollup_status(doctor, monkeypatch):
    now = DOCTOR_NOW_MS

    cases = [
        (
            "active",
            "ok",
            [bound_observer(name="desktop", last_seen=now)],
            "rollup=active; observers reaching the journal",
            True,
        ),
        (
            "stale",
            "warn",
            [bound_observer(name="desktop", last_seen=now - MINUTE_MS)],
            "rollup=stale; observers: desktop=stale",
            True,
        ),
        (
            "offline",
            "warn",
            [bound_observer(name="desktop")],
            "rollup=offline; observers: desktop=offline",
            False,
        ),
        (
            "degraded",
            "warn",
            [bound_observer(name="desktop", health={"ingest_rejection": {}})],
            "rollup=degraded; observers: desktop=degraded",
            False,
        ),
        (
            "no_observers",
            "skip",
            [],
            "rollup=no_observers; no registered observers",
            False,
        ),
    ]

    for rollup, expected_status, observers, detail, needs_clock in cases:
        if needs_clock:
            monkeypatch.setattr("solstone.think.capture_health.now_ms", lambda: now)
        monkeypatch.setattr(
            "solstone.apps.observer.utils.list_observers",
            lambda observers=observers: observers,
        )

        result = doctor.capture_health_check(args(doctor))

        assert result.name == "capture_health"
        assert result.status == expected_status, rollup
        assert result.detail == detail
        if expected_status == "warn":
            assert result.fix == doctor._CAPTURE_HEALTH_FIX

    def raise_at_call_time() -> list[dict]:
        raise RuntimeError("boom")

    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        raise_at_call_time,
    )

    result = doctor.capture_health_check(args(doctor))

    assert result.status == "skip"
    assert result.detail == "rollup=unknown; observer records unavailable"


def test_observer_binding_check_counts_active_unbound_records(doctor, monkeypatch):
    now = DOCTOR_NOW_MS
    monkeypatch.setattr("solstone.think.capture_health.now_ms", lambda: now)
    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [
            {"name": "unbound-a", "enabled": True, "last_seen": now},
            {"name": "unbound-b", "enabled": True, "last_seen": now},
            bound_observer(name="bound", enabled=True, last_seen=now),
        ],
    )

    result = doctor.observer_binding_check(args(doctor))

    assert result.name == "observer_binding"
    assert result.status == "ok"
    assert result.detail == (
        "active observer records=3; unbound=2; streams=unbound-a, unbound-b"
    )


def test_observer_binding_check_ignores_revoked_unbound_records(
    doctor,
    monkeypatch,
):
    now = DOCTOR_NOW_MS
    monkeypatch.setattr("solstone.think.capture_health.now_ms", lambda: now)
    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [
            {"name": "unbound-a", "enabled": True, "last_seen": now},
            {"name": "revoked-unbound", "revoked": True, "last_seen": now},
            bound_observer(name="bound", enabled=True, last_seen=now),
        ],
    )

    result = doctor.observer_binding_check(args(doctor))

    assert result.status == "ok"
    assert result.detail == "active observer records=2; unbound=1; streams=unbound-a"


def test_observer_binding_check_zero_observer_and_zero_unbound_wording(
    doctor,
    monkeypatch,
):
    monkeypatch.setattr("solstone.apps.observer.utils.list_observers", lambda: [])

    result = doctor.observer_binding_check(args(doctor))

    assert result.status == "ok"
    assert result.detail == "active observer records=0; unbound=0"

    now = DOCTOR_NOW_MS
    monkeypatch.setattr("solstone.think.capture_health.now_ms", lambda: now)
    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [bound_observer(name="bound", enabled=True, last_seen=now)],
    )

    result = doctor.observer_binding_check(args(doctor))

    assert result.status == "ok"
    assert result.detail == "active observer records=1; unbound=0"


def test_observer_binding_check_warning_counts_match_bound_and_unbound(
    doctor,
    monkeypatch,
):
    now = DOCTOR_NOW_MS
    monkeypatch.setattr("solstone.think.capture_health.now_ms", lambda: now)
    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [{"name": "unbound", "enabled": True, "last_seen": now}],
    )
    unbound_counts = doctor.summary_counts(
        [doctor.observer_binding_check(args(doctor))]
    )

    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [bound_observer(name="bound", enabled=True, last_seen=now)],
    )
    bound_counts = doctor.summary_counts([doctor.observer_binding_check(args(doctor))])

    assert unbound_counts["failed"] == bound_counts["failed"] == 0
    assert unbound_counts["warnings"] == bound_counts["warnings"] == 0


def test_lockstep_stale_stamps_warn_on_capture_health_only(doctor, monkeypatch):
    now = DOCTOR_NOW_MS
    stale_stamp = now - (7 * HOUR_MS)
    observer = bound_observer(
        name="desktop",
        last_seen=stale_stamp,
        last_segment_received_at=stale_stamp,
        enabled=True,
        stats={},
    )
    monkeypatch.setattr("solstone.think.capture_health.now_ms", lambda: now)
    # doctor.now_ms is the clock seam for observer_delivery_stall_check.
    monkeypatch.setattr(doctor, "now_ms", lambda: now)
    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [observer],
    )

    capture_result = doctor.capture_health_check(args(doctor))
    delivery_result = doctor.observer_delivery_stall_check(args(doctor))

    # Lockstep staleness is what a wedged upload path produces, and it is check 1
    # reachability staleness that reports it.
    assert capture_result.status == "warn"
    assert "rollup=offline" in capture_result.detail
    assert delivery_result.status == "ok"
    assert delivery_result.detail == "delivery could not be assessed for any observer"


def test_observer_delivery_stall_warns_when_reachable_but_delivery_is_old(
    doctor, monkeypatch
):
    now = DOCTOR_NOW_MS
    # doctor.now_ms is the clock seam for observer_delivery_stall_check.
    monkeypatch.setattr(doctor, "now_ms", lambda: now)
    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [
            {
                "name": "desktop",
                "last_seen": now - MINUTE_MS,
                "last_segment_received_at": now - (7 * HOUR_MS),
                "enabled": True,
                "stats": {},
            }
        ],
    )

    result = doctor.observer_delivery_stall_check(args(doctor))

    assert result.status == "warn"
    assert "observer desktop is reaching the journal" in result.detail
    assert "last reach 1m ago" in result.detail
    assert "last upload landed 420m ago" in result.detail
    assert result.fix == doctor._OBSERVER_DELIVERY_STALL_FIX


def test_observer_delivery_stall_names_duplicate_evidence(doctor, monkeypatch):
    now = DOCTOR_NOW_MS
    # doctor.now_ms is the clock seam for observer_delivery_stall_check.
    monkeypatch.setattr(doctor, "now_ms", lambda: now)
    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [
            {
                "name": "desktop",
                "last_seen": now - MINUTE_MS,
                "last_segment_received_at": now - (7 * HOUR_MS),
                "enabled": True,
                "stats": {"duplicates_rejected": 2},
            }
        ],
    )

    result = doctor.observer_delivery_stall_check(args(doctor))

    assert result.status == "warn"
    assert "prior duplicate responses=2" in result.detail
    assert "repeated uploads may be landing without a newer upload" in result.detail


def test_observer_delivery_stall_names_beacon_evidence_without_duplicate_counter(
    doctor, monkeypatch
):
    now = DOCTOR_NOW_MS
    # doctor.now_ms is the clock seam for observer_delivery_stall_check.
    monkeypatch.setattr(doctor, "now_ms", lambda: now)
    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [
            {
                "name": "desktop",
                "last_seen": now - MINUTE_MS,
                "last_segment_received_at": now - (7 * HOUR_MS),
                "enabled": True,
                "stats": {},
                "health": {
                    "beacon": {
                        "received_at": now - MINUTE_MS,
                        "pending_queue_depth": 4,
                    }
                },
            }
        ],
    )

    result = doctor.observer_delivery_stall_check(args(doctor))

    assert result.status == "warn"
    assert "pending queue depth 4" in result.detail
    assert "uploads may not be landing" in result.detail


def test_observer_delivery_stall_silent_when_observer_is_not_reaching_the_journal(
    doctor, monkeypatch
):
    now = DOCTOR_NOW_MS
    # doctor.now_ms is the clock seam for observer_delivery_stall_check.
    monkeypatch.setattr(doctor, "now_ms", lambda: now)
    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [
            {
                "name": "desktop",
                "last_seen": now - (3 * MINUTE_MS),
                "last_segment_received_at": now - (7 * HOUR_MS),
                "enabled": True,
                "stats": {},
            }
        ],
    )

    result = doctor.observer_delivery_stall_check(args(doctor))

    assert result.status == "ok"
    assert result.detail == "delivery could not be assessed for any observer"


def test_observer_delivery_stall_tolerates_unusable_stamps(doctor, monkeypatch):
    now = DOCTOR_NOW_MS
    # doctor.now_ms is the clock seam for observer_delivery_stall_check.
    monkeypatch.setattr(doctor, "now_ms", lambda: now)
    records = [
        {
            "name": "missing-reach",
            "last_seen": None,
            "last_segment_received_at": now - (7 * HOUR_MS),
            "enabled": True,
            "stats": {},
        },
        {
            "name": "missing-delivery",
            "last_seen": now - MINUTE_MS,
            "last_segment_received_at": None,
            "enabled": True,
            "stats": {},
        },
        {
            "name": "missing-both",
            "last_seen": None,
            "last_segment_received_at": None,
            "enabled": True,
            "stats": {},
        },
        {
            "name": "bad-value",
            "last_seen": "x",
            "last_segment_received_at": now - (7 * HOUR_MS),
            "enabled": True,
            "stats": {},
        },
    ]

    for record in records:
        monkeypatch.setattr(
            "solstone.apps.observer.utils.list_observers",
            lambda record=record: [record],
        )

        result = doctor.observer_delivery_stall_check(args(doctor))

        assert result.status == "ok"
        assert result.detail == "delivery could not be assessed for any observer"


def test_new_checks_complete_the_narrow_battery_with_unusable_stamps(
    doctor, monkeypatch
):
    now = DOCTOR_NOW_MS
    # doctor.now_ms is the clock seam for observer_delivery_stall_check.
    monkeypatch.setattr(doctor, "now_ms", lambda: now)
    monkeypatch.setattr("solstone.think.capture_health.now_ms", lambda: now)
    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [
            {
                "name": "bad-value",
                "last_seen": "x",
                "last_segment_received_at": now - (7 * HOUR_MS),
                "enabled": True,
                "stats": {},
            }
        ],
    )

    # This narrow battery exists because run_checks appends func(args) without a
    # per-check guard at doctor.py:1469.
    results = doctor.run_checks(
        args(doctor),
        checks=[
            (doctor.CAPTURE_HEALTH_CHECK, doctor.capture_health_check),
            (
                doctor.OBSERVER_DELIVERY_STALL_CHECK,
                doctor.observer_delivery_stall_check,
            ),
        ],
    )

    assert [result.name for result in results] == [
        "capture_health",
        "observer_delivery_stall",
    ]


def test_observer_delivery_stall_delivery_threshold_boundary_pair(doctor, monkeypatch):
    now = DOCTOR_NOW_MS
    # doctor.now_ms is the clock seam for observer_delivery_stall_check.
    monkeypatch.setattr(doctor, "now_ms", lambda: now)

    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [
            {
                "name": "inside",
                "last_seen": now - MINUTE_MS,
                "last_segment_received_at": now - doctor._OBSERVER_DELIVERY_STALL_MS,
                "enabled": True,
                "stats": {},
            }
        ],
    )
    inside = doctor.observer_delivery_stall_check(args(doctor))

    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [
            {
                "name": "outside",
                "last_seen": now - MINUTE_MS,
                "last_segment_received_at": now
                - doctor._OBSERVER_DELIVERY_STALL_MS
                - MINUTE_MS,
                "enabled": True,
                "stats": {},
            }
        ],
    )
    outside = doctor.observer_delivery_stall_check(args(doctor))

    assert inside.status == "ok"
    assert inside.detail == "every observer is delivering"
    assert outside.status == "warn"


def test_observer_delivery_stall_reachability_window_boundary_pair(doctor, monkeypatch):
    now = DOCTOR_NOW_MS
    # doctor.now_ms is the clock seam for observer_delivery_stall_check.
    monkeypatch.setattr(doctor, "now_ms", lambda: now)

    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [
            {
                "name": "inside",
                "last_seen": now - MINUTE_MS,
                "last_segment_received_at": now - (7 * HOUR_MS),
                "enabled": True,
                "stats": {},
            }
        ],
    )
    inside = doctor.observer_delivery_stall_check(args(doctor))

    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [
            {
                "name": "outside",
                "last_seen": now - doctor._STALE_MS,
                "last_segment_received_at": now - (7 * HOUR_MS),
                "enabled": True,
                "stats": {},
            }
        ],
    )
    outside = doctor.observer_delivery_stall_check(args(doctor))

    assert inside.status == "warn"
    assert outside.status == "ok"
    assert outside.detail == "delivery could not be assessed for any observer"


def test_observer_delivery_stall_population_and_skip_semantics(doctor, monkeypatch):
    now = DOCTOR_NOW_MS
    # doctor.now_ms is the clock seam for observer_delivery_stall_check.
    monkeypatch.setattr(doctor, "now_ms", lambda: now)
    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        lambda: [
            {
                "name": "revoked",
                "last_seen": now - MINUTE_MS,
                "last_segment_received_at": now - (7 * HOUR_MS),
                "enabled": True,
                "revoked": True,
                "stats": {},
            },
            {
                "name": "disabled",
                "last_seen": now - MINUTE_MS,
                "last_segment_received_at": now - (7 * HOUR_MS),
                "enabled": False,
                "stats": {},
            },
        ],
    )

    result = doctor.observer_delivery_stall_check(args(doctor))

    assert result.status == "skip"
    assert result.detail == "no registered observers"

    def raise_at_call_time() -> list[dict]:
        raise RuntimeError("boom")

    monkeypatch.setattr(
        "solstone.apps.observer.utils.list_observers",
        raise_at_call_time,
    )

    result = doctor.observer_delivery_stall_check(args(doctor))

    assert result.status == "skip"
    assert result.detail == "observer records unavailable: RuntimeError: boom"


def test_new_doctor_checks_are_registered(doctor):
    assert (
        doctor.CAPTURE_HEALTH_CHECK,
        doctor.capture_health_check,
    ) in doctor.JOURNAL_CHECKS
    assert (
        doctor.OBSERVER_DELIVERY_STALL_CHECK,
        doctor.observer_delivery_stall_check,
    ) in doctor.JOURNAL_CHECKS
    assert (
        doctor.OBSERVER_BINDING_CHECK,
        doctor.observer_binding_check,
    ) in doctor.JOURNAL_CHECKS
    assert doctor.CAPTURE_HEALTH_CHECK.severity == "advisory"
    assert doctor.CAPTURE_HEALTH_CHECK.platforms == ("linux", "darwin")
    assert doctor.OBSERVER_BINDING_CHECK.severity == "advisory"
    assert doctor.OBSERVER_BINDING_CHECK.platforms == ("linux", "darwin")
    assert doctor.OBSERVER_DELIVERY_STALL_CHECK.severity == "advisory"
    assert doctor.OBSERVER_DELIVERY_STALL_CHECK.platforms == ("linux", "darwin")


def test_get_delivery_divergence_returns_none_for_unusable_last_seen(doctor):
    from solstone.apps.observer.utils import get_delivery_divergence

    now = DOCTOR_NOW_MS
    unusable = [
        {},
        {"last_seen": None},
        {"last_seen": True},
        {"last_seen": "x"},
        {"last_seen": 1.5},
        {"last_seen": -1},
        {"last_seen": now + 1},
    ]

    for partial in unusable:
        record = {
            "name": "desktop",
            "last_segment_received_at": now - MINUTE_MS,
            **partial,
        }

        result = get_delivery_divergence(
            record,
            now_ms=now,
            reachable_within_ms=doctor._STALE_MS,
        )

        assert result is None


def test_get_delivery_divergence_returns_none_for_unusable_last_segment_received_at(
    doctor,
):
    from solstone.apps.observer.utils import get_delivery_divergence

    now = DOCTOR_NOW_MS
    unusable = [
        {},
        {"last_segment_received_at": None},
        {"last_segment_received_at": True},
        {"last_segment_received_at": "x"},
        {"last_segment_received_at": 1.5},
        {"last_segment_received_at": -1},
        {"last_segment_received_at": now + 1},
    ]

    for partial in unusable:
        record = {
            "name": "desktop",
            "last_seen": now - MINUTE_MS,
            **partial,
        }

        result = get_delivery_divergence(
            record,
            now_ms=now,
            reachable_within_ms=doctor._STALE_MS,
        )

        assert result is None


def test_get_delivery_divergence_returns_none_outside_reachability_window(doctor):
    from solstone.apps.observer.utils import get_delivery_divergence

    now = DOCTOR_NOW_MS
    record = {
        "name": "desktop",
        "last_seen": now - doctor._STALE_MS,
        "last_segment_received_at": now - MINUTE_MS,
    }

    result = get_delivery_divergence(
        record,
        now_ms=now,
        reachable_within_ms=doctor._STALE_MS,
    )

    assert result is None


def test_get_delivery_divergence_returns_facts_without_mutating_the_record(doctor):
    from solstone.apps.observer.utils import get_delivery_divergence

    now = DOCTOR_NOW_MS
    record = {
        "name": "desktop",
        "last_seen": now - MINUTE_MS,
        "last_segment_received_at": now - (7 * HOUR_MS),
        "stats": {"duplicates_rejected": 1},
    }
    before = copy.deepcopy(record)

    result = get_delivery_divergence(
        record,
        now_ms=now,
        reachable_within_ms=doctor._STALE_MS,
    )
    second = get_delivery_divergence(
        record,
        now_ms=now,
        reachable_within_ms=doctor._STALE_MS,
    )

    assert result == {
        "name": "desktop",
        "last_seen_age_ms": MINUTE_MS,
        "last_segment_received_age_ms": 7 * HOUR_MS,
    }
    assert second == result
    assert record == before


def test_orphan_segment_pdf_check_warns_for_pdf_without_transcript(
    doctor, monkeypatch, tmp_path
):
    monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(tmp_path), "source"))
    segment = tmp_path / "chronicle" / "20250101" / "import.document" / "120000_0"
    segment.mkdir(parents=True)
    (segment / "original.pdf").write_bytes(b"%PDF-1.4 synthetic")

    result = doctor.orphan_segment_pdf_check(args(doctor))

    assert result.status == "warn"
    assert "1 raw PDF original" in result.detail
    assert "without a readable document transcript" in result.detail
    assert "chronicle/20250101/import.document/120000_0/original.pdf" in result.detail
    assert result.fix is not None
    assert (
        "add a readable *_transcript.md beside the PDF original, then re-run journal doctor"
        in result.fix
    )


def test_orphan_segment_pdf_check_warns_for_uppercase_pdf(
    doctor, monkeypatch, tmp_path
):
    monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(tmp_path), "source"))
    segment = tmp_path / "chronicle" / "20250101" / "import.document" / "120000_0"
    segment.mkdir(parents=True)
    (segment / "ORIGINAL.PDF").write_bytes(b"%PDF-1.4 synthetic")

    result = doctor.orphan_segment_pdf_check(args(doctor))

    assert result.status == "warn"
    assert "chronicle/20250101/import.document/120000_0/ORIGINAL.PDF" in result.detail


def test_orphan_segment_pdf_check_ignores_pdf_with_transcript(
    doctor, monkeypatch, tmp_path
):
    monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(tmp_path), "source"))
    segment = tmp_path / "chronicle" / "20250101" / "import.document" / "120000_0"
    segment.mkdir(parents=True)
    (segment / "original.pdf").write_bytes(b"%PDF-1.4 synthetic")
    (segment / "document_transcript.md").write_text("ready", encoding="utf-8")

    result = doctor.orphan_segment_pdf_check(args(doctor))

    assert result.status == "ok"
    assert result.detail == "all raw PDF originals have readable document transcripts"


def test_service_running_crash_loop_fails(doctor, monkeypatch):
    monkeypatch.setattr(doctor, "service_is_installed", lambda: True)
    monkeypatch.setattr(
        doctor,
        "fetch_supervisor_status",
        lambda: {"crashed": [{"name": "cortex", "restart_attempts": 3}]},
    )

    result = doctor.service_running_check(args(doctor))

    assert result.status == "fail"
    assert result.detail == "crash-loop: cortex (3 restart attempts)"
    assert result.fix == "run journal service logs"


def test_service_identity_not_installed_skips(doctor, monkeypatch):
    monkeypatch.setattr(
        doctor,
        "check_service_target_identity",
        lambda: SimpleNamespace(
            installed=False,
            target="",
            matches_current_install=False,
            detail="service not installed",
        ),
    )

    result = doctor.service_identity_check(args(doctor))

    assert result.status == "skip"
    assert result.detail == "no local journal service"


def test_service_identity_malformed_fails(doctor, monkeypatch):
    monkeypatch.setattr(
        doctor,
        "check_service_target_identity",
        lambda: SimpleNamespace(
            installed=True,
            target="",
            matches_current_install=False,
            detail="service config invalid",
        ),
    )

    result = doctor.service_identity_check(args(doctor))

    assert result.status == "fail"
    assert result.detail == "service config invalid"
    assert result.fix == "run journal setup to reinstall the service"


def test_service_identity_mismatch_fails_with_force_fix(doctor, monkeypatch):
    monkeypatch.setattr(
        doctor,
        "check_service_target_identity",
        lambda: SimpleNamespace(
            installed=True,
            target="/tmp/old/journal",
            matches_current_install=False,
            detail="service target mismatch",
        ),
    )

    result = doctor.service_identity_check(args(doctor))

    assert result.status == "fail"
    assert "journal setup --force" in (result.fix or "")


def test_service_identity_match_ok(doctor, monkeypatch):
    monkeypatch.setattr(
        doctor,
        "check_service_target_identity",
        lambda: SimpleNamespace(
            installed=True,
            target="/tmp/current/journal",
            matches_current_install=True,
            detail="service target matches current install",
        ),
    )

    result = doctor.service_identity_check(args(doctor))

    assert result.status == "ok"
    assert result.detail == "service target matches current install"


SUPERVISOR_CONFLICT_GRID = [
    ("present", "loaded", "running", "fail", ()),
    ("present", "loaded", "absent", "ok", ()),
    ("present", "loaded", "unknown", "warn", ("app",)),
    ("present", "unloaded", "running", "fail", ()),
    ("present", "unloaded", "absent", "ok", ()),
    ("present", "unloaded", "unknown", "warn", ("app",)),
    ("present", "unknown", "running", "fail", ()),
    ("present", "unknown", "absent", "warn", ("label",)),
    ("present", "unknown", "unknown", "warn", ("label", "app")),
    ("malformed", "loaded", "running", "fail", ()),
    ("malformed", "loaded", "absent", "ok", ()),
    ("malformed", "loaded", "unknown", "warn", ("app",)),
    ("malformed", "unloaded", "running", "fail", ()),
    ("malformed", "unloaded", "absent", "ok", ()),
    ("malformed", "unloaded", "unknown", "warn", ("app",)),
    ("malformed", "unknown", "running", "fail", ()),
    ("malformed", "unknown", "absent", "warn", ("label",)),
    ("malformed", "unknown", "unknown", "warn", ("label", "app")),
    ("absent", "loaded", "running", "fail", ()),
    ("absent", "loaded", "absent", "ok", ()),
    ("absent", "loaded", "unknown", "warn", ("app",)),
    ("absent", "unloaded", "running", "ok", ()),
    ("absent", "unloaded", "absent", "ok", ()),
    ("absent", "unloaded", "unknown", "warn", ("app",)),
    ("absent", "unknown", "running", "warn", ("label",)),
    ("absent", "unknown", "absent", "warn", ("label",)),
    ("absent", "unknown", "unknown", "warn", ("label", "app")),
    ("unknown", "loaded", "running", "fail", ()),
    ("unknown", "loaded", "absent", "warn", ("plist",)),
    ("unknown", "loaded", "unknown", "warn", ("plist", "app")),
    ("unknown", "unloaded", "running", "warn", ("plist",)),
    ("unknown", "unloaded", "absent", "warn", ("plist",)),
    ("unknown", "unloaded", "unknown", "warn", ("plist", "app")),
    ("unknown", "unknown", "running", "warn", ("plist", "label")),
    ("unknown", "unknown", "absent", "warn", ("plist", "label")),
    ("unknown", "unknown", "unknown", "warn", ("plist", "label", "app")),
]


@pytest.mark.parametrize(
    ("plist_state", "label_state", "app_state", "expected_status", "unknown_axes"),
    SUPERVISOR_CONFLICT_GRID,
)
def test_supervisor_conflict_grid(
    doctor,
    monkeypatch,
    plist_state,
    label_state,
    app_state,
    expected_status,
    unknown_axes,
):
    plist_path = "/tmp/Library/LaunchAgents/org.solpbc.solstone.plist"
    label_pid = 12345 if label_state == "loaded" else None
    app_pid = 2468 if app_state == "running" else None
    app_executable = (
        "/private/var/folders/xx/AppTranslocation/ABC/d/"
        "journal.app/Contents/MacOS/journal"
        if app_pid is not None
        else None
    )
    evidence = service.SupervisorConflictEvidence(
        plist_path=plist_path,
        plist_state=plist_state,
        label_state=label_state,
        label_pid=label_pid,
        app_state=app_state,
        app_pid=app_pid,
        app_executable=app_executable,
        detail=(
            f"plist_path={plist_path} plist_state={plist_state}; "
            f"launchd_label={label_state} pid={label_pid or 'none'}; "
            f"journal_app={app_state} pid={app_pid or 'none'} "
            f"exe={app_executable or 'none'}"
        ),
        foreign=service.ForeignLauncherScan(matches=(), incomplete_paths=()),
    )
    monkeypatch.setattr(doctor, "inspect_supervisor_conflict", lambda: evidence)

    result = doctor.supervisor_conflict_check(args(doctor))

    assert result.status == expected_status
    if expected_status == "fail":
        assert result.fix == "journal service uninstall"
        assert "macOS supervisor conflict" in result.detail
    elif expected_status == "warn":
        assert result.fix is None
        for axis in unknown_axes:
            assert axis in result.detail
    else:
        assert result.fix is None
        assert "no macOS supervisor conflict" in result.detail


def test_supervisor_conflict_grid_counts():
    counts = {
        status: sum(
            1
            for *_states, row_status, _axes in SUPERVISOR_CONFLICT_GRID
            if row_status == status
        )
        for status in {"fail", "ok", "warn"}
    }

    assert counts == {"fail": 8, "ok": 7, "warn": 21}


def test_foreign_launcher_remedy_renders_single_match_example(doctor):
    match = service.ForeignLauncherMatch(
        label="com.example.solstone-watchdog",
        plist_path="/Users/.../com.example.solstone-watchdog.plist",
        service_target="gui/501/com.example.solstone-watchdog",
    )

    assert doctor._foreign_launcher_remedy((match,)) == (
        "remove foreign launchers targeting /Applications/solstone.app: "
        "launchctl bootout gui/501/com.example.solstone-watchdog; "
        "rm /Users/.../com.example.solstone-watchdog.plist; "
        "then rerun journal doctor"
    )


def test_foreign_launcher_incident_regression_blocks_without_legacy_or_app(
    doctor, monkeypatch, home_root
):
    """Incident regression: a hand-written user LaunchAgent with KeepAlive=True
    relaunches /Applications/solstone.app and must block journal doctor even
    when the legacy service is absent and journal.app is not currently running.
    This is the standing reality proxy for macOS-only behavior that cannot be
    validated on this Linux host.
    """

    plist_path = write_foreign_plist(
        home_root,
        "com.example.solstone-watchdog.plist",
        label="com.example.solstone-watchdog",
        program_arguments=["/usr/bin/open", "-a", "/Applications/solstone.app"],
    )
    force_darwin_supervisor_reader(
        doctor,
        monkeypatch,
        launchctl=subprocess.CompletedProcess(
            args=["launchctl"],
            returncode=113,
            stdout="",
            stderr="Bootstrap lookup failed: service not found",
        ),
    )
    before = tree_snapshot(home_root)

    results = doctor.run_checks(
        args(doctor),
        checks=[(doctor.SUPERVISOR_CONFLICT_CHECK, doctor.supervisor_conflict_check)],
    )

    assert tree_snapshot(home_root) == before
    result = results[0]
    assert result.status == "fail"
    assert "1 foreign KeepAlive launcher targets /Applications/solstone.app" in (
        result.detail
    )
    assert "foreign_launchers=1" in result.detail
    assert f"com.example.solstone-watchdog@{plist_path}" in result.detail
    assert result.fix == (
        "remove foreign launchers targeting /Applications/solstone.app: "
        "launchctl bootout gui/501/com.example.solstone-watchdog; "
        f"rm {plist_path}; then rerun journal doctor"
    )


def test_foreign_launcher_fix_shell_quotes_single_line_adversarial_label(
    doctor, monkeypatch, home_root
):
    label = "com.example; rm -rf ~ `tick` $(echo hi) space 'quote"
    write_foreign_plist(
        home_root,
        "shell-safe.plist",
        label=label,
        program_arguments=["/usr/bin/open", "-a", "/Applications/solstone.app"],
    )
    force_darwin_supervisor_reader(
        doctor,
        monkeypatch,
        launchctl=subprocess.CompletedProcess(
            args=["launchctl"],
            returncode=113,
            stdout="",
            stderr="service not found",
        ),
    )

    result = doctor.supervisor_conflict_check(args(doctor))

    assert result.status == "fail"
    assert result.fix is not None
    assert "\n" not in result.fix
    target = f"gui/501/{label}"
    quoted_target = shlex.quote(target)
    assert result.fix.count(quoted_target) == 1
    assert shlex.split(quoted_target) == [target]


@pytest.mark.parametrize(
    "unsafe_char",
    ["\u0085", "\u2028", "\u2029", "\u202e"],
)
def test_unsafe_unicode_foreign_label_warns_without_remedy(
    doctor, monkeypatch, home_root, unsafe_char
):
    write_foreign_plist(
        home_root,
        f"unsafe-label-{ord(unsafe_char):x}.plist",
        label=f"com.example.bad{unsafe_char}label",
        program_arguments=["/usr/bin/open", "-a", "/Applications/solstone.app"],
    )
    force_darwin_supervisor_reader(
        doctor,
        monkeypatch,
        launchctl=subprocess.CompletedProcess(
            args=["launchctl"],
            returncode=113,
            stdout="",
            stderr="service not found",
        ),
    )

    result = doctor.supervisor_conflict_check(args(doctor))

    assert result.status == "warn"
    assert result.fix is None
    assert "foreign launcher scan incomplete" in result.detail
    assert unsafe_char not in result.detail


def test_surrogateescape_foreign_filename_warns_without_remedy(
    doctor, monkeypatch, home_root
):
    if not sys.platform.startswith("linux"):
        pytest.skip("surrogateescape filename creation is only exercised on Linux")
    write_foreign_plist(
        home_root,
        os.fsdecode(b"bad\xffname.plist"),
        label="com.example.solstone-watchdog",
        program_arguments=["/usr/bin/open", "-a", "/Applications/solstone.app"],
    )
    force_darwin_supervisor_reader(
        doctor,
        monkeypatch,
        launchctl=subprocess.CompletedProcess(
            args=["launchctl"],
            returncode=113,
            stdout="",
            stderr="service not found",
        ),
    )

    result = doctor.supervisor_conflict_check(args(doctor))

    assert result.status == "warn"
    assert result.fix is None
    assert "\udcff" not in result.detail
    assert "bad?name.plist" in result.detail


def test_printable_unicode_foreign_label_with_shell_metacharacters_matches(
    doctor, monkeypatch, home_root
):
    label = "com.example.cafe-\u00e9.\u6771\u4eac; $(echo hi) space 'quote"
    write_foreign_plist(
        home_root,
        "printable-unicode-shell-safe.plist",
        label=label,
        program_arguments=["/usr/bin/open", "-a", "/Applications/solstone.app"],
    )
    force_darwin_supervisor_reader(
        doctor,
        monkeypatch,
        launchctl=subprocess.CompletedProcess(
            args=["launchctl"],
            returncode=113,
            stdout="",
            stderr="service not found",
        ),
    )

    result = doctor.supervisor_conflict_check(args(doctor))

    assert result.status == "fail"
    assert result.fix is not None
    assert "foreign_launchers=1" in result.detail
    target = f"gui/501/{label}"
    quoted_target = shlex.quote(target)
    assert result.fix.count(quoted_target) == 1
    assert shlex.split(quoted_target) == [target]


def test_printable_unicode_foreign_plist_path_with_shell_metacharacters_matches(
    doctor, monkeypatch, home_root
):
    plist_path = write_foreign_plist(
        home_root,
        "cafeé 東京; $(echo hi) it's.plist",
        label="com.example.solstone-watchdog",
        program_arguments=["/usr/bin/open", "-a", "/Applications/solstone.app"],
    )
    force_darwin_supervisor_reader(
        doctor,
        monkeypatch,
        launchctl=subprocess.CompletedProcess(
            args=["launchctl"], returncode=113, stdout="", stderr="service not found"
        ),
    )

    result = doctor.supervisor_conflict_check(args(doctor))

    assert result.status == "fail"
    assert result.fix is not None
    assert "foreign_launchers=1" in result.detail
    assert str(plist_path) in result.detail
    assert "foreign_incomplete" not in result.detail
    lex = shlex.shlex(result.fix, posix=True, punctuation_chars=True)
    lex.whitespace_split = True
    tokens = list(lex)
    assert tokens.count("rm") == 1
    assert tokens[tokens.index("rm") + 1] == str(plist_path)


def test_control_character_foreign_label_warns_without_remedy(
    doctor, monkeypatch, home_root
):
    plist_path = write_foreign_plist(
        home_root,
        "control-label.plist",
        label="com.example.bad\nlabel",
        program_arguments=["/usr/bin/open", "-a", "/Applications/solstone.app"],
    )
    force_darwin_supervisor_reader(
        doctor,
        monkeypatch,
        launchctl=subprocess.CompletedProcess(
            args=["launchctl"],
            returncode=113,
            stdout="",
            stderr="service not found",
        ),
    )

    evidence = service.inspect_supervisor_conflict()
    result = doctor.supervisor_conflict_check(args(doctor))

    assert evidence.foreign.matches == ()
    assert evidence.foreign.incomplete_paths == (str(plist_path),)
    assert result.status == "warn"
    assert result.fix is None
    assert "foreign_incomplete=" in result.detail
    assert "\n" not in result.detail


def test_combined_exact_and_foreign_conflict_fix_contains_both_actions(
    doctor, monkeypatch, home_root
):
    write_legacy_plist(home_root, ["/tmp/legacy-journal", "start"])
    foreign_path = write_foreign_plist(
        home_root,
        "com.example.foreign.plist",
        label="com.example.foreign",
        program_arguments=["/usr/bin/open", "-a", "/Applications/solstone.app"],
    )
    force_darwin_supervisor_reader(
        doctor,
        monkeypatch,
        launchctl=subprocess.CompletedProcess(
            args=["launchctl"],
            returncode=0,
            stdout="\tpid = 12345\n",
            stderr="",
        ),
        processes=[
            _FakeProcess(
                pid=4242,
                exe="/Applications/journal.app/Contents/MacOS/journal",
            )
        ],
    )

    result = doctor.supervisor_conflict_check(args(doctor))

    assert result.status == "fail"
    assert result.fix == (
        "journal service uninstall; "
        "remove foreign launchers targeting /Applications/solstone.app: "
        "launchctl bootout gui/501/com.example.foreign; "
        f"rm {foreign_path}; then rerun journal doctor"
    )


def test_foreign_match_with_incomplete_scan_fails_with_match_remedy_only(
    doctor, monkeypatch, home_root
):
    foreign_path = write_foreign_plist(
        home_root,
        "com.example.foreign.plist",
        label="com.example.foreign",
        program_arguments=["/usr/bin/open", "-a", "/Applications/solstone.app"],
    )
    bad_path = home_root / "Library" / "LaunchAgents" / "broken.plist"
    bad_path.write_text("not a plist", encoding="utf-8")
    force_darwin_supervisor_reader(
        doctor,
        monkeypatch,
        launchctl=subprocess.CompletedProcess(
            args=["launchctl"],
            returncode=113,
            stdout="",
            stderr="service not found",
        ),
    )

    result = doctor.supervisor_conflict_check(args(doctor))

    assert result.status == "fail"
    assert result.fix is not None
    assert str(foreign_path) in result.fix
    assert str(bad_path) not in result.fix
    assert "foreign launcher scan incomplete" in result.detail
    assert f"foreign_incomplete={bad_path}" in result.detail


def test_incomplete_foreign_scan_warns_without_remedy(doctor, monkeypatch, home_root):
    bad_path = home_root / "Library" / "LaunchAgents" / "broken.plist"
    bad_path.parent.mkdir(parents=True)
    bad_path.write_text("not a plist", encoding="utf-8")
    force_darwin_supervisor_reader(
        doctor,
        monkeypatch,
        launchctl=subprocess.CompletedProcess(
            args=["launchctl"],
            returncode=113,
            stdout="",
            stderr="service not found",
        ),
    )

    result = doctor.supervisor_conflict_check(args(doctor))

    assert result.status == "warn"
    assert result.fix is None
    assert "foreign launcher scan incomplete" in result.detail
    assert f"foreign_incomplete={bad_path}" in result.detail


def test_conflict_report_suppresses_orthogonal_upgrade_fix(
    doctor, monkeypatch, home_root
):
    plist_path = write_legacy_plist(home_root, ["/tmp/legacy-journal", "start"])
    app_executable = (
        "/private/var/folders/xx/AppTranslocation/ABC/d/"
        "journal.app/Contents/MacOS/journal"
    )
    force_darwin_supervisor_reader(
        doctor,
        monkeypatch,
        launchctl=subprocess.CompletedProcess(
            args=["launchctl"],
            returncode=0,
            stdout="\tpid = 12345\n",
            stderr="",
        ),
        processes=[_FakeProcess(pid=4242, exe=app_executable)],
    )
    monkeypatch.setattr(
        doctor,
        "_installed_packaging_versions",
        lambda: {
            "solstone": "2.0.0",
            "solstone-journal": "1.0.0",
            "solstone-journal-cuda": None,
            "solstone-journal-host": None,
        },
    )
    before = tree_snapshot(home_root)

    results = doctor.run_checks(
        args(doctor),
        checks=[
            (doctor.SUPERVISOR_CONFLICT_CHECK, doctor.supervisor_conflict_check),
            (
                doctor.JOURNAL_PACKAGE_VERSION_CHECK,
                doctor.journal_package_version_check,
            ),
        ],
    )

    assert tree_snapshot(home_root) == before
    by_name = {result.name: result for result in results}
    assert by_name["supervisor_conflict"].status == "fail"
    assert by_name["supervisor_conflict"].fix == "journal service uninstall"
    assert str(plist_path) in by_name["supervisor_conflict"].detail
    assert "plist_state=present" in by_name["supervisor_conflict"].detail
    assert "launchd_label=loaded pid=12345" in by_name["supervisor_conflict"].detail
    assert (
        f"journal_app=running pid=4242 exe={app_executable}"
        in by_name["supervisor_conflict"].detail
    )
    drift = by_name["journal_package_version"]
    assert drift.status == "fail"
    assert "journal package version mismatch" in drift.detail
    assert "solstone 2.0.0" in drift.detail
    assert drift.fix == doctor._SUPERVISOR_CONFLICT_FIX_POINTER
    fixes = {result.fix for result in results if result.fix}
    assert fixes == {
        "journal service uninstall",
        doctor._SUPERVISOR_CONFLICT_FIX_POINTER,
    }
    fix_text = "\n".join(fixes)
    assert "pip install --upgrade" not in fix_text
    assert "journal setup" not in fix_text
    assert "journal service install" not in fix_text
    assert "journal service start" not in fix_text
    assert "journal service restart" not in fix_text


def test_conflict_full_journal_report_suppresses_every_other_fix(
    doctor, monkeypatch, tmp_path, home_root
):
    from solstone.think import pipeline_health

    journal = tmp_path / "journal"
    journal.mkdir()
    write_legacy_plist(home_root, ["/tmp/missing-journal", "start"])
    force_darwin_supervisor_reader(
        doctor,
        monkeypatch,
        launchctl=subprocess.CompletedProcess(
            args=["launchctl"],
            returncode=0,
            stdout="\tpid = 12345\n",
            stderr="",
        ),
        processes=[
            _FakeProcess(
                pid=4242,
                exe="/Applications/journal.app/Contents/MacOS/journal",
            )
        ],
    )
    monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(journal), "test"))
    monkeypatch.setattr(doctor, "_host_module_present", lambda _module: True)
    monkeypatch.setattr(doctor, "service_is_installed", lambda: True)
    monkeypatch.setattr(
        doctor,
        "check_service_target_identity",
        lambda: SimpleNamespace(
            installed=True,
            target="/tmp/current/journal",
            matches_current_install=True,
            detail="service target matches current install",
        ),
    )
    monkeypatch.setattr(
        doctor,
        "fetch_supervisor_status",
        lambda: {"crashed": [], "tasks": []},
    )
    monkeypatch.setattr(
        doctor,
        "_installed_packaging_versions",
        lambda: {
            "solstone": "2.0.0",
            "solstone-journal": "1.0.0",
            "solstone-journal-cuda": None,
            "solstone-journal-host": None,
        },
    )
    monkeypatch.setattr(
        doctor,
        "check_journal_sync",
        lambda: SimpleNamespace(is_conflict=False),
    )
    monkeypatch.setattr(doctor, "format_doctor_report", lambda _result: "synced")
    monkeypatch.setattr(
        pipeline_health,
        "read_backlog_view",
        lambda: SimpleNamespace(
            days=[],
            errors=[],
            pending_days=0,
            stuck_days=0,
            oldest_pending_day=None,
        ),
    )
    monkeypatch.setattr("solstone.apps.observer.utils.list_observers", lambda: [])

    results = doctor.run_checks(args(doctor), checks=doctor.JOURNAL_CHECKS)

    by_name = {result.name: result for result in results}
    assert by_name["supervisor_conflict"].status == "fail"
    for result in results:
        if result.name == "supervisor_conflict":
            continue
        assert result.fix in {None, doctor._SUPERVISOR_CONFLICT_FIX_POINTER}
    suppressed = [
        result.name
        for result in results
        if result.name != "supervisor_conflict"
        and result.fix == doctor._SUPERVISOR_CONFLICT_FIX_POINTER
    ]
    assert suppressed, "no fix was suppressed; the test proves nothing"

    fix_text = "\n".join(result.fix or "" for result in results)
    for token in doctor._UNSAFE_SERVICE_ACTIONS:
        assert token not in fix_text
    assert "rm " not in fix_text
    assert "pip install --upgrade" not in fix_text


def test_unknown_topology_suppresses_only_service_lifecycle_fixes(
    doctor, monkeypatch, home_root
):
    write_legacy_plist(home_root, ["/tmp/missing-journal", "start"])
    force_darwin_supervisor_reader(
        doctor,
        monkeypatch,
        launchctl=subprocess.CompletedProcess(
            args=["launchctl"],
            returncode=5,
            stdout="",
            stderr="opaque launchctl failure",
        ),
    )
    monkeypatch.setattr(
        service.psutil,
        "process_iter",
        lambda: (_ for _ in ()).throw(service.psutil.Error("boom")),
    )
    monkeypatch.setattr(
        doctor,
        "_installed_packaging_versions",
        lambda: {
            "solstone": "2.0.0",
            "solstone-journal": "1.0.0",
            "solstone-journal-cuda": None,
            "solstone-journal-host": None,
        },
    )
    before = tree_snapshot(home_root)

    results = doctor.run_checks(
        args(doctor),
        checks=[
            (doctor.SUPERVISOR_CONFLICT_CHECK, doctor.supervisor_conflict_check),
            (doctor.LAUNCHD_STALE_PLIST_CHECK, doctor.launchd_stale_plist_check),
            (
                doctor.JOURNAL_PACKAGE_VERSION_CHECK,
                doctor.journal_package_version_check,
            ),
        ],
    )

    assert tree_snapshot(home_root) == before
    by_name = {result.name: result for result in results}
    assert by_name["supervisor_conflict"].status == "warn"
    assert "unknown axis(es): label, app" in by_name["supervisor_conflict"].detail
    stale = by_name["launchd_stale_plist"]
    assert stale.status == "fail"
    assert "plist points to missing executable" in stale.detail
    assert stale.fix == doctor._SUPERVISOR_TOPOLOGY_WARN_POINTER
    drift = by_name["journal_package_version"]
    assert drift.status == "fail"
    assert "journal package version mismatch" in drift.detail
    assert "pip install --upgrade solstone-journal" in (drift.fix or "")

    fix_text = "\n".join(result.fix or "" for result in results)
    assert "journal service uninstall" not in fix_text
    for token in doctor._UNSAFE_SERVICE_ACTIONS:
        assert token not in fix_text


def test_supervisor_conflict_execution_error_uses_unknown_topology_policy(doctor):
    conflict_check = doctor.Check(
        doctor.SUPERVISOR_CONFLICT_CHECK.name,
        doctor.SUPERVISOR_CONFLICT_CHECK.severity,
        ("linux", "darwin"),
    )
    unsafe_check = doctor.Check("unsafe_fix", "blocker", ("linux", "darwin"))
    unrelated_check = doctor.Check("unrelated_fix", "blocker", ("linux", "darwin"))
    unrelated_fix = "pip install --upgrade solstone-journal"

    def raising_conflict(_args):
        raise RuntimeError("launchctl unavailable")

    def unsafe(_args):
        return doctor.make_result(
            unsafe_check,
            "fail",
            "unsafe lifecycle fix",
            "journal service restart",
        )

    def unrelated(_args):
        return doctor.make_result(
            unrelated_check,
            "fail",
            "unrelated fix",
            unrelated_fix,
        )

    results = doctor.run_checks(
        args(doctor),
        checks=[
            (conflict_check, raising_conflict),
            (unsafe_check, unsafe),
            (unrelated_check, unrelated),
        ],
    )

    by_name = {result.name: result for result in results}
    conflict = by_name["supervisor_conflict"]
    assert conflict.status == "fail"
    assert doctor.has_execution_error(conflict)
    assert conflict.fix is None
    assert by_name["unsafe_fix"].fix == doctor._SUPERVISOR_TOPOLOGY_WARN_POINTER
    assert by_name["unrelated_fix"].fix == unrelated_fix
    assert all("None" not in result.fix for result in results if result.fix)


def test_unsafe_service_action_match_does_not_match_uninstall(doctor):
    assert not doctor._fix_mentions_unsafe_service_action("journal service uninstall")
    assert doctor._fix_mentions_unsafe_service_action("journal service install")
    assert doctor._fix_mentions_unsafe_service_action("journal service start")
    assert doctor._fix_mentions_unsafe_service_action("journal service restart")
    assert doctor._fix_mentions_unsafe_service_action("journal setup")


def test_role_skip_without_local_journal(doctor, monkeypatch, tmp_path, home_root):
    journal = tmp_path / "missing-journal"
    monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(journal), "env"))
    monkeypatch.setattr(doctor, "_host_module_present", lambda _module: True)
    monkeypatch.setattr(doctor, "service_is_installed", lambda: False)
    monkeypatch.setattr(
        doctor,
        "check_journal_sync",
        lambda: pytest.fail("journal_sync should be role-skipped"),
    )
    patch_alias_absent(doctor, monkeypatch)
    monkeypatch.setattr(doctor, "ROOT", make_repo(tmp_path))

    results = doctor.run_checks(args(doctor), checks=doctor.JOURNAL_CHECKS)
    by_name = {result.name: result for result in results}

    assert by_name["journal_dir_writable"].status == "skip"
    assert by_name["journal_sync"].status == "skip"
    assert by_name["service_identity"].status == "skip"
    assert by_name["service_running"].status == "skip"
    assert by_name["skill_state"].status == "skip"
    assert by_name["host_dependencies"].status == "ok"
    assert by_name["disk_space"].status in {"ok", "warn"}
    assert by_name["config_dir_readable"].status == "ok"
    assert by_name["feature:pdf-import"].status in {"ok", "warn"}
    assert by_name["feature:pdf-export"].status in {"ok", "warn"}


def test_skill_state_no_local_journal_skips(doctor, monkeypatch, tmp_path):
    journal = tmp_path / "missing-journal"
    monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(journal), "env"))
    monkeypatch.setattr(doctor, "is_packaged_install", lambda: False)

    result = doctor.skill_state_check(args(doctor))

    assert result.status == "skip"
    assert result.detail == "no local journal"


def test_skill_state_current_router_links_ok(doctor, monkeypatch, tmp_path):
    journal = tmp_path / "journal"
    journal.mkdir()
    install_router_skill_links(doctor, journal)
    monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(journal), "env"))
    monkeypatch.setattr(doctor, "is_packaged_install", lambda: False)

    result = doctor.skill_state_check(args(doctor))

    assert result.status == "ok"
    assert result.detail == "router skills sol, journal are installed and current"


def test_skill_state_warns_for_stale_and_missing_links_without_writing(
    doctor, monkeypatch, tmp_path
):
    journal = tmp_path / "journal"
    skills_dir = journal / ".claude" / "skills"
    skills_dir.mkdir(parents=True)
    sources = {
        source.name: source for source in doctor._discover_project_sources(doctor.ROOT)
    }
    (skills_dir / "journal").symlink_to(os.path.relpath(sources["journal"], skills_dir))
    (skills_dir / "entities").symlink_to(
        "../../../solstone/apps/entities/talent/entities"
    )
    before = tree_snapshot(journal)
    monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(journal), "env"))
    monkeypatch.setattr(doctor, "is_packaged_install", lambda: False)

    result = doctor.skill_state_check(args(doctor))

    assert result.status == "warn"
    assert f"missing router sol at {skills_dir / 'sol'}" in result.detail
    assert f"stale skill link entities at {skills_dir / 'entities'}" in result.detail
    assert result.fix is not None
    assert "journal setup" in result.fix
    assert f"sol skills install --project {journal} --agent all" in result.fix
    assert tree_snapshot(journal) == before


class TestJournalAlias:
    @pytest.fixture(autouse=True)
    def isolated_legacy_backups(self, doctor, monkeypatch, tmp_path):
        backup_dir = tmp_path / "legacy-backups"
        backup_dir.mkdir()
        monkeypatch.setattr(install_guard, "_legacy_backup_dir", lambda: backup_dir)
        self.backup_dir = backup_dir

    def test_journal_only_absent_ok_even_if_sol_is_foreign(
        self, doctor, monkeypatch, home_root, tmp_path
    ):
        patch_alias_absent(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        sol_target = make_existing_target(tmp_path / "other" / ".venv" / "bin" / "sol")
        make_alias(home_root, "sol", sol_target)
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary="journal")

        assert result.status == "ok"

    def test_journal_uv_tool_reports_only_journal(
        self, doctor, monkeypatch, home_root, tmp_path
    ):
        patch_alias_absent(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        target = make_existing_target(
            home_root
            / ".local"
            / "share"
            / "uv"
            / "tools"
            / "solstone"
            / "bin"
            / "journal"
        )
        alias = make_alias(home_root, "journal", target)
        original_target = alias.readlink()
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary="journal")

        assert result.status == "warn"
        assert "uv-tool" in result.detail
        assert result.fix is not None
        assert "journal setup" in result.fix
        assert alias.is_symlink()
        assert alias.readlink() == original_target
        assert not (home_root / ".local" / "bin" / "sol").exists()
        assert list(self.backup_dir.glob("*.old-symlink-*")) == []


class TestLaunchdStalePlist:
    def test_skip_on_linux(self, doctor, monkeypatch):
        monkeypatch.setattr(doctor, "platform_tag", lambda: "linux")
        result = doctor.launchd_stale_plist_check(args(doctor))
        assert result.status == "skip"

    def test_skip_when_absent(self, doctor, monkeypatch, home_root):
        monkeypatch.setattr(doctor, "platform_tag", lambda: "darwin")
        result = doctor.launchd_stale_plist_check(args(doctor))
        assert result.status == "skip"

    def test_fail_when_target_missing(self, doctor, monkeypatch, home_root):
        monkeypatch.setattr(doctor, "platform_tag", lambda: "darwin")
        plist_path = (
            home_root / "Library" / "LaunchAgents" / "org.solpbc.solstone.plist"
        )
        plist_path.parent.mkdir(parents=True)
        plist_path.write_bytes(
            plistlib.dumps({"ProgramArguments": ["/tmp/missing-sol"]})
        )
        result = doctor.launchd_stale_plist_check(args(doctor))
        assert result.status == "fail"
        assert result.fix == (
            "run journal service uninstall, then run journal service install "
            "separately to reinstall a headless background service"
        )
        assert "journal setup" not in (result.fix or "")
        assert "&&" not in (result.fix or "")

    def test_ok_when_target_exists(self, doctor, monkeypatch, home_root, tmp_path):
        monkeypatch.setattr(doctor, "platform_tag", lambda: "darwin")
        exe = tmp_path / "sol"
        exe.write_text("", encoding="utf-8")
        plist_path = (
            home_root / "Library" / "LaunchAgents" / "org.solpbc.solstone.plist"
        )
        plist_path.parent.mkdir(parents=True)
        plist_path.write_bytes(plistlib.dumps({"ProgramArguments": [str(exe)]}))
        result = doctor.launchd_stale_plist_check(args(doctor))
        assert result.status == "ok"
