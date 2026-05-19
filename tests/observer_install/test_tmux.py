# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import subprocess

import pytest

from solstone.apps.observer.utils import save_observer
from solstone.observe.observer_install import common, tmux


def test_tmux_happy_path_writes_config_and_marker(monkeypatch, args_factory):
    steps = _install_success(monkeypatch)

    assert tmux.TmuxDriver().run(args_factory(platform="tmux")) == 0

    assert (
        "install solstone-tmux==0.1.0",
        ["pipx", "install", "--force", "solstone-tmux==0.1.0"],
    ) in steps
    assert all("--system-site-packages" not in cmd for _label, cmd in steps)
    assert (
        "run solstone-tmux install-service",
        ["solstone-tmux", "install-service"],
    ) in steps
    config = json.loads(tmux.CONFIG_PATH.read_text(encoding="utf-8"))
    assert config["stream"] == "archon"
    assert config["status_indicator"] is True
    marker = common.read_marker(tmux.INSTALL_NAME)
    assert marker["source"] == "pypi:solstone-tmux"
    assert marker["version"] == "0.1.0"


def test_tmux_missing_tmux_warns_but_continues(monkeypatch, args_factory, capsys):
    _install_success(monkeypatch, tmux_present=False)

    assert tmux.TmuxDriver().run(args_factory(platform="tmux")) == 0

    assert "warning: tmux not detected on PATH" in capsys.readouterr().out


def test_tmux_marker_present_matching_version_is_noop(
    monkeypatch, args_factory, capsys
):
    _seed_matching_install()
    steps = _install_success(monkeypatch, service_active=True)

    assert tmux.TmuxDriver().run(args_factory(platform="tmux")) == 0

    assert all(cmd[0] != "pipx" for _label, cmd in steps)
    assert all(cmd != ["solstone-tmux", "install-service"] for _label, cmd in steps)
    assert "already installed" in capsys.readouterr().out


def test_tmux_second_run_after_install_is_noop(monkeypatch, args_factory, capsys):
    steps = _install_success(monkeypatch, service_active=True)
    config_writes = 0
    marker_writes = 0
    original_write_config = tmux._write_config
    original_write_marker = tmux.write_marker

    def count_config_write(server_url, key, name):
        nonlocal config_writes
        config_writes += 1
        original_write_config(server_url, key, name)

    def count_marker_write(install_name, data):
        nonlocal marker_writes
        marker_writes += 1
        original_write_marker(install_name, data)

    monkeypatch.setattr(tmux, "_write_config", count_config_write)
    monkeypatch.setattr(tmux, "write_marker", count_marker_write)

    assert tmux.TmuxDriver().run(args_factory(platform="tmux")) == 0
    assert tmux.TmuxDriver().run(args_factory(platform="tmux")) == 0

    assert _count_step(steps, ["pipx", "install"]) == 1
    assert _count_step(steps, ["solstone-tmux", "install-service"]) == 1
    assert config_writes == 1
    assert marker_writes == 1
    assert "already installed" in capsys.readouterr().out


def test_tmux_force_bypasses_short_circuit_and_runs_pipx(monkeypatch, args_factory):
    _seed_matching_install()
    steps = _install_success(monkeypatch, service_active=True)

    assert tmux.TmuxDriver().run(args_factory(platform="tmux", force=True)) == 0

    assert _count_step(steps, ["pipx", "install"]) == 1
    assert _count_step(steps, ["solstone-tmux", "install-service"]) == 1


def test_tmux_observer_version_flag_threads_to_pipx(monkeypatch, args_factory):
    steps = _install_success(monkeypatch)

    assert (
        tmux.TmuxDriver().run(args_factory(platform="tmux", observer_version="9.9.9"))
        == 0
    )

    assert any(cmd[-1] == "solstone-tmux==9.9.9" for _label, cmd in steps)
    assert common.read_marker(tmux.INSTALL_NAME)["version"] == "9.9.9"


def test_tmux_stale_git_sha_marker_triggers_reinstall(monkeypatch, args_factory):
    _seed_matching_install(
        source="https://github.com/solpbc/solstone-tmux.git",
        version="abc123def456abc123def456abc123def456abc1",
    )
    steps = _install_success(monkeypatch, service_active=True)

    assert tmux.TmuxDriver().run(args_factory(platform="tmux")) == 0

    assert _count_step(steps, ["pipx", "install"]) == 1
    marker = common.read_marker(tmux.INSTALL_NAME)
    assert marker["source"] == "pypi:solstone-tmux"
    assert marker["version"] == "0.1.0"


