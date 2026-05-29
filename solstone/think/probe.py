# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Stdlib-only shared probe/check base for preflight.py and doctor.py.

This module must import only the Python standard library so install-readiness
checks can run before `.venv` or `uv` exist.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from importlib.metadata import PackageNotFoundError, distribution
from pathlib import Path
from typing import Literal, Sequence

# See doctor.py's decision-log for the MIN_UV=0.7.12 and MIN_FREE_GIB=10
# rationale.
ROOT = Path(__file__).resolve().parents[2]
MIN_UV = (0, 7, 12)
MIN_FREE_GIB = 10.0
DEFAULT_REQUIRES_PYTHON = ">=3.11"
PYTHON_VERSION_FIX = "install Python >=3.11, then retry"
LOCAL_BIN_SOL_FIX = (
    "Install via `uv tool install solstone` or `pipx install solstone` for the "
    "canonical layout, or run `ln -s $(command -v sol) ~/.local/bin/sol` to keep "
    "your custom layout."
)

Severity = Literal["blocker", "advisory"]
Status = Literal["ok", "fail", "warn", "skip"]
Platform = Literal["linux", "darwin"]


@dataclass(frozen=True)
class Check:
    name: str
    severity: Severity
    platforms: tuple[Platform, ...]


@dataclass(frozen=True)
class CheckResult:
    name: str
    severity: Severity
    status: Status
    detail: str
    fix: str | None
    platform: str | None = None


@dataclass(frozen=True)
class ProbeOutput:
    stdout: str
    stderr: str
    returncode: int


PYTHON_VERSION_CHECK = Check("python_version", "blocker", ("linux", "darwin"))
UV_INSTALLED_CHECK = Check("uv_installed", "blocker", ("linux", "darwin"))
VENV_CONSISTENT_CHECK = Check("venv_consistent", "blocker", ("linux", "darwin"))
LOCAL_BIN_SOL_REACHABLE_CHECK = Check(
    "local_bin_sol_reachable", "advisory", ("linux", "darwin")
)
DISK_SPACE_CHECK = Check("disk_space", "advisory", ("linux", "darwin"))
CONFIG_DIR_READABLE_CHECK = Check("config_dir_readable", "blocker", ("linux", "darwin"))


def platform_tag() -> Platform:
    if sys.platform == "darwin":
        return "darwin"
    return "linux"


def make_result(
    check: Check,
    status: Status,
    detail: str,
    fix: str | None = None,
    *,
    platform: str | None = None,
) -> CheckResult:
    return CheckResult(
        name=check.name,
        severity=check.severity,
        status=status,
        detail=detail,
        fix=fix,
        platform=platform,
    )


def truncate(text: str, limit: int) -> str:
    text = " ".join(text.split())
    if len(text) <= limit:
        return text
    return text[: limit - 3] + "..."


def version_text(version: tuple[int, int, int]) -> str:
    return ".".join(str(part) for part in version)


def parse_version(text: str) -> tuple[int, int, int] | None:
    match = re.search(r"(\d+)\.(\d+)\.(\d+)", text)
    if not match:
        return None
    return tuple(int(part) for part in match.groups())


def compare_versions(left: tuple[int, int, int], right: tuple[int, int, int]) -> int:
    if left < right:
        return -1
    if left > right:
        return 1
    return 0


def unexpected_output_result(
    check: Check,
    output: str,
    *,
    fix: str | None = None,
) -> CheckResult:
    snippet = truncate(output or "<empty>", 80)
    return make_result(
        check,
        "fail",
        f"probe returned unexpected output: {snippet}",
        fix,
    )


def command_text(cmd: Sequence[str]) -> str:
    return " ".join(cmd)


