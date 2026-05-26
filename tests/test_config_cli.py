# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from dataclasses import replace
from pathlib import Path
from unittest.mock import MagicMock

import pytest

from solstone.think import config_cli, install_guard


@pytest.fixture
def home_root(monkeypatch, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    return home


def ensure_expected_target(repo: Path, binary: str = "sol") -> Path:
    target = install_guard.expected_target(repo, binary)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("", encoding="utf-8")
    return target


def make_alias(home_root: Path, target: Path | str) -> Path:
    alias = home_root / ".local" / "bin" / "sol"
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.symlink_to(target)
    return alias


def make_managed_wrapper(
    home_root: Path,
    *,
    journal: str,
    sol_bin: str,
    binary: str = "sol",
) -> Path:
    alias = home_root / ".local" / "bin" / binary
    alias.parent.mkdir(parents=True, exist_ok=True)
    alias.write_text(
        install_guard.render_wrapper(journal, sol_bin, binary),
        encoding="utf-8",
    )
    alias.chmod(0o755)
    return alias


def make_managed_wrappers(
    home_root: Path, repo: Path, *, journal: str
) -> dict[str, Path]:
    aliases: dict[str, Path] = {}
    for binary in install_guard.alias_paths():
        target = ensure_expected_target(repo, binary)
        aliases[binary] = make_managed_wrapper(
            home_root,
            journal=journal,
            sol_bin=str(target),
            binary=binary,
        )
    return aliases


def make_journal(path: Path, *, active: bool | None = None) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    if active is None:
        return path
    config_dir = path / "config"
    config_dir.mkdir(parents=True, exist_ok=True)
    (config_dir / "journal.json").write_text(
        json.dumps({"setup": {"completed_at": 1700000000000 if active else 0}}),
        encoding="utf-8",
    )
    return path


def assert_wrapper(alias: Path, *, journal: str, sol_bin: str) -> None:
    assert install_guard.parse_wrapper(alias.read_text(encoding="utf-8")) == {
        "journal": journal,
        "sol_bin": sol_bin,
        "version": 7,
    }


def patch_service(monkeypatch, *, installed: bool, running: bool):
    monkeypatch.setattr(config_cli, "service_is_installed", lambda: installed)
    monkeypatch.setattr(config_cli, "service_is_running", lambda: running)


def service_run_mock(*, returncodes: list[int] | None = None):
    if returncodes is None:
        return MagicMock(return_value=MagicMock(returncode=0, stdout="", stderr=""))
    return MagicMock(
        side_effect=[
            MagicMock(returncode=code, stdout="", stderr="") for code in returncodes
        ]
    )


def test_config_command_registered():
    from solstone.think import sol_cli as sol

    assert sol.COMMANDS["config"].module == "solstone.think.config_cli"
    assert "config" in sol.service_help_group().commands


def test_show_reports_wrapper_embedded(home_root, monkeypatch, tmp_path, capsys):
    journal = str((tmp_path / "journal").resolve())
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=journal, sol_bin=str(target))
    monkeypatch.setenv("SOLSTONE_JOURNAL", journal)

    rc = config_cli.cmd_show()
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.err == ""
    assert captured.out.splitlines() == [
        f"path: {journal}",
        "source: wrapper-embedded",
        "wrapper-status: managed",
    ]


def test_show_reports_caller_override(home_root, monkeypatch, tmp_path, capsys):
    embedded = str((tmp_path / "embedded").resolve())
    override = str((tmp_path / "override").resolve())
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=embedded, sol_bin=str(target))
    monkeypatch.setenv("SOLSTONE_JOURNAL", override)

    rc = config_cli.cmd_show()
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.err == ""
    assert captured.out.splitlines() == [
        f"path: {override}",
        "source: caller-override",
        "wrapper-status: managed",
    ]


def test_show_reports_source_tree_fallback(home_root, monkeypatch, capsys):
    monkeypatch.delenv("SOLSTONE_JOURNAL", raising=False)

    rc = config_cli.cmd_show()
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.err == ""
    assert captured.out.splitlines() == [
        f"path: {Path(config_cli.get_project_root()) / 'journal'}",
        "source: source-tree fallback",
        "wrapper-status: absent",
    ]


