# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from unittest.mock import Mock

import pytest

from solstone.think import setup
from solstone.think.setup_events import (
    ERROR_CODES,
    EVENT_TYPES,
    SKIPPED_REASONS,
    STEP_NAMES,
)
from tests.test_setup import (
    expected_service_install_command,
    expected_skills_journal_command,
    expected_skills_user_command,
    expected_wrapper_command,
    patch_home,
    patch_service_health,
    patch_source_checkout,
    patch_subprocess,
    patch_tty,
)


def parse_jsonl(text: str) -> list[dict]:
    return [json.loads(line) for line in text.splitlines() if line.strip()]


def doctor_ok_lines() -> list[dict]:
    return [
        {
            "event": "doctor.started",
            "ts": "2026-05-11T00:00:00Z",
            "started_at": "2026-05-11T00:00:00Z",
            "version": "test",
        },
        {
            "event": "check.completed",
            "ts": "2026-05-11T00:00:00Z",
            "name": "python_version",
            "severity": "blocker",
            "status": "ok",
            "detail": "fine",
            "fix": "",
        },
        {
            "event": "doctor.completed",
            "ts": "2026-05-11T00:00:00Z",
            "status": "ok",
            "duration_ms": 1,
            "summary": {"total": 1, "failed": 0, "warnings": 0, "skipped": 0},
        },
    ]


def doctor_non_port_warning_lines() -> list[dict]:
    return [
        doctor_ok_lines()[0],
        {
            "event": "check.completed",
            "ts": "2026-05-11T00:00:00Z",
            "name": "local_bin_sol_reachable",
            "severity": "advisory",
            "status": "warning",
            "detail": ".local/bin/sol is not reachable",
            "fix": "uv tool install solstone",
        },
        {
            "event": "doctor.completed",
            "ts": "2026-05-11T00:00:00Z",
            "status": "warning",
            "duration_ms": 1,
            "summary": {"total": 1, "failed": 0, "warnings": 1, "skipped": 0},
        },
    ]


def run_setup_jsonl(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    args: list[str] | None = None,
    *,
    doctor_lines: list[dict] | None = None,
    command_returncode: int = 0,
    command_stderr: str = "",
) -> tuple[int, list[dict], str]:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    patch_subprocess(
        monkeypatch,
        doctor_jsonl_lines=doctor_lines or doctor_ok_lines(),
        command_returncode=command_returncode,
        command_stderr=command_stderr,
    )
    journal = tmp_path / "journal"
    argv = ["--jsonl", "--yes", "--journal", str(journal)]
    if args:
        argv.extend(args)
    rc = setup.main(argv)
    out = capsys.readouterr().out
    return rc, parse_jsonl(out), out


