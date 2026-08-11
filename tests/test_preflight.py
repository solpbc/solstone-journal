# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

import pytest

from solstone.think.probe import ExecutionError
from tests.helpers.module_mocks import module_mock


@pytest.fixture
def preflight():
    from solstone.think import preflight as preflight_module

    yield preflight_module


@pytest.fixture
def probe(monkeypatch):
    from solstone.think import probe as probe_module

    monkeypatch.setattr(
        probe_module,
        "subprocess",
        module_mock(probe_module.subprocess),
    )
    monkeypatch.setattr(
        probe_module,
        "shutil",
        module_mock(probe_module.shutil),
    )
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
        if list(cmd) == ["cargo", "--version"]:
            return probe.ProbeOutput("cargo 1.87.0\n", "", 0)
        if list(cmd) == ["rustc", "--version"]:
            return probe.ProbeOutput("rustc 1.87.0\n", "", 0)
        if len(cmd) >= 3 and cmd[1:] == ["-c", "import sys; print(sys.prefix)"]:
            return probe.ProbeOutput(f"{repo / '.venv'}\n", "", 0)
        raise AssertionError(f"unexpected probe command: {cmd}")

    return run_probe


def patch_green_environment(probe, monkeypatch, home_root, repo: Path) -> None:
    monkeypatch.setattr(probe, "ROOT", repo)
    monkeypatch.setattr(
        probe,
        "run_probe",
        Mock(side_effect=fake_probe_dispatcher(probe, repo)),
    )
    monkeypatch.setattr(
        probe.shutil,
        "disk_usage",
        Mock(return_value=SimpleNamespace(total=100, used=80, free=20 * 1024**3)),
    )
    real_which = probe.shutil.which
    monkeypatch.setattr(
        probe.shutil,
        "which",
        lambda name: "/usr/bin/nasm" if name == "nasm" else real_which(name),
    )
    monkeypatch.setattr(
        probe,
        "clang_builtin_include_dir",
        lambda: Path("/usr/lib/clang/18/include"),
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
    assert payload["summary"]["errors"] == 0
    assert list(payload["summary"]) == [
        "total",
        "failed",
        "warnings",
        "skipped",
        "errors",
    ]
    assert isinstance(payload["checks"], list)
    assert all(check["execution_error"] is None for check in payload["checks"])
    assert isinstance(payload["summary"], dict)


def test_main_isolates_execution_error_and_continues(preflight, monkeypatch, capsys):
    before_check = preflight.Check("before_check", "blocker", ("linux", "darwin"))
    raising_check = preflight.Check("raising_check", "advisory", ("linux", "darwin"))
    after_check = preflight.Check("after_check", "blocker", ("linux", "darwin"))
    calls: list[str] = []

    def before(_args):
        calls.append("before")
        return preflight.make_result(before_check, "ok", "before complete")

    def raising(_args):
        calls.append("raising")
        raise RuntimeError("boom")

    def after(_args):
        calls.append("after")
        return preflight.make_result(after_check, "ok", "after complete")

    monkeypatch.setattr(
        preflight,
        "CHECKS",
        [
            (before_check, before),
            (raising_check, raising),
            (after_check, after),
        ],
    )

    results = preflight.run_checks(args(preflight))

    assert [result.name for result in results] == [
        "before_check",
        "raising_check",
        "after_check",
    ]
    assert calls == ["before", "raising", "after"]
    assert results[1].execution_error == ExecutionError("RuntimeError", "boom")

    calls.clear()
    rc = preflight.main([])
    output = capsys.readouterr().out

    assert rc == 1
    assert calls == ["before", "raising", "after"]
    assert "ERROR raising_check" in output
    assert output.rstrip().endswith(
        "preflight: 3 checks, 1 failed, 0 warnings, 0 skipped, 1 errors"
    )


def test_main_json_carries_structured_execution_error(preflight, monkeypatch, capsys):
    before_check = preflight.Check("before_check", "blocker", ("linux", "darwin"))
    raising_check = preflight.Check("raising_check", "advisory", ("linux", "darwin"))
    after_check = preflight.Check("after_check", "blocker", ("linux", "darwin"))

    def before(_args):
        return preflight.make_result(before_check, "ok", "before complete")

    def raising(_args):
        raise RuntimeError("boom")

    def after(_args):
        return preflight.make_result(after_check, "ok", "after complete")

    monkeypatch.setattr(
        preflight,
        "CHECKS",
        [
            (before_check, before),
            (raising_check, raising),
            (after_check, after),
        ],
    )

    rc = preflight.main(["--json"])
    payload = json.loads(capsys.readouterr().out)
    by_name = {check["name"]: check for check in payload["checks"]}

    assert rc == 1
    assert by_name["raising_check"]["execution_error"] == {
        "type": "RuntimeError",
        "message": "boom",
    }
    assert by_name["before_check"]["execution_error"] is None
    assert by_name["after_check"]["execution_error"] is None
    assert payload["summary"]["errors"] == 1
    assert payload["summary"]["errors"] <= payload["summary"]["failed"]


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


def test_solstone_core_rust_toolchain_ok(preflight, probe, monkeypatch):
    monkeypatch.setattr(probe, "_is_source_checkout", lambda: True)
    monkeypatch.setattr(
        probe,
        "current_solstone_core_platform",
        lambda: ("linux", "x86_64"),
    )

    def run_probe(_check, cmd, **_kwargs):
        if list(cmd) == ["cargo", "--version"]:
            return probe.ProbeOutput("cargo 1.87.0\n", "", 0)
        if list(cmd) == ["rustc", "--version"]:
            return probe.ProbeOutput("rustc 1.87.0\n", "", 0)
        raise AssertionError(f"unexpected probe command: {cmd}")

    monkeypatch.setattr(probe, "run_probe", run_probe)

    result = preflight.solstone_core_rust_toolchain_check(args(preflight))

    assert result.status == "ok"
    assert "cargo 1.87.0" in result.detail
    assert "rustc 1.87.0" in result.detail


def test_solstone_core_rust_toolchain_skips_packaged_install(
    preflight, probe, monkeypatch
):
    monkeypatch.setattr(probe, "_is_source_checkout", lambda: False)

    result = preflight.solstone_core_rust_toolchain_check(args(preflight))

    assert result.status == "skip"
    assert "packaged install" in result.detail


def test_solstone_core_rust_toolchain_skips_uncovered_platform(
    preflight, probe, monkeypatch
):
    monkeypatch.setattr(probe, "_is_source_checkout", lambda: True)
    monkeypatch.setattr(
        probe,
        "current_solstone_core_platform",
        lambda: ("linux", "riscv64"),
    )

    result = preflight.solstone_core_rust_toolchain_check(args(preflight))

    assert result.status == "skip"
    assert "linux/riscv64" in result.detail


def test_solstone_core_rust_toolchain_failure_names_rust_toolchain(
    preflight, probe, monkeypatch
):
    def raise_missing(*_args, **_kwargs):
        raise FileNotFoundError

    monkeypatch.setattr(probe, "_is_source_checkout", lambda: True)
    monkeypatch.setattr(
        probe,
        "current_solstone_core_platform",
        lambda: ("linux", "x86_64"),
    )
    monkeypatch.setattr(probe.subprocess, "run", Mock(side_effect=raise_missing))

    result = preflight.solstone_core_rust_toolchain_check(args(preflight))

    assert result.status == "fail"
    assert "cargo" in result.detail
    assert result.fix is not None
    assert "Rust" in result.fix
    assert "cargo" in result.fix
    assert "rustc" in result.fix


def test_solstone_core_native_build_dependencies_ok(preflight, probe, monkeypatch):
    monkeypatch.setattr(probe, "_is_source_checkout", lambda: True)
    monkeypatch.setattr(
        probe,
        "current_solstone_core_platform",
        lambda: ("linux", "x86_64"),
    )
    monkeypatch.setattr(probe.shutil, "which", lambda name: f"/usr/bin/{name}")
    monkeypatch.setattr(
        probe,
        "clang_builtin_include_dir",
        lambda: Path("/usr/lib/llvm-18/lib/clang/18/include"),
    )

    result = preflight.solstone_core_native_build_dependencies_check(args(preflight))

    assert result.status == "ok"
    assert "NASM" in result.detail
    assert "Clang builtin headers" in result.detail


def test_solstone_core_native_build_dependencies_reports_every_missing_input(
    preflight, probe, monkeypatch
):
    monkeypatch.setattr(probe, "_is_source_checkout", lambda: True)
    monkeypatch.setattr(
        probe,
        "current_solstone_core_platform",
        lambda: ("linux", "x86_64"),
    )
    monkeypatch.setattr(probe.shutil, "which", lambda _name: None)
    monkeypatch.setattr(probe, "clang_builtin_include_dir", lambda: None)

    result = preflight.solstone_core_native_build_dependencies_check(args(preflight))

    assert result.status == "fail"
    assert "NASM" in result.detail
    assert "Clang builtin headers" in result.detail
    assert result.fix is not None
    assert "CONTRIBUTING.md" in result.fix


def test_clang_builtin_include_dir_honors_bindgen_include_args(
    probe, monkeypatch, tmp_path
):
    include_dir = tmp_path / "clang resource dir" / "include"
    include_dir.mkdir(parents=True)
    (include_dir / "limits.h").write_text("", encoding="utf-8")
    monkeypatch.setenv(
        "BINDGEN_EXTRA_CLANG_ARGS",
        f"-nostdinc -isystem '{include_dir}'",
    )
    monkeypatch.setattr(probe, "CLANG_BUILTIN_INCLUDE_PATTERNS", ())

    assert probe.clang_builtin_include_dir() == include_dir


def test_solstone_core_native_build_dependencies_do_not_require_nasm_on_aarch64(
    preflight, probe, monkeypatch
):
    monkeypatch.setattr(probe, "_is_source_checkout", lambda: True)
    monkeypatch.setattr(
        probe,
        "current_solstone_core_platform",
        lambda: ("linux", "aarch64"),
    )
    monkeypatch.setattr(probe.shutil, "which", Mock(side_effect=AssertionError))
    monkeypatch.setattr(
        probe,
        "clang_builtin_include_dir",
        lambda: Path("/usr/lib/llvm-18/lib/clang/18/include"),
    )

    result = preflight.solstone_core_native_build_dependencies_check(args(preflight))

    assert result.status == "ok"
    assert "NASM" not in result.detail


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
    monkeypatch.setattr(probe.shutil, "which", Mock(return_value=str(local)))

    result = preflight.local_bin_sol_reachable_check(args(preflight))

    assert result.status == "ok"


def test_uv_missing_fails(preflight, probe, monkeypatch):
    def raise_missing(*_args, **_kwargs):
        raise FileNotFoundError

    monkeypatch.setattr(probe, "_is_source_checkout", lambda: True)
    monkeypatch.setattr(probe.subprocess, "run", Mock(side_effect=raise_missing))

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
    monkeypatch.setattr(probe.subprocess, "run", Mock(side_effect=raise_missing))
    monkeypatch.setattr(
        probe.shutil,
        "disk_usage",
        Mock(return_value=SimpleNamespace(total=100, used=80, free=20 * 1024**3)),
    )
    (home_root / ".config").mkdir()

    rc = preflight.main(["--json"])
    payload = json.loads(capsys.readouterr().out)

    assert rc == 1
    assert payload["summary"]["failed"] >= 1
    assert payload["summary"]["errors"] == 0