def test_tmux_pipx_failure_raises_with_hint_and_no_marker(monkeypatch, args_factory):
    _install_success(monkeypatch)

    def fail_pipx(label, cmd, **kwargs):
        if cmd[0] == "pipx":
            raise common.InstallError("pipx failed")
        return common.StepResult(subprocess.CompletedProcess(cmd, 0, "", ""))

    monkeypatch.setattr(common, "run_step", fail_pipx)

    with pytest.raises(common.InstallError) as exc_info:
        tmux.TmuxDriver().run(args_factory(platform="tmux"))

    assert "pipx install --force solstone-tmux==0.1.0" in exc_info.value.hint
    assert common.read_marker(tmux.INSTALL_NAME) is None


def test_tmux_install_service_failure_after_pipx_preserves_prior_marker(
    monkeypatch, args_factory
):
    common.write_marker(
        tmux.INSTALL_NAME,
        {
            "name": "archon",
            "platform": "tmux",
            "source": "pypi:solstone-tmux",
            "installed_at": "2026-05-02T00:00:00Z",
            "last_run": "2026-05-02T00:00:00Z",
            "version": "0.0.9",
        },
    )
    _install_success(monkeypatch)

    def fail_install_service(label, cmd, **kwargs):
        if cmd == ["solstone-tmux", "install-service"]:
            raise common.InstallError("install-service failed")
        return common.StepResult(subprocess.CompletedProcess(cmd, 0, "", ""))

    monkeypatch.setattr(tmux, "run_step", fail_install_service)

    with pytest.raises(common.InstallError) as exc_info:
        tmux.TmuxDriver().run(args_factory(platform="tmux"))

    assert (
        "install-service failed after pipx placed the script on PATH"
        in exc_info.value.hint
    )
    assert common.read_marker(tmux.INSTALL_NAME)["version"] == "0.0.9"


def test_tmux_path_missing_after_pipx_raises_with_ensurepath_hint(
    monkeypatch, args_factory
):
    steps = _install_success(monkeypatch, command_on_path=False)

    with pytest.raises(common.InstallError) as exc_info:
        tmux.TmuxDriver().run(args_factory(platform="tmux"))

    assert "pipx ensurepath" in exc_info.value.hint
    assert "restart your shell" in exc_info.value.hint
    assert _count_step(steps, ["pipx", "install"]) == 1
    assert _count_step(steps, ["solstone-tmux", "install-service"]) == 0


def _install_success(
    monkeypatch,
    *,
    service_active: bool = False,
    command_on_path: bool = True,
    tmux_present: bool = True,
) -> list[tuple[str, list[str]]]:
    monkeypatch.setattr(tmux, "poll_status_until", lambda name: "connected")
    monkeypatch.setattr(
        tmux.shutil,
        "which",
        lambda name: f"/home/test/.local/bin/{name}" if command_on_path else None,
    )

    def fake_probe(cmd, *, cwd=None):
        text = " ".join(cmd)
        if text == "sh -c command -v tmux":
            code = 0 if tmux_present else 1
            return subprocess.CompletedProcess(
                cmd, code, "ok\n" if tmux_present else "", ""
            )
        if text == f"systemctl --user is-active {tmux.UNIT_NAME}":
            stdout = "active\n" if service_active else "inactive\n"
            return subprocess.CompletedProcess(cmd, 0, stdout, "")
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    steps: list[tuple[str, list[str]]] = []

    def fake_step(label, cmd, **kwargs):
        steps.append((label, cmd))
        return common.StepResult(subprocess.CompletedProcess(cmd, 0, "", ""))

    monkeypatch.setattr(tmux, "run_probe", fake_probe)
    monkeypatch.setattr(common, "run_step", fake_step)
    monkeypatch.setattr(tmux, "run_step", fake_step)
    return steps


def _seed_matching_install(
    *,
    source: str = "pypi:solstone-tmux",
    version: str = "0.1.0",
) -> None:
    common.write_marker(
        tmux.INSTALL_NAME,
        {
            "name": "archon",
            "platform": "tmux",
            "source": source,
            "installed_at": "2026-05-02T00:00:00Z",
            "last_run": "2026-05-02T00:00:00Z",
            "version": version,
        },
    )
    save_observer(
        {
            "key": "abcdefgh",
            "name": "archon",
            "created_at": None,
            "last_seen": None,
            "last_segment": None,
            "enabled": True,
            "stats": {"segments_received": 0, "bytes_received": 0},
        }
    )
    tmux.CONFIG_PATH.parent.mkdir(parents=True)
    tmux.CONFIG_PATH.write_text('{"key": "abcdefgh"}\n', encoding="utf-8")


def _count_step(steps: list[tuple[str, list[str]]], prefix: list[str]) -> int:
    return sum(1 for _label, cmd in steps if cmd[: len(prefix)] == prefix)
