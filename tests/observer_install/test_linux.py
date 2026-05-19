# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from solstone.apps.observer.utils import list_observers, save_observer
from solstone.observe.observer_install import common, linux


@pytest.mark.parametrize(
    ("content", "expected"),
    [
        ("ID=fedora\n", "fedora"),
        ("ID=ubuntu\n", "debian-ubuntu"),
        ("ID=arch\n", "arch"),
        ("ID=opensuse-tumbleweed\n", "opensuse"),
        ("ID=unknown\nID_LIKE=debian\n", "debian-ubuntu"),
    ],
)
def test_detect_distro_from_os_release(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, content: str, expected: str
):
    os_release = tmp_path / "os-release"
    os_release.write_text(content, encoding="utf-8")
    monkeypatch.setattr(linux, "OS_RELEASE_PATH", os_release)

    assert linux.detect_distro() == expected


def test_missing_system_package_reports_install_command(
    monkeypatch: pytest.MonkeyPatch, args_factory
):
    monkeypatch.setattr(linux, "detect_distro", lambda: "fedora")

    def fake_probe(cmd, *, cwd=None):
        text = " ".join(cmd)
        if text == "rpm -q gtk4":
            return subprocess.CompletedProcess(cmd, 1, "", "")
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    monkeypatch.setattr(linux, "run_probe", fake_probe)

    with pytest.raises(common.InstallError) as exc_info:
        linux.LinuxDriver().run(args_factory())

    assert "gtk4" in str(exc_info.value)
    assert (
        "sudo dnf install python3-gobject gtk4 gstreamer1-plugins-base"
        in exc_info.value.hint
    )


def test_happy_path_writes_config_and_marker(
    monkeypatch: pytest.MonkeyPatch, args_factory
):
    steps = _install_success(monkeypatch)

    assert linux.LinuxDriver().run(args_factory()) == 0

    assert (
        "install solstone-linux==0.1.0",
        [
            "pipx",
            "install",
            "--force",
            "--system-site-packages",
            "solstone-linux==0.1.0",
        ],
    ) in steps
    assert (
        "run solstone-linux install-service",
        ["solstone-linux", "install-service"],
    ) in steps
    config = json.loads(linux.CONFIG_PATH.read_text(encoding="utf-8"))
    assert config["server_url"] == "http://127.0.0.1:5015"
    assert config["stream"] == "archon"
    assert config["key"]
    marker = common.read_marker(linux.INSTALL_NAME)
    assert marker["name"] == "archon"
    assert marker["source"] == "pypi:solstone-linux"
    assert marker["version"] == "0.1.0"
    assert isinstance(marker["version"], str) and marker["version"]


def test_marker_present_matching_version_is_noop(
    monkeypatch: pytest.MonkeyPatch, args_factory, capsys
):
    _seed_matching_install()
    steps = _install_success(monkeypatch, service_active=True)

    assert linux.LinuxDriver().run(args_factory()) == 0

    assert all(cmd[0] != "pipx" for _label, cmd in steps)
    assert all(cmd != ["solstone-linux", "install-service"] for _label, cmd in steps)
    assert "already installed" in capsys.readouterr().out
    assert common.read_marker(linux.INSTALL_NAME)["last_run"] == "2026-05-02T00:00:00Z"


def test_second_run_after_install_is_noop(
    monkeypatch: pytest.MonkeyPatch, args_factory, capsys
):
    steps = _install_success(monkeypatch, service_active=True)
    config_writes = 0
    marker_writes = 0
    original_write_config = linux._write_config
    original_write_marker = linux.write_marker

    def count_config_write(server_url, key, name):
        nonlocal config_writes
        config_writes += 1
        original_write_config(server_url, key, name)

    def count_marker_write(install_name, data):
        nonlocal marker_writes
        marker_writes += 1
        original_write_marker(install_name, data)

    monkeypatch.setattr(linux, "_write_config", count_config_write)
    monkeypatch.setattr(linux, "write_marker", count_marker_write)

    assert linux.LinuxDriver().run(args_factory()) == 0
    assert linux.LinuxDriver().run(args_factory()) == 0

    assert _count_step(steps, ["pipx", "install"]) == 1
    assert _count_step(steps, ["solstone-linux", "install-service"]) == 1
    assert config_writes == 1
    assert marker_writes == 1
    assert "already installed" in capsys.readouterr().out


def test_force_bypasses_short_circuit_and_runs_pipx(
    monkeypatch: pytest.MonkeyPatch, args_factory
):
    _seed_matching_install()
    steps = _install_success(monkeypatch, service_active=True)

    assert linux.LinuxDriver().run(args_factory(force=True)) == 0

    assert _count_step(steps, ["pipx", "install"]) == 1
    assert _count_step(steps, ["solstone-linux", "install-service"]) == 1


def test_observer_version_flag_threads_to_pipx(
    monkeypatch: pytest.MonkeyPatch, args_factory
):
    steps = _install_success(monkeypatch)

    assert linux.LinuxDriver().run(args_factory(observer_version="9.9.9")) == 0

    assert any(cmd[-1] == "solstone-linux==9.9.9" for _label, cmd in steps)
    assert common.read_marker(linux.INSTALL_NAME)["version"] == "9.9.9"


