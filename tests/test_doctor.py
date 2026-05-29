# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.think import install_guard
from solstone.think.probe import ProbeOutput

ROOT = Path(__file__).resolve().parent.parent


@pytest.fixture
def doctor():
    from solstone.think import doctor as doctor_module

    yield doctor_module


@pytest.fixture
def home_root(monkeypatch, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    return home


def args(doctor, *, port: int = 5015):
    return doctor.Args(verbose=False, json=False, jsonl=False, port=port)


def make_repo(tmp_path: Path, *, worktree: bool = False) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    if worktree:
        (repo / ".git").write_text("gitdir: /tmp/worktree\n", encoding="utf-8")
    else:
        (repo / ".git").mkdir()
    return repo


def ensure_expected_target(repo: Path) -> Path:
    target = repo / ".venv" / "bin" / "sol"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("", encoding="utf-8")
    return target


def make_alias(home_root: Path, target: Path | str) -> Path:
    alias = home_root / ".local" / "bin" / "sol"
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.symlink_to(target)
    return alias


def other_target(tmp_path: Path) -> Path:
    target = tmp_path / "other" / ".venv" / "bin" / "sol"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("", encoding="utf-8")
    return target


def test_install_guard_import_succeeds_when_frontmatter_is_shadowed(tmp_path):
    shadow_dir = tmp_path / "shadow"
    shadow_dir.mkdir()
    (shadow_dir / "frontmatter.py").write_text(
        'raise ImportError("blocked for test")\n',
        encoding="utf-8",
    )
    env = os.environ.copy()
    pythonpath_parts = [str(shadow_dir), str(ROOT)]
    existing_pythonpath = env.get("PYTHONPATH")
    if existing_pythonpath:
        pythonpath_parts.append(existing_pythonpath)
    env["PYTHONPATH"] = os.pathsep.join(pythonpath_parts)

    result = subprocess.run(
        [
            sys.executable,
            "-c",
            "from solstone.think.install_guard import parse_wrapper; print('ok')",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        env=env,
    )

    assert result.returncode == 0
    assert result.stdout.strip() == "ok"


class TestPythonVersion:
    def test_ok(self, doctor):
        result = doctor.python_sanity_check(args(doctor))
        assert result.status == "ok"

    def test_fail_when_too_old(self, doctor, monkeypatch):
        monkeypatch.setattr(doctor.sys, "version_info", (3, 9, 18))
        result = doctor.python_sanity_check(args(doctor))
        assert result.status == "fail"
        assert "does not satisfy" in result.detail


class TestSolImportable:
    def test_skip_when_absent(self, doctor, monkeypatch, tmp_path):
        monkeypatch.setattr(doctor, "ROOT", tmp_path)
        result = doctor.sol_importable_check(args(doctor))
        assert result.status == "skip"

    def test_ok(self, doctor, monkeypatch, tmp_path):
        monkeypatch.setattr(doctor, "ROOT", tmp_path)
        python_bin = tmp_path / ".venv" / "bin" / "python"
        python_bin.parent.mkdir(parents=True)
        python_bin.write_text("", encoding="utf-8")
        monkeypatch.setattr(
            doctor,
            "run_probe",
            lambda *_args, **_kwargs: ProbeOutput("", "", 0),
        )
        result = doctor.sol_importable_check(args(doctor))
        assert result.status == "ok"
        assert (
            result.detail
            == "from solstone.think.sol_cli import main succeeded outside repo cwd"
        )

    def test_fail_on_module_not_found(self, doctor, monkeypatch, tmp_path):
        monkeypatch.setattr(doctor, "ROOT", tmp_path)
        python_bin = tmp_path / ".venv" / "bin" / "python"
        python_bin.parent.mkdir(parents=True)
        python_bin.write_text("", encoding="utf-8")
        monkeypatch.setattr(
            doctor,
            "run_probe",
            lambda *_args, **_kwargs: ProbeOutput(
                "",
                "Traceback (most recent call last):\nModuleNotFoundError: No module named 'solstone'\n",
                1,
            ),
        )
        result = doctor.sol_importable_check(args(doctor))
        assert result.status == "fail"
        assert result.detail == "ModuleNotFoundError: No module named 'solstone'"

    def test_fail_on_other_exception(self, doctor, monkeypatch, tmp_path):
        monkeypatch.setattr(doctor, "ROOT", tmp_path)
        python_bin = tmp_path / ".venv" / "bin" / "python"
        python_bin.parent.mkdir(parents=True)
        python_bin.write_text("", encoding="utf-8")
        monkeypatch.setattr(
            doctor,
            "run_probe",
            lambda *_args, **_kwargs: ProbeOutput(
                "", "SyntaxError: broken import\n", 1
            ),
        )
        result = doctor.sol_importable_check(args(doctor))
        assert result.status == "fail"
        assert result.detail == "SyntaxError: broken import"


class TestPackagedInstall:
    def setup_packaged(self, doctor, monkeypatch, tmp_path):
        monkeypatch.setattr(doctor, "ROOT", tmp_path)
        monkeypatch.setattr(doctor, "is_packaged_install", lambda: True)

    def test_python_version_uses_metadata_when_pyproject_absent(
        self, doctor, monkeypatch, tmp_path
    ):
        self.setup_packaged(doctor, monkeypatch, tmp_path)
        monkeypatch.setattr(
            doctor,
            "distribution",
            lambda name: SimpleNamespace(metadata={"Requires-Python": ">=3.11"}),
        )

        result = doctor.python_sanity_check(args(doctor))

        assert result.status == "ok"
        assert ">=3.11" in result.detail

    def test_sol_importable_uses_in_process_import(self, doctor, monkeypatch, tmp_path):
        self.setup_packaged(doctor, monkeypatch, tmp_path)

        result = doctor.sol_importable_check(args(doctor))

        assert result.status == "ok"
        assert result.detail == "import solstone succeeded in packaged install"


class TestPortCheckRemoved:
    def test_port_check_is_not_registered(self, doctor):
        assert "port_5015_free" not in doctor.CHECK_MAP
        assert "port_5015_free" not in {
            check.name for check, _runner in doctor.UNIVERSAL_CHECKS
        }


def test_default_universal_battery_check_names(doctor):
    assert {check.name for check, _runner in doctor.UNIVERSAL_CHECKS} == {
        "python_version",
        "sol_importable",
        "local_bin_sol_reachable",
        "stale_alias_symlink",
    }


class TestStaleAliasSymlink:
    @pytest.fixture(autouse=True)
    def isolated_legacy_backups(self, doctor, monkeypatch, tmp_path):
        backup_dir = tmp_path / "legacy-backups"
        backup_dir.mkdir()
        monkeypatch.setattr(doctor, "_legacy_backup_dir", lambda: backup_dir)
        self.backup_dir = backup_dir

    def setup_import(self, doctor, monkeypatch):
        monkeypatch.setattr(
            doctor,
            "import_install_guard",
            lambda: (install_guard.AliasState, install_guard.check_alias),
        )

    def setup_auto_migration(self, doctor, monkeypatch, tmp_path):
        self.setup_import(doctor, monkeypatch)
        fake_bin = tmp_path / "fakevenv" / "bin"
        fake_bin.mkdir(parents=True)
        journal = tmp_path / "journal"
        monkeypatch.setattr(sys, "executable", str(fake_bin / "python"))
        monkeypatch.setattr(
            "solstone.think.install_guard._current_journal_for_alias",
            lambda: journal,
        )
        return fake_bin, journal

    @staticmethod
    def make_existing_target(path: Path) -> Path:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("", encoding="utf-8")
        return path

    @staticmethod
    def assert_managed_wrapper(home_root, binary, journal, fake_bin):
        alias = home_root / ".local" / "bin" / binary
        assert alias.exists()
        parsed = install_guard.parse_wrapper(alias.read_text(encoding="utf-8"))
        assert parsed is not None
        assert parsed["journal"] == str(journal)
        assert parsed["sol_bin"] == str(fake_bin / binary)

    def test_absent_ok(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        monkeypatch.setattr(doctor, "ROOT", repo)
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "ok"

    def test_owned_ok(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        make_alias(home_root, ensure_expected_target(repo))
        monkeypatch.setattr(doctor, "ROOT", repo)
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "ok"

    def test_cross_repo_fail(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        make_alias(home_root, other_target(tmp_path))
        monkeypatch.setattr(doctor, "ROOT", repo)
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "fail"

    def test_dangling_fail(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        missing = tmp_path / "missing" / ".venv" / "bin" / "sol"
        make_alias(home_root, missing)
        monkeypatch.setattr(doctor, "ROOT", repo)
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "fail"

    def test_not_symlink_fail(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        alias = home_root / ".local" / "bin" / "sol"
        alias.parent.mkdir(parents=True, exist_ok=True)
        alias.write_text("not a symlink", encoding="utf-8")
        monkeypatch.setattr(doctor, "ROOT", repo)
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "fail"

    def test_worktree_skip(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path, worktree=True)
        monkeypatch.setattr(doctor, "ROOT", repo)
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "skip"

    def test_import_failure_skips(self, doctor, monkeypatch):
        monkeypatch.setattr(
            doctor,
            "import_install_guard",
            lambda: (_ for _ in ()).throw(ImportError("boom")),
        )
        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        assert result.status == "skip"
        assert "could not import solstone.think.install_guard" in result.detail

    def test_uv_tool_auto_migrates(self, doctor, monkeypatch, home_root, tmp_path):
        fake_bin, journal = self.setup_auto_migration(doctor, monkeypatch, tmp_path)
        repo = make_repo(tmp_path)
        target = self.make_existing_target(
            home_root / ".local" / "share" / "uv" / "tools" / "solstone" / "bin" / "sol"
        )
        make_alias(home_root, target)
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")

        assert result.status == "ok"
        assert "auto-migrated" in result.detail
        assert "uv-tool" in result.detail
        self.assert_managed_wrapper(home_root, "sol", journal, fake_bin)
        assert not (home_root / ".local" / "bin" / "journal").exists()
        backups = list(self.backup_dir.glob("sol.old-symlink-*"))
        assert len(backups) == 1
        assert backups[0].exists()

    def test_pipx_xdg_auto_migrates(self, doctor, monkeypatch, home_root, tmp_path):
        fake_bin, journal = self.setup_auto_migration(doctor, monkeypatch, tmp_path)
        repo = make_repo(tmp_path)
        target = self.make_existing_target(
            home_root
            / ".local"
            / "share"
            / "pipx"
            / "venvs"
            / "solstone"
            / "bin"
            / "sol"
        )
        make_alias(home_root, target)
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")

        assert result.status == "ok"
        assert "pipx-xdg" in result.detail
        self.assert_managed_wrapper(home_root, "sol", journal, fake_bin)
        assert not (home_root / ".local" / "bin" / "journal").exists()

    def test_pipx_legacy_auto_migrates(self, doctor, monkeypatch, home_root, tmp_path):
        fake_bin, journal = self.setup_auto_migration(doctor, monkeypatch, tmp_path)
        repo = make_repo(tmp_path)
        target = self.make_existing_target(
            home_root / ".local" / "pipx" / "venvs" / "solstone" / "bin" / "sol"
        )
        make_alias(home_root, target)
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")

        assert result.status == "ok"
        assert "pipx-legacy" in result.detail
        self.assert_managed_wrapper(home_root, "sol", journal, fake_bin)
        assert not (home_root / ".local" / "bin" / "journal").exists()

    def test_uv_tool_dangling_auto_migrates(
        self, doctor, monkeypatch, home_root, tmp_path
    ):
        fake_bin, journal = self.setup_auto_migration(doctor, monkeypatch, tmp_path)
        repo = make_repo(tmp_path)
        target = (
            home_root / ".local" / "share" / "uv" / "tools" / "solstone" / "bin" / "sol"
        )
        target.parent.mkdir(parents=True, exist_ok=True)
        make_alias(home_root, target)
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")

        assert result.status == "ok"
        assert "migrated legacy uv-tool symlink" in result.detail
        self.assert_managed_wrapper(home_root, "sol", journal, fake_bin)
        assert not (home_root / ".local" / "bin" / "journal").exists()
        backups = list(self.backup_dir.glob("sol.old-symlink-*"))
        assert len(backups) == 1
        assert backups[0].is_symlink()

    def test_pipx_xdg_dangling_auto_migrates(
        self, doctor, monkeypatch, home_root, tmp_path
    ):
        fake_bin, journal = self.setup_auto_migration(doctor, monkeypatch, tmp_path)
        repo = make_repo(tmp_path)
        target = (
            home_root
            / ".local"
            / "share"
            / "pipx"
            / "venvs"
            / "solstone"
            / "bin"
            / "sol"
        )
        target.parent.mkdir(parents=True, exist_ok=True)
        make_alias(home_root, target)
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")

        assert result.status == "ok"
        assert "migrated legacy pipx symlink" in result.detail
        self.assert_managed_wrapper(home_root, "sol", journal, fake_bin)
        assert not (home_root / ".local" / "bin" / "journal").exists()
        backups = list(self.backup_dir.glob("sol.old-symlink-*"))
        assert len(backups) == 1
        assert backups[0].is_symlink()

    def test_non_legacy_target_still_fails(
        self, doctor, monkeypatch, home_root, tmp_path
    ):
        self.setup_auto_migration(doctor, monkeypatch, tmp_path)
        repo = make_repo(tmp_path)
        target = self.make_existing_target(
            tmp_path / "opt" / "random" / "foo" / "bin" / "sol"
        )
        make_alias(home_root, target)
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")

        assert result.status == "fail"
        assert list(self.backup_dir.glob("sol.old-symlink-*")) == []

    def test_idempotent_after_migration(self, doctor, monkeypatch, home_root, tmp_path):
        self.setup_auto_migration(doctor, monkeypatch, tmp_path)
        repo = make_repo(tmp_path)
        target = self.make_existing_target(
            home_root / ".local" / "share" / "uv" / "tools" / "solstone" / "bin" / "sol"
        )
        make_alias(home_root, target)
        monkeypatch.setattr(doctor, "ROOT", repo)

        first = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        backups_after_first = sorted(self.backup_dir.glob("*.old-symlink-*"))
        second = doctor.stale_alias_symlink_check(args(doctor), binary="sol")
        backups_after_second = sorted(self.backup_dir.glob("*.old-symlink-*"))

        assert first.status == "ok"
        assert second.status == "ok"
        assert backups_after_second == backups_after_first
        assert len(list(self.backup_dir.glob("sol.old-symlink-*"))) == 1
        assert list(self.backup_dir.glob("journal.old-symlink-*")) == []

    def test_partial_migration_recovery_detail(
        self, doctor, monkeypatch, home_root, tmp_path
    ):
        self.setup_import(doctor, monkeypatch)
        repo = make_repo(tmp_path)
        backup = self.backup_dir / "sol.old-symlink-20260101000000"
        backup.write_text("", encoding="utf-8")
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary="sol")

        assert result.status == "fail"
        assert "partial migration detected" in result.detail
        assert str(backup) in result.detail


class TestJsonAndExitCodes:
    def test_json_output(self, doctor, monkeypatch, capsys):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [
                doctor.CheckResult("a", "blocker", "ok", "fine", None),
                doctor.CheckResult("b", "advisory", "warn", "careful", "fix me"),
            ],
        )
        rc = doctor.main(["--json"])
        payload = json.loads(capsys.readouterr().out)
        assert rc == 0
        assert sorted(payload) == ["checks", "summary"]
        assert set(payload["checks"][0]) == {
            "name",
            "severity",
            "status",
            "detail",
            "fix",
        }

    def test_exit_code_matrix(self, doctor, monkeypatch, capsys):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [doctor.CheckResult("a", "blocker", "fail", "boom", None)],
        )
        assert doctor.main([]) == 1
        capsys.readouterr()

        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [doctor.CheckResult("a", "advisory", "fail", "boom", None)],
        )
        assert doctor.main([]) == 0
        capsys.readouterr()

        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [doctor.CheckResult("a", "blocker", "skip", "skip", None)],
        )
        assert doctor.main([]) == 0

    def test_summary_line_format(self, doctor, monkeypatch, capsys):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [
                doctor.CheckResult("a", "blocker", "fail", "boom", None),
                doctor.CheckResult("b", "advisory", "warn", "warn", None),
                doctor.CheckResult("c", "blocker", "skip", "skip", None),
            ],
        )
        doctor.main([])
        output = capsys.readouterr().out.strip().splitlines()
        assert output[-1] == "doctor: 3 checks, 1 failed, 1 warnings, 1 skipped"

    def test_doctor_jsonl_emits_started_and_completed(
        self, doctor, monkeypatch, capsys
    ):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [doctor.CheckResult("a", "blocker", "ok", "fine", None)],
        )

        rc = doctor.main(["--jsonl"])
        events = [json.loads(line) for line in capsys.readouterr().out.splitlines()]

        assert rc == 0
        assert events[0]["event"] == "doctor.started"
        assert events[-1]["event"] == "doctor.completed"
        assert events[-1]["status"] == "ok"

    def test_doctor_jsonl_emits_check_completed_per_check(
        self, doctor, monkeypatch, capsys
    ):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [
                doctor.CheckResult(check.name, check.severity, "ok", "fine", None)
                for check, _func in doctor.UNIVERSAL_CHECKS
            ],
        )

        doctor.main(["--jsonl"])
        events = [json.loads(line) for line in capsys.readouterr().out.splitlines()]

        checks = [event for event in events if event["event"] == "check.completed"]
        assert len(checks) == len(doctor.UNIVERSAL_CHECKS)

    def test_doctor_jsonl_status_translates_short_to_long(
        self, doctor, monkeypatch, capsys
    ):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [
                doctor.CheckResult("ok", "blocker", "ok", "ok", None),
                doctor.CheckResult("warn", "advisory", "warn", "warn", None),
                doctor.CheckResult("fail", "advisory", "fail", "fail", None),
                doctor.CheckResult("skip", "blocker", "skip", "skip", None),
            ],
        )

        rc = doctor.main(["--jsonl"])
        events = [json.loads(line) for line in capsys.readouterr().out.splitlines()]

        assert rc == 0
        statuses = {
            event["name"]: event["status"]
            for event in events
            if event["event"] == "check.completed"
        }
        assert statuses == {
            "ok": "ok",
            "warn": "warning",
            "fail": "failed",
            "skip": "skipped",
        }
        assert events[-1]["status"] == "warning"

    def test_doctor_jsonl_json_and_jsonl_mutually_exclusive(self, doctor):
        with pytest.raises(SystemExit) as raised:
            doctor.main(["--json", "--jsonl"])

        assert raised.value.code == 2

    def test_doctor_jsonl_preserves_json_payload_unchanged(
        self, doctor, monkeypatch, capsys
    ):
        monkeypatch.setattr(
            doctor,
            "run_checks",
            lambda _args: [
                doctor.CheckResult("a", "advisory", "warn", "careful", None)
            ],
        )

        rc = doctor.main(["--json"])
        payload = json.loads(capsys.readouterr().out)

        assert rc == 0
        assert payload["checks"][0]["status"] == "warn"

    def test_doctor_jsonl_subprocess_e2e(self):
        result = subprocess.run(
            [sys.executable, "-m", "solstone.think.sol_cli", "doctor", "--jsonl"],
            capture_output=True,
            text=True,
            cwd=ROOT,
            timeout=60,
        )

        assert result.returncode in (
            0,
            1,
        ), f"unexpected exit code {result.returncode}: {result.stderr}"
        lines = [line for line in result.stdout.splitlines() if line.strip()]
        assert lines
        for line in lines:
            json.loads(line)