def run_probe(
    check: Check,
    cmd: Sequence[str],
    *,
    timeout: float,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    ok_returncodes: tuple[int, ...] = (0,),
    allow_nonzero: bool = False,
    allow_empty_stdout: bool = False,
    fix: str | None = None,
) -> ProbeOutput | CheckResult:
    merged_env = os.environ.copy()
    if env:
        merged_env.update(env)
    try:
        completed = subprocess.run(
            list(cmd),
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=str(cwd) if cwd else None,
            env=merged_env,
            check=False,
        )
    except FileNotFoundError:
        return make_result(check, "fail", f"probe command not found: {cmd[0]}", fix)
    except subprocess.TimeoutExpired:
        return make_result(
            check,
            "fail",
            f"probe timed out after {timeout:g}s: {command_text(cmd)}",
            fix,
        )
    except OSError as exc:
        return make_result(
            check,
            "fail",
            f"probe failed: {type(exc).__name__}: {exc}",
            fix,
        )

    if completed.returncode not in ok_returncodes and not allow_nonzero:
        detail = completed.stderr.strip() or completed.stdout.strip() or "<empty>"
        return make_result(
            check,
            "fail",
            f"probe exited {completed.returncode}: {truncate(detail, 80)}",
            fix,
        )

    if not allow_empty_stdout and not completed.stdout.strip():
        return unexpected_output_result(
            check,
            completed.stderr.strip() or completed.stdout.strip(),
            fix=fix,
        )

    return ProbeOutput(
        stdout=completed.stdout,
        stderr=completed.stderr,
        returncode=completed.returncode,
    )


def _is_source_checkout() -> bool:
    # Inline solstone.think.utils.is_source_checkout; importing utils pulls timefhuman.
    return (ROOT / "pyproject.toml").exists() and (ROOT / ".git").exists()


def python_version_check(args: object) -> CheckResult:
    del args
    check = PYTHON_VERSION_CHECK
    pyproject = ROOT / "pyproject.toml"
    spec_from_metadata = False
    try:
        text = pyproject.read_text(encoding="utf-8")
        match = re.search(r'^requires-python\s*=\s*"([^"]+)"', text, re.MULTILINE)
        if not match:
            return make_result(
                check,
                "fail",
                "could not parse requires-python from pyproject.toml",
                PYTHON_VERSION_FIX,
            )
        spec = match.group(1)
    except FileNotFoundError:
        spec_from_metadata = True
        try:
            spec = distribution("solstone").metadata.get("Requires-Python")
        except PackageNotFoundError:
            spec = None
        if not spec:
            spec = DEFAULT_REQUIRES_PYTHON
    except OSError as exc:
        return make_result(
            check,
            "fail",
            f"could not read {pyproject.name}: {type(exc).__name__}: {exc}",
            PYTHON_VERSION_FIX,
        )
    min_match = re.search(r">=\s*(\d+)\.(\d+)(?:\.(\d+))?", spec)
    if not min_match:
        if spec_from_metadata:
            spec = DEFAULT_REQUIRES_PYTHON
            min_match = re.search(r">=\s*(\d+)\.(\d+)(?:\.(\d+))?", spec)
        if not min_match:
            return make_result(
                check,
                "fail",
                f"unsupported requires-python specifier: {spec}",
                PYTHON_VERSION_FIX,
            )
    minimum = (
        int(min_match.group(1)),
        int(min_match.group(2)),
        int(min_match.group(3) or 0),
    )
    current = sys.version_info[:3]
    if compare_versions(current, minimum) < 0:
        return make_result(
            check,
            "fail",
            f"python {version_text(current)} does not satisfy {spec}",
            "install Python >=3.11, then `rm -rf .venv .installed && make install`",
        )
    return make_result(
        check,
        "ok",
        f"python {version_text(current)} satisfies {spec}",
    )


def uv_installed_check(args: object) -> CheckResult:
    del args
    check = UV_INSTALLED_CHECK
    if not _is_source_checkout():
        return make_result(
            check,
            "skip",
            "uv is only required for source-checkout development",
        )
    fix = "curl -LsSf https://astral.sh/uv/install.sh | sh"
    probe = run_probe(check, ["uv", "--version"], timeout=0.5, fix=fix)
    if isinstance(probe, CheckResult):
        return probe
    version = parse_version(probe.stdout)
    if version is None:
        return unexpected_output_result(check, probe.stdout, fix=fix)
    if compare_versions(version, MIN_UV) < 0:
        return make_result(
            check,
            "fail",
            f"uv {version_text(version)} is older than required {version_text(MIN_UV)}",
            fix,
        )
    return make_result(
        check,
        "ok",
        f"uv {version_text(version)} >= {version_text(MIN_UV)}",
    )


