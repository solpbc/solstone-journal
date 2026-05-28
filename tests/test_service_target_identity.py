# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import plistlib
import shlex
import sys
from pathlib import Path

import pytest

from solstone.think import install_guard, service

PLATFORMS = ("darwin", "linux")


def _touch(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.touch()


def _service_path(tmp_path: Path, platform: str) -> Path:
    if platform == "darwin":
        return tmp_path / "org.solpbc.solstone.plist"
    return tmp_path / "solstone.service"


def _write_service_definition(path: Path, platform: str, target: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if platform == "darwin":
        path.write_bytes(
            plistlib.dumps(
                {
                    "Label": service.SERVICE_LABEL,
                    "ProgramArguments": [target, "start", "5015"],
                }
            )
        )
        return

    path.write_text(
        "[Unit]\n"
        "Description=Solstone Supervisor\n"
        "[Service]\n"
        f"ExecStart={shlex.join([target, 'start', '5015'])}\n",
        encoding="utf-8",
    )


def _patch_platform(
    monkeypatch: pytest.MonkeyPatch,
    *,
    platform: str,
    service_path: Path,
    executable: Path,
) -> None:
    monkeypatch.setattr(sys, "platform", platform)
    monkeypatch.setattr(sys, "executable", str(executable))
    if platform == "darwin":
        monkeypatch.setattr(service, "_plist_path", lambda: service_path)
    else:
        monkeypatch.setattr(service, "_unit_path", lambda: service_path)


@pytest.mark.parametrize("platform", PLATFORMS)
def test_target_matches_via_wrapper(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, platform: str
) -> None:
    install_bin = tmp_path / "install" / "bin"
    install_bin.mkdir(parents=True)
    sol_bin = install_bin / "journal"
    _touch(sol_bin)
    wrapper = tmp_path / "local" / "bin" / "journal"
    wrapper.parent.mkdir(parents=True)
    wrapper.write_text(
        install_guard.render_wrapper(
            str(tmp_path / "journal"), str(sol_bin), "journal"
        ),
        encoding="utf-8",
    )
    service_path = _service_path(tmp_path, platform)
    _write_service_definition(service_path, platform, str(wrapper))
    _patch_platform(
        monkeypatch,
        platform=platform,
        service_path=service_path,
        executable=install_bin / "python",
    )

    result = service.check_service_target_identity()

    assert result.installed
    assert result.matches_current_install
    assert result.resolved_target == str(sol_bin.resolve())
    assert result.target == str(wrapper)


@pytest.mark.parametrize("platform", PLATFORMS)
def test_target_mismatch_via_wrapper(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, platform: str
) -> None:
    install_bin = tmp_path / "install" / "bin"
    install_bin.mkdir(parents=True)
    other_bin = tmp_path / "other" / "bin"
    other_bin.mkdir(parents=True)
    sol_bin = other_bin / "journal"
    _touch(sol_bin)
    wrapper = tmp_path / "local" / "bin" / "journal"
    wrapper.parent.mkdir(parents=True)
    wrapper.write_text(
        install_guard.render_wrapper(
            str(tmp_path / "journal"), str(sol_bin), "journal"
        ),
        encoding="utf-8",
    )
    service_path = _service_path(tmp_path, platform)
    _write_service_definition(service_path, platform, str(wrapper))
    _patch_platform(
        monkeypatch,
        platform=platform,
        service_path=service_path,
        executable=install_bin / "python",
    )
    expected = (install_bin / "journal").resolve()

    result = service.check_service_target_identity()

    assert result.installed
    assert not result.matches_current_install
    assert str(sol_bin.resolve()) in result.detail
    assert str(expected) in result.detail


@pytest.mark.parametrize("platform", PLATFORMS)
def test_target_matches_via_symlink(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, platform: str
) -> None:
    install_bin = tmp_path / "install" / "bin"
    install_bin.mkdir(parents=True)
    sol_bin = install_bin / "journal"
    _touch(sol_bin)
    wrapper = tmp_path / "local" / "bin" / "journal"
    wrapper.parent.mkdir(parents=True)
    wrapper.symlink_to(sol_bin)
    service_path = _service_path(tmp_path, platform)
    _write_service_definition(service_path, platform, str(wrapper))
    _patch_platform(
        monkeypatch,
        platform=platform,
        service_path=service_path,
        executable=install_bin / "python",
    )

    result = service.check_service_target_identity()

    assert result.installed
    assert result.matches_current_install
    assert result.resolved_target == str(sol_bin.resolve())


@pytest.mark.parametrize("platform", PLATFORMS)
def test_not_installed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, platform: str
) -> None:
    install_bin = tmp_path / "install" / "bin"
    service_path = _service_path(tmp_path, platform)
    _patch_platform(
        monkeypatch,
        platform=platform,
        service_path=service_path,
        executable=install_bin / "python",
    )

    result = service.check_service_target_identity()

    assert not result.installed
    assert result.target == ""
    assert result.resolved_target == ""
    assert not result.matches_current_install
    assert result.detail == "service not installed"


def test_malformed_unit_darwin(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    install_bin = tmp_path / "install" / "bin"
    plist_path = tmp_path / "org.solpbc.solstone.plist"
    plist_path.write_bytes(plistlib.dumps({"Label": service.SERVICE_LABEL}))
    _patch_platform(
        monkeypatch,
        platform="darwin",
        service_path=plist_path,
        executable=install_bin / "python",
    )

    result = service.check_service_target_identity()

    assert result.installed
    assert result.target == ""
    assert not result.matches_current_install
    assert str(plist_path) in result.detail


def test_malformed_unit_linux(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    install_bin = tmp_path / "install" / "bin"
    unit_path = tmp_path / "solstone.service"
    unit_path.write_text('[Service]\nExecStart="unterminated\n', encoding="utf-8")
    _patch_platform(
        monkeypatch,
        platform="linux",
        service_path=unit_path,
        executable=install_bin / "python",
    )

    result = service.check_service_target_identity()

    assert result.installed
    assert result.target == ""
    assert not result.matches_current_install
    assert str(unit_path) in result.detail