def test_show_ignores_service_mock(home_root, monkeypatch, tmp_path, capsys):
    journal = str((tmp_path / "journal").resolve())
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=journal, sol_bin=str(target))
    monkeypatch.setenv("SOLSTONE_JOURNAL", journal)
    monkeypatch.setattr(config_cli, "service_is_installed", lambda: False)

    rc = config_cli.cmd_show()
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.err == ""
    assert "wrapper-status: managed" in captured.out


def test_journal_noops_when_path_already_embedded(
    home_root, monkeypatch, tmp_path, capsys
):
    target_path = str((tmp_path / "journal").resolve())
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=target_path, sol_bin=str(target))
    original = alias.read_text(encoding="utf-8")
    patch_service(monkeypatch, installed=False, running=False)
    run_mock = service_run_mock()
    monkeypatch.setattr(config_cli.subprocess, "run", run_mock)

    rc = config_cli.cmd_journal(target_path)
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.err == ""
    assert captured.out == f"sol config: journal already set to {target_path}\n"
    assert alias.read_text(encoding="utf-8") == original
    run_mock.assert_not_called()


def test_journal_rewrites_wrapper(home_root, monkeypatch, tmp_path, capsys):
    source = make_journal(tmp_path / "source", active=False)
    target_path = str((tmp_path / "target").resolve())
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)
    run_mock = service_run_mock()
    monkeypatch.setattr(config_cli.subprocess, "run", run_mock)

    rc = config_cli.cmd_journal(target_path)
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.err == ""
    assert captured.out == "service not installed; wrapper updated.\n"
    assert_wrapper(alias, journal=target_path, sol_bin=str(target))
    run_mock.assert_not_called()


def test_journal_rewrites_both_wrappers(home_root, monkeypatch, tmp_path, capsys):
    source = make_journal(tmp_path / "source", active=False)
    target_path = str((tmp_path / "target").resolve())
    repo = tmp_path / "repo"
    aliases = make_managed_wrappers(home_root, repo, journal=str(source))
    patch_service(monkeypatch, installed=False, running=False)
    run_mock = service_run_mock()
    monkeypatch.setattr(config_cli.subprocess, "run", run_mock)

    rc = config_cli.cmd_journal(target_path)
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.err == ""
    assert captured.out == "service not installed; wrapper updated.\n"
    for binary, alias in aliases.items():
        assert_wrapper(
            alias,
            journal=target_path,
            sol_bin=str(install_guard.expected_target(repo, binary)),
        )
    run_mock.assert_not_called()


def test_journal_rewrite_mid_failure_rolls_back_both_wrappers(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=False)
    target_path = str((tmp_path / "target").resolve())
    repo = tmp_path / "repo"
    aliases = make_managed_wrappers(home_root, repo, journal=str(source))
    before = {binary: alias.read_bytes() for binary, alias in aliases.items()}
    patch_service(monkeypatch, installed=False, running=False)
    real_replace = install_guard.os.replace

    def fail_second_replace(src: Path | str, dst: Path | str) -> None:
        if Path(dst).name == "journal":
            raise OSError("simulated replace failure")
        real_replace(src, dst)

    monkeypatch.setattr(install_guard.os, "replace", fail_second_replace)

    rc = config_cli.cmd_journal(target_path)
    captured = capsys.readouterr()

    assert rc == 1
    assert captured.out == ""
    assert "sol config: refused: cannot rewrite" in captured.err
    for binary, alias in aliases.items():
        assert alias.read_bytes() == before[binary]


@pytest.mark.parametrize(
    ("current_active", "target_active", "expected_flags"),
    [
        (False, True, "--switch, --merge, --force"),
        (True, False, "--move, --switch"),
        (True, True, "--switch, --merge, --force"),
    ],
)
def test_journal_refuses_without_flag_for_active_matrix_cells(
    home_root,
    monkeypatch,
    tmp_path,
    capsys,
    current_active,
    target_active,
    expected_flags,
):
    source = make_journal(tmp_path / "source", active=current_active)
    target_path = tmp_path / "target"
    if target_active:
        make_journal(target_path, active=True)
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)

    rc = config_cli.cmd_journal(str(target_path))
    captured = capsys.readouterr()

    assert rc == 1
    assert captured.out == ""
    assert f"current is {'active' if current_active else 'not active'}" in captured.err
    assert f"target is {'active' if target_active else 'not active'}" in captured.err
    assert f"valid flags: {expected_flags}" in captured.err
    if not target_active:
        assert not target_path.exists()