def test_sol_doctor_subprocess_json_shape():
    """End-to-end: `sol doctor --json` via the venv entry point produces valid diagnostic JSON."""
    repo_root = Path(__file__).resolve().parent.parent
    result = subprocess.run(
        [sys.executable, "-m", "solstone.think.sol_cli", "doctor", "--json"],
        capture_output=True,
        text=True,
        cwd=repo_root,
        timeout=60,
    )
    # Exit code: 0 if all checks pass, 1 if any blocker fails. Either is valid
    # here; this test asserts CLI routing and payload shape, not machine health.
    assert result.returncode in (
        0,
        1,
    ), f"unexpected exit code {result.returncode}: {result.stderr}"
    payload = json.loads(result.stdout)
    assert "checks" in payload and isinstance(payload["checks"], list)
    assert "summary" in payload and isinstance(payload["summary"], dict)
    assert {check["name"] for check in payload["checks"]} == {
        "python_version",
        "sol_importable",
        "local_bin_sol_reachable",
        "stale_alias_symlink",
    }


class TestMakefileIntegration:
    def test_dry_run_install_does_not_run_doctor(self):
        result = subprocess.run(
            ["make", "--dry-run", "-B", "install"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
        )
        assert result.returncode == 0
        lines = result.stdout.splitlines()
        assert all("python3 scripts/doctor.py" not in line for line in lines)
        assert any("uv sync" in line for line in lines)


def test_doctor_runs_with_minimal_path_env(tmp_path):
    """Doctor must complete with PATH=/usr/bin:/bin (launchd-style minimal env)."""
    journal = tmp_path / "journal"
    journal.mkdir()
    env = {
        "PATH": "/usr/bin:/bin",
        "HOME": str(tmp_path),
        "SOLSTONE_JOURNAL": str(journal),
    }
    result = subprocess.run(
        [sys.executable, "-m", "solstone.think.sol_cli", "doctor", "--json"],
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode in {0, 1}, (
        f"doctor crashed: rc={result.returncode}\n"
        f"stdout={result.stdout}\nstderr={result.stderr}"
    )
    payload = json.loads(result.stdout)
    names = {check["name"] for check in payload["checks"]}
    assert names == {
        "python_version",
        "sol_importable",
        "local_bin_sol_reachable",
        "stale_alias_symlink",
    }
    assert not any(
        name.startswith("service_")
        or name == "journal_sync"
        or name.startswith("feature:")
        for name in names
    )