def test_stale_git_sha_marker_triggers_reinstall(
    monkeypatch: pytest.MonkeyPatch, args_factory
):
    _seed_matching_install(
        source="https://github.com/solpbc/solstone-linux.git",
        version="abc123def456abc123def456abc123def456abc1",
    )
    steps = _install_success(monkeypatch, service_active=True)

    assert linux.LinuxDriver().run(args_factory()) == 0

    assert _count_step(steps, ["pipx", "install"]) == 1
    marker = common.read_marker(linux.INSTALL_NAME)
    assert marker["source"] == "pypi:solstone-linux"
    assert marker["version"] == "0.1.0"


def test_pipx_failure_raises_with_hint_and_no_marker(
    monkeypatch: pytest.MonkeyPatch, args_factory
):
    _install_success(monkeypatch)

    def fail_pipx(label, cmd, **kwargs):
        if cmd[0] == "pipx":
            raise common.InstallError("pipx failed")
        return common.StepResult(subprocess.CompletedProcess(cmd, 0, "", ""))

    monkeypatch.setattr(common, "run_step", fail_pipx)

    with pytest.raises(common.InstallError) as exc_info:
        linux.LinuxDriver().run(args_factory())

    assert "pipx install --force solstone-linux==0.1.0" in exc_info.value.hint
    assert common.read_marker(linux.INSTALL_NAME) is None


def test_install_service_failure_after_pipx_preserves_prior_marker(
    monkeypatch: pytest.MonkeyPatch, args_factory
):
    common.write_marker(
        linux.INSTALL_NAME,
        {
            "name": "archon",
            "platform": "linux",
            "source": "pypi:solstone-linux",
            "installed_at": "2026-05-02T00:00:00Z",
            "last_run": "2026-05-02T00:00:00Z",
            "version": "0.0.9",
        },
    )
    _install_success(monkeypatch)

    def fail_install_service(label, cmd, **kwargs):
        if cmd == ["solstone-linux", "install-service"]:
            raise common.InstallError("install-service failed")
        return common.StepResult(subprocess.CompletedProcess(cmd, 0, "", ""))

    monkeypatch.setattr(linux, "run_step", fail_install_service)

    with pytest.raises(common.InstallError) as exc_info:
        linux.LinuxDriver().run(args_factory())

    assert (
        "install-service failed after pipx placed the script on PATH"
        in exc_info.value.hint
    )
    assert common.read_marker(linux.INSTALL_NAME)["version"] == "0.0.9"


def test_path_missing_after_pipx_raises_with_ensurepath_hint(
    monkeypatch: pytest.MonkeyPatch, args_factory
):
    steps = _install_success(monkeypatch, command_on_path=False)

    with pytest.raises(common.InstallError) as exc_info:
        linux.LinuxDriver().run(args_factory())

    assert "pipx ensurepath" in exc_info.value.hint
    assert "restart your shell" in exc_info.value.hint
    assert _count_step(steps, ["pipx", "install"]) == 1
    assert _count_step(steps, ["solstone-linux", "install-service"]) == 0


def test_force_revokes_and_recreates_registration(monkeypatch: pytest.MonkeyPatch):
    save_observer(
        {
            "key": "old-key",
            "name": "archon",
            "created_at": 1,
            "last_seen": None,
            "last_segment": None,
            "enabled": True,
            "stats": {"segments_received": 0, "bytes_received": 0},
        }
    )
    monkeypatch.setattr(
        "solstone.observe.observer_cli._generate_key", lambda: "new-key"
    )

    result = common.create_or_reuse_registration("archon", force=True)

    observers = list_observers()
    assert result.key == "new-key"
    assert any(
        observer.get("key") == "old-key" and observer.get("revoked")
        for observer in observers
    )
    assert any(
        observer.get("key") == "new-key" and not observer.get("revoked")
        for observer in observers
    )


def _install_success(
    monkeypatch: pytest.MonkeyPatch,
    *,
    service_active: bool = False,
    command_on_path: bool = True,
) -> list[tuple[str, list[str]]]:
    monkeypatch.setattr(linux, "detect_distro", lambda: "fedora")
    monkeypatch.setattr(linux, "poll_status_until", lambda name: "connected")
    monkeypatch.setattr(
        linux.shutil,
        "which",
        lambda name: f"/home/test/.local/bin/{name}" if command_on_path else None,
    )

    def fake_probe(cmd, *, cwd=None):
        text = " ".join(cmd)
        if text == f"systemctl --user is-active {linux.UNIT_NAME}":
            stdout = "active\n" if service_active else "inactive\n"
            return subprocess.CompletedProcess(cmd, 0, stdout, "")
        return subprocess.CompletedProcess(cmd, 0, "ok\n", "")

    steps: list[tuple[str, list[str]]] = []

    def fake_step(label, cmd, **kwargs):
        steps.append((label, cmd))
        return common.StepResult(subprocess.CompletedProcess(cmd, 0, "", ""))

    monkeypatch.setattr(linux, "run_probe", fake_probe)
    monkeypatch.setattr(common, "run_step", fake_step)
    monkeypatch.setattr(linux, "run_step", fake_step)
    return steps


def _seed_matching_install(
    *,
    source: str = "pypi:solstone-linux",
    version: str = "0.1.0",
) -> None:
    common.write_marker(
        linux.INSTALL_NAME,
        {
            "name": "archon",
            "platform": "linux",
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
    linux.CONFIG_PATH.parent.mkdir(parents=True)
    linux.CONFIG_PATH.write_text('{"key": "abcdefgh"}\n', encoding="utf-8")


def _count_step(steps: list[tuple[str, list[str]]], prefix: list[str]) -> int:
    return sum(1 for _label, cmd in steps if cmd[: len(prefix)] == prefix)