def test_journal_refuses_without_managed_wrapper(home_root, tmp_path, capsys):
    target_path = str((tmp_path / "journal").resolve())

    rc = config_cli.cmd_journal(target_path)
    captured = capsys.readouterr()

    assert rc == 1
    assert captured.out == ""
    assert "journal setup" in captured.err


def test_journal_refuses_legacy_symlink(home_root, tmp_path, capsys):
    target_path = str((tmp_path / "journal").resolve())
    make_alias(home_root, "/tmp/elsewhere/.venv/bin/sol")

    rc = config_cli.cmd_journal(target_path)
    captured = capsys.readouterr()

    assert rc == 1
    assert captured.out == ""
    assert "journal setup" in captured.err


def test_journal_refuses_invalid_chars(home_root, capsys):
    rc = config_cli.cmd_journal("/tmp/bad$path")
    captured = capsys.readouterr()

    assert rc == 1
    assert captured.out == ""
    assert "shell-active character '$'" in captured.err


def test_journal_refuses_source_tree_path_outside_source_checkout(
    home_root, monkeypatch, tmp_path, capsys
):
    monkeypatch.setattr(config_cli, "get_project_root", lambda: str(tmp_path))
    monkeypatch.setattr(config_cli, "is_source_checkout", lambda: False)

    rc = config_cli.cmd_journal(str((tmp_path / "journal").resolve()))
    captured = capsys.readouterr()

    assert rc == 1
    assert captured.out == ""
    assert "source-tree fallback path" in captured.err


def test_journal_exits_2_on_restart_failure(home_root, monkeypatch, tmp_path, capsys):
    source = make_journal(tmp_path / "source", active=False)
    target_path = str((tmp_path / "target").resolve())
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=True, running=True)
    monkeypatch.setattr(
        config_cli.subprocess,
        "run",
        service_run_mock(returncodes=[1]),
    )

    rc = config_cli.cmd_journal(target_path)
    captured = capsys.readouterr()

    assert rc == 2
    assert captured.out == ""
    assert "wrapper rewritten to" in captured.err
    assert_wrapper(alias, journal=target_path, sol_bin=str(target))


def test_journal_switch_without_yes_prints_plan(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=True)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)
    run_mock = service_run_mock()
    monkeypatch.setattr(config_cli.subprocess, "run", run_mock)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.SWITCH,
    )
    captured = capsys.readouterr()

    assert rc == 1
    assert "journal config journal - plan summary" in captured.out
    assert "action:  switch" in captured.out
    assert "re-run with --yes to proceed" in captured.out
    assert "current journal is left intact." in captured.out
    assert not target_path.exists()
    assert_wrapper(alias, journal=str(source), sol_bin=str(target))
    run_mock.assert_not_called()


def test_journal_switch_dry_run_prints_plan(home_root, monkeypatch, tmp_path, capsys):
    source = make_journal(tmp_path / "source", active=True)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.SWITCH,
        dry_run=True,
    )
    captured = capsys.readouterr()

    assert rc == 0
    assert "dry-run: yes; nothing will be changed" in captured.out
    assert not target_path.exists()
    assert_wrapper(alias, journal=str(source), sol_bin=str(target))


def test_journal_switch_active_to_not_active_updates_wrapper(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=True)
    target_path = tmp_path / "target"
    make_journal(target_path, active=False)
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.SWITCH,
        yes=True,
    )
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.out == "service not installed; wrapper updated.\n"
    assert source.exists()
    assert target_path.exists()
    assert_wrapper(alias, journal=str(target_path.resolve()), sol_bin=str(target))


def test_journal_switch_active_to_active_updates_wrapper(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=True)
    target_path = make_journal(tmp_path / "target", active=True)
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.SWITCH,
        yes=True,
    )
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.out == "service not installed; wrapper updated.\n"
    assert source.exists()
    assert target_path.exists()
    assert_wrapper(alias, journal=str(target_path.resolve()), sol_bin=str(target))


