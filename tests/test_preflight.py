# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

ROOT = Path(__file__).resolve().parent.parent


@pytest.fixture
def preflight():
    from solstone.think import preflight as preflight_module

    yield preflight_module


@pytest.fixture
def probe():
    from solstone.think import probe as probe_module

    yield probe_module


@pytest.fixture
def home_root(monkeypatch, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    return home


def args(preflight):
    return preflight.Args(verbose=False, json=False)


def make_repo(tmp_path: Path, *, with_venv: bool = False) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".git").mkdir()
    (repo / "pyproject.toml").write_text(
        '[project]\nrequires-python = ">=3.12"\n',
        encoding="utf-8",
    )
    if with_venv:
        python_bin = repo / ".venv" / "bin" / "python"
        python_bin.parent.mkdir(parents=True)
        python_bin.write_text("", encoding="utf-8")
    return repo


def fake_probe_dispatcher(probe, repo: Path):
    def run_probe(_check, cmd, **_kwargs):
        if list(cmd) == ["uv", "--version"]:
            return probe.ProbeOutput("uv 0.7.12\n", "", 0)
        if len(cmd) >= 3 and cmd[1:] == ["-c", "import sys; print(sys.prefix)"]:
            return probe.ProbeOutput(f"{repo / '.venv'}\n", "", 0)
        raise AssertionError(f"unexpected probe command: {cmd}")

    return run_probe


def patch_green_environment(probe, monkeypatch, home_root, repo: Path) -> None:
    monkeypatch.setattr(probe, "ROOT", repo)
    monkeypatch.setattr(
        probe,
        "run_probe",
        fake_probe_dispatcher(probe, repo),
    )
    monkeypatch.setattr(
        probe.shutil,
        "disk_usage",
        lambda _root: SimpleNamespace(total=100, used=80, free=20 * 1024**3),
    )
    config_dir = home_root / ".config"
    config_dir.mkdir()


def test_main_json_passes_when_blockers_pass(
    preflight, probe, monkeypatch, tmp_path, home_root, capsys
):
    repo = make_repo(tmp_path, with_venv=True)
    patch_green_environment(probe, monkeypatch, home_root, repo)

    rc = preflight.main(["--json"])
    payload = json.loads(capsys.readouterr().out)

    assert rc == 0
    assert payload["summary"]["failed"] == 0
    assert isinstance(payload["checks"], list)
    assert isinstance(payload["summary"], dict)


def test_python_version_ok(preflight, probe, monkeypatch, tmp_path):
    repo = make_repo(tmp_path)
    monkeypatch.setattr(probe, "ROOT", repo)

    result = preflight.python_version_check(args(preflight))

    assert result.status == "ok"


def test_uv_installed_ok(preflight, probe, monkeypatch):
    monkeypatch.setattr(probe, "_is_source_checkout", lambda: True)
    monkeypatch.setattr(
        probe,
        "run_probe",
        lambda *_args, **_kwargs: probe.ProbeOutput("uv 0.10.0\n", "", 0),
    )

    result = preflight.uv_installed_check(args(preflight))

    assert result.status == "ok"


def test_venv_consistent_ok(preflight, probe, monkeypatch, tmp_path):
    repo = make_repo(tmp_path, with_venv=True)
    monkeypatch.setattr(probe, "ROOT", repo)
    monkeypatch.setattr(
        probe,
        "run_probe",
        lambda *_args, **_kwargs: probe.ProbeOutput(f"{repo / '.venv'}\n", "", 0),
    )

    result = preflight.venv_consistent_check(args(preflight))

    assert result.status == "ok"


def test_disk_space_ok(preflight, probe, monkeypatch):
    monkeypatch.setattr(
        probe.shutil,
        "disk_usage",
        lambda _root: SimpleNamespace(total=100, used=80, free=20 * 1024**3),
    )

    result = preflight.disk_space_check(args(preflight))

    assert result.status == "ok"


def test_config_dir_readable_ok(preflight, home_root):
    config_dir = home_root / ".config"
    config_dir.mkdir()

    result = preflight.config_dir_readable_check(args(preflight))

    assert result.status == "ok"


def test_local_bin_sol_reachable_ok(preflight, probe, monkeypatch, home_root):
    local = home_root / ".local" / "bin" / "sol"
    local.parent.mkdir(parents=True)
    local.write_text("#!/bin/sh\n", encoding="utf-8")
    monkeypatch.setattr(probe.shutil, "which", lambda name: str(local))

    result = preflight.local_bin_sol_reachable_check(args(preflight))

    assert result.status == "ok"


def test_uv_missing_fails(preflight, probe, monkeypatch):
    def raise_missing(*_args, **_kwargs):
        raise FileNotFoundError

    monkeypatch.setattr(probe, "_is_source_checkout", lambda: True)
    monkeypatch.setattr(probe.subprocess, "run", raise_missing)

    result = preflight.uv_installed_check(args(preflight))

    assert result.status == "fail"
    assert "probe command not found" in result.detail


def test_main_returns_one_when_uv_missing(
    preflight, probe, monkeypatch, tmp_path, home_root, capsys
):
    def raise_missing(*_args, **_kwargs):
        raise FileNotFoundError

    repo = make_repo(tmp_path)
    monkeypatch.setattr(probe, "ROOT", repo)
    monkeypatch.setattr(probe.subprocess, "run", raise_missing)
    monkeypatch.setattr(
        probe.shutil,
        "disk_usage",
        lambda _root: SimpleNamespace(total=100, used=80, free=20 * 1024**3),
    )
    (home_root / ".config").mkdir()

    rc = preflight.main(["--json"])
    payload = json.loads(capsys.readouterr().out)

    assert rc == 1
    assert payload["summary"]["failed"] >= 1


def stdlib_guard_code(import_body: str) -> str:
    return f"""
import builtins
_real = builtins.__import__
_blocked = {{"timefhuman", "numpy", "PIL", "flask"}}
def _guard(name, g=None, l=None, fromlist=(), level=0):
    if level == 0 and name.split(".", 1)[0] in _blocked:
        raise ModuleNotFoundError(f"No module named {{name.split('.', 1)[0]!r}}")
    return _real(name, g, l, fromlist, level)
builtins.__import__ = _guard
import sys
sys.path.insert(0, {str(ROOT)!r})
{import_body}
"""


def test_preflight_runs_under_stdlib_import_guard():
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            stdlib_guard_code(
                "from solstone.think import probe\n"
                "from solstone.think.preflight import main\n"
                'raise SystemExit(main(["--json"]))'
            ),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=15,
    )

    assert result.returncode in {0, 1}
    assert "ModuleNotFoundError" not in result.stderr
    assert "timefhuman" not in result.stderr


def test_doctor_fails_under_same_stdlib_import_guard():
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            stdlib_guard_code("from solstone.think.doctor import main\nprint(main)"),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=15,
    )

    assert result.returncode != 0
    assert "ModuleNotFoundError" in result.stderr
    assert "timefhuman" in result.stderr


def test_install_dry_run_runs_preflight_before_uv_sync():
    result = subprocess.run(
        ["make", "--dry-run", "-B", "install"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=15,
    )

    assert result.returncode == 0
    lines = result.stdout.splitlines()
    preflight_index = next(
        index
        for index, line in enumerate(lines)
        if "python3 scripts/preflight.py" in line
    )
    uv_sync_index = next(index for index, line in enumerate(lines) if "uv sync" in line)
    assert preflight_index < uv_sync_index
    assert any("python3 scripts/preflight.py" in line for line in lines)
    assert any("uv sync" in line for line in lines)
    assert all("python3 scripts/doctor.py" not in line for line in lines)