def venv_consistent_check(args: object) -> CheckResult:
    del args
    check = VENV_CONSISTENT_CHECK
    if not _is_source_checkout():
        return make_result(
            check,
            "skip",
            "packaged install: env managed by uv tool / pipx",
        )
    python_bin = ROOT / ".venv" / "bin" / "python"
    expected = (ROOT / ".venv").resolve()
    if not python_bin.exists():
        return make_result(
            check,
            "skip",
            ".venv absent; run make install",
        )
    probe = run_probe(
        check,
        [str(python_bin), "-c", "import sys; print(sys.prefix)"],
        timeout=0.5,
        fix="rm -rf .venv .installed && make install",
    )
    if isinstance(probe, CheckResult):
        return probe
    prefix_text = probe.stdout.strip()
    if not prefix_text:
        return unexpected_output_result(
            check,
            probe.stdout,
            fix="rm -rf .venv .installed && make install",
        )
    actual = Path(prefix_text).resolve()
    if actual != expected:
        return make_result(
            check,
            "fail",
            f".venv points at {actual}, expected {expected}",
            "rm -rf .venv .installed && make install",
        )
    return make_result(check, "ok", f".venv points at this repo ({expected})")


def local_bin_sol_reachable_check(args: object) -> CheckResult:
    del args
    check = LOCAL_BIN_SOL_REACHABLE_CHECK
    local = Path.home() / ".local" / "bin" / "sol"
    which = shutil.which("sol")
    if local.exists() and local.is_file() and which is not None:
        which_path = Path(which)
        local_resolved = local.resolve()
        which_resolved = which_path.resolve()
        if (
            which_path != local
            and local.is_symlink()
            and local_resolved == which_resolved
        ):
            return make_result(
                check,
                "ok",
                f"~/.local/bin/sol symlinks to PATH sol at {which}",
            )
        if which_resolved == local_resolved:
            return make_result(
                check,
                "ok",
                f"~/.local/bin/sol is on PATH at {local}",
            )

    failures: list[str] = []
    if not local.exists():
        failures.append(f"{local} is missing")
    elif not local.is_file():
        failures.append(f"{local} is not a file")
    if which is None:
        failures.append("sol is not on PATH")
    else:
        try:
            failures.append(
                f"PATH sol resolves to {Path(which).resolve()}, expected {local.resolve()}"
            )
        except OSError:
            failures.append(f"PATH sol is {which}, but it could not be resolved")
    return make_result(check, "warn", "; ".join(failures), LOCAL_BIN_SOL_FIX)


def disk_space_check(args: object) -> CheckResult:
    del args
    check = DISK_SPACE_CHECK
    usage = shutil.disk_usage(ROOT)
    free_gib = usage.free / (1024**3)
    if free_gib < MIN_FREE_GIB:
        return make_result(
            check,
            "warn",
            f"only {free_gib:.1f} GiB free on the repo filesystem (<{MIN_FREE_GIB:.0f} GiB)",
            "free disk on the repo filesystem before `make install`",
        )
    return make_result(
        check,
        "ok",
        f"{free_gib:.1f} GiB free (>= {MIN_FREE_GIB:.0f} GiB)",
    )


def config_dir_readable_check(args: object) -> CheckResult:
    del args
    check = CONFIG_DIR_READABLE_CHECK
    home = Path.home()
    if not home.exists():
        return make_result(
            check,
            "fail",
            f"home directory does not exist: {home}",
            f"fix ownership/permissions of {home}",
        )
    required_access = os.R_OK | os.W_OK | os.X_OK
    if not os.access(home, required_access):
        return make_result(
            check,
            "fail",
            f"home directory is not readable and writable: {home}",
            f"fix ownership/permissions of {home}",
        )
    current_platform = platform_tag()
    if current_platform == "darwin":
        config_dir = home / "Library" / "LaunchAgents"
    else:
        config_dir = home / ".config"
    if config_dir.exists() and not os.access(config_dir, required_access):
        return make_result(
            check,
            "fail",
            f"service config directory is not writable: {config_dir}",
            f"fix ownership/permissions of {config_dir}",
        )
    if config_dir.exists():
        detail = f"home and service config dir are writable ({config_dir})"
    else:
        detail = f"home is writable; install will create {config_dir}"
    return make_result(check, "ok", detail)
