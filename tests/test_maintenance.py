# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import importlib
import json
import logging
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.think import maintenance, maintenance_cli, schedule_config, scheduler
from solstone.think.maintenance import MaintenanceDescriptorError, MaintenanceRoutine


def _use_journal(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    journal = tmp_path / "journal"
    (journal / "config").mkdir(parents=True)
    (journal / "health").mkdir(parents=True)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    return journal


def _use_apps(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    apps_dir = tmp_path / "external-apps"
    apps_dir.mkdir()
    monkeypatch.setattr(maintenance, "APPS_DIR", apps_dir)
    return apps_dir


def _write_app(apps_dir: Path, app: str, body: str) -> Path:
    app_dir = apps_dir / app
    app_dir.mkdir(parents=True, exist_ok=True)
    path = app_dir / "maintenance.py"
    path.write_text(body, encoding="utf-8")
    return path


def _routine_module(
    *,
    name: str = "cleanup",
    every: str = "daily",
    max_runtime: str | None = "5m",
    exit_expr: str = "0",
) -> str:
    max_runtime_value = repr(max_runtime)
    return f"""
from solstone.think.maintenance import MaintenanceRoutine


def run(args):
    return {exit_expr}


ROUTINES = [
    MaintenanceRoutine(
        name={name!r},
        description="test routine",
        every={every!r},
        run=run,
        max_runtime={max_runtime_value},
    )
]
"""


def _expected_entry(routine_id: str, every: str = "daily") -> dict:
    return {
        "cmd": ["journal", "maintenance", "run", routine_id],
        "every": every,
        "enabled": True,
        "max_runtime": "5m",
    }


def _run_cli(monkeypatch: pytest.MonkeyPatch, argv: list[str]) -> int:
    monkeypatch.setattr(sys, "argv", ["journal maintenance", *argv])
    with pytest.raises(SystemExit) as exc_info:
        maintenance_cli.main()
    return int(exc_info.value.code)


@pytest.fixture(autouse=True)
def _speakers_analyze_installation_ready(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from tests.helpers.speakers_analyze import install_enter_generation_stub

    install_enter_generation_stub(monkeypatch, tmp_path)


@pytest.fixture(autouse=True)
def reset_scheduler_state():
    scheduler._entries = {}
    scheduler._state = {}
    scheduler._callosum = None
    scheduler._last_minute = None
    scheduler._last_hour = None
    scheduler._daily_time = None
    scheduler._last_daily_mark = None
    scheduler._weekly_day = None
    scheduler._weekly_time = None
    scheduler._last_weekly_mark = None
    yield
    scheduler._entries = {}
    scheduler._state = {}
    scheduler._callosum = None
    scheduler._last_minute = None
    scheduler._last_hour = None
    scheduler._daily_time = None
    scheduler._last_daily_mark = None
    scheduler._weekly_day = None
    scheduler._weekly_time = None
    scheduler._last_weekly_mark = None


def test_discover_routines_uses_file_path_apps_dir_seam(tmp_path, monkeypatch):
    apps_dir = _use_apps(tmp_path, monkeypatch)
    _write_app(apps_dir, "alpha", _routine_module())
    _write_app(apps_dir / "_private", "ignored", _routine_module())

    routines = maintenance.discover_routines()

    assert sorted(routines) == ["alpha:cleanup"]
    assert routines["alpha:cleanup"].every == "daily"
    assert not any(
        name.startswith("_solstone_maintenance_alpha_") for name in sys.modules
    )


def test_discover_routines_skips_import_errors_missing_and_nonlist_routines(
    tmp_path, monkeypatch, caplog
):
    apps_dir = _use_apps(tmp_path, monkeypatch)
    _write_app(apps_dir, "alpha", _routine_module())
    _write_app(apps_dir, "broken", "raise RuntimeError('boom')\n")
    _write_app(apps_dir, "missing", "VALUE = 1\n")
    _write_app(apps_dir, "nonlist", "ROUTINES = ('not', 'a', 'list')\n")

    with caplog.at_level(logging.WARNING, logger="solstone.think.maintenance"):
        routines = maintenance.discover_routines()

    assert sorted(routines) == ["alpha:cleanup"]
    assert "Failed to load maintenance routines from app 'broken': boom" in caplog.text
    assert "apps/missing/maintenance.py has no ROUTINES list" in caplog.text
    assert "apps/nonlist/maintenance.py ROUTINES is not a list" in caplog.text


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        (
            _routine_module(name=""),
            "maintenance routine alpha:<missing>: invalid name: must not be empty",
        ),
        (
            _routine_module(name="bad:name"),
            "maintenance routine alpha:bad:name: invalid name: must not contain ':'",
        ),
        (
            _routine_module(name="BadName"),
            "maintenance routine alpha:BadName: invalid name:",
        ),
        (
            _routine_module(every="monthly"),
            "maintenance routine alpha:cleanup: invalid every:",
        ),
        (
            """
from solstone.think.maintenance import MaintenanceRoutine
ROUTINES = [
    MaintenanceRoutine(
        name="cleanup",
        description="test",
        every="daily",
        run=42,
        max_runtime="5m",
    )
]
""",
            "maintenance routine alpha:cleanup: invalid run: expected callable",
        ),
        (
            _routine_module(max_runtime=None).replace(
                "max_runtime=None", "max_runtime=5"
            ),
            "maintenance routine alpha:cleanup: invalid max_runtime:",
        ),
        (
            _routine_module(max_runtime="0m"),
            "maintenance routine alpha:cleanup: invalid max_runtime:",
        ),
        (
            "ROUTINES = [object()]\n",
            "maintenance routine alpha:<invalid>: invalid routine:",
        ),
    ],
)
def test_discover_routines_raises_descriptor_error_for_invalid_fields(
    tmp_path, monkeypatch, source, expected
):
    apps_dir = _use_apps(tmp_path, monkeypatch)
    _write_app(apps_dir, "alpha", source)

    with pytest.raises(MaintenanceDescriptorError) as exc_info:
        maintenance.discover_routines()

    assert expected in str(exc_info.value)


def test_expected_schedule_entry_uses_locked_contract():
    routine = MaintenanceRoutine("cleanup", "desc", "weekly", lambda _args: 0, "10m")

    assert maintenance.maintenance_schedule_name("alpha:cleanup") == (
        "maintenance:alpha:cleanup"
    )
    assert maintenance.expected_schedule_entry("alpha:cleanup", routine) == {
        "cmd": ["journal", "maintenance", "run", "alpha:cleanup"],
        "every": "weekly",
        "enabled": True,
        "max_runtime": "10m",
    }

    without_cap = MaintenanceRoutine("cleanup", "desc", "daily", lambda _args: 0)
    assert "max_runtime" not in maintenance.expected_schedule_entry(
        "alpha:cleanup", without_cap
    )


def test_schedule_recognition_uses_raw_read_and_maintenance_prefix_only(
    tmp_path, monkeypatch
):
    _use_journal(tmp_path, monkeypatch)
    routine = MaintenanceRoutine("cleanup", "desc", "daily", lambda _args: 0, "5m")
    schedule_config.set_schedule_entries(
        {
            "maintenance:alpha:cleanup": {
                **_expected_entry("alpha:cleanup"),
                "extra_field": "kept",
            }
        }
    )
    monkeypatch.setattr(
        scheduler,
        "load_config",
        lambda: pytest.fail("maintenance status used scheduler.load_config"),
    )

    statuses = maintenance.get_routine_statuses({"alpha:cleanup": routine})

    assert statuses == {"alpha:cleanup": "synced"}
    assert maintenance.is_maintenance_schedule_name("maintenance:alpha:cleanup")
    for name in (
        "sync:plaud",
        "heartbeat",
        "weekly-agents",
        "providers",
        "facet-candidates",
        "timeline-rollup-work",
        "weekly_day",
    ):
        assert not maintenance.is_maintenance_schedule_name(name)


def test_routine_statuses_report_missing_synced_disabled_and_divergent():
    routines = {
        name: MaintenanceRoutine(name.split(":", 1)[1], "desc", "daily", lambda _a: 0)
        for name in (
            "alpha:missing",
            "alpha:synced",
            "alpha:disabled",
            "alpha:divergent",
        )
    }
    raw = {
        "maintenance:alpha:synced": {
            "cmd": ["journal", "maintenance", "run", "alpha:synced"],
            "every": "daily",
            "enabled": True,
        },
        "maintenance:alpha:disabled": {
            "cmd": ["journal", "maintenance", "run", "alpha:disabled"],
            "every": "daily",
            "enabled": False,
        },
        "maintenance:alpha:divergent": {
            "cmd": ["journal", "maintenance", "run", "alpha:divergent"],
            "every": "weekly",
            "enabled": True,
        },
    }

    assert maintenance.get_routine_statuses(routines, raw) == {
        "alpha:disabled": "disabled",
        "alpha:divergent": "divergent",
        "alpha:missing": "missing",
        "alpha:synced": "synced",
    }


def test_status_max_runtime_treats_absent_and_null_as_none():
    no_cap = MaintenanceRoutine("cleanup", "desc", "daily", lambda _args: 0)
    capped = MaintenanceRoutine("cleanup", "desc", "daily", lambda _args: 0, "5m")

    base = {
        "cmd": ["journal", "maintenance", "run", "alpha:cleanup"],
        "every": "daily",
        "enabled": True,
    }

    assert maintenance.get_routine_statuses(
        {"alpha:cleanup": no_cap}, {"maintenance:alpha:cleanup": base}
    ) == {"alpha:cleanup": "synced"}
    assert maintenance.get_routine_statuses(
        {"alpha:cleanup": no_cap},
        {"maintenance:alpha:cleanup": {**base, "max_runtime": None}},
    ) == {"alpha:cleanup": "synced"}
    assert maintenance.get_routine_statuses(
        {"alpha:cleanup": capped}, {"maintenance:alpha:cleanup": base}
    ) == {"alpha:cleanup": "divergent"}
    assert maintenance.get_routine_statuses(
        {"alpha:cleanup": capped},
        {"maintenance:alpha:cleanup": {**base, "max_runtime": "5m"}},
    ) == {"alpha:cleanup": "synced"}


def test_register_maintenance_schedules_adds_missing_only_and_is_idempotent(
    tmp_path, monkeypatch
):
    _use_journal(tmp_path, monkeypatch)
    apps_dir = _use_apps(tmp_path, monkeypatch)
    _write_app(
        apps_dir,
        "alpha",
        """
from solstone.think.maintenance import MaintenanceRoutine


def run(args):
    return 0


ROUTINES = [
    MaintenanceRoutine("missing", "desc", "daily", run, "5m"),
    MaintenanceRoutine("disabled", "desc", "daily", run, "5m"),
    MaintenanceRoutine("divergent", "desc", "daily", run, "5m"),
]
""",
    )
    disabled = {**_expected_entry("alpha:disabled"), "enabled": False}
    divergent = {**_expected_entry("alpha:divergent"), "every": "weekly"}
    schedule_config.set_schedule_entries(
        {
            "maintenance:alpha:disabled": disabled,
            "maintenance:alpha:divergent": divergent,
        }
    )

    summary = maintenance.register_maintenance_schedules()
    raw = schedule_config.read_schedules()

    assert summary == {
        "added": ["alpha:missing"],
        "synced": [],
        "divergent": ["alpha:divergent"],
        "disabled": ["alpha:disabled"],
    }
    assert raw["maintenance:alpha:missing"] == _expected_entry("alpha:missing")
    assert raw["maintenance:alpha:disabled"] == disabled
    assert raw["maintenance:alpha:divergent"] == divergent

    monkeypatch.setattr(
        maintenance,
        "set_schedule_entries",
        lambda _entries: pytest.fail("idempotent sync wrote entries"),
    )

    assert maintenance.register_maintenance_schedules() == {
        "added": [],
        "synced": ["alpha:missing"],
        "divergent": ["alpha:divergent"],
        "disabled": ["alpha:disabled"],
    }


def test_cli_list_and_sync_print_statuses_and_warnings(tmp_path, monkeypatch, capsys):
    _use_journal(tmp_path, monkeypatch)
    apps_dir = _use_apps(tmp_path, monkeypatch)
    _write_app(
        apps_dir,
        "alpha",
        """
from solstone.think.maintenance import MaintenanceRoutine


def run(args):
    return 0


ROUTINES = [
    MaintenanceRoutine("missing", "desc", "daily", run, "5m"),
    MaintenanceRoutine("disabled", "desc", "daily", run, "5m"),
    MaintenanceRoutine("divergent", "desc", "daily", run, "5m"),
]
""",
    )
    schedule_config.set_schedule_entries(
        {
            "maintenance:alpha:disabled": {
                **_expected_entry("alpha:disabled"),
                "enabled": False,
            },
            "maintenance:alpha:divergent": {
                **_expected_entry("alpha:divergent"),
                "cmd": ["journal", "maintenance", "run", "alpha:other"],
            },
        }
    )
    before = schedule_config.read_schedules()

    assert _run_cli(monkeypatch, ["list"]) == 0
    list_out = capsys.readouterr().out
    assert "alpha:missing" in list_out
    assert "missing" in list_out
    assert "disabled" in list_out
    assert "divergent" in list_out
    assert schedule_config.read_schedules() == before

    assert _run_cli(monkeypatch, ["sync"]) == 0
    sync_out = capsys.readouterr().out
    assert "added: 1: alpha:missing" in sync_out
    assert "WARNING: alpha:divergent schedule is divergent" in sync_out
    assert "WARNING: alpha:disabled schedule is disabled" in sync_out


def test_cli_typer_dispatch_is_house_style():
    from typer.testing import CliRunner

    runner = CliRunner()

    help_result = runner.invoke(maintenance_cli.app, ["--help"])
    assert help_result.exit_code == 0
    assert "list" in help_result.output
    assert "sync" in help_result.output
    assert "run" in help_result.output

    bogus_result = runner.invoke(maintenance_cli.app, ["bogus"])
    assert bogus_result.exit_code == 2

    no_cmd_result = runner.invoke(maintenance_cli.app, [])
    assert no_cmd_result.exit_code == 2


def test_cli_run_forwards_remainder_args_and_exits_with_routine_code(
    tmp_path, monkeypatch
):
    _use_journal(tmp_path, monkeypatch)
    apps_dir = _use_apps(tmp_path, monkeypatch)
    args_path = tmp_path / "args.json"
    monkeypatch.setenv("ROUTINE_ARGS_FILE", str(args_path))
    _write_app(
        apps_dir,
        "alpha",
        """
import json
import os
from pathlib import Path

from solstone.think.maintenance import MaintenanceRoutine


def run(args):
    Path(os.environ["ROUTINE_ARGS_FILE"]).write_text(json.dumps(args), encoding="utf-8")
    return len(args)


ROUTINES = [MaintenanceRoutine("cleanup", "desc", "daily", run, "5m")]
""",
    )

    exit_code = _run_cli(
        monkeypatch,
        ["run", "alpha:cleanup", "--", "--flag", "value", "pos"],
    )

    assert exit_code == 3
    assert json.loads(args_path.read_text(encoding="utf-8")) == [
        "--flag",
        "value",
        "pos",
    ]


def test_cli_run_unknown_id_exits_and_points_to_list(tmp_path, monkeypatch, capsys):
    _use_journal(tmp_path, monkeypatch)
    apps_dir = _use_apps(tmp_path, monkeypatch)
    _write_app(apps_dir, "alpha", _routine_module())

    assert _run_cli(monkeypatch, ["run", "alpha:missing"]) == 1

    err = capsys.readouterr().err
    assert "alpha:missing" in err
    assert "journal maintenance list" in err


def test_cli_sync_malformed_schedules_prints_path_and_cause(
    tmp_path, monkeypatch, capsys
):
    journal = _use_journal(tmp_path, monkeypatch)
    apps_dir = _use_apps(tmp_path, monkeypatch)
    _write_app(apps_dir, "alpha", _routine_module())
    (journal / "config" / "schedules.json").write_text("{ not json", encoding="utf-8")

    assert _run_cli(monkeypatch, ["sync"]) == 1

    err = capsys.readouterr().err
    assert "config/schedules.json" in err
    assert "Expecting property name" in err


def test_register_then_scheduler_init_surfaces_runtime_cap(tmp_path, monkeypatch):
    _use_journal(tmp_path, monkeypatch)
    apps_dir = _use_apps(tmp_path, monkeypatch)
    _write_app(apps_dir, "alpha", _routine_module())

    maintenance.register_maintenance_schedules()
    scheduler.init(object())

    assert scheduler._entries["maintenance:alpha:cleanup"] == {
        "cmd": ["journal", "maintenance", "run", "alpha:cleanup"],
        "every": "daily",
        "max_runtime": 300,
    }
    assert scheduler.collect_runtime_caps() == [
        (["journal", "maintenance", "run", "alpha:cleanup"], 300)
    ]


def test_supervisor_registers_maintenance_before_scheduler_init(tmp_path, monkeypatch):
    _use_journal(tmp_path, monkeypatch)
    apps_dir = _use_apps(tmp_path, monkeypatch)
    _write_app(apps_dir, "alpha", _routine_module())
    mod = importlib.reload(importlib.import_module("solstone.think.supervisor"))
    order: list[tuple[str, bool | None]] = []

    monkeypatch.delenv("SOL_SUPERVISOR_SPAWNED", raising=False)
    monkeypatch.delenv("SOLSTONE_APP_SUPERVISED", raising=False)
    monkeypatch.setattr(
        sys,
        "argv",
        ["supervisor", "0", "--no-daily", "--no-convey", "--no-cortex", "--no-spl"],
    )
    monkeypatch.setattr(mod, "run_pending_tasks", lambda *a, **k: [])
    monkeypatch.setattr(mod, "_sweep_orphaned_sol_processes", lambda *_a, **_k: 0)
    monkeypatch.setattr(
        mod,
        "check_journal_sync",
        lambda journal: SimpleNamespace(is_conflict=False),
    )
    monkeypatch.setattr(mod, "write_self_heartbeat", lambda journal: None)
    monkeypatch.setattr(mod, "clear_self_heartbeat", lambda: None)
    monkeypatch.setattr(mod, "start_callosum_in_process", lambda: None)
    monkeypatch.setattr(mod, "stop_callosum_in_process", lambda **_kwargs: None)
    monkeypatch.setattr(mod, "is_local_provider_needed", lambda: False)
    monkeypatch.setattr(mod, "signal_ready", lambda: None)
    monkeypatch.setattr(mod, "clear_ready", lambda: None)
    monkeypatch.setattr(mod, "_sd_notify", lambda _state: None)
    monkeypatch.setattr(mod, "_stop_process", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(mod.time, "sleep", lambda _seconds: None)

    class FakeCallosumConnection:
        def __init__(self, *args, **kwargs):
            pass

        def start(self, *args, **kwargs):
            pass

        def emit(self, *args, **kwargs):
            return True

        def stop(self):
            pass

    monkeypatch.setattr(mod, "CallosumConnection", FakeCallosumConnection)
    monkeypatch.setattr(
        mod,
        "start_sense",
        lambda: SimpleNamespace(name="sense", cmd=["journal", "sense"]),
    )

    original_register = mod.maintenance.register_maintenance_schedules

    def register_spy():
        order.append(("register", None))
        return original_register()

    def init_spy(_callosum):
        raw = schedule_config.read_schedules()
        order.append(("scheduler.init", "maintenance:alpha:cleanup" in raw))

    monkeypatch.setattr(mod.maintenance, "register_maintenance_schedules", register_spy)
    monkeypatch.setattr(mod.scheduler, "init", init_spy)
    monkeypatch.setattr(
        mod.scheduler,
        "register_defaults",
        lambda: order.append(("register_defaults", None)),
    )
    monkeypatch.setattr(mod.scheduler, "collect_runtime_caps", lambda: [])
    monkeypatch.setattr(
        mod.scheduler, "catch_up", lambda: order.append(("catch_up", None))
    )

    def interrupt_supervise(coro):
        coro.close()
        raise KeyboardInterrupt

    monkeypatch.setattr(mod.asyncio, "run", interrupt_supervise)

    mod.main()

    assert order[0] == ("register", None)
    assert ("scheduler.init", True) in order
    assert order.index(("register", None)) < order.index(("scheduler.init", True))
