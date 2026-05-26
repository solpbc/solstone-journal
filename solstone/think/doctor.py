# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Pre-install diagnostics for solstone.

Runs a fixed battery of blocker and advisory checks using only the Python
standard library so a fresh clone can be diagnosed before `uv sync`. Exit code
`0` means no blocker failed; exit code `1` means at least one blocker-severity
check failed.

Decision log:
- uv floor: 0.7.12 — `uv.lock` revision=3 requires >= 0.7.12 per
  astral-sh/uv#15220.
- disk threshold: 10 GiB — measured `.venv`=7.88 GiB +
  uv-cache first-install growth ~1 GiB + buffer.
- Makefile UV-guard strategy: MAKECMDGOALS filter; prep verified the
  doctor-only matrix on GNU make.
- Ramon triage docs are absent in this worktree; the battery follows the task
  spec directly.
- Feature-extras checks (pdf, whisper) are dynamically registered from
  `solstone.think.features.FEATURES`, severity advisory, never affect
  exit code. Filter via `--feature <name>`.
"""

from __future__ import annotations

import argparse
import importlib
import json
import os
import plistlib
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from importlib.metadata import PackageNotFoundError, distribution
from importlib.metadata import version as _pkg_version
from pathlib import Path
from typing import IO, Callable, Literal, Sequence

from solstone.think import features as _features
from solstone.think.setup_events import STATUS_TRANSLATION, JsonlEmitter, utc_now_iso
from solstone.think.sync_check import check_journal_sync, format_doctor_report
from solstone.think.utils import is_packaged_install

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
class Args:
    verbose: bool
    json: bool
    jsonl: bool
    port: int
    feature: str | None = None


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


def python_version_check(args: Args) -> CheckResult:
    del args
    check = CHECK_MAP["python_version"]
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


def uv_installed_check(args: Args) -> CheckResult:
    del args
    check = CHECK_MAP["uv_installed"]
    if is_packaged_install():
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


def venv_consistent_check(args: Args) -> CheckResult:
    del args
    check = CHECK_MAP["venv_consistent"]
    if is_packaged_install():
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


def sol_importable_check(args: Args) -> CheckResult:
    del args
    check = CHECK_MAP["sol_importable"]
    if is_packaged_install():
        try:
            import solstone  # noqa: F401
        except Exception as exc:
            return make_result(
                check,
                "fail",
                f"import solstone failed: {type(exc).__name__}: {exc}",
            )
        return make_result(check, "ok", "import solstone succeeded in packaged install")
    python_bin = ROOT / ".venv" / "bin" / "python"
    fix = "rm -rf .venv .installed && make install"
    if not python_bin.exists():
        return make_result(check, "skip", ".venv absent; run make install")
    probe = run_probe(
        check,
        [str(python_bin), "-c", "from solstone.think.sol_cli import main"],
        cwd=Path("/"),
        timeout=2.0,
        allow_nonzero=True,
        allow_empty_stdout=True,
        fix=fix,
    )
    if isinstance(probe, CheckResult):
        return probe
    if probe.returncode == 0:
        return make_result(
            check,
            "ok",
            "from solstone.think.sol_cli import main succeeded outside repo cwd",
        )
    stderr = probe.stderr.strip()
    if "ModuleNotFoundError: No module named 'solstone'" in stderr:
        return make_result(
            check,
            "fail",
            "ModuleNotFoundError: No module named 'solstone'",
            fix,
        )
    first_line = next((line for line in stderr.splitlines() if line.strip()), "")
    detail = truncate(
        first_line
        or f"from solstone.think.sol_cli import main failed with exit {probe.returncode}",
        120,
    )
    return make_result(check, "fail", detail, fix)


def local_bin_sol_reachable_check(args: Args) -> CheckResult:
    del args
    check = CHECK_MAP["local_bin_sol_reachable"]
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


def disk_space_check(args: Args) -> CheckResult:
    del args
    check = CHECK_MAP["disk_space"]
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


def config_dir_readable_check(args: Args) -> CheckResult:
    del args
    check = CHECK_MAP["config_dir_readable"]
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


def import_install_guard() -> tuple[object, object]:
    root_text = str(ROOT)
    if root_text not in sys.path:
        sys.path.insert(0, root_text)
    module = importlib.import_module("solstone.think.install_guard")
    return module.AliasState, module.check_alias


def stale_alias_symlink_check(args: Args) -> CheckResult:
    del args
    check = CHECK_MAP["stale_alias_symlink"]
    try:
        alias_state_cls, check_alias = import_install_guard()
    except Exception as exc:
        return make_result(
            check,
            "skip",
            f"could not import solstone.think.install_guard: {type(exc).__name__}: {exc}",
        )
    state, other = check_alias(ROOT)
    worktree = alias_state_cls.WORKTREE
    absent = alias_state_cls.ABSENT
    owned = alias_state_cls.OWNED
    cross_repo = alias_state_cls.CROSS_REPO
    dangling = alias_state_cls.DANGLING
    foreign = alias_state_cls.FOREIGN
    if state is worktree:
        return make_result(
            check,
            "skip",
            "git worktree; run doctor from the primary clone",
        )
    if state in {absent, owned}:
        return make_result(
            check,
            "ok",
            "sol alias absent or owned by this repo",
        )
    if state is cross_repo:
        detail = f"~/.local/bin/sol points at another repo ({other})"
    elif state is dangling:
        detail = f"~/.local/bin/sol is dangling ({other})"
    elif state is foreign:
        detail = "~/.local/bin/sol exists but is not a symlink"
    else:
        detail = f"unexpected alias state: {state}"
    return make_result(
        check,
        "fail",
        detail,
        "run `journal setup` from the repo that owns the wrapper, or remove `~/.local/bin/sol` manually if the repo is gone",
    )


def launchd_stale_plist_check(args: Args) -> CheckResult:
    del args
    check = CHECK_MAP["launchd_stale_plist"]
    if platform_tag() != "darwin":
        return make_result(check, "skip", "not supported on linux", platform="linux")
    plist_path = Path.home() / "Library" / "LaunchAgents" / "org.solpbc.solstone.plist"
    if not plist_path.exists():
        return make_result(check, "skip", "launchd plist absent")
    try:
        with plist_path.open("rb") as handle:
            data = plistlib.load(handle)
    except Exception as exc:
        return make_result(
            check,
            "fail",
            f"could not parse plist: {type(exc).__name__}: {exc}",
            "rm ~/Library/LaunchAgents/org.solpbc.solstone.plist && journal setup",
        )
    program_arguments = data.get("ProgramArguments")
    if not isinstance(program_arguments, list) or not program_arguments:
        return make_result(
            check,
            "fail",
            "plist is missing ProgramArguments[0]",
            "rm ~/Library/LaunchAgents/org.solpbc.solstone.plist && journal setup",
        )
    executable = Path(str(program_arguments[0]))
    if not executable.exists():
        return make_result(
            check,
            "fail",
            f"plist points to missing executable: {executable}",
            "rm ~/Library/LaunchAgents/org.solpbc.solstone.plist && journal setup",
        )
    return make_result(check, "ok", f"launchd plist target exists ({executable})")


def journal_sync_check(args: Args) -> CheckResult:
    del args
    check = CHECK_MAP["journal_sync"]
    try:
        result = check_journal_sync()
    except Exception as exc:
        return make_result(check, "fail", f"sync check failed: {exc}")

    status: Status = "fail" if result.is_conflict else "ok"
    return make_result(check, status, format_doctor_report(result))


def _make_feature_check(
    feat_name: str,
) -> tuple[Check, Callable[[Args], CheckResult]]:
    feat = _features.FEATURES[feat_name]
    check = Check(f"feature:{feat_name}", "advisory", ("linux", "darwin"))

    def _run(args: Args) -> CheckResult:
        del args
        if _features.is_available(feat_name):
            return make_result(check, "ok", f"{feat.summary} available")
        return make_result(
            check,
            "warn",
            f"{feat.summary} not installed",
            _features.install_hint(feat_name, platform_tag()),
        )

    return check, _run


CHECKS: list[tuple[Check, Callable[[Args], CheckResult]]] = [
    (Check("python_version", "blocker", ("linux", "darwin")), python_version_check),
    (Check("uv_installed", "blocker", ("linux", "darwin")), uv_installed_check),
    (Check("venv_consistent", "blocker", ("linux", "darwin")), venv_consistent_check),
    (Check("sol_importable", "blocker", ("linux", "darwin")), sol_importable_check),
    (
        Check("local_bin_sol_reachable", "advisory", ("linux", "darwin")),
        local_bin_sol_reachable_check,
    ),
    (Check("disk_space", "advisory", ("linux", "darwin")), disk_space_check),
    (
        Check("config_dir_readable", "blocker", ("linux", "darwin")),
        config_dir_readable_check,
    ),
    (
        Check("stale_alias_symlink", "blocker", ("linux", "darwin")),
        stale_alias_symlink_check,
    ),
    (
        Check("launchd_stale_plist", "advisory", ("darwin",)),
        launchd_stale_plist_check,
    ),
    (Check("journal_sync", "blocker", ("linux", "darwin")), journal_sync_check),
]

for _feat_name in _features.FEATURES:
    CHECKS.append(_make_feature_check(_feat_name))

CHECK_MAP = {check.name: check for check, _func in CHECKS}


def parse_args(argv: Sequence[str] | None = None) -> Args:
    parser = argparse.ArgumentParser(
        description="Run pre-install diagnostics for solstone.",
        epilog=(
            "If 'sol doctor' is unavailable (e.g. before 'make install' completes), "
            "run 'python3 scripts/doctor.py' from the repo root for the same diagnostic."
        ),
    )
    parser.add_argument(
        "--verbose", action="store_true", help="print every check result"
    )
    parser.add_argument("--json", action="store_true", help="emit JSON instead of text")
    parser.add_argument(
        "--jsonl",
        action="store_true",
        help="emit one-JSON-per-line events instead of text",
    )
    parser.add_argument(
        "--port", type=int, default=5015, help="port to probe (default: 5015)"
    )
    parser.add_argument(
        "--feature",
        default=None,
        help=f"Run only the named feature check ({', '.join(sorted(_features.FEATURES))})",
    )
    namespace = parser.parse_args(argv)
    if namespace.json and namespace.jsonl:
        parser.error("--json and --jsonl are mutually exclusive")
    if namespace.feature is not None and namespace.feature not in _features.FEATURES:
        known = ", ".join(sorted(_features.FEATURES))
        parser.error(f"unknown feature {namespace.feature!r}; known features: {known}")
    return Args(
        verbose=namespace.verbose,
        json=namespace.json,
        jsonl=namespace.jsonl,
        port=namespace.port,
        feature=namespace.feature,
    )


def run_checks(args: Args) -> list[CheckResult]:
    current_platform = platform_tag()
    if args.feature is not None:
        check_name = f"feature:{args.feature}"
        check = CHECK_MAP[check_name]
        if current_platform not in check.platforms:
            return [
                make_result(
                    check,
                    "skip",
                    f"not supported on {current_platform}",
                    platform=current_platform,
                )
            ]
        for candidate, func in CHECKS:
            if candidate.name == check_name:
                return [func(args)]
        raise RuntimeError(f"missing check runner for {check_name}")

    results: list[CheckResult] = []
    for check, func in CHECKS:
        if current_platform not in check.platforms:
            results.append(
                make_result(
                    check,
                    "skip",
                    f"not supported on {current_platform}",
                    platform=current_platform,
                )
            )
            continue
        results.append(func(args))
    return results


def print_result_line(result: CheckResult) -> None:
    label = result.status.upper()
    print(f"  {label} {result.name} — {result.detail}")
    if result.fix:
        print(f"    → {result.fix}")


def summary_counts(results: Sequence[CheckResult]) -> dict[str, int]:
    return {
        "total": len(results),
        "failed": sum(1 for result in results if result.status == "fail"),
        "warnings": sum(1 for result in results if result.status == "warn"),
        "skipped": sum(1 for result in results if result.status == "skip"),
    }


def emit_text(results: Sequence[CheckResult], *, verbose: bool) -> None:
    if verbose:
        for result in results:
            print_result_line(result)
    else:
        for result in results:
            if result.status in {"fail", "warn"}:
                print_result_line(result)
    summary = summary_counts(results)
    print(
        "doctor: "
        f"{summary['total']} checks, "
        f"{summary['failed']} failed, "
        f"{summary['warnings']} warnings, "
        f"{summary['skipped']} skipped"
    )


def emit_json(results: Sequence[CheckResult]) -> None:
    payload = {
        "checks": [
            {
                "name": result.name,
                "severity": result.severity,
                "status": result.status,
                "detail": result.detail,
                "fix": result.fix,
            }
            for result in results
        ],
        "summary": summary_counts(results),
    }
    print(json.dumps(payload))


def solstone_version() -> str:
    try:
        return _pkg_version("solstone")
    except PackageNotFoundError:
        return "0.0.0+source"


def jsonl_summary_status(results: Sequence[CheckResult]) -> str:
    if any(
        result.severity == "blocker" and result.status == "fail" for result in results
    ):
        return "failed"
    if any(
        result.status == "warn"
        or (result.severity == "advisory" and result.status == "fail")
        for result in results
    ):
        return "warning"
    return "ok"


def emit_jsonl(
    results: Sequence[CheckResult],
    *,
    started_at_iso: str,
    duration_ms: int,
    summary_status: str,
    writer: IO[str] | None = None,
) -> None:
    emitter = JsonlEmitter(writer if writer is not None else sys.stdout)
    for result in results:
        emitter.emit(
            "check.completed",
            name=result.name,
            severity=result.severity,
            status=STATUS_TRANSLATION[result.status],
            detail=result.detail or "",
            fix=result.fix or "",
        )
    emitter.emit(
        "doctor.completed",
        started_at=started_at_iso,
        status=summary_status,
        duration_ms=duration_ms,
        summary=summary_counts(results),
    )


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    started_at_iso = utc_now_iso()
    t0 = time.monotonic()
    if args.jsonl:
        emitter = JsonlEmitter(sys.stdout)
        emitter.emit(
            "doctor.started",
            started_at=started_at_iso,
            version=solstone_version(),
            port=args.port,
            feature=args.feature or "",
        )
    results = run_checks(args)
    if args.json:
        emit_json(results)
    elif args.jsonl:
        emit_jsonl(
            results,
            started_at_iso=started_at_iso,
            duration_ms=round((time.monotonic() - t0) * 1000),
            summary_status=jsonl_summary_status(results),
            writer=sys.stdout,
        )
    else:
        emit_text(results, verbose=args.verbose)
    blocker_failed = any(
        result.severity == "blocker" and result.status == "fail" for result in results
    )
    return 1 if blocker_failed else 0
