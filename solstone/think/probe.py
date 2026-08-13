# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Stdlib-only shared probe/check base for preflight.py and doctor.py.

This module must import only the Python standard library so install-readiness
checks can run before `.venv` or `uv` exist.
"""

from __future__ import annotations

import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
from dataclasses import dataclass
from importlib.metadata import PackageNotFoundError, distribution
from pathlib import Path
from typing import Callable, Literal, Sequence, TypeVar

# See doctor.py's decision-log for the MIN_UV=0.7.12 and MIN_FREE_GIB=10
# rationale.
ROOT = Path(__file__).resolve().parents[2]
MIN_UV = (0, 7, 12)
MIN_FREE_GIB = 10.0
DEFAULT_REQUIRES_PYTHON = ">=3.12"
PYTHON_VERSION_FIX = "install Python >=3.12, then retry"
LOCAL_BIN_SOL_FIX = (
    "Install via `uv tool install solstone` or `pipx install solstone` for the "
    "canonical layout, or run `ln -s $(command -v sol) ~/.local/bin/sol` to keep "
    "your custom layout."
)

Severity = Literal["blocker", "advisory"]
Status = Literal["ok", "fail", "warn", "skip"]
Platform = Literal["linux", "darwin"]
CorePlatform = tuple[Platform, str]
ArgsT = TypeVar("ArgsT")


@dataclass(frozen=True)
class Check:
    name: str
    severity: Severity
    platforms: tuple[Platform, ...]


@dataclass(frozen=True)
class ExecutionError:
    type: str
    message: str

    def to_dict(self) -> dict[str, str]:
        return {
            "type": self.type,
            "message": self.message,
        }


@dataclass(frozen=True)
class CheckResult:
    name: str
    severity: Severity
    status: Status
    detail: str
    fix: str | None
    platform: str | None = None
    execution_error: ExecutionError | None = None


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
SOLSTONE_CORE_RUST_TOOLCHAIN_CHECK = Check(
    "solstone_core_rust_toolchain", "blocker", ("linux", "darwin")
)
SOLSTONE_CORE_NATIVE_BUILD_DEPENDENCIES_CHECK = Check(
    "solstone_core_native_build_dependencies", "blocker", ("linux",)
)

CLANG_BUILTIN_INCLUDE_PATTERNS = (
    "/usr/lib/clang/*/include",
    "/usr/lib64/clang/*/include",
    "/usr/lib/llvm-*/lib/clang/*/include",
    "/usr/local/lib/clang/*/include",
)

SOLSTONE_CORE_COVERED_PLATFORMS: tuple[CorePlatform, ...] = (
    ("linux", "x86_64"),
    ("linux", "aarch64"),
    ("darwin", "arm64"),
)


def _solstone_core_platform_marker(platform_tuple: CorePlatform) -> str:
    system, machine = platform_tuple
    return f"sys_platform == '{system}' and platform_machine == '{machine}'"


def _solstone_core_platform_tag(platform_tuple: CorePlatform) -> str:
    system, machine = platform_tuple
    if system == "darwin":
        return f"macosx_14_0_{machine}"
    return f"manylinux_2_17_{machine}.manylinux2014_{machine}"


SOLSTONE_CORE_PLATFORM_MARKERS: tuple[str, ...] = tuple(
    _solstone_core_platform_marker(platform_tuple)
    for platform_tuple in SOLSTONE_CORE_COVERED_PLATFORMS
)
SOLSTONE_CORE_UNSUPPORTED_PLATFORM_MARKER = (
    "(sys_platform != 'linux' and sys_platform != 'darwin') or "
    "(sys_platform == 'linux' and platform_machine != 'x86_64' and "
    "platform_machine != 'aarch64') or "
    "(sys_platform == 'darwin' and platform_machine != 'arm64')"
)
SOLSTONE_CORE_PLATFORM_TAGS: dict[CorePlatform, str] = {
    platform_tuple: _solstone_core_platform_tag(platform_tuple)
    for platform_tuple in SOLSTONE_CORE_COVERED_PLATFORMS
}

SOLSTONE_CORE_SPEAKERS_ANALYZE_COVERED_PLATFORMS: tuple[CorePlatform, ...] = (
    ("linux", "x86_64"),
    ("linux", "aarch64"),
    ("darwin", "arm64"),
)

SOLSTONE_CORE_VULKAN_PROBE_COVERED_PLATFORMS: tuple[CorePlatform, ...] = (
    ("linux", "x86_64"),
    ("linux", "aarch64"),
)

# Describe is Linux-only. Its family builds on the zig GNU manylinux_2_27 lane and
# this wave ships no macOS wheel, so a darwin pin here would be a marker nothing
# can satisfy -- an unresolvable `pip install solstone-journal` on macOS.
SOLSTONE_CORE_DESCRIBE_COVERED_PLATFORMS: tuple[CorePlatform, ...] = (
    ("linux", "x86_64"),
    ("linux", "aarch64"),
)


def _solstone_core_speakers_analyze_platform_tag(
    platform_tuple: CorePlatform,
) -> str:
    system, machine = platform_tuple
    if system == "darwin":
        # Derived from the measured ONNX Runtime 1.25.0 dylib minos=14.0.0.
        # Its match with solstone-core's literal tag is coincidental.
        return f"macosx_14_0_{machine}"
    return f"manylinux_2_27_{machine}"


SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_TAGS: dict[CorePlatform, str] = {
    platform_tuple: _solstone_core_speakers_analyze_platform_tag(platform_tuple)
    for platform_tuple in SOLSTONE_CORE_SPEAKERS_ANALYZE_COVERED_PLATFORMS
}
SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_MARKERS: tuple[str, ...] = tuple(
    _solstone_core_platform_marker(platform_tuple)
    for platform_tuple in SOLSTONE_CORE_SPEAKERS_ANALYZE_COVERED_PLATFORMS
)
SOLSTONE_CORE_VULKAN_PROBE_PLATFORM_TAGS: dict[CorePlatform, str] = {
    platform_tuple: f"manylinux_2_27_{platform_tuple[1]}"
    for platform_tuple in SOLSTONE_CORE_VULKAN_PROBE_COVERED_PLATFORMS
}
SOLSTONE_CORE_VULKAN_PROBE_PLATFORM_MARKERS: tuple[str, ...] = tuple(
    _solstone_core_platform_marker(platform_tuple)
    for platform_tuple in SOLSTONE_CORE_VULKAN_PROBE_COVERED_PLATFORMS
)
SOLSTONE_CORE_DESCRIBE_PLATFORM_MARKERS: tuple[str, ...] = tuple(
    _solstone_core_platform_marker(platform_tuple)
    for platform_tuple in SOLSTONE_CORE_DESCRIBE_COVERED_PLATFORMS
)


def platform_tag() -> Platform:
    if sys.platform == "darwin":
        return "darwin"
    return "linux"


def normalize_solstone_core_machine(system: str, machine: str) -> str:
    system_name = "darwin" if system == "darwin" else "linux"
    machine_name = machine.lower()
    if system_name == "linux" and machine_name == "arm64":
        return "aarch64"
    if system_name == "darwin" and machine_name == "aarch64":
        return "arm64"
    return machine_name


def current_solstone_core_platform() -> CorePlatform:
    system = platform_tag()
    return (system, normalize_solstone_core_machine(system, platform.machine()))


def is_solstone_core_covered_platform(system: str, machine: str) -> bool:
    platform_tuple = (
        "darwin" if system == "darwin" else "linux",
        normalize_solstone_core_machine(system, machine),
    )
    return platform_tuple in SOLSTONE_CORE_COVERED_PLATFORMS


def solstone_core_marker_pins(version: str) -> tuple[str, ...]:
    return tuple(
        f"solstone-core=={version}; {marker}"
        for marker in SOLSTONE_CORE_PLATFORM_MARKERS
    )


def solstone_core_speakers_analyze_marker_pins(version: str) -> tuple[str, ...]:
    return tuple(
        f"solstone-core-speakers-analyze=={version}; {marker}"
        for marker in SOLSTONE_CORE_SPEAKERS_ANALYZE_PLATFORM_MARKERS
    )


def solstone_core_describe_marker_pins(version: str) -> tuple[str, ...]:
    return tuple(
        f"solstone-core-describe=={version}; {marker}"
        for marker in SOLSTONE_CORE_DESCRIBE_PLATFORM_MARKERS
    )


def solstone_core_vulkan_probe_marker_pins(version: str) -> tuple[str, ...]:
    return tuple(
        f"solstone-core-vulkan-probe=={version}; {marker}"
        for marker in SOLSTONE_CORE_VULKAN_PROBE_PLATFORM_MARKERS
    )


def solstone_core_unsupported_platform_pin(version: str) -> str:
    return (
        f"solstone-core-unsupported-platform=={version}; "
        f"{SOLSTONE_CORE_UNSUPPORTED_PLATFORM_MARKER}"
    )


def make_result(
    check: Check,
    status: Status,
    detail: str,
    fix: str | None = None,
    *,
    platform: str | None = None,
    execution_error: ExecutionError | None = None,
) -> CheckResult:
    return CheckResult(
        name=check.name,
        severity=check.severity,
        status=status,
        detail=detail,
        fix=fix,
        platform=platform,
        execution_error=execution_error,
    )


def has_execution_error(result: CheckResult) -> bool:
    return result.execution_error is not None


def run_check(
    check: Check,
    runner: Callable[[ArgsT], CheckResult],
    args: ArgsT,
) -> CheckResult:
    try:
        return runner(args)
    except Exception as exc:
        message = truncate(str(exc), 512)
        exc_type = type(exc).__name__
        detail = f"check execution failed: {exc_type}"
        if message:
            detail = f"{detail}: {message}"
        return make_result(
            check,
            "fail",
            detail,
            execution_error=ExecutionError(type=exc_type, message=message),
        )


def summary_counts(results: Sequence[CheckResult]) -> dict[str, int]:
    return {
        "total": len(results),
        "failed": sum(1 for result in results if result.status == "fail"),
        "warnings": sum(1 for result in results if result.status == "warn"),
        "skipped": sum(1 for result in results if result.status == "skip"),
        "errors": sum(1 for result in results if has_execution_error(result)),
    }


def results_failed(results: Sequence[CheckResult]) -> bool:
    return any(
        has_execution_error(result)
        or (result.severity == "blocker" and result.status == "fail")
        for result in results
    )


def status_label(result: CheckResult) -> str:
    if has_execution_error(result):
        return "ERROR"
    return result.status.upper()


def check_result_to_json_dict(result: CheckResult) -> dict[str, object]:
    return {
        "name": result.name,
        "severity": result.severity,
        "status": result.status,
        "detail": result.detail,
        "fix": result.fix,
        "execution_error": (
            result.execution_error.to_dict() if has_execution_error(result) else None
        ),
    }


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
    # Inline solstone.think.utils.is_source_checkout so preflight can run before
    # broader utility imports and provider dependency checks.
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
            "install Python >=3.12, then `rm -rf .venv .installed && make install`",
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


def solstone_core_rust_toolchain_check(args: object) -> CheckResult:
    del args
    check = SOLSTONE_CORE_RUST_TOOLCHAIN_CHECK
    if not _is_source_checkout():
        return make_result(
            check,
            "skip",
            "packaged install already carries solstone-core wheel metadata",
        )

    system, machine = current_solstone_core_platform()
    if not is_solstone_core_covered_platform(system, machine):
        return make_result(
            check,
            "skip",
            f"solstone-core is not required on {system}/{machine}",
            platform=system,
        )

    fix = "install Rust with cargo and rustc, then retry `make install`"
    cargo_output = run_probe(
        check,
        ["cargo", "--version"],
        timeout=5,
        fix=fix,
    )
    if isinstance(cargo_output, CheckResult):
        return cargo_output

    rustc_output = run_probe(
        check,
        ["rustc", "--version"],
        timeout=5,
        fix=fix,
    )
    if isinstance(rustc_output, CheckResult):
        return rustc_output

    return make_result(
        check,
        "ok",
        f"Rust toolchain available for solstone-core on {system}/{machine}: "
        f"{cargo_output.stdout.strip()}; {rustc_output.stdout.strip()}",
        platform=system,
    )


def _bindgen_include_dirs() -> tuple[Path, ...]:
    tokens = shlex.split(os.environ.get("BINDGEN_EXTRA_CLANG_ARGS", ""))
    paths: list[Path] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token in {"-I", "-isystem"} and index + 1 < len(tokens):
            index += 1
            paths.append(Path(tokens[index]))
        elif token.startswith("-I") and len(token) > 2:
            paths.append(Path(token[2:]))
        index += 1
    return tuple(paths)


def clang_builtin_include_dir() -> Path | None:
    candidates = list(_bindgen_include_dirs())
    for pattern in CLANG_BUILTIN_INCLUDE_PATTERNS:
        pattern_path = Path(pattern)
        anchor = Path(pattern_path.anchor)
        candidates.extend(anchor.glob(str(pattern_path.relative_to(anchor))))
    for candidate in candidates:
        if (candidate / "limits.h").is_file():
            return candidate
    return None


def solstone_core_native_build_dependencies_check(args: object) -> CheckResult:
    del args
    check = SOLSTONE_CORE_NATIVE_BUILD_DEPENDENCIES_CHECK
    if not _is_source_checkout():
        return make_result(
            check,
            "skip",
            "native build dependencies are only required for source development",
        )

    system, machine = current_solstone_core_platform()
    if system != "linux" or not is_solstone_core_covered_platform(system, machine):
        return make_result(
            check,
            "skip",
            f"Linux native build dependencies are not required on {system}/{machine}",
            platform=system,
        )

    nasm_path = shutil.which("nasm") if machine == "x86_64" else None
    clang_include = clang_builtin_include_dir()
    missing: list[str] = []
    if machine == "x86_64" and nasm_path is None:
        missing.append("NASM")
    if clang_include is None:
        missing.append("Clang builtin headers (limits.h)")
    if missing:
        packages = (
            "NASM and Clang development headers"
            if machine == "x86_64"
            else "Clang development headers"
        )
        return make_result(
            check,
            "fail",
            f"missing native Rust build dependencies: {', '.join(missing)}",
            f"install {packages}; see CONTRIBUTING.md platform prerequisites",
            platform=system,
        )

    details = [f"Clang builtin headers at {clang_include}"]
    if nasm_path is not None:
        details.insert(0, f"NASM at {nasm_path}")
    return make_result(
        check,
        "ok",
        "; ".join(details),
        platform=system,
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