@pytest.mark.parametrize(
    ("current_active", "target_active"),
    [(False, False), (False, True), (True, False), (True, True)],
)
def test_journal_merge_always_refuses_with_instructions(
    home_root,
    monkeypatch,
    tmp_path,
    capsys,
    current_active,
    target_active,
):
    source = make_journal(tmp_path / "source", active=current_active)
    target_path = tmp_path / "target"
    if target_active:
        make_journal(target_path, active=True)
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=True, running=True)
    run_mock = service_run_mock()
    monkeypatch.setattr(config_cli.subprocess, "run", run_mock)
    original = alias.read_text(encoding="utf-8")

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.MERGE,
    )
    captured = capsys.readouterr()

    assert rc == 1
    assert captured.err == ""
    assert captured.out == config_cli.MERGE_INSTRUCTIONS + "\n"
    assert target_path.exists() is target_active
    assert alias.read_text(encoding="utf-8") == original
    run_mock.assert_not_called()


def test_journal_force_switches_active_target_without_yes(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=False)
    target_path = make_journal(tmp_path / "target", active=True)
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.FORCE,
    )
    captured = capsys.readouterr()

    assert rc == 0
    assert (
        "warning: --force bypasses confirmation and target activity checks"
        in captured.err
    )
    assert captured.out.endswith("service not installed; wrapper updated.\n")
    assert_wrapper(alias, journal=str(target_path.resolve()), sol_bin=str(target))


def test_journal_proceed_installed_not_running_does_not_restart(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=False)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=True, running=False)
    run_mock = service_run_mock()
    monkeypatch.setattr(config_cli.subprocess, "run", run_mock)

    rc = config_cli.cmd_journal(str(target_path))
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.out == "service installed but not running; wrapper updated.\n"
    run_mock.assert_not_called()


def test_journal_proceed_installed_running_restarts(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=False)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=True, running=True)
    run_mock = service_run_mock(returncodes=[0])
    monkeypatch.setattr(config_cli.subprocess, "run", run_mock)

    rc = config_cli.cmd_journal(str(target_path))
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.out == "wrapper updated; service restarted.\n"
    run_mock.assert_called_once_with(
        [str(target.with_name("journal")), "service", "restart", "--if-installed"],
        check=False,
    )


def test_journal_move_happy_path(home_root, monkeypatch, tmp_path, capsys):
    source = make_journal(tmp_path / "source", active=True)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)
    run_mock = service_run_mock()
    monkeypatch.setattr(config_cli.subprocess, "run", run_mock)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.MOVE,
        yes=True,
    )
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.out == "service not installed; journal moved; wrapper updated.\n"
    assert not source.exists()
    assert target_path.exists()
    assert_wrapper(alias, journal=str(target_path.resolve()), sol_bin=str(target))
    run_mock.assert_not_called()


def test_journal_move_cross_filesystem_refuses(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=True)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)
    original_build_change = config_cli.build_change

    def fake_build_change(*args, **kwargs):
        change = original_build_change(*args, **kwargs)
        return replace(
            change, same_filesystem=False, current_device=1, target_parent_device=2
        )

    monkeypatch.setattr(config_cli, "build_change", fake_build_change)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.MOVE,
        yes=True,
    )
    captured = capsys.readouterr()

    assert rc == 1
    assert "cannot move across filesystems" in captured.err
    assert "device=1" in captured.err
    assert "device=2" in captured.err
    assert "sol call journal merge" in captured.err


def test_journal_move_target_exists_refuses(home_root, monkeypatch, tmp_path, capsys):
    source = make_journal(tmp_path / "source", active=True)
    target_path = make_journal(tmp_path / "target", active=False)
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.MOVE,
        yes=True,
    )
    captured = capsys.readouterr()

    assert rc == 1
    assert "move target already exists" in captured.err


def test_journal_move_missing_current_refuses(home_root, monkeypatch, tmp_path, capsys):
    source_path = tmp_path / "missing-source"
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=str(source_path), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.MOVE,
        yes=True,
    )
    captured = capsys.readouterr()

    assert rc == 1
    assert "move source does not exist" in captured.err


def test_journal_move_target_parent_missing_refuses(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=True)
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    missing_target = tmp_path / "missing-parent" / "target"

    rc = config_cli.cmd_journal(
        str(missing_target),
        action=config_cli.RequestedAction.MOVE,
        yes=True,
    )
    captured = capsys.readouterr()

    assert rc == 1
    assert "move target parent does not exist" in captured.err


