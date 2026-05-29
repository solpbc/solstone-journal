# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import plistlib
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from solstone.think import install_guard


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


def assert_managed_wrapper(home_root: Path, binary: str, journal: Path, fake_bin: Path):
    alias = home_root / ".local" / "bin" / binary
    assert alias.exists()
    parsed = install_guard.parse_wrapper(alias.read_text(encoding="utf-8"))
    assert parsed is not None
    assert parsed["journal"] == str(journal)
    assert parsed["sol_bin"] == str(fake_bin / binary)


def patch_alias_absent(doctor, monkeypatch):
    monkeypatch.setattr(
        doctor,
        "import_install_guard",
        lambda: (install_guard.AliasState, install_guard.check_alias),
    )


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


def test_role_skip_without_local_journal(doctor, monkeypatch, tmp_path, home_root):
    journal = tmp_path / "missing-journal"
    monkeypatch.setattr(doctor, "get_journal_info", lambda: (str(journal), "env"))
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
    assert by_name["disk_space"].status in {"ok", "warn"}
    assert by_name["config_dir_readable"].status == "ok"
    assert by_name["feature:pdf"].status in {"ok", "warn"}
    assert by_name["feature:whisper"].status in {"ok", "warn"}


class TestJournalAlias:
    @pytest.fixture(autouse=True)
    def isolated_legacy_backups(self, doctor, monkeypatch, tmp_path):
        backup_dir = tmp_path / "legacy-backups"
        backup_dir.mkdir()
        monkeypatch.setattr(doctor, "_legacy_backup_dir", lambda: backup_dir)
        self.backup_dir = backup_dir

    def setup_auto_migration(self, doctor, monkeypatch, tmp_path):
        patch_alias_absent(doctor, monkeypatch)
        fake_bin = tmp_path / "fakevenv" / "bin"
        fake_bin.mkdir(parents=True)
        journal = tmp_path / "journal"
        monkeypatch.setattr(sys, "executable", str(fake_bin / "python"))
        monkeypatch.setattr(
            "solstone.think.install_guard._current_journal_for_alias",
            lambda: journal,
        )
        return fake_bin, journal

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

    def test_journal_uv_tool_auto_migrates_only_journal(
        self, doctor, monkeypatch, home_root, tmp_path
    ):
        fake_bin, journal = self.setup_auto_migration(doctor, monkeypatch, tmp_path)
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
        make_alias(home_root, "journal", target)
        monkeypatch.setattr(doctor, "ROOT", repo)

        result = doctor.stale_alias_symlink_check(args(doctor), binary="journal")

        assert result.status == "ok"
        assert "uv-tool" in result.detail
        assert_managed_wrapper(home_root, "journal", journal, fake_bin)
        assert not (home_root / ".local" / "bin" / "sol").exists()
        backups = list(self.backup_dir.glob("journal.old-symlink-*"))
        assert len(backups) == 1


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
