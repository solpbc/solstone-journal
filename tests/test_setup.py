# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path
from typing import Any
from unittest.mock import Mock

import pytest

from solstone.think import health_cli, install_guard, service, setup
from solstone.think.user_config import write_user_config


def patch_home(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> Path:
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    return home


def patch_source_checkout(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / "pyproject.toml").write_text("[project]\nname = 'solstone'\n")
    (repo / ".git").mkdir()
    monkeypatch.setattr(setup, "get_project_root", lambda: str(repo))
    monkeypatch.setattr(setup, "source_checkout", lambda: True)
    return repo


def patch_packaged_install(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> Path:
    root = tmp_path / "site-packages"
    root.mkdir()
    monkeypatch.setattr(setup, "get_project_root", lambda: str(root))
    monkeypatch.setattr(setup, "source_checkout", lambda: False)
    return root


def patch_tty(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(sys.stdin, "isatty", lambda: True)
    monkeypatch.setattr(sys.stdout, "isatty", lambda: True)


def doctor_payload(checks: list[dict[str, Any]] | None = None) -> str:
    return json.dumps(
        {
            "checks": checks or [],
            "summary": {
                "total": len(checks or []),
                "failed": 0,
                "warnings": 0,
                "skipped": 0,
            },
        }
    )


def patch_subprocess(
    monkeypatch: pytest.MonkeyPatch,
    *,
    doctor_stdout: str | None = None,
    doctor_returncode: int = 0,
    command_returncode: int = 0,
    command_stdout: str = "",
    command_stderr: str = "",
    doctor_timeout: bool = False,
    popen_timeout_command: list[str] | None = None,
    doctor_jsonl_lines: list[str | dict[str, Any]] | None = None,
    doctor_jsonl_returncode: int = 0,
    doctor_jsonl_stderr: str = "",
) -> list[list[str]]:
    calls: list[list[str]] = []

    def fake_run(
        command: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        calls.append(command)
        if "doctor" in command:
            if doctor_timeout:
                raise subprocess.TimeoutExpired(command, setup.DOCTOR_TIMEOUT_SECONDS)
            return subprocess.CompletedProcess(
                command,
                doctor_returncode,
                stdout=doctor_stdout if doctor_stdout is not None else doctor_payload(),
                stderr="doctor failed\n" if doctor_returncode else "",
            )
        return subprocess.CompletedProcess(
            command,
            command_returncode,
            stdout=command_stdout,
            stderr=command_stderr,
        )

    class FakePopen:
        def __init__(self, command: list[str], **kwargs: object) -> None:
            del kwargs
            self.command = command
            self.terminated = False
            self.returncode: int | None = None
            self._returncode = command_returncode
            self.stdout = None
            self.stderr = None
            if (
                doctor_jsonl_lines is not None
                and "doctor" in command
                and "--jsonl" in command
            ):
                serialized = [
                    item if isinstance(item, str) else json.dumps(item)
                    for item in doctor_jsonl_lines
                ]
                self.stdout = iter(
                    line if line.endswith("\n") else f"{line}\n" for line in serialized
                )
                self.stderr = iter(
                    line if line.endswith("\n") else f"{line}\n"
                    for line in doctor_jsonl_stderr.splitlines()
                )
                self._returncode = doctor_jsonl_returncode
            calls.append(command)

        def wait(self, timeout: float | None = None) -> int:
            if self.command == popen_timeout_command and not self.terminated:
                raise subprocess.TimeoutExpired(self.command, timeout)
            self.returncode = self._returncode
            return self._returncode

        def terminate(self) -> None:
            self.terminated = True
            self.returncode = -15

        def kill(self) -> None:
            self.terminated = True
            self.returncode = -9

    monkeypatch.setattr(setup.subprocess, "run", fake_run)
    monkeypatch.setattr(setup.subprocess, "Popen", FakePopen)
    return calls


def patch_service_health(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(service, "_up", lambda port=5015: 0)
    monkeypatch.setattr(health_cli, "health_check", lambda: 0)


def patch_journal_os_defaults(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "solstone.think.utils._resolve_os_identity", lambda: ("Setup User", "setup")
    )
    monkeypatch.setattr(
        "solstone.think.utils._resolve_os_timezone", lambda: "America/Denver"
    )


STEP_NAMES = [
    "doctor",
    "journal",
    "install_models",
    "skills_user",
    "skills_journal",
    "wrapper",
    "service",
]


def expected_doctor_command(port: int = 5015) -> list[str]:
    return [
        sys.executable,
        "-m",
        "solstone.think.sol_cli",
        "doctor",
        "--json",
        "--port",
        str(port),
    ]


def expected_install_models_command() -> list[str]:
    return [
        sys.executable,
        "-m",
        "solstone.think.sol_cli",
        "install-models",
        "--variant",
        "auto",
    ]


def expected_skills_user_command() -> list[str]:
    return [
        sys.executable,
        "-m",
        "solstone.think.sol_cli",
        "skills",
        "install",
        "--agent",
        "all",
    ]


def expected_skills_journal_command(journal: Path) -> list[str]:
    return [
        sys.executable,
        "-m",
        "solstone.think.sol_cli",
        "skills",
        "install",
        "--project",
        str(journal),
        "--agent",
        "all",
    ]


def expected_wrapper_command() -> list[str]:
    return [sys.executable, "-m", "solstone.think.install_guard", "install"]


def expected_service_install_command(port: int = 5015) -> list[str]:
    return [
        sys.executable,
        "-m",
        "solstone.think.sol_cli",
        "service",
        "install",
        "--port",
        str(port),
    ]


def expected_service_restart_command() -> list[str]:
    return [sys.executable, "-m", "solstone.think.sol_cli", "service", "restart"]


def assert_command(
    calls: list[list[str]], position: int, expected_argv: list[str]
) -> None:
    assert position < len(calls), (
        f"expected {position + 1}+ subprocess calls, got {len(calls)}"
    )
    assert calls[position] == expected_argv, (
        f"call[{position}] mismatch:\n  want: {expected_argv}\n  got:  {calls[position]}"
    )


def assert_step_names_and_statuses(
    manifest: dict[str, Any], statuses: list[str]
) -> None:
    assert [step["name"] for step in manifest["steps"]] == STEP_NAMES
    assert [step["status"] for step in manifest["steps"]] == statuses


def read_manifest(journal: Path) -> dict[str, Any]:
    return json.loads(
        (journal / "health" / "setup-state.json").read_text(encoding="utf-8")
    )


def touch_file(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.touch()


def write_owned_wrapper(home: Path, repo: Path, journal: Path) -> Path:
    first_wrapper: Path | None = None
    for binary, wrapper in install_guard.alias_paths().items():
        if first_wrapper is None:
            first_wrapper = wrapper
        wrapper.parent.mkdir(parents=True, exist_ok=True)
        target = install_guard.expected_target(repo, binary)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text("", encoding="utf-8")
        wrapper.write_text(
            install_guard.render_wrapper(str(journal), str(target), binary),
            encoding="utf-8",
        )
        wrapper.chmod(0o755)
    assert first_wrapper is not None
    return first_wrapper


def seed_clean_uninstall_artifacts(
    home: Path, repo: Path, journal: Path
) -> dict[str, Path]:
    service_path = setup._service_path_for_uninstall()
    touch_file(service_path)
    wrapper_path = write_owned_wrapper(home, repo, journal)
    journal_wrapper_path = home / ".local" / "bin" / "journal"
    write_user_config(journal=str(journal))
    manifest_path = journal / "health" / "setup-state.json"
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    manifest_path.write_text("{}", encoding="utf-8")
    return {
        "service": service_path,
        "wrapper": wrapper_path,
        "wrapper_journal": journal_wrapper_path,
        "config": setup.config_path(),
        "manifest": manifest_path,
    }


def digest_journal_tree(journal: Path, exclude: set[Path] | None = None) -> str:
    excluded = {path.resolve() for path in (exclude or set())}
    digest = hashlib.sha256()
    for path in sorted(p for p in journal.rglob("*") if p.is_file()):
        if path.resolve() in excluded:
            continue
        digest.update(path.relative_to(journal).as_posix().encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def prior_artifact_paths(journal: Path) -> dict[str, list[Path]]:
    service_path = setup.service_artifact_path()
    return {
        "doctor": [],
        "journal": [setup.config_path(), journal],
        "install_models": setup.model_paths(),
        "skills_user": [
            Path.home() / ".claude" / "skills" / "solstone" / "SKILL.md",
            Path.home() / ".codex" / "skills" / "solstone" / "SKILL.md",
            Path.home() / ".gemini" / "skills" / "solstone" / "SKILL.md",
        ],
        "skills_journal": [
            journal / ".claude" / "skills",
            journal / ".agents" / "skills",
        ],
        "wrapper": list(install_guard.alias_paths().values()),
        "service": [service_path] if service_path is not None else [],
    }


def write_clean_prior_manifest(journal: Path) -> dict[str, list[Path]]:
    journal.mkdir(parents=True, exist_ok=True)
    paths_by_name = prior_artifact_paths(journal)
    for paths in paths_by_name.values():
        for path in paths:
            if path == journal:
                path.mkdir(parents=True, exist_ok=True)
            else:
                touch_file(path)
    started_at = "2026-05-02T21:29:42Z"
    completed_at = "2026-05-02T21:30:42Z"
    steps = [
        {
            "name": name,
            "status": "ok",
            "paths": [str(path.expanduser().resolve()) for path in paths_by_name[name]],
            "started_at": started_at,
            "finished_at": completed_at,
            "error": None,
        }
        for name in STEP_NAMES
    ]
    (journal / "health").mkdir(parents=True, exist_ok=True)
    (journal / "health" / "setup-state.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "started_at": started_at,
                "completed_at": completed_at,
                "mode": "non_interactive",
                "args_resolved": {},
                "steps": steps,
            }
        ),
        encoding="utf-8",
    )
    return paths_by_name


def test_interactive_happy_path_default_journal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    patch_tty(monkeypatch)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    (home / ".claude").mkdir()
    calls = patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)

    rc = setup.main([])

    assert rc == 0
    journal = home / "journal"
    assert (home / ".config" / "solstone" / "config.toml").read_text(
        encoding="utf-8"
    ) == f'journal = "{journal}"\n'
    manifest = read_manifest(journal)
    assert_step_names_and_statuses(manifest, ["ok", "ok", "ok", "ok", "ok", "ok", "ok"])
    assert "solstone is running at http://localhost:5015" in capsys.readouterr().out
    assert_command(calls, 0, expected_doctor_command())
    assert_command(calls, 1, expected_install_models_command())
    assert_command(calls, 2, expected_skills_user_command())
    assert_command(calls, 3, expected_skills_journal_command(journal))
    assert_command(calls, 4, expected_wrapper_command())
    assert_command(calls, 5, expected_service_install_command())
    assert len(calls) == 6


def test_resolve_journal_path_precedence_chain(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    parser = setup.build_parser()

    cli_journal = tmp_path / "cli-journal"
    path, source = setup.resolve_journal_path(
        parser.parse_args(["--journal", str(cli_journal)])
    )
    assert path == cli_journal
    assert source == "cli"

    env_journal = tmp_path / "env-journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(env_journal))
    path, source = setup.resolve_journal_path(parser.parse_args([]))
    assert path == env_journal
    assert source == "env"

    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    config_journal = tmp_path / "config-journal"
    write_user_config(journal=str(config_journal))
    path, source = setup.resolve_journal_path(parser.parse_args([]))
    assert path == config_journal
    assert source == "config"

    write_user_config(journal="")
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    path, source = setup.resolve_journal_path(parser.parse_args([]))
    assert path == home / "journal"
    assert source == "default"


def test_interactive_happy_path_journal_override(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    patch_tty(monkeypatch)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    (home / ".claude").mkdir()
    calls = patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)
    journal = tmp_path / "custom-journal"

    rc = setup.main(["--journal", str(journal)])

    assert rc == 0
    assert (home / ".config" / "solstone" / "config.toml").read_text(
        encoding="utf-8"
    ) == f'journal = "{journal}"\n'
    assert read_manifest(journal)["args_resolved"]["journal"]["source"] == "cli"
    assert_command(calls, 4, expected_wrapper_command())
    assert len(calls) == 6


def test_non_interactive_happy_path(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    (home / ".claude").mkdir()
    calls = patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)
    journal = tmp_path / "journal"

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 0
    manifest = read_manifest(journal)
    assert manifest["completed_at"] is not None
    assert_step_names_and_statuses(manifest, ["ok", "ok", "ok", "ok", "ok", "ok", "ok"])
    assert_command(calls, 5, expected_service_install_command())
    assert len(calls) == 6


@pytest.mark.parametrize("use_journal_flag", [False, True])
def test_non_interactive_dead_end_on_existing_journal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    use_journal_flag: bool,
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    calls = patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)
    journal = tmp_path / "journal" if use_journal_flag else home / "journal"
    (journal / "config").mkdir(parents=True)
    argv = ["--yes"]
    if use_journal_flag:
        argv.extend(["--journal", str(journal)])

    rc = setup.main(argv)

    assert rc == 2
    err = capsys.readouterr().err
    assert "already contains journal data" in err
    assert "--accept-existing-journal" in err
    assert_command(calls, 0, expected_doctor_command())
    assert len(calls) == 1


def test_dry_run_side_effect_free(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    calls = patch_subprocess(monkeypatch)
    journal = tmp_path / "journal"

    rc = setup.main(["--dry-run", "--journal", str(journal)])

    assert rc == 0
    assert calls == []
    assert not (home / ".config" / "solstone" / "config.toml").exists()
    assert not (journal / "health" / "setup-state.json").exists()
    assert "setup dry-run:" in capsys.readouterr().out


def test_step_journal_materializes_journal_config(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    patch_journal_os_defaults(monkeypatch)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    argv = ["--yes", "--journal", str(journal)]
    ctx = setup.resolve_context(setup.build_parser().parse_args(argv), argv)

    result = setup.step_journal(ctx, 2)

    config_path = journal / "config" / "journal.json"
    config = json.loads(config_path.read_text(encoding="utf-8"))
    assert result.status == "ok"
    assert config["identity"]["name"] == "Setup User"
    assert config["identity"]["preferred"] == "setup"
    assert config["identity"]["timezone"] == "America/Denver"
    assert config["convey"]["secret"]
    assert str(config_path.resolve()) in result.paths


def test_step_journal_dry_run_does_not_materialize_journal_config(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    patch_journal_os_defaults(monkeypatch)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    argv = ["--dry-run", "--journal", str(journal)]
    ctx = setup.resolve_context(setup.build_parser().parse_args(argv), argv)

    result = setup.step_journal(ctx, 2)

    assert result.status == "ok"
    assert not (journal / "config" / "journal.json").exists()


def test_step_journal_is_idempotent(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    patch_journal_os_defaults(monkeypatch)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    argv = ["--yes", "--journal", str(journal)]
    ctx = setup.resolve_context(setup.build_parser().parse_args(argv), argv)

    setup.step_journal(ctx, 2)
    config_path = journal / "config" / "journal.json"
    first = config_path.stat()
    setup.step_journal(ctx, 2)
    second = config_path.stat()

    assert second.st_ino == first.st_ino
    assert second.st_size == first.st_size


def test_explain_early_exit(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    calls = patch_subprocess(monkeypatch)

    rc = setup.main(["--explain"])

    assert rc == 0
    assert calls == []
    assert "setup plan:" in capsys.readouterr().out


def test_manifest_round_trip(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    (home / ".claude").mkdir()
    patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)
    journal = tmp_path / "journal"

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 0
    manifest = read_manifest(journal)
    assert manifest["schema_version"] == 1
    assert manifest["mode"] == "non_interactive"
    assert all(
        Path(path).is_absolute() for step in manifest["steps"] for path in step["paths"]
    )
    assert {step["status"] for step in manifest["steps"]} <= {"ok", "skipped", "failed"}


def test_persisted_journal_skips_existing_journal_check_non_interactive(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    (journal / "config").mkdir(parents=True)
    write_user_config(journal=str(journal))
    patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)

    rc = setup.main(["--yes"])

    assert rc == 0
    assert "already contains journal data" not in capsys.readouterr().err
    journal_step = next(
        step for step in read_manifest(journal)["steps"] if step["name"] == "journal"
    )
    assert journal_step["status"] == "ok"


def test_persisted_journal_skips_existing_journal_check_interactive(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    patch_tty(monkeypatch)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    (journal / "config").mkdir(parents=True)
    write_user_config(journal=str(journal))
    patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)

    def fail_on_prompt(path: Path) -> bool:
        raise AssertionError(f"unexpected existing-journal prompt for {path}")

    monkeypatch.setattr(setup, "prompt_accept_existing_journal", fail_on_prompt)

    rc = setup.main([])

    assert rc == 0
    assert "Use existing journal" not in capsys.readouterr().out


def test_existing_journal_dead_end_still_fires_when_path_not_persisted(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)
    journal = tmp_path / "journal"
    (journal / "config").mkdir(parents=True)

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 2
    assert "already contains journal data" in capsys.readouterr().err


def test_clean_rerun_preface_when_manifest_complete(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)
    journal = tmp_path / "journal"
    journal.mkdir()
    completed_at = "2026-05-02T21:30:42Z"
    started_at = "2026-05-02T21:29:42Z"
    steps = [
        {
            "name": name,
            "status": "ok",
            "paths": [],
            "started_at": started_at,
            "finished_at": completed_at,
            "error": None,
        }
        for name in (
            "doctor",
            "journal",
            "install_models",
            "skills_user",
            "skills_journal",
            "wrapper",
            "service",
        )
    ]
    (journal / "health").mkdir(parents=True, exist_ok=True)
    (journal / "health" / "setup-state.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "started_at": started_at,
                "completed_at": completed_at,
                "mode": "non_interactive",
                "args_resolved": {},
                "steps": steps,
            }
        ),
        encoding="utf-8",
    )

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 0
    lines = capsys.readouterr().out.splitlines()
    assert lines[0] == (
        f"journal setup last ran cleanly on {completed_at}; verifying current state."
    )
    assert lines[1] == "Use --force to re-run all steps unconditionally."


def test_partial_rerun_preface_when_steps_failed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)
    journal = tmp_path / "journal"
    journal.mkdir()
    started_at = "2026-05-02T21:29:42Z"
    (journal / "health").mkdir(parents=True, exist_ok=True)
    (journal / "health" / "setup-state.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "started_at": started_at,
                "completed_at": None,
                "mode": "non_interactive",
                "args_resolved": {},
                "steps": [
                    {
                        "name": "install_models",
                        "status": "failed",
                        "paths": [],
                        "started_at": started_at,
                        "finished_at": "2026-05-02T21:30:42Z",
                        "error": {"message": "install failed", "exit_code": 1},
                    }
                ],
            }
        ),
        encoding="utf-8",
    )

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 0
    assert (
        f"journal setup last run on {started_at} left these steps incomplete:\n"
        "  - install_models (failed)\n"
        "Re-running will verify state and re-run incomplete steps."
    ) in capsys.readouterr().out


def test_no_preface_without_manifest(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)
    journal = tmp_path / "journal"

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 0
    out = capsys.readouterr().out
    assert "last ran cleanly" not in out and "left these steps incomplete" not in out


def test_force_flag_changes_preface_text(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)
    journal = tmp_path / "journal"
    journal.mkdir()
    completed_at = "2026-05-02T21:30:42Z"
    started_at = "2026-05-02T21:29:42Z"
    steps = [
        {
            "name": name,
            "status": "ok",
            "paths": [],
            "started_at": started_at,
            "finished_at": completed_at,
            "error": None,
        }
        for name in (
            "doctor",
            "journal",
            "install_models",
            "skills_user",
            "skills_journal",
            "wrapper",
            "service",
        )
    ]
    (journal / "health").mkdir(parents=True, exist_ok=True)
    (journal / "health" / "setup-state.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "started_at": started_at,
                "completed_at": completed_at,
                "mode": "non_interactive",
                "args_resolved": {},
                "steps": steps,
            }
        ),
        encoding="utf-8",
    )

    rc = setup.main(["--yes", "--force", "--journal", str(journal)])

    assert rc == 0
    out = capsys.readouterr().out
    assert out.startswith(
        f"journal setup last ran cleanly on {completed_at}; re-running all steps (--force)."
    )
    assert "Use --force to re-run all steps unconditionally." not in out


def test_partial_completion_runs_remaining_steps(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    (home / ".claude").mkdir()
    journal = tmp_path / "journal"
    journal.mkdir()
    (journal / "health").mkdir(parents=True, exist_ok=True)
    (journal / "health" / "setup-state.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "steps": [
                    {"name": "doctor", "status": "ok"},
                    {"name": "service", "status": "failed"},
                ],
            }
        ),
        encoding="utf-8",
    )
    calls = patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 0
    manifest = read_manifest(journal)
    assert_step_names_and_statuses(
        manifest, ["skipped", "ok", "ok", "ok", "ok", "ok", "ok"]
    )
    assert manifest["steps"][0]["reason"] == "prior_run_ok"
    assert_command(calls, 0, expected_install_models_command())
    assert_command(calls, 1, expected_skills_user_command())
    assert_command(calls, 2, expected_skills_journal_command(journal))
    assert_command(calls, 3, expected_wrapper_command())
    assert_command(calls, 4, expected_service_install_command())
    assert len(calls) == 5


