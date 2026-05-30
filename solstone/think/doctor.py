# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Diagnostics for solstone CLI and journal hosts.

`sol doctor` runs universal CLI-usability checks that must be meaningful on a
journal-less machine. `journal doctor` runs journal-host service, folder, and
processing-health checks. `--readiness` runs the setup step-1 battery.

Exit code `0` means no blocker failed; exit code `1` means at least one
blocker-severity check failed.

Decision log:
- Universal python check reads installed package metadata (with a static
  fallback), not pyproject.toml, so packaged installs and repo-less hosts can
  be diagnosed.
- disk threshold: 10 GiB — measured `.venv`=7.88 GiB +
  uv-cache first-install growth ~1 GiB + buffer.
- Feature-extras checks (pdf, whisper) are dynamically registered from
  `solstone.think.features.FEATURES`, severity advisory, never affect
  exit code. Filter via `--feature <name>`.
"""

from __future__ import annotations

import argparse
import importlib
import importlib.util
import json
import os
import plistlib
import re
import sys
import time
from dataclasses import dataclass
from functools import partial
from importlib.metadata import PackageNotFoundError, distribution
from importlib.metadata import version as _pkg_version
from pathlib import Path
from typing import IO, Any, Callable, Sequence

from solstone.think import features as _features
from solstone.think.health_cli import fetch_supervisor_status
from solstone.think.probe import (
    CONFIG_DIR_READABLE_CHECK,
    DEFAULT_REQUIRES_PYTHON,
    DISK_SPACE_CHECK,
    LOCAL_BIN_SOL_REACHABLE_CHECK,
    PYTHON_VERSION_CHECK,
    PYTHON_VERSION_FIX,
    ROOT,
    Check,
    CheckResult,
    Status,
    compare_versions,
    config_dir_readable_check,
    disk_space_check,
    local_bin_sol_reachable_check,
    make_result,
    platform_tag,
    run_probe,
    truncate,
    version_text,
)
from solstone.think.service import (
    check_service_target_identity,
    service_is_failed,
    service_is_installed,
)
from solstone.think.setup_events import STATUS_TRANSLATION, JsonlEmitter, utc_now_iso
from solstone.think.sync_check import check_journal_sync, format_doctor_report
from solstone.think.utils import get_journal_info, is_packaged_install


class _InstallModelsProxy:
    """Lazy module proxy; install_models imports observe audio deps at import time."""

    def __getattr__(self, name: str) -> Any:
        from solstone.think import install_models as module

        return getattr(module, name)


install_models = _InstallModelsProxy()


@dataclass(frozen=True)
class Args:
    verbose: bool
    json: bool
    jsonl: bool
    port: int
    feature: str | None = None
    readiness: bool = False


Runner = Callable[[Args], CheckResult]

SOL_IMPORTABLE_CHECK = Check("sol_importable", "blocker", ("linux", "darwin"))
STALE_ALIAS_CHECK = Check("stale_alias_symlink", "blocker", ("linux", "darwin"))
JOURNAL_DIR_WRITABLE_CHECK = Check(
    "journal_dir_writable", "blocker", ("linux", "darwin")
)
SERVICE_IDENTITY_CHECK = Check("service_identity", "blocker", ("linux", "darwin"))
SERVICE_RUNNING_CHECK = Check("service_running", "blocker", ("linux", "darwin"))
LAUNCHD_STALE_PLIST_CHECK = Check("launchd_stale_plist", "advisory", ("darwin",))
JOURNAL_SYNC_CHECK = Check("journal_sync", "blocker", ("linux", "darwin"))
DEFAULT_STT_READY_CHECK = Check("default_stt_ready", "advisory", ("linux", "darwin"))
_DEFAULT_STT_RUNTIME_FIX = (
    "parakeet runtime (onnx-asr) is not installed — reinstall to add it: "
    "uv tool install --reinstall solstone"
)
_DEFAULT_STT_MODEL_FIX = (
    "parakeet model is not downloaded — fetch it with: journal install-models"
)


def python_sanity_check(args: Args) -> CheckResult:
    del args
    check = PYTHON_VERSION_CHECK
    try:
        spec = distribution("solstone").metadata.get("Requires-Python")
    except PackageNotFoundError:
        spec = None
    if not spec:
        spec = DEFAULT_REQUIRES_PYTHON

    min_match = re.search(r">=\s*(\d+)\.(\d+)(?:\.(\d+))?", spec)
    if not min_match:
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
            PYTHON_VERSION_FIX,
        )
    return make_result(
        check,
        "ok",
        f"python {version_text(current)} satisfies {spec}",
    )


def sol_importable_check(args: Args) -> CheckResult:
    del args
    check = SOL_IMPORTABLE_CHECK
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


def _nearest_existing_ancestor(path: Path) -> Path:
    current = path
    while not current.exists() and current.parent != current:
        current = current.parent
    return current


def _journal_writability_result(check: Check) -> CheckResult:
    path_text, _source = get_journal_info()
    path = Path(path_text)
    if path.is_dir():
        if os.access(path, os.W_OK):
            return make_result(check, "ok", f"journal dir writable: {path}")
        return make_result(
            check,
            "fail",
            f"journal dir not writable: {path}",
            f"fix ownership/permissions of {path}",
        )
    if path.exists():
        return make_result(
            check,
            "fail",
            f"journal path exists but is not a directory: {path}",
            f"move or remove {path}, then re-run",
        )

    ancestor = _nearest_existing_ancestor(path)
    if ancestor.is_dir() and os.access(ancestor, os.W_OK):
        return make_result(
            check,
            "ok",
            f"journal dir absent; parent {ancestor} is writable",
        )
    return make_result(
        check,
        "fail",
        f"journal dir absent; nearest existing ancestor is not writable: {ancestor}",
        f"fix ownership/permissions of {ancestor}",
    )


def journal_dir_writable_readiness(args: Args) -> CheckResult:
    del args
    return _journal_writability_result(JOURNAL_DIR_WRITABLE_CHECK)


def journal_dir_writable_journal(args: Args) -> CheckResult:
    del args
    path_text, _source = get_journal_info()
    if not Path(path_text).exists():
        return make_result(JOURNAL_DIR_WRITABLE_CHECK, "skip", "no local journal")
    return _journal_writability_result(JOURNAL_DIR_WRITABLE_CHECK)


def service_identity_check(args: Args) -> CheckResult:
    del args
    identity = check_service_target_identity()
    if not identity.installed:
        return make_result(SERVICE_IDENTITY_CHECK, "skip", "no local journal service")
    if identity.target == "":
        return make_result(
            SERVICE_IDENTITY_CHECK,
            "fail",
            identity.detail,
            "run journal setup to reinstall the service",
        )
    if not identity.matches_current_install:
        return make_result(
            SERVICE_IDENTITY_CHECK,
            "fail",
            identity.detail,
            "run journal setup --force from this install to refresh the service",
        )
    return make_result(SERVICE_IDENTITY_CHECK, "ok", identity.detail)


def service_running_check(args: Args) -> CheckResult:
    del args
    if not service_is_installed():
        return make_result(SERVICE_RUNNING_CHECK, "skip", "no local journal service")

    status = fetch_supervisor_status()
    if status is None:
        if service_is_failed():
            return make_result(
                SERVICE_RUNNING_CHECK,
                "fail",
                "journal service unit is failed",
                "run journal service restart; if it persists, run journal service logs",
            )
        return make_result(
            SERVICE_RUNNING_CHECK,
            "warn",
            "service installed but not running",
            "run journal service start",
        )

    crashed = status.get("crashed") or []
    if crashed:
        crashed_details = []
        for item in crashed:
            name = item.get("name", "?")
            attempts = item.get("restart_attempts", 0)
            crashed_details.append(f"{name} ({attempts} restart attempts)")
        return make_result(
            SERVICE_RUNNING_CHECK,
            "fail",
            f"crash-loop: {', '.join(crashed_details)}",
            "run journal service logs",
        )

    return make_result(SERVICE_RUNNING_CHECK, "ok", "journal service is running")


def import_install_guard() -> tuple[object, object]:
    root_text = str(ROOT)
    if root_text not in sys.path:
        sys.path.insert(0, root_text)
    module = importlib.import_module("solstone.think.install_guard")
    return module.AliasState, module.check_alias


def _recognized_legacy_target(target: Path) -> str | None:
    resolved = target.resolve(strict=False)
    home = Path.home()
    legacy_prefixes = (
        (home / ".local" / "share" / "uv" / "tools" / "solstone", "uv-tool"),
        (home / ".local" / "share" / "pipx" / "venvs" / "solstone", "pipx-xdg"),
        (home / ".local" / "pipx" / "venvs" / "solstone", "pipx-legacy"),
    )
    for prefix, tag in legacy_prefixes:
        if resolved.is_relative_to(prefix):
            return tag
    return None


def _legacy_backup_dir() -> Path:
    return Path("/tmp")


def _legacy_backup_path(binary: str) -> Path:
    timestamp = time.strftime("%Y%m%d%H%M%S", time.gmtime())
    base = _legacy_backup_dir() / f"{binary}.old-symlink-{timestamp}"
    for index in range(100):
        candidate = base if index == 0 else base.with_name(f"{base.name}-{index}")
        if not candidate.exists() and not candidate.is_symlink():
            return candidate
    raise RuntimeError(
        "could not find unique backup path under /tmp after 100 attempts"
    )


def _latest_legacy_backup(binary: str) -> Path | None:
    matches = list(_legacy_backup_dir().glob(f"{binary}.old-symlink-*"))
    if not matches:
        return None

    def sort_key(path: Path) -> int:
        try:
            return path.lstat().st_mtime_ns
        except OSError:
            return -1

    return max(matches, key=sort_key)


def _partial_migration_detail(binary: str, backup: Path) -> str:
    return (
        f"partial migration detected; backup at {backup} — restore with "
        f"`mv {backup} ~/.local/bin/{binary}` or re-run from a fresh shell"
    )


def _legacy_target_from_symlink(alias: Path) -> tuple[Path, str] | None:
    if not alias.is_symlink():
        return None
    target = Path(os.readlink(alias))
    if not target.is_absolute():
        target = alias.parent / target
    resolved = target.resolve(strict=False)
    tag = _recognized_legacy_target(resolved)
    if tag is None:
        return None
    return resolved, tag


def _auto_migrate_legacy_aliases(
    check: Check,
    install_guard: object,
    binary: str,
) -> CheckResult:
    try:
        journal = install_guard._current_journal_for_alias()
    except Exception as exc:
        return make_result(
            check,
            "fail",
            f"legacy {binary} alias detected but journal resolution failed: {type(exc).__name__}: {exc} — run from venv to auto-migrate",
            f"run `journal setup` from the repo that owns the wrapper, or remove `~/.local/bin/{binary}` manually if the repo is gone",
        )

    alias = install_guard.alias_paths()[binary]
    legacy = _legacy_target_from_symlink(alias)
    if legacy is None:
        return make_result(
            check,
            "fail",
            f"legacy {binary} alias auto-migration failed: alias is no longer a recognized legacy symlink",
        )
    _target, tag = legacy

    try:
        backup = _legacy_backup_path(binary)
        backup.parent.mkdir(parents=True, exist_ok=True)
        alias.replace(backup)

        sol_bin = Path(sys.executable).parent / binary
        install_guard.install_wrappers(
            str(journal),
            {binary: str(sol_bin)},
            paths={binary: alias},
        )
    except Exception as exc:
        return make_result(
            check,
            "fail",
            f"legacy {binary} alias auto-migration failed: {type(exc).__name__}: {exc}",
        )

    if tag == "uv-tool":
        migration_phrase = "migrated legacy uv-tool symlink"
    elif tag.startswith("pipx"):
        migration_phrase = "migrated legacy pipx symlink"
    else:
        migration_phrase = "migrated legacy symlink"
    return make_result(
        check,
        "ok",
        f"auto-migrated legacy {tag} install for {binary} ({migration_phrase}): backed up {binary} → {backup}; installed managed wrapper at ~/.local/bin/{binary}",
    )


def stale_alias_symlink_check(args: Args, binary: str) -> CheckResult:
    del args
    check = STALE_ALIAS_CHECK
    try:
        alias_state_cls, check_alias = import_install_guard()
    except Exception as exc:
        return make_result(
            check,
            "skip",
            f"could not import solstone.think.install_guard: {type(exc).__name__}: {exc}",
        )
    install_guard = importlib.import_module(check_alias.__module__)
    alias = install_guard.alias_paths()[binary]
    if not alias.exists() and not alias.is_symlink():
        backup = _latest_legacy_backup(binary)
        if backup is not None:
            return make_result(
                check,
                "fail",
                _partial_migration_detail(binary, backup),
                "restore the backup or re-run from a fresh shell",
            )

    worktree = alias_state_cls.WORKTREE
    absent = alias_state_cls.ABSENT
    owned = alias_state_cls.OWNED
    cross_repo = alias_state_cls.CROSS_REPO
    dangling = alias_state_cls.DANGLING
    foreign = alias_state_cls.FOREIGN

    state, other = check_alias(ROOT, binary)
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
            f"{binary} alias absent or owned by this repo",
        )

    if state in {cross_repo, dangling, foreign} and other is not None:
        tag = _recognized_legacy_target(other)
        if tag is not None:
            return _auto_migrate_legacy_aliases(check, install_guard, binary)

    if state is cross_repo:
        fail_detail = f"~/.local/bin/{binary} points at another repo ({other})"
    elif state is dangling:
        fail_detail = f"~/.local/bin/{binary} is dangling ({other})"
    elif state is foreign:
        fail_detail = f"~/.local/bin/{binary} exists but is not a symlink"
    else:
        fail_detail = f"unexpected alias state for {binary}: {state}"
    return make_result(
        check,
        "fail",
        fail_detail,
        f"run `journal setup` from the repo that owns the wrapper, or remove `~/.local/bin/{binary}` manually if the repo is gone",
    )


def launchd_stale_plist_check(args: Args) -> CheckResult:
    del args
    check = LAUNCHD_STALE_PLIST_CHECK
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
    check = JOURNAL_SYNC_CHECK
    path_text, _source = get_journal_info()
    if not Path(path_text).is_dir():
        return make_result(check, "skip", "no local journal")
    try:
        result = check_journal_sync()
    except Exception as exc:
        return make_result(check, "fail", f"sync check failed: {exc}")

    status: Status = "fail" if result.is_conflict else "ok"
    return make_result(check, status, format_doctor_report(result))


def _resolve_configured_backend() -> str | None:
    """Read transcribe.backend from an existing journal config without creating anything.

    Returns the configured backend, or None when no config is present (caller
    treats None as the parakeet default). Never materializes the journal dir.
    """
    path_text, _source = get_journal_info()
    config_path = Path(path_text) / "config" / "journal.json"
    if not config_path.is_file():
        return None
    try:
        data = json.loads(config_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return None
    backend = data.get("transcribe", {}).get("backend")
    return backend if isinstance(backend, str) else None


def default_stt_ready_check(args: Args) -> CheckResult:
    del args
    check = DEFAULT_STT_READY_CHECK
    backend = _resolve_configured_backend()
    if backend and backend != "parakeet":
        return make_result(
            check,
            "skip",
            f"configured backend is {backend}; parakeet readiness not applicable",
        )

    os_name, arch = install_models._platform_info()
    if (os_name, arch) not in {("linux", "x86_64"), ("darwin", "arm64")}:
        return make_result(check, "skip", "parakeet not supported on this platform")

    variant = "cpu" if os_name == "linux" else "coreml"
    if os_name == "linux" and importlib.util.find_spec("onnx_asr") is None:
        return make_result(
            check,
            "warn",
            "onnx-asr runtime not installed",
            _DEFAULT_STT_RUNTIME_FIX,
        )

    try:
        ready_cache = install_models._check_parakeet_ready(
            os_name,
            arch,
            variant,
            install_models._sentinel_path(variant),
        )
    except RuntimeError as exc:
        return make_result(check, "warn", str(exc), _DEFAULT_STT_MODEL_FIX)
    return make_result(check, "ok", f"parakeet model ready at {ready_cache}")


def _make_feature_check(
    feat_name: str,
) -> tuple[Check, Runner]:
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


FEATURE_CHECKS: dict[str, tuple[Check, Runner]] = {
    name: _make_feature_check(name) for name in _features.FEATURES
}

UNIVERSAL_CHECKS: list[tuple[Check, Runner]] = [
    (PYTHON_VERSION_CHECK, python_sanity_check),
    (SOL_IMPORTABLE_CHECK, sol_importable_check),
    (LOCAL_BIN_SOL_REACHABLE_CHECK, local_bin_sol_reachable_check),
    (STALE_ALIAS_CHECK, partial(stale_alias_symlink_check, binary="sol")),
]

JOURNAL_CHECKS: list[tuple[Check, Runner]] = [
    (DISK_SPACE_CHECK, disk_space_check),
    (CONFIG_DIR_READABLE_CHECK, config_dir_readable_check),
    (JOURNAL_DIR_WRITABLE_CHECK, journal_dir_writable_journal),
    (SERVICE_IDENTITY_CHECK, service_identity_check),
    (SERVICE_RUNNING_CHECK, service_running_check),
    (JOURNAL_SYNC_CHECK, journal_sync_check),
    (STALE_ALIAS_CHECK, partial(stale_alias_symlink_check, binary="journal")),
    (LAUNCHD_STALE_PLIST_CHECK, launchd_stale_plist_check),
    (DEFAULT_STT_READY_CHECK, default_stt_ready_check),
    *FEATURE_CHECKS.values(),
]

READINESS_CHECKS: list[tuple[Check, Runner]] = [
    (PYTHON_VERSION_CHECK, python_sanity_check),
    (SOL_IMPORTABLE_CHECK, sol_importable_check),
    (LOCAL_BIN_SOL_REACHABLE_CHECK, local_bin_sol_reachable_check),
    (STALE_ALIAS_CHECK, partial(stale_alias_symlink_check, binary="sol")),
    (DISK_SPACE_CHECK, disk_space_check),
    (JOURNAL_DIR_WRITABLE_CHECK, journal_dir_writable_readiness),
    (DEFAULT_STT_READY_CHECK, default_stt_ready_check),
    *FEATURE_CHECKS.values(),
]

_ALL_CHECKS = UNIVERSAL_CHECKS + JOURNAL_CHECKS + READINESS_CHECKS
CHECK_MAP: dict[str, Check] = {}
for _check, _runner in _ALL_CHECKS:
    CHECK_MAP.setdefault(_check.name, _check)


def parse_args(argv: Sequence[str] | None = None) -> Args:
    parser = argparse.ArgumentParser(
        description="Run solstone diagnostics.",
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
    parser.add_argument(
        "--readiness",
        action="store_true",
        help="run the setup readiness battery",
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
        readiness=namespace.readiness,
    )


def select_battery(args: Args) -> list[tuple[Check, Runner]]:
    if args.readiness:
        return READINESS_CHECKS
    if sys.argv[0] == "journal doctor":
        return JOURNAL_CHECKS
    return UNIVERSAL_CHECKS


def run_checks(
    args: Args,
    checks: list[tuple[Check, Runner]] | None = None,
) -> list[CheckResult]:
    current_platform = platform_tag()
    if args.feature is not None:
        check_name = args.feature
        check, runner = FEATURE_CHECKS[check_name]
        if current_platform not in check.platforms:
            return [
                make_result(
                    check,
                    "skip",
                    f"not supported on {current_platform}",
                    platform=current_platform,
                )
            ]
        return [runner(args)]

    selected_checks = select_battery(args) if checks is None else checks
    results: list[CheckResult] = []
    for check, func in selected_checks:
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
