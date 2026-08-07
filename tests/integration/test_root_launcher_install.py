# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import base64
import email
import hashlib
import json
import os
import shutil
import subprocess
import tomllib
import venv
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator

import pytest
from packaging.requirements import Requirement
from packaging.utils import canonicalize_name

import scripts.check_wheel_contents as wheel_checker

ROOT = Path(__file__).resolve().parents[2]
pytestmark = [
    pytest.mark.integration,
    pytest.mark.release,
    pytest.mark.timeout(900),
]


@dataclass(frozen=True)
class BuiltWheels:
    version: str
    dist: Path
    root: Path
    core: Path


def _run(
    argv: list[str | Path],
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
    timeout: int = 300,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(token) for token in argv],
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        check=False,
        timeout=timeout,
    )


def _root_version() -> str:
    data = tomllib.loads((ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    return data["project"]["version"]


def _lock_versions() -> dict[str, str]:
    data = tomllib.loads((ROOT / "uv.lock").read_text(encoding="utf-8"))
    return {
        canonicalize_name(package["name"]): package["version"]
        for package in data["package"]
    }


def _record_hash(content: bytes) -> str:
    digest = hashlib.sha256(content).digest()
    encoded = base64.urlsafe_b64encode(digest).decode("ascii").rstrip("=")
    return f"sha256={encoded}"


def _write_wheel(
    wheel_dir: Path,
    distribution: str,
    version: str,
    *,
    scripts: dict[str, bytes] | None = None,
    entry_points: str | None = None,
) -> Path:
    wheel_dir.mkdir(parents=True, exist_ok=True)
    normalized = distribution.replace("-", "_")
    dist_info = f"{normalized}-{version}.dist-info"
    wheel_path = wheel_dir / f"{normalized}-{version}-py3-none-any.whl"
    members = {
        f"{dist_info}/METADATA": (
            f"Metadata-Version: 2.1\nName: {distribution}\nVersion: {version}\n"
        ).encode(),
        f"{dist_info}/WHEEL": (
            "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n"
        ).encode(),
    }
    if entry_points is not None:
        members[f"{dist_info}/entry_points.txt"] = entry_points.encode("utf-8")
    for name, content in (scripts or {}).items():
        members[f"{normalized}-{version}.data/scripts/{name}"] = content
    rows = [
        f"{name},{_record_hash(content)},{len(content)}"
        for name, content in members.items()
    ]
    rows.append(f"{dist_info}/RECORD,,")
    with zipfile.ZipFile(wheel_path, "w") as wheel:
        for name, content in members.items():
            info = zipfile.ZipInfo(name)
            info.create_system = 3
            info.external_attr = (
                0o755 << 16 if ".data/scripts/" in name else 0o644 << 16
            )
            wheel.writestr(info, content)
        wheel.writestr(f"{dist_info}/RECORD", "\n".join(rows).encode("utf-8"))
    return wheel_path


def _runtime_requirements(root_wheel: Path) -> tuple[Requirement, ...]:
    with zipfile.ZipFile(root_wheel) as wheel:
        metadata_name = next(
            name for name in wheel.namelist() if name.endswith("METADATA")
        )
        message = email.message_from_bytes(wheel.read(metadata_name))
    requirements = []
    for line in message.get_all("Requires-Dist") or ():
        requirement = Requirement(line)
        name = canonicalize_name(requirement.name)
        if name == "solstone-core":
            continue
        if requirement.marker is not None and not requirement.marker.evaluate(
            {"extra": ""}
        ):
            continue
        requirements.append(requirement)
    return tuple(requirements)


def _write_dependency_stub_wheels(root_wheel: Path, wheel_dir: Path) -> None:
    versions = _lock_versions()
    for requirement in _runtime_requirements(root_wheel):
        name = canonicalize_name(requirement.name)
        version = versions[name]
        assert requirement.specifier.contains(version, prereleases=True)
        _write_wheel(wheel_dir, name, version)


@pytest.fixture(scope="module")
def built_wheels(tmp_path_factory: pytest.TempPathFactory) -> Iterator[BuiltWheels]:
    if shutil.which("uv") is None:
        pytest.skip("uv is not installed")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is not installed")

    dist = tmp_path_factory.mktemp("root-launcher-wheels")
    env = os.environ.copy()
    env.pop("MATURIN_PEP517_ARGS", None)
    for argv in (
        [
            "uv",
            "build",
            "--offline",
            "--package",
            "solstone-core",
            "--wheel",
            "--out-dir",
            dist,
        ],
        ["uv", "build", "--offline", "--wheel", "--out-dir", dist],
    ):
        result = _run(argv, cwd=ROOT, env=env, timeout=900)
        assert result.returncode == 0, result.stderr or result.stdout

    version = _root_version()
    root_wheels = sorted(dist.glob(f"solstone-{version}-py3-none-any.whl"))
    core_wheels = sorted(dist.glob(f"solstone_core-{version}-*.whl"))
    assert len(root_wheels) == 1
    assert len(core_wheels) == 1
    _write_dependency_stub_wheels(root_wheels[0], dist)
    yield BuiltWheels(
        version=version, dist=dist, root=root_wheels[0], core=core_wheels[0]
    )


def _isolated_uv_env(tmp_path: Path) -> tuple[dict[str, str], Path, Path, Path]:
    home = tmp_path / "home"
    tool_dir = tmp_path / "uv-tools"
    tool_bin = tmp_path / "uv-bin"
    cache = tmp_path / "uv-cache"
    unrelated = tmp_path / "unrelated"
    for path in (home, tool_dir, tool_bin, cache, unrelated):
        path.mkdir(parents=True)
    env = os.environ.copy()
    env.update(
        {
            "HOME": str(home),
            "XDG_DATA_HOME": str(tmp_path / "xdg-data"),
            "UV_TOOL_DIR": str(tool_dir),
            "UV_TOOL_BIN_DIR": str(tool_bin),
            "UV_CACHE_DIR": str(cache),
            "UV_PYTHON_DOWNLOADS": "never",
        }
    )
    env.pop("PYTHONPATH", None)
    return env, tool_bin, tool_dir, unrelated


def _uv_tool_install(
    *,
    root_wheel: Path,
    core_wheel: Path,
    env: dict[str, str],
    cwd: Path,
    force: bool = False,
) -> subprocess.CompletedProcess[str]:
    argv: list[str | Path] = [
        "uv",
        "tool",
        "install",
        "--offline",
        "--no-index",
        "--find-links",
        root_wheel.parent,
        "--with",
        core_wheel,
    ]
    if force:
        argv.append("--force")
    argv.append(root_wheel)
    return _run(argv, cwd=cwd, env=env, timeout=900)


def _tool_bin_names(tool_bin: Path) -> set[str]:
    return {path.name for path in tool_bin.iterdir() if not path.name.startswith(".")}


def _tool_env_bin(tool_bin: Path, name: str = "sol") -> Path:
    return (tool_bin / name).resolve().parent


def _install_python_spy(env_bin: Path, log: Path) -> None:
    for name in ("python3", "python"):
        interpreter = env_bin / name
        real = env_bin / f"{name}.real"
        interpreter.rename(real)
        wrapper = (
            "#!/bin/sh\n"
            f"printf '%s %s\\n' {name!r} \"$*\" >> {str(log)!r}\n"
            f'exec {str(real)!r} "$@"\n'
        )
        interpreter.write_text(wrapper, encoding="utf-8")
        interpreter.chmod(0o755)


def _script_owners(python: Path, names: tuple[str, ...]) -> dict[str, list[str]]:
    script = """
import csv
import importlib.metadata as metadata
import json
import os
from pathlib import Path, PurePosixPath

bin_dir = Path(os.environ["BIN_DIR"])
targets = set(json.loads(os.environ["SCRIPT_NAMES"]))
scripts = {str((bin_dir / name).resolve()): name for name in targets}
owners = {name: [] for name in targets}
for dist in metadata.distributions():
    dist_name = dist.metadata.get("Name", "")
    for file in dist.files or []:
        basename = PurePosixPath(str(file).replace("\\\\", "/")).name
        if basename.endswith(".exe"):
            basename = basename[:-4]
        if basename not in targets:
            continue
        try:
            located = str(dist.locate_file(file).resolve())
        except OSError:
            continue
        if located in scripts:
            owners[scripts[located]].append(dist_name)
print(json.dumps({key: sorted(value) for key, value in owners.items()}, sort_keys=True))
"""
    result = _run(
        [python, "-c", script],
        cwd=python.parent,
        env={
            **os.environ,
            "BIN_DIR": str(python.parent),
            "SCRIPT_NAMES": json.dumps(list(names)),
        },
    )
    assert result.returncode == 0, result.stderr or result.stdout
    return json.loads(result.stdout)


def _assert_new_owners(python: Path) -> None:
    expected = {
        **{name: ["solstone"] for name in wheel_checker.ROOT_LAUNCHER_NAMES},
        **{name: ["solstone-core"] for name in wheel_checker.CORE_SCRIPT_NAMES},
    }
    assert (
        _script_owners(
            python, wheel_checker.ROOT_LAUNCHER_NAMES + wheel_checker.CORE_SCRIPT_NAMES
        )
        == expected
    )


def _create_venv(path: Path) -> Path:
    venv.EnvBuilder(with_pip=True, symlinks=False).create(path)
    return path / "bin" / "python"


def _pip_install(
    python: Path,
    wheels: tuple[Path, ...],
    *,
    force: bool = False,
) -> subprocess.CompletedProcess[str]:
    argv: list[str | Path] = [
        python,
        "-m",
        "pip",
        "install",
        "--no-index",
        "--no-deps",
    ]
    if force:
        argv.append("--force-reinstall")
    argv.extend(wheels)
    return _run(argv, cwd=python.parent)


def _old_layout_wheels(tmp_path: Path) -> tuple[Path, Path]:
    old_version = "1.0.14"
    wheel_dir = tmp_path / "old-wheels"
    core_scripts = {
        "sol": b"#!/bin/sh\necho old sol\n",
        "solstone": b"#!/bin/sh\necho old solstone\n",
        "solstone-core": b"#!/bin/sh\necho old solstone-core\n",
    }
    old_core = _write_wheel(
        wheel_dir,
        "solstone-core",
        old_version,
        scripts=core_scripts,
    )
    old_root = _write_wheel(
        wheel_dir,
        "solstone",
        old_version,
        entry_points=(
            "[console_scripts]\n"
            "solstone-python-compat = solstone.think.sol_compat_cli:main\n"
        ),
    )
    return old_root, old_core


def test_cold_uv_tool_install_exports_root_launchers_only(
    tmp_path: Path,
    built_wheels: BuiltWheels,
) -> None:
    env, tool_bin, _tool_dir, unrelated = _isolated_uv_env(tmp_path)
    assert _tool_bin_names(tool_bin) == set()

    install = _uv_tool_install(
        root_wheel=built_wheels.root,
        core_wheel=built_wheels.core,
        env=env,
        cwd=unrelated,
    )

    assert install.returncode == 0, install.stderr or install.stdout
    assert _tool_bin_names(tool_bin) == set(wheel_checker.ROOT_LAUNCHER_NAMES)
    assert not (tool_bin / "solstone-core").exists()
    assert not (tool_bin / "solstone-python-compat").exists()

    env_bin = _tool_env_bin(tool_bin)
    assert (env_bin / "solstone-core").exists()
    assert (tool_bin / "sol").resolve().parent == env_bin
    assert (tool_bin / "solstone").resolve().parent == env_bin

    log = tmp_path / "python-spy.log"
    _install_python_spy(env_bin, log)
    native = _run([tool_bin / "sol", "--version"], cwd=unrelated, env=env)
    assert native.returncode == 0, native.stderr
    assert native.stdout == f"sol (solstone) {built_wheels.version}\n"
    native_solstone = _run(
        [tool_bin / "solstone", "--version"],
        cwd=unrelated,
        env=env,
    )
    assert native_solstone.returncode == 0, native_solstone.stderr
    assert native_solstone.stdout == f"sol (solstone) {built_wheels.version}\n"
    assert not log.exists()

    journal_help = _run(
        [tool_bin / "sol", "call", "journal", "--help"],
        cwd=unrelated,
        env=env,
    )
    assert journal_help.returncode == 0, journal_help.stderr
    assert "Usage: sol call journal <command> [args...]" in journal_help.stdout
    assert not log.exists()


def test_isolated_pip_venv_install_exposes_new_layout(
    tmp_path: Path,
    built_wheels: BuiltWheels,
) -> None:
    python = _create_venv(tmp_path / "venv")
    install = _pip_install(python, (built_wheels.core, built_wheels.root))
    assert install.returncode == 0, install.stderr or install.stdout

    bin_dir = python.parent
    for name in wheel_checker.ROOT_LAUNCHER_NAMES + wheel_checker.CORE_SCRIPT_NAMES:
        result = _run([bin_dir / name, "--version"], cwd=tmp_path)
        assert result.returncode == 0, result.stderr
    _assert_new_owners(python)
    assert not (bin_dir / "solstone-python-compat").exists()


@pytest.mark.parametrize("installer", ("pip", "uv"))
def test_upgrade_from_old_script_ownership_converges(
    tmp_path: Path,
    built_wheels: BuiltWheels,
    installer: str,
) -> None:
    old_root, old_core = _old_layout_wheels(tmp_path)
    if installer == "pip":
        python = _create_venv(tmp_path / "upgrade-venv")
        old_install = _pip_install(python, (old_core, old_root))
        assert old_install.returncode == 0, old_install.stderr or old_install.stdout
        bin_dir = python.parent
        assert (bin_dir / "solstone-python-compat").exists()
        upgrade = _pip_install(
            python, (built_wheels.core, built_wheels.root), force=True
        )
        assert upgrade.returncode == 0, upgrade.stderr or upgrade.stdout
        reinstall = _pip_install(
            python, (built_wheels.core, built_wheels.root), force=True
        )
        assert reinstall.returncode == 0, reinstall.stderr or reinstall.stdout
        assert not (bin_dir / "solstone-python-compat").exists()
        _assert_new_owners(python)
        return

    env, tool_bin, _tool_dir, unrelated = _isolated_uv_env(tmp_path)
    old_install = _uv_tool_install(
        root_wheel=old_root,
        core_wheel=old_core,
        env=env,
        cwd=unrelated,
    )
    assert old_install.returncode == 0, old_install.stderr or old_install.stdout
    assert _tool_bin_names(tool_bin) == {"solstone-python-compat"}
    upgrade = _uv_tool_install(
        root_wheel=built_wheels.root,
        core_wheel=built_wheels.core,
        env=env,
        cwd=unrelated,
        force=True,
    )
    assert upgrade.returncode == 0, upgrade.stderr or upgrade.stdout
    reinstall = _uv_tool_install(
        root_wheel=built_wheels.root,
        core_wheel=built_wheels.core,
        env=env,
        cwd=unrelated,
        force=True,
    )
    assert reinstall.returncode == 0, reinstall.stderr or reinstall.stdout
    assert _tool_bin_names(tool_bin) == set(wheel_checker.ROOT_LAUNCHER_NAMES)
    env_bin = _tool_env_bin(tool_bin)
    assert not (env_bin / "solstone-python-compat").exists()
    _assert_new_owners(env_bin / "python")


def test_partial_uninstall_ownership_boundaries(
    tmp_path: Path,
    built_wheels: BuiltWheels,
) -> None:
    root_only_python = _create_venv(tmp_path / "root-only")
    assert (
        _pip_install(
            root_only_python, (built_wheels.core, built_wheels.root)
        ).returncode
        == 0
    )
    root_only_bin = root_only_python.parent
    result = _run(
        [root_only_python, "-m", "pip", "uninstall", "-y", "solstone"], cwd=tmp_path
    )
    assert result.returncode == 0, result.stderr or result.stdout
    assert not any(
        (root_only_bin / name).exists() for name in wheel_checker.ROOT_LAUNCHER_NAMES
    )
    assert (root_only_bin / "solstone-core").exists()

    core_only_python = _create_venv(tmp_path / "core-only")
    assert (
        _pip_install(
            core_only_python, (built_wheels.core, built_wheels.root)
        ).returncode
        == 0
    )
    core_only_bin = core_only_python.parent
    result = _run(
        [core_only_python, "-m", "pip", "uninstall", "-y", "solstone-core"],
        cwd=tmp_path,
    )
    assert result.returncode == 0, result.stderr or result.stdout
    assert not (core_only_bin / "solstone-core").exists()
    for name in wheel_checker.ROOT_LAUNCHER_NAMES:
        launcher = _run([core_only_bin / name, "--version"], cwd=tmp_path)
        assert launcher.returncode == 78
        assert launcher.stdout == ""
        assert "native solstone-core sibling is missing" in launcher.stderr

    full_python = _create_venv(tmp_path / "full")
    assert (
        _pip_install(full_python, (built_wheels.core, built_wheels.root)).returncode
        == 0
    )
    full_bin = full_python.parent
    result = _run(
        [full_python, "-m", "pip", "uninstall", "-y", "solstone", "solstone-core"],
        cwd=tmp_path,
    )
    assert result.returncode == 0, result.stderr or result.stdout
    assert not any(
        (full_bin / name).exists()
        for name in wheel_checker.ROOT_LAUNCHER_NAMES + wheel_checker.CORE_SCRIPT_NAMES
    )