def test_journal_move_without_yes_prints_plan(home_root, monkeypatch, tmp_path, capsys):
    source = make_journal(tmp_path / "source", active=True)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.MOVE,
    )
    captured = capsys.readouterr()

    assert rc == 1
    assert "journal config journal - plan summary" in captured.out
    assert "action:  move" in captured.out
    assert "filesystem: same device" in captured.out
    assert "re-run with --yes to proceed" in captured.out
    assert source.exists()
    assert not target_path.exists()
    assert_wrapper(alias, journal=str(source), sol_bin=str(target))


def test_journal_move_dry_run_prints_plan(home_root, monkeypatch, tmp_path, capsys):
    source = make_journal(tmp_path / "source", active=True)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.MOVE,
        dry_run=True,
    )
    captured = capsys.readouterr()

    assert rc == 0
    assert "dry-run: yes; nothing will be changed" in captured.out
    assert source.exists()
    assert not target_path.exists()
    assert_wrapper(alias, journal=str(source), sol_bin=str(target))


def test_journal_move_rolls_back_on_wrapper_write_failure(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=True)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)
    monkeypatch.setattr(
        config_cli,
        "install_wrappers",
        MagicMock(side_effect=OSError("disk full")),
    )

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.MOVE,
        yes=True,
    )
    captured = capsys.readouterr()

    assert rc == 2
    assert "restored original journal" in captured.err
    assert source.exists()
    assert not target_path.exists()
    assert_wrapper(alias, journal=str(source), sol_bin=str(target))


def test_journal_move_exits_2_when_service_start_fails(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=True)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    alias = make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=True, running=True)
    run_mock = service_run_mock(returncodes=[0, 1])
    monkeypatch.setattr(config_cli.subprocess, "run", run_mock)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.MOVE,
        yes=True,
    )
    captured = capsys.readouterr()

    assert rc == 2
    assert (
        captured.err
        == f"wrapper updated to {target_path.resolve()} but service start failed; restart manually\n"
    )
    assert not source.exists()
    assert target_path.exists()
    assert_wrapper(alias, journal=str(target_path.resolve()), sol_bin=str(target))


def test_journal_switch_service_not_installed_does_not_restart(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=True)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=False, running=False)
    run_mock = service_run_mock()
    monkeypatch.setattr(config_cli.subprocess, "run", run_mock)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.SWITCH,
        yes=True,
    )
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.out == "service not installed; wrapper updated.\n"
    run_mock.assert_not_called()


def test_journal_switch_service_installed_not_running_does_not_restart(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=True)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=True, running=False)
    run_mock = service_run_mock()
    monkeypatch.setattr(config_cli.subprocess, "run", run_mock)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.SWITCH,
        yes=True,
    )
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.out == "service installed but not running; wrapper updated.\n"
    run_mock.assert_not_called()


def test_journal_move_service_installed_not_running_does_not_touch_service(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=True)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=True, running=False)
    run_mock = service_run_mock()
    monkeypatch.setattr(config_cli.subprocess, "run", run_mock)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.MOVE,
        yes=True,
    )
    captured = capsys.readouterr()

    assert rc == 0
    assert (
        captured.out
        == "service installed but not running; journal moved; wrapper updated.\n"
    )
    run_mock.assert_not_called()


def test_journal_move_service_running_stops_and_starts(
    home_root, monkeypatch, tmp_path, capsys
):
    source = make_journal(tmp_path / "source", active=True)
    target_path = tmp_path / "target"
    target = ensure_expected_target(tmp_path / "repo")
    make_managed_wrapper(home_root, journal=str(source), sol_bin=str(target))
    patch_service(monkeypatch, installed=True, running=True)
    run_mock = service_run_mock(returncodes=[0, 0])
    monkeypatch.setattr(config_cli.subprocess, "run", run_mock)

    rc = config_cli.cmd_journal(
        str(target_path),
        action=config_cli.RequestedAction.MOVE,
        yes=True,
    )
    captured = capsys.readouterr()

    assert rc == 0
    assert captured.out == "journal moved; wrapper updated; service restarted.\n"
    assert [call.args[0] for call in run_mock.call_args_list] == [
        [str(target.with_name("journal")), "service", "stop"],
        [str(target.with_name("journal")), "service", "start"],
    ]