def test_non_interactive_setup_has_no_port_preflight_dead_end(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Post-removal there is no port pre-flight.

    On a real upgrade port 5015 is held by the running solstone service; setup no
    longer probes it, so non-interactive setup completes with no dead-end or
    prompt. The doctor subprocess is faked by the harness; the assertion is the
    absence of the former port dead-end.
    """
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    (home / ".claude").mkdir()
    input_mock = Mock()
    monkeypatch.setattr("builtins.input", input_mock)
    calls = patch_subprocess(monkeypatch, doctor_stdout=doctor_payload())
    patch_service_health(monkeypatch)
    journal = tmp_path / "journal"

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 0
    input_mock.assert_not_called()
    captured = capsys.readouterr()
    assert "port 5015 is already in use" not in captured.err
    assert "port_in_use_non_interactive" not in captured.err
    assert "port 5015 is already in use" not in captured.out
    assert "port_in_use_non_interactive" not in captured.out
    assert_command(calls, 0, expected_doctor_command())
    assert_command(calls, 5, expected_service_install_command())
    assert len(calls) == 6


def test_doctor_timeout_records_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    patch_subprocess(monkeypatch, doctor_timeout=True)

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 1
    step = read_manifest(journal)["steps"][-1]
    assert step["name"] == "doctor"
    assert step["status"] == "failed"
    assert "timed out after 30s" in step["error"]["message"]
    assert step["error"]["exit_code"] == 1


def test_install_models_timeout_records_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    patch_subprocess(
        monkeypatch,
        popen_timeout_command=expected_install_models_command(),
    )

    rc = setup.main(["--yes", "--journal", str(journal), "--step-timeout-seconds", "1"])

    assert rc == 1
    step = read_manifest(journal)["steps"][-1]
    assert step["name"] == "install_models"
    assert step["status"] == "failed"
    assert "timed out after 1s" in step["error"]["message"]
    assert step["error"]["exit_code"] == 1


def test_step_timeout_seconds_passes_through_to_help_and_explain(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    with pytest.raises(SystemExit) as raised:
        setup.main(["--help"])
    assert raised.value.code == 0
    assert "--step-timeout-seconds" in capsys.readouterr().out

    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)

    rc = setup.main(["--explain", "--yes"])

    assert rc == 0
    assert "step_timeout_seconds: 1800" in capsys.readouterr().out


def test_empty_journal_arg_rejected_at_parse_time(
    capsys: pytest.CaptureFixture[str],
) -> None:
    with pytest.raises(SystemExit) as raised:
        setup.main(["--journal", ""])

    assert raised.value.code == 2
    assert "--journal must not be empty" in capsys.readouterr().err


@pytest.mark.parametrize("port", ["0", "99999"])
def test_port_out_of_range_rejected_at_parse_time(
    capsys: pytest.CaptureFixture[str],
    port: str,
) -> None:
    with pytest.raises(SystemExit) as raised:
        setup.main(["--port", port])

    assert raised.value.code == 2
    assert "--port must be in 1024-65535" in capsys.readouterr().err


def test_clean_uninstall_appears_in_help(
    capsys: pytest.CaptureFixture[str],
) -> None:
    with pytest.raises(SystemExit):
        setup.main(["--help"])

    assert "--clean-uninstall" in capsys.readouterr().out


def test_clean_uninstall_rejects_jsonl_with_specific_message(
    capsys: pytest.CaptureFixture[str],
) -> None:
    with pytest.raises(SystemExit) as raised:
        setup.main(["--clean-uninstall", "--jsonl"])

    assert raised.value.code == 2
    assert (
        "JSONL output is not supported for --clean-uninstall in this version."
        in capsys.readouterr().err
    )


@pytest.mark.parametrize(
    "flag_argv",
    [
        ["--journal", "journal"],
        ["--port", "5015"],
        ["--port=5015"],
        ["--variant", "auto"],
        ["--variant=auto"],
        ["--step-timeout-seconds", "1800"],
        ["--step-timeout-seconds=1800"],
        ["--dry-run"],
        ["--explain"],
        ["--skip-models"],
        ["--skip-skills"],
        ["--skip-service"],
        ["--accept-existing-journal"],
        ["--force"],
    ],
)
def test_clean_uninstall_rejects_other_setup_flags(
    flag_argv: list[str],
    capsys: pytest.CaptureFixture[str],
) -> None:
    with pytest.raises(SystemExit) as raised:
        setup.main(["--clean-uninstall", *flag_argv])

    assert raised.value.code == 2
    assert "--clean-uninstall cannot be combined with" in capsys.readouterr().err


@pytest.mark.parametrize("answer", ["", "n", "no"])
def test_clean_uninstall_prompt_cancels_without_mutation(
    answer: str,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    repo = patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.chdir(repo)
    patch_tty(monkeypatch)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = home / "journal"
    paths = seed_clean_uninstall_artifacts(home, repo, journal)
    monkeypatch.setattr("builtins.input", lambda _prompt: answer)

    rc = setup.main(["--clean-uninstall"])

    assert rc == 0
    assert all(path.exists() for path in paths.values())
    out = capsys.readouterr().out
    assert "journal setup --clean-uninstall will remove these runtime artifacts:" in out
    assert "[present] service:" in out
    assert "[present] wrapper:" in out
    assert "[present] config:" in out
    assert "[present] manifest:" in out
    assert "will not remove:" in out
    assert "journal directory:" in out
    assert "/Applications/solstone.app" in out
    assert "~/Library/Application Support/solstone/" in out
    assert "macOS microphone or screen recording permissions" in out
    assert "the python package" in out
    assert "cancelled" in out


def test_clean_uninstall_eof_cancels_without_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    repo = patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.chdir(repo)
    patch_tty(monkeypatch)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = home / "journal"
    paths = seed_clean_uninstall_artifacts(home, repo, journal)

    def raise_eof(_prompt: str) -> str:
        raise EOFError

    monkeypatch.setattr("builtins.input", raise_eof)

    rc = setup.main(["--clean-uninstall"])

    assert rc == 0
    assert all(path.exists() for path in paths.values())
    assert "cancelled" in capsys.readouterr().out


def test_clean_uninstall_non_tty_cancels_without_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    repo = patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.chdir(repo)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = home / "journal"
    paths = seed_clean_uninstall_artifacts(home, repo, journal)

    rc = setup.main(["--clean-uninstall"])

    assert rc == 0
    assert all(path.exists() for path in paths.values())
    assert (
        "not a tty; rerun with --yes to proceed non-interactively (cancelled)"
        in capsys.readouterr().out
    )


def test_clean_uninstall_yes_removes_all_seeded_artifacts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    repo = patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.chdir(repo)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = home / "journal"
    paths = seed_clean_uninstall_artifacts(home, repo, journal)
    monkeypatch.setattr(setup, "_run_service_uninstall", lambda: 0)

    rc = setup.main(["--clean-uninstall", "--yes"])

    assert rc == 0
    assert not any(path.exists() or path.is_symlink() for path in paths.values())
    out = capsys.readouterr().out
    assert out.index("[step 1/4] running service uninstall...") < out.index(
        "[step 2/4] running wrapper uninstall..."
    )
    assert out.index("[step 2/4] running wrapper uninstall...") < out.index(
        "[step 3/4] running config uninstall..."
    )
    assert out.index("[step 3/4] running config uninstall...") < out.index(
        "[step 4/4] running manifest uninstall..."
    )
    assert "[step 1/4] removed service:" in out
    assert "[step 2/4] removed wrapper:" in out
    assert "[step 3/4] removed config:" in out
    assert "[step 4/4] removed manifest:" in out
    assert (
        "clean uninstall complete: 4 removed, 0 already-absent, 0 skipped, 0 failed"
        in out
    )


def test_clean_uninstall_noop_when_all_paths_absent(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    repo = patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.chdir(repo)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    service_calls: list[str] = []
    monkeypatch.setattr(
        setup, "_run_service_uninstall", lambda: service_calls.append("service") or 0
    )

    rc = setup.main(["--clean-uninstall", "--yes"])

    assert rc == 0
    assert service_calls == []
    assert capsys.readouterr().out == "nothing to remove (all paths already absent)\n"


def test_clean_uninstall_partial_wrapper_state_removes_remaining_wrapper(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    repo = patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.chdir(repo)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = home / "journal"
    journal_wrapper = home / ".local" / "bin" / "journal"
    target = install_guard.expected_target(repo, "journal")
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("", encoding="utf-8")
    journal_wrapper.parent.mkdir(parents=True, exist_ok=True)
    journal_wrapper.write_text(
        install_guard.render_wrapper(str(journal), str(target), "journal"),
        encoding="utf-8",
    )
    journal_wrapper.chmod(0o755)
    monkeypatch.setattr(setup, "_run_service_uninstall", lambda: 0)

    rc = setup.main(["--clean-uninstall", "--yes"])

    assert rc == 0
    assert not journal_wrapper.exists()
    assert not (home / ".local" / "bin" / "sol").exists()
    out = capsys.readouterr().out
    assert "[step 2/4] removed wrapper:" in out
    assert (
        "clean uninstall complete: 1 removed, 3 already-absent, 0 skipped, 0 failed"
        in out
    )


def test_clean_uninstall_service_failure_is_best_effort(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    repo = patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.chdir(repo)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = home / "journal"
    paths = seed_clean_uninstall_artifacts(home, repo, journal)

    def fail_service() -> int:
        raise RuntimeError("service exploded")

    monkeypatch.setattr(setup, "_run_service_uninstall", fail_service)

    rc = setup.main(["--clean-uninstall", "--yes"])

    assert rc == 1
    assert paths["service"].exists()
    assert not paths["wrapper"].exists()
    assert not paths["config"].exists()
    assert not paths["manifest"].exists()
    out = capsys.readouterr().out
    assert "[step 1/4] failed service: RuntimeError: service exploded" in out
    assert "[step 2/4] removed wrapper:" in out
    assert "[step 3/4] removed config:" in out
    assert "[step 4/4] removed manifest:" in out
    assert (
        "clean uninstall complete: 3 removed, 0 already-absent, 0 skipped, 1 failed"
        in out
    )


def test_clean_uninstall_wrapper_cross_repo_is_skipped(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    repo = patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.chdir(repo)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    other = tmp_path / "other" / ".venv" / "bin" / "sol"
    other.parent.mkdir(parents=True)
    other.write_text("", encoding="utf-8")
    wrapper = home / ".local" / "bin" / "sol"
    wrapper.parent.mkdir(parents=True)
    wrapper.symlink_to(other)
    monkeypatch.setattr(setup, "_run_service_uninstall", lambda: 0)

    rc = setup.main(["--clean-uninstall", "--yes"])

    assert rc == 0
    assert wrapper.is_symlink()
    assert wrapper.resolve() == other
    out = capsys.readouterr().out
    assert f"[step 2/4] skipped wrapper: alias points at {other}, not removing" in out
    assert (
        "clean uninstall complete: 0 removed, 3 already-absent, 1 skipped, 0 failed"
        in out
    )


def test_clean_uninstall_wrapper_worktree_is_skipped(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".git").write_text("gitdir: /tmp/worktree\n", encoding="utf-8")
    monkeypatch.chdir(repo)
    monkeypatch.setattr(setup, "get_project_root", lambda: str(repo))
    monkeypatch.setattr(setup, "source_checkout", lambda: True)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    wrapper = home / ".local" / "bin" / "sol"
    wrapper.parent.mkdir(parents=True)
    wrapper.write_text("foreign", encoding="utf-8")
    monkeypatch.setattr(setup, "_run_service_uninstall", lambda: 0)

    rc = setup.main(["--clean-uninstall", "--yes"])

    assert rc == 0
    assert wrapper.read_text(encoding="utf-8") == "foreign"
    out = capsys.readouterr().out
    assert "[step 2/4] skipped wrapper: refusing to act from a git worktree" in out
    assert (
        "clean uninstall complete: 0 removed, 3 already-absent, 1 skipped, 0 failed"
        in out
    )


def test_clean_uninstall_uses_env_config_default_without_source_fallback(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    parser = setup.build_parser()
    args = parser.parse_args(["--clean-uninstall", "--yes"])

    assert setup.resolve_clean_uninstall_context(args).journal_path == home / "journal"

    config_journal = tmp_path / "config-journal"
    write_user_config(journal=str(config_journal))
    assert setup.resolve_clean_uninstall_context(args).journal_path == config_journal

    env_journal = tmp_path / "env-journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(env_journal))
    assert setup.resolve_clean_uninstall_context(args).journal_path == env_journal


@pytest.mark.parametrize(
    ("platform", "expected_rel"),
    [
        ("darwin", "Library/LaunchAgents/org.solpbc.solstone.plist"),
        ("linux", ".config/systemd/user/solstone.service"),
    ],
)
def test_clean_uninstall_service_path_uses_platform_constants(
    platform: str,
    expected_rel: str,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    monkeypatch.setattr(setup.sys, "platform", platform)

    assert setup._service_path_for_uninstall() == home / expected_rel


def test_clean_uninstall_preserves_journal_contents_except_manifest(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    repo = patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.chdir(repo)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = home / "journal"
    paths = seed_clean_uninstall_artifacts(home, repo, journal)
    seeded_files = {
        "chronicle/20260520/_default/090000_60/transcript.jsonl": "{}\n",
        "config/journal.json": "{}\n",
        "entities/alice/entity.json": '{"name":"Alice"}\n',
        "facets/work/activities/20260520.jsonl": "{}\n",
        "health/other-health.json": "{}\n",
    }
    for rel, content in seeded_files.items():
        path = journal / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
    before = digest_journal_tree(journal, exclude={paths["manifest"]})
    monkeypatch.setattr(setup, "_run_service_uninstall", lambda: 0)

    rc = setup.main(["--clean-uninstall", "--yes"])

    assert rc == 0
    assert digest_journal_tree(journal, exclude={paths["manifest"]}) == before
    assert journal.exists()
    assert not paths["manifest"].exists()
    for rel in seeded_files:
        assert (journal / rel).exists()


def test_packaged_install_runs_service_step(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_packaged_install(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    calls = patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)
    journal = tmp_path / "journal"

    rc = setup.main(
        ["--yes", "--journal", str(journal), "--skip-models", "--skip-skills"]
    )

    assert rc == 0
    out = capsys.readouterr().out
    unsupported_message = " ".join(
        ["packaged-install service support", "is not implemented"]
    )
    assert unsupported_message not in out
    assert "solstone is running at http://localhost:5015" in out
    assert_command(calls, 0, expected_doctor_command())
    assert_command(
        calls,
        1,
        [
            sys.executable,
            "-m",
            "solstone.think.sol_cli",
            "service",
            "install",
            "--port",
            "5015",
        ],
    )
    assert len(calls) == 2
    steps = read_manifest(journal)["steps"]
    assert steps[-2]["status"] == "skipped"
    assert steps[-2]["reason"] == "packaged_install"
    assert steps[-1]["name"] == "service"
    assert steps[-1]["status"] == "ok"


def test_step_skills_user_installs_solstone_bundle_for_all_agents(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    argv = ["--yes", "--journal", str(journal)]
    ctx = setup.resolve_context(setup.build_parser().parse_args(argv), argv)
    commands: list[list[str]] = []

    def fake_run_step_subprocess(
        ctx: setup.SetupContext,
        command: list[str],
        *,
        timeout: float | None = None,
    ) -> setup.StepProcessResult:
        del ctx, timeout
        commands.append(command)
        return setup.StepProcessResult(0, "", "", False)

    monkeypatch.setattr(setup, "run_step_subprocess", fake_run_step_subprocess)

    result = setup.step_skills_user(ctx, 4)

    assert result.status == "ok"
    assert commands == [expected_skills_user_command()]
    assert "--agent" in commands[0]
    assert commands[0][-1] == "all"
    assert "claude" not in commands[0]


def test_step_skills_journal_installs_into_journal_path(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    argv = ["--yes", "--journal", str(journal)]
    ctx = setup.resolve_context(setup.build_parser().parse_args(argv), argv)
    commands: list[list[str]] = []

    def fake_run_step_subprocess(
        ctx: setup.SetupContext,
        command: list[str],
        *,
        timeout: float | None = None,
    ) -> setup.StepProcessResult:
        del ctx, timeout
        commands.append(command)
        return setup.StepProcessResult(0, "", "", False)

    monkeypatch.setattr(setup, "run_step_subprocess", fake_run_step_subprocess)

    result = setup.step_skills_journal(ctx, 5)

    assert result.status == "ok"
    assert commands == [expected_skills_journal_command(journal)]
    assert commands[0][-3:] == [str(journal), "--agent", "all"]


def test_step_skills_user_failure_does_not_block_skills_journal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    patch_subprocess(monkeypatch)
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
            return setup.StepProcessResult(3, "", "user skills failed\n", False)
        return setup.StepProcessResult(0, "", "", False)

    monkeypatch.setattr(setup, "run_step_subprocess", fake_run_step_subprocess)

    rc = setup.main(["--yes", "--journal", str(journal), "--skip-models"])

    assert rc == 3
    assert expected_skills_user_command() in commands
    assert expected_skills_journal_command(journal) in commands
    manifest = read_manifest(journal)
    assert_step_names_and_statuses(
        manifest, ["ok", "ok", "skipped", "failed", "ok", "ok", "ok"]
    )


def test_step_skills_journal_failure_does_not_block_subsequent_steps(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    patch_subprocess(monkeypatch)
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
        if command == expected_skills_journal_command(journal):
            return setup.StepProcessResult(4, "", "journal skills failed\n", False)
        return setup.StepProcessResult(0, "", "", False)

    monkeypatch.setattr(setup, "run_step_subprocess", fake_run_step_subprocess)

    rc = setup.main(["--yes", "--journal", str(journal), "--skip-models"])

    assert rc == 4
    assert commands[-2:] == [
        expected_wrapper_command(),
        expected_service_install_command(),
    ]
    manifest = read_manifest(journal)
    assert_step_names_and_statuses(
        manifest, ["ok", "ok", "skipped", "ok", "failed", "ok", "ok"]
    )


def test_skip_skills_flag_skips_both_skill_steps(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)
    journal = tmp_path / "journal"

    rc = setup.main(
        ["--yes", "--journal", str(journal), "--skip-models", "--skip-skills"]
    )

    assert rc == 0
    steps = {step["name"]: step for step in read_manifest(journal)["steps"]}
    assert steps["skills_user"]["status"] == "skipped"
    assert steps["skills_user"]["reason"] == "--skip-skills"
    assert steps["skills_journal"]["status"] == "skipped"
    assert steps["skills_journal"]["reason"] == "--skip-skills"


def test_doctor_failure_still_early_exits(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    calls = patch_subprocess(monkeypatch, doctor_returncode=1)

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 1
    assert calls == [expected_doctor_command()]
    manifest = read_manifest(journal)
    assert [step["name"] for step in manifest["steps"]] == ["doctor"]
    assert manifest["steps"][0]["status"] == "failed"


def test_resumption_skips_completed_steps(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    write_clean_prior_manifest(journal)
    calls = patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 0
    assert calls == []
    manifest = read_manifest(journal)
    assert_step_names_and_statuses(manifest, ["skipped"] * 7)
    assert {step["reason"] for step in manifest["steps"]} == {"prior_run_ok"}


def test_resumption_runs_step_when_artifact_missing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    paths_by_name = write_clean_prior_manifest(journal)
    paths_by_name["wrapper"][0].unlink()
    calls = patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 0
    assert_command(calls, 0, expected_wrapper_command())
    assert len(calls) == 1
    manifest = read_manifest(journal)
    assert_step_names_and_statuses(
        manifest,
        ["skipped", "skipped", "skipped", "skipped", "skipped", "ok", "skipped"],
    )


def test_resumption_wedged_service_restarts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    write_clean_prior_manifest(journal)
    calls = patch_subprocess(monkeypatch)
    health_check = Mock(side_effect=[1, 0])
    monkeypatch.setattr(health_cli, "health_check", health_check)

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 0
    assert health_check.call_count == 2
    assert_command(calls, 0, expected_service_restart_command())
    assert len(calls) == 1
    service_step = read_manifest(journal)["steps"][-1]
    assert service_step["status"] == "ok"
    assert service_step["reason"] == "resumed_after_restart"


def test_resumption_wedged_service_falls_through_when_service_up_fails(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    monkeypatch.setattr(service, "_up", lambda port=5015: 7)
    journal = tmp_path / "journal"
    write_clean_prior_manifest(journal)
    calls = patch_subprocess(monkeypatch)
    monkeypatch.setattr(health_cli, "health_check", lambda: 1)

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 1
    assert_command(calls, 0, expected_service_restart_command())
    assert_command(calls, 1, expected_service_install_command())
    assert len(calls) == 2


def test_step_service_failure_message(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    argv = ["--yes", "--journal", str(tmp_path / "journal")]
    ctx = setup.resolve_context(setup.build_parser().parse_args(argv), argv)
    monkeypatch.setattr(setup, "run_inherited", lambda command: 0)
    artifact = setup.service_artifact_path()
    expected_paths = [setup.absolute_string(artifact)] if artifact is not None else []

    monkeypatch.setattr(service, "_up", lambda port=5015: 7)
    result = setup.step_service(ctx, 6)

    assert result.status == "failed"
    assert result.paths == expected_paths
    assert result.error == {
        "code": "service_up_failed",
        "message": "service up failed (exit 7)",
        "details": "",
        "exit_code": 1,
    }


def test_force_skips_resumption(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    write_clean_prior_manifest(journal)
    calls = patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)

    rc = setup.main(["--yes", "--force", "--journal", str(journal)])

    assert rc == 0
    assert_command(calls, 0, expected_doctor_command())
    assert_command(calls, 1, expected_install_models_command())
    assert_command(calls, 2, expected_skills_user_command())
    assert_command(calls, 3, expected_skills_journal_command(journal))
    assert_command(calls, 4, expected_wrapper_command())
    assert_command(calls, 5, expected_service_install_command())
    assert len(calls) == 6
    manifest = read_manifest(journal)
    assert_step_names_and_statuses(manifest, ["ok", "ok", "ok", "ok", "ok", "ok", "ok"])
    assert all(step["reason"] is None for step in manifest["steps"])


def test_step_exception_records_failed_row(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"

    def boom(ctx: setup.SetupContext, step_index: int) -> setup.StepResult:
        raise RuntimeError("boom")

    monkeypatch.setattr(setup, "_STEPS", (boom,))
    monkeypatch.setattr(setup, "_STEP_NAME", {boom: "doctor"})

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 1
    manifest = read_manifest(journal)
    assert len(manifest["steps"]) == 1
    step = manifest["steps"][0]
    assert step["name"] == "doctor"
    assert step["status"] == "failed"
    assert step["error"]["message"] == "boom"


@pytest.mark.parametrize("exc", [KeyboardInterrupt(), SystemExit(7)])
def test_base_exceptions_propagate(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    exc: BaseException,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"

    def boom(ctx: setup.SetupContext, step_index: int) -> setup.StepResult:
        raise exc

    monkeypatch.setattr(setup, "_STEPS", (boom,))
    monkeypatch.setattr(setup, "_STEP_NAME", {boom: "doctor"})

    with pytest.raises(type(exc)) as raised:
        setup.main(["--yes", "--journal", str(journal)])

    if isinstance(exc, SystemExit):
        assert raised.value.code == 7
    assert not (journal / "health" / "setup-state.json").exists()


def test_env_journal_overrides_config(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    config_journal = tmp_path / "from_config"
    env_journal = tmp_path / "from_env"
    write_user_config(journal=str(config_journal))
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(env_journal))
    calls = patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)

    rc = setup.main(["--yes"])

    assert rc == 0
    assert (home / ".config" / "solstone" / "config.toml").read_text(
        encoding="utf-8"
    ) == f'journal = "{env_journal}"\n'
    manifest = read_manifest(env_journal)
    assert manifest["args_resolved"]["journal"]["source"] == "env"
    assert env_journal.is_dir()
    assert not config_journal.exists()
    assert_command(calls, 0, expected_doctor_command())


def test_journal_is_regular_file_dead_ends(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal_file = tmp_path / "journal-file"
    journal_file.write_text("not a directory", encoding="utf-8")
    calls = patch_subprocess(monkeypatch)

    rc = setup.main(["--yes", "--journal", str(journal_file)])

    assert rc == 2
    assert calls == []
    assert "directory" in capsys.readouterr().err
    assert not (journal_file / "health" / "setup-state.json").exists()


def test_doctor_parse_failure_records_failed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    journal = tmp_path / "journal"
    patch_subprocess(monkeypatch, doctor_stdout="not json")

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 1
    manifest = read_manifest(journal)
    assert len(manifest["steps"]) == 1
    step = manifest["steps"][0]
    assert step["name"] == "doctor"
    assert step["status"] == "failed"
    assert "doctor JSON parse failed" in step["error"]["message"]


def test_invalid_manifest_treated_as_no_prior(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    (home / ".claude").mkdir()
    journal = tmp_path / "journal"
    journal.mkdir()
    (journal / "health").mkdir(parents=True, exist_ok=True)
    (journal / "health" / "setup-state.json").write_text("{", encoding="utf-8")
    calls = patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)

    rc = setup.main(["--yes", "--journal", str(journal)])

    assert rc == 0
    out = capsys.readouterr().out
    assert "last ran cleanly" not in out and "left these steps incomplete" not in out
    assert_command(calls, 0, expected_doctor_command())
    assert_command(calls, 1, expected_install_models_command())
    assert_command(calls, 2, expected_skills_user_command())
    assert_command(calls, 3, expected_skills_journal_command(journal))
    assert_command(calls, 4, expected_wrapper_command())
    assert_command(calls, 5, expected_service_install_command())
    assert len(calls) == 6


def test_port_propagates_to_subprocess_argv(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    home = patch_home(monkeypatch, tmp_path)
    patch_source_checkout(monkeypatch, tmp_path)
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)
    (home / ".claude").mkdir()
    journal = tmp_path / "journal"
    calls = patch_subprocess(monkeypatch)
    patch_service_health(monkeypatch)

    rc = setup.main(["--yes", "--journal", str(journal), "--port", "8080"])

    assert rc == 0
    assert_command(calls, 0, expected_doctor_command(port=8080))
    assert_command(calls, 5, expected_service_install_command(port=8080))
    assert len(calls) == 6