def test_setup_jsonl_emits_started_and_completed_ok(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    rc, events, _out = run_setup_jsonl(
        tmp_path,
        monkeypatch,
        capsys,
        ["--skip-models", "--skip-skills", "--skip-service"],
    )

    assert rc == 0
    assert events[0]["event"] == "setup.started"
    assert events[-1]["event"] == "setup.completed"
    assert events[-1]["status"] == "ok"


def test_setup_jsonl_suppresses_all_human_prints(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    rc, _events, out = run_setup_jsonl(
        tmp_path,
        monkeypatch,
        capsys,
        ["--skip-models", "--skip-skills", "--skip-service"],
    )

    assert rc == 0
    assert "journal setup:" not in out
    assert "setup plan:" not in out
    for line in out.splitlines():
        json.loads(line)


def test_setup_jsonl_step_events_paired(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    rc, events, _out = run_setup_jsonl(
        tmp_path,
        monkeypatch,
        capsys,
        ["--skip-models", "--skip-skills", "--skip-service"],
    )

    assert rc == 0
    started = [event for event in events if event["event"] == "step.started"]
    terminal = [
        event for event in events if event["event"] in {"step.completed", "step.failed"}
    ]
    assert [event["step"] for event in started] == list(STEP_NAMES)
    assert [event["step"] for event in terminal] == list(STEP_NAMES)
    assert {event["total"] for event in started} == {7}


def test_setup_jsonl_forwards_doctor_events_byte_for_byte(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    doctor_lines = doctor_ok_lines()
    expected_lines = [json.dumps(item) for item in doctor_lines]

    rc, _events, out = run_setup_jsonl(
        tmp_path,
        monkeypatch,
        capsys,
        ["--skip-models", "--skip-skills", "--skip-service"],
        doctor_lines=doctor_lines,
    )

    assert rc == 0
    output_lines = out.splitlines()
    for line in expected_lines:
        assert line in output_lines


def test_setup_jsonl_translates_doctor_advisories_to_step_warning(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    rc, events, _out = run_setup_jsonl(
        tmp_path,
        monkeypatch,
        capsys,
        ["--skip-models", "--skip-skills", "--skip-service", "--port", "5015"],
        doctor_lines=doctor_non_port_warning_lines(),
    )

    assert rc == 0
    warnings = [event for event in events if event["event"] == "step.warning"]
    assert warnings[-1]["step"] == "doctor"
    assert warnings[-1]["text"] == ".local/bin/sol is not reachable"
    assert warnings[-1]["fix_hint"] == "uv tool install solstone"


def test_setup_jsonl_existing_journal_emits_dead_end(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    patch_subprocess(monkeypatch, doctor_jsonl_lines=doctor_ok_lines())
    journal = tmp_path / "journal"
    (journal / "config").mkdir(parents=True)

    rc = setup.main(["--jsonl", "--yes", "--journal", str(journal)])
    events = parse_jsonl(capsys.readouterr().out)

    expected = "\n".join(
        [
            (
                "journal setup: cannot proceed in non-interactive mode - "
                f"{journal} already contains journal data."
            ),
            "Setup will not auto-claim an existing journal.",
            "",
            "Retry with one of:",
            "  journal setup --accept-existing-journal",
            "  journal setup --journal /path/to/new-journal --accept-existing-journal",
            "",
            "Interactive escape:",
            "  journal setup",
            "",
            "Run 'journal setup --explain' for full step list.",
        ]
    )
    failed = [event for event in events if event["event"] == "step.failed"][-1]
    assert rc == 2
    assert failed["step"] == "journal"
    assert failed["error"]["code"] == "journal_existing_blocked"
    assert failed["error"]["message"] == expected


def test_setup_jsonl_journal_is_file_emits_dead_end(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    journal.write_text("not a directory", encoding="utf-8")

    rc = setup.main(["--jsonl", "--yes", "--journal", str(journal)])
    events = parse_jsonl(capsys.readouterr().out)

    failed = [event for event in events if event["event"] == "step.failed"][-1]
    assert rc == 2
    assert failed["step"] == "journal"
    assert failed["error"]["code"] == "journal_dir_invalid"


def test_setup_jsonl_skipped_step_emits_completed_with_outcome_skipped(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    rc, events, _out = run_setup_jsonl(
        tmp_path,
        monkeypatch,
        capsys,
        ["--skip-models", "--skip-skills", "--skip-service"],
    )

    assert rc == 0
    install_models = [
        event
        for event in events
        if event["event"] == "step.completed" and event["step"] == "install_models"
    ][0]
    assert install_models["outcome"] == "skipped"
    assert install_models["reason"] == "--skip-models"


def test_setup_jsonl_subprocess_failure_emits_step_failed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    (home / ".claude").mkdir()
    patch_subprocess(monkeypatch, doctor_jsonl_lines=doctor_ok_lines())
    journal = tmp_path / "journal"

    def fake_run_step_subprocess(
        ctx: setup.SetupContext,
        command: list[str],
        *,
        timeout: float | None = None,
    ) -> setup.StepProcessResult:
        del ctx, timeout
        if command == expected_skills_user_command():
            return setup.StepProcessResult(
                1, "", "first stderr line\nsecond stderr line\n", False
            )
        return setup.StepProcessResult(0, "", "", False)

    monkeypatch.setattr(setup, "run_step_subprocess", fake_run_step_subprocess)

    rc = setup.main(
        [
            "--jsonl",
            "--yes",
            "--journal",
            str(journal),
            "--skip-models",
            "--skip-service",
        ]
    )
    events = parse_jsonl(capsys.readouterr().out)

    failed = [event for event in events if event["event"] == "step.failed"][0]
    assert rc == 1
    assert failed["step"] == "skills_user"
    assert failed["error"]["code"] == "step_subprocess_failed"
    assert failed["error"]["message"] == "first stderr line"
    assert "second stderr line" in failed["error"]["details"]


def test_setup_jsonl_skills_failure_continues_and_completes_once(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    patch_subprocess(monkeypatch, doctor_jsonl_lines=doctor_ok_lines())
    patch_service_health(monkeypatch)
    journal = tmp_path / "journal"
    commands: list[list[str]] = []

    def fake_run_step_subprocess(
        ctx: setup.SetupContext,
        command: list[str],
        *,
        timeout: float | None = None,
    ) -> setup.StepProcessResult:
        del ctx, timeout
        commands.append(command)
        if command == expected_skills_user_command():
            return setup.StepProcessResult(1, "", "user skills failed\n", False)
        return setup.StepProcessResult(0, "", "", False)

    monkeypatch.setattr(setup, "run_step_subprocess", fake_run_step_subprocess)

    rc = setup.main(["--jsonl", "--yes", "--journal", str(journal), "--skip-models"])
    events = parse_jsonl(capsys.readouterr().out)

    assert rc == 1
    assert expected_skills_user_command() in commands
    assert expected_skills_journal_command(journal) in commands
    assert expected_wrapper_command() in commands
    assert expected_service_install_command() in commands
    started_steps = [
        event["step"] for event in events if event["event"] == "step.started"
    ]
    terminal_steps = [
        event["step"]
        for event in events
        if event["event"] in {"step.completed", "step.failed"}
    ]
    for step in ["skills_user", "skills_journal", "wrapper", "service"]:
        assert step in started_steps
        assert step in terminal_steps
    completed = [event for event in events if event["event"] == "setup.completed"]
    assert len(completed) == 1
    assert completed[0] == events[-1]
    assert completed[0]["status"] == "failed"
    assert completed[0]["failed_step"] == "skills_user"


def test_setup_jsonl_does_not_call_input(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_tty(monkeypatch)
    journal = tmp_path / "journal"
    (journal / "config").mkdir(parents=True)
    input_mock = Mock()
    monkeypatch.setattr("builtins.input", input_mock)
    rc, events, _out = run_setup_jsonl(
        tmp_path,
        monkeypatch,
        capsys,
        ["--skip-models", "--skip-skills"],
        doctor_lines=doctor_ok_lines(),
    )

    assert rc == 2
    failed = [event for event in events if event["event"] == "step.failed"][-1]
    assert failed["step"] == "journal"
    assert failed["error"]["code"] == "journal_existing_blocked"
    input_mock.assert_not_called()


def test_setup_jsonl_forces_non_interactive_mode() -> None:
    args = setup.build_parser().parse_args(["--jsonl"])
    assert setup.resolve_mode(args) is setup.SetupMode.NON_INTERACTIVE


def test_setup_jsonl_dry_run_short_circuit(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    rc, events, _out = run_setup_jsonl(tmp_path, monkeypatch, capsys, ["--dry-run"])

    assert rc == 0
    assert [event["event"] for event in events] == ["setup.started", "setup.completed"]
    assert events[-1]["status"] == "ok"


def test_setup_jsonl_explain_short_circuit(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    rc, events, _out = run_setup_jsonl(tmp_path, monkeypatch, capsys, ["--explain"])

    assert rc == 0
    assert [event["event"] for event in events] == ["setup.started", "setup.completed"]
    assert events[-1]["status"] == "ok"


def test_setup_jsonl_subprocess_large_stderr_no_deadlock(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    (home / ".claude").mkdir()
    patch_subprocess(
        monkeypatch,
        doctor_jsonl_lines=doctor_ok_lines(),
        command_returncode=1,
        command_stderr="x" * (300 * 1024),
    )

    rc = setup.main(
        [
            "--jsonl",
            "--yes",
            "--journal",
            str(tmp_path / "journal"),
            "--skip-models",
            "--skip-service",
        ]
    )
    events = parse_jsonl(capsys.readouterr().out)

    failed = [event for event in events if event["event"] == "step.failed"][-1]
    assert rc == 1
    assert failed["error"]["code"] == "step_subprocess_failed"
    assert len(failed["error"]["details"].encode("utf-8")) <= 8192


def test_setup_jsonl_end_to_end_first_line_setup_started(tmp_path: Path) -> None:
    home = tmp_path / "home"
    home.mkdir()
    journal = tmp_path / "journal"
    env = {**os.environ, "HOME": str(home)}
    proc = subprocess.Popen(
        [
            sys.executable,
            "-m",
            "solstone.think.sol_cli",
            "setup",
            "--jsonl",
            "--yes",
            "--journal",
            str(journal),
            "--skip-models",
            "--skip-skills",
            "--skip-service",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=Path(__file__).resolve().parent.parent,
        env=env,
    )
    try:
        assert proc.stdout is not None
        first = json.loads(proc.stdout.readline())
        assert first["event"] == "setup.started"
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()


def test_setup_jsonl_doc_drift() -> None:
    content = (Path(__file__).resolve().parent.parent / "docs" / "SOLCLI.md").read_text(
        encoding="utf-8"
    )
    for value in EVENT_TYPES | ERROR_CODES | SKIPPED_REASONS | set(STEP_NAMES):
        assert value in content


def test_setup_jsonl_enums_module_exports_frozensets() -> None:
    assert isinstance(EVENT_TYPES, frozenset)
    assert isinstance(ERROR_CODES, frozenset)
    assert isinstance(STEP_NAMES, tuple)
    assert isinstance(SKIPPED_REASONS, frozenset)
