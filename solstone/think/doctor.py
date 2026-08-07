# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Diagnostics for solstone CLI and journal hosts.

`sol doctor` runs universal CLI-usability checks that must be meaningful on a
journal-less machine. `journal doctor` runs journal-host service, folder, and
processing-health checks. `--readiness` runs the setup step-1 battery.

Exit code `0` means no blocker failed and no check raised during execution;
exit code `1` means at least one blocker-severity check failed or any check
raised during execution.

Decision log:
- Universal python check reads installed package metadata (with a static
  fallback), not pyproject.toml, so packaged installs and repo-less hosts can
  be diagnosed.
- disk threshold: 10 GiB — measured `.venv`=7.88 GiB +
  uv-cache first-install growth ~1 GiB + buffer.
- Feature-extras checks are dynamically registered from
  `solstone.think.features.FEATURES`, severity advisory, never affect
  exit code. Filter via `--feature <name>`.
"""

# doctor EXAMINES ONLY — never mutate (no writes, installs, downloads,
# migrations, network) and never import the inference (solstone.observe.*) or
# installer (install_models) layers. Diagnose and report; setup performs changes.

from __future__ import annotations

import argparse
import importlib
import importlib.util
import json
import os
import plistlib
import re
import shlex
import sys
import time
from dataclasses import dataclass, replace
from datetime import datetime, timezone
from functools import partial
from importlib.metadata import PackageNotFoundError, distribution
from importlib.metadata import version as _pkg_version
from pathlib import Path
from typing import IO, Callable, Sequence

from solstone.think import features as _features
from solstone.think import maint, parakeet_readiness
from solstone.think.capture_health import _STALE_MS, get_capture_health
from solstone.think.health_cli import fetch_supervisor_status
from solstone.think.media import PDF_EXTENSIONS
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
    check_result_to_json_dict,
    compare_versions,
    config_dir_readable_check,
    disk_space_check,
    has_execution_error,
    local_bin_sol_reachable_check,
    make_result,
    platform_tag,
    results_failed,
    run_check,
    run_probe,
    status_label,
    summary_counts,
    truncate,
    version_text,
)
from solstone.think.service import (
    SOLSTONE_APP_BUNDLE_PATH,
    ForeignLauncherMatch,
    SupervisorConflictEvidence,
    check_service_target_identity,
    inspect_supervisor_conflict,
    service_is_failed,
    service_is_installed,
)
from solstone.think.setup_events import STATUS_TRANSLATION, JsonlEmitter, utc_now_iso
from solstone.think.sync_check import check_journal_sync, format_doctor_report
from solstone.think.utils import (
    CorruptConfigError,
    _read_existing_journal_config,
    get_journal_info,
    is_packaged_install,
    now_ms,
)


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
SKILL_STATE_CHECK = Check("skill_state", "advisory", ("linux", "darwin"))
JOURNAL_DIR_WRITABLE_CHECK = Check(
    "journal_dir_writable", "blocker", ("linux", "darwin")
)
HOST_DEPENDENCIES_CHECK = Check("host_dependencies", "blocker", ("linux", "darwin"))
JOURNAL_LEAF_EXCLUSIVITY_CHECK = Check(
    "journal_leaf_exclusivity", "blocker", ("linux", "darwin")
)
JOURNAL_PACKAGE_VERSION_CHECK = Check(
    "journal_package_version", "blocker", ("linux", "darwin")
)
RETIRED_HOST_SHIM_CHECK = Check("retired_host_shim", "advisory", ("linux", "darwin"))
_ROUTER_SKILL_NAMES = ("sol", "journal")
_HOST_DEPENDENCY_MODULES = (
    ("frontmatter", "python-frontmatter"),
    ("flask", "Flask"),
    ("onnxruntime", "ONNX runtime"),
)
_CORRUPT_CONFIG_FIX = "repair or restore config/journal.json from a backup"
HOST_DEPENDENCY_REINSTALL_GUIDANCE = (
    "Reinstall the journal host stack: "
    "pip install --upgrade solstone-journal  |  "
    "uv tool install --upgrade solstone-journal  |  "
    "pipx install --force solstone-journal. "
    "On an NVIDIA host use solstone-journal-cuda instead — never install both."
)
SERVICE_IDENTITY_CHECK = Check("service_identity", "blocker", ("linux", "darwin"))
SERVICE_RUNNING_CHECK = Check("service_running", "blocker", ("linux", "darwin"))
SUPERVISOR_CONFLICT_CHECK = Check("supervisor_conflict", "blocker", ("darwin",))
LAUNCHD_STALE_PLIST_CHECK = Check("launchd_stale_plist", "advisory", ("darwin",))
JOURNAL_SYNC_CHECK = Check("journal_sync", "blocker", ("linux", "darwin"))
DEFAULT_STT_READY_CHECK = Check("default_stt_ready", "advisory", ("linux", "darwin"))
PARAKEET_CPP_STT_READY_CHECK = Check(
    "parakeet_cpp_stt_ready", "advisory", ("linux", "darwin")
)
SPEAKERS_ANALYZE_INSTALLATION_CHECK = Check(
    "speakers_analyze_installation", "blocker", ("linux", "darwin")
)
_PARAKEET_CPP_INSTALL_FIX = (
    "parakeet-cpp artifacts are not installed — fetch them with: "
    "journal install-provider parakeet"
)
_PARAKEET_CPP_START_FIX = (
    "parakeet-server is not reachable — start the journal service: journal start"
)
_COREML_MODEL_FIX = (
    "CoreML parakeet model is not downloaded — fetch it with: journal install-models"
)
JOURNAL_CAUGHT_UP_CHECK = Check("journal_caught_up", "advisory", ("linux", "darwin"))
JOURNAL_MAINT_TASKS_CHECK = Check("journal_maint_tasks", "blocker", ("linux", "darwin"))
TASK_PACE_CHECK = Check("task_pace", "advisory", ("linux", "darwin"))
CAPTURE_HEALTH_CHECK = Check("capture_health", "advisory", ("linux", "darwin"))
OBSERVER_BINDING_CHECK = Check("observer_binding", "advisory", ("linux", "darwin"))
OBSERVER_INGEST_HEALTH_CHECK = Check(
    "observer_ingest_health", "advisory", ("linux", "darwin")
)
OBSERVER_DELIVERY_STALL_CHECK = Check(
    "observer_delivery_stall", "advisory", ("linux", "darwin")
)
ORPHAN_SEGMENT_PDF_CHECK = Check("orphan_segment_pdf", "advisory", ("linux", "darwin"))
BRAIN_CHECK = Check("brain", "advisory", ("linux", "darwin"))
_CAUGHT_UP_BACKLOG_FIX = (
    "solstone catches up on its own; reprocess a day from the health surface "
    "to prioritize it"
)
_CAUGHT_UP_CANT_TELL_FIX = "re-run journal doctor; check the health logs if it persists"
_MAINT_STALE_MS = 5 * 60 * 1000
_MAINT_TASK_FIX = (
    "inspect with journal maint <task>; re-run with journal maint --force <task>"
)
_TASK_PACE_FIX = (
    "a job is running long; it will be stopped automatically if it passes its cap "
    "— no action needed unless it persists"
)
_OBSERVER_INGEST_FIX = (
    "update or restart the observer, then confirm a valid upload clears the rejection"
)
_CAPTURE_HEALTH_FIX = "open /app/health to inspect observer health"
_OBSERVER_DELIVERY_STALL_FIX = "restart the observer, then confirm a new upload lands"
# Reachability beacons can arrive every few seconds and segment keys are normally
# bounded at five minutes, but a reachable laptop can be awake while delivery is
# intentionally quiet. Use a generous six-hour window so doctor warns only when
# uploads have not landed for a sustained period. See solstone/observe/sense.py:
# 974-978 and solstone/observe/protocol.schema.json:17,:52.
_OBSERVER_DELIVERY_STALL_MS = 6 * 60 * 60 * 1000
_ORPHAN_SEGMENT_PDF_FIX = (
    "journal maint --force settings:007_migrate_pdf_extractions, "
    "then re-run journal doctor"
)
_SUPERVISOR_CONFLICT_FIX = "journal service uninstall"
_SUPERVISOR_CONFLICT_FIX_POINTER_TEMPLATE = (
    "resolve the macOS supervisor conflict first: {fix}"
)
_SUPERVISOR_CONFLICT_FIX_POINTER = _SUPERVISOR_CONFLICT_FIX_POINTER_TEMPLATE.format(
    fix=_SUPERVISOR_CONFLICT_FIX
)
_SUPERVISOR_TOPOLOGY_WARN_POINTER = (
    "resolve the macOS supervisor topology warning before changing the journal service"
)
_UNSAFE_SERVICE_ACTIONS = (
    "journal setup",
    "journal service install",
    "journal service start",
    "journal service restart",
)
_LAUNCHD_STALE_PLIST_REPAIR_FIX = (
    "run journal service uninstall, then run journal service install separately "
    "to reinstall a headless background service"
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
        [str(python_bin), "-c", "from solstone.think.utils import get_journal"],
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
            "from solstone.think.utils import get_journal succeeded outside repo cwd",
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
        or (
            "from solstone.think.utils import get_journal failed with exit "
            f"{probe.returncode}"
        ),
        120,
    )
    return make_result(check, "fail", detail, fix)


def _installed_packaging_versions() -> dict[str, str | None]:
    result: dict[str, str | None] = {}
    for name in (
        "solstone",
        "solstone-journal",
        "solstone-journal-cuda",
        "solstone-journal-host",
    ):
        try:
            result[name] = _pkg_version(name)
        except PackageNotFoundError:
            result[name] = None
    return result


def _installed_journal_leaves(versions: dict[str, str | None]) -> dict[str, str]:
    leaves: dict[str, str] = {}
    for name in ("solstone-journal", "solstone-journal-cuda"):
        version = versions[name]
        if version is not None:
            leaves[name] = version
    return leaves


def _preferred_leaf_name(leaves: dict[str, str]) -> str:
    if "solstone-journal-cuda" in leaves:
        return "solstone-journal-cuda"
    return "solstone-journal"


def journal_leaf_exclusivity_check(args: Args) -> CheckResult:
    del args
    versions = _installed_packaging_versions()
    leaves = _installed_journal_leaves(versions)
    if not leaves:
        return make_result(
            JOURNAL_LEAF_EXCLUSIVITY_CHECK,
            "skip",
            "no journal leaf installed",
        )
    if len(leaves) == 1:
        leaf_name = next(iter(leaves))
        return make_result(
            JOURNAL_LEAF_EXCLUSIVITY_CHECK,
            "ok",
            f"single journal leaf installed: {leaf_name} {leaves[leaf_name]}",
        )
    return make_result(
        JOURNAL_LEAF_EXCLUSIVITY_CHECK,
        "fail",
        (
            "both journal leaves are installed: "
            f"solstone-journal {leaves['solstone-journal']}, "
            f"solstone-journal-cuda {leaves['solstone-journal-cuda']}; "
            "CPU and CUDA ONNX runtimes own the same files"
        ),
        (
            "uninstall both journal leaves, then reinstall exactly one: "
            "pip uninstall -y solstone-journal solstone-journal-cuda; "
            "then pip install solstone-journal OR pip install solstone-journal-cuda"
        ),
    )


def journal_package_version_check(args: Args) -> CheckResult:
    del args
    versions = _installed_packaging_versions()
    solstone_version = versions["solstone"]
    if solstone_version is None:
        return make_result(
            JOURNAL_PACKAGE_VERSION_CHECK,
            "skip",
            "solstone distribution metadata unavailable",
        )
    leaves = _installed_journal_leaves(versions)
    if not leaves:
        return make_result(
            JOURNAL_PACKAGE_VERSION_CHECK,
            "skip",
            "no journal leaf installed",
        )
    for leaf_name, leaf_version in leaves.items():
        if leaf_version == solstone_version:
            return make_result(
                JOURNAL_PACKAGE_VERSION_CHECK,
                "ok",
                (
                    "journal leaf version matches solstone: "
                    f"solstone {solstone_version}, {leaf_name} {leaf_version}"
                ),
            )
    leaf_name = _preferred_leaf_name(leaves)
    return make_result(
        JOURNAL_PACKAGE_VERSION_CHECK,
        "fail",
        (
            "journal package version mismatch: "
            f"solstone {solstone_version}, {leaf_name} {leaves[leaf_name]}; "
            "a bare solstone upgrade may have outrun the journal leaf"
        ),
        (
            "upgrade the installed journal leaf: "
            f"pip install --upgrade {leaf_name}  |  uv tool install --upgrade {leaf_name}"
        ),
    )


def retired_host_shim_check(args: Args) -> CheckResult:
    del args
    versions = _installed_packaging_versions()
    host_version = versions["solstone-journal-host"]
    if host_version is None:
        return make_result(
            RETIRED_HOST_SHIM_CHECK,
            "ok",
            "retired solstone-journal-host not installed",
        )
    leaves = _installed_journal_leaves(versions)
    if not leaves:
        return make_result(
            RETIRED_HOST_SHIM_CHECK,
            "skip",
            (
                f"solstone-journal-host {host_version} installed without a journal "
                "leaf; journal service commands will show migration guidance"
            ),
        )
    leaf_name = _preferred_leaf_name(leaves)
    return make_result(
        RETIRED_HOST_SHIM_CHECK,
        "warn",
        (
            f"retired solstone-journal-host {host_version} is installed alongside "
            f"{leaf_name} {leaves[leaf_name]}"
        ),
        "pip uninstall solstone-journal-host",
    )


def _host_module_present(module: str) -> bool:
    try:
        return importlib.util.find_spec(module) is not None
    except ModuleNotFoundError:
        return False


def host_dependencies_check(args: Args) -> CheckResult:
    del args
    check = HOST_DEPENDENCIES_CHECK
    missing = [
        label
        for module, label in _HOST_DEPENDENCY_MODULES
        if not _host_module_present(module)
    ]
    if missing:
        names = ", ".join(missing)
        return make_result(
            check,
            "fail",
            (
                f"journal host stack incomplete — missing {names}; "
                "the journal host is not installed or is incomplete."
            ),
            HOST_DEPENDENCY_REINSTALL_GUIDANCE,
        )
    return make_result(
        check,
        "ok",
        "journal host dependencies present: python-frontmatter, Flask, ONNX runtime",
    )


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


def _format_unknown_axes(axes: tuple[str, ...]) -> str:
    return ", ".join(axes)


def _foreign_launcher_remedy(matches: tuple[ForeignLauncherMatch, ...]) -> str:
    commands = "; ".join(
        "launchctl bootout "
        f"{shlex.quote(match.service_target)}; rm {shlex.quote(match.plist_path)}"
        for match in matches
    )
    return (
        f"remove foreign launchers targeting {SOLSTONE_APP_BUNDLE_PATH}: "
        f"{commands}; then rerun journal doctor"
    )


def _supervisor_conflict_fix(evidence: SupervisorConflictEvidence) -> str:
    parts: list[str] = []
    if evidence.exact_label_conflict:
        parts.append(_SUPERVISOR_CONFLICT_FIX)
    if evidence.foreign_conflict:
        parts.append(_foreign_launcher_remedy(evidence.foreign.matches))
    return "; ".join(parts)


def _supervisor_conflict_fail_reason(evidence: SupervisorConflictEvidence) -> str:
    reasons: list[str] = []
    if evidence.exact_label_conflict:
        reasons.append(
            "journal.app is running while the legacy LaunchAgent is installed or loaded"
        )
    if evidence.foreign_conflict:
        count = len(evidence.foreign.matches)
        noun = "launcher" if count == 1 else "launchers"
        verb = "targets" if count == 1 else "target"
        reasons.append(
            f"{count} foreign KeepAlive {noun} {verb} {SOLSTONE_APP_BUNDLE_PATH}"
        )
    if evidence.foreign.is_incomplete:
        reasons.append("foreign launcher scan incomplete")
    return "; ".join(reasons)


def _supervisor_topology_unknown_reason(
    unknown_axes: tuple[str, ...], evidence: SupervisorConflictEvidence
) -> str:
    reasons: list[str] = []
    if unknown_axes:
        reasons.append(f"unknown axis(es): {_format_unknown_axes(unknown_axes)}")
    if evidence.foreign.is_incomplete:
        reasons.append("foreign launcher scan incomplete")
    return "; ".join(reasons)


def supervisor_conflict_check(args: Args) -> CheckResult:
    del args
    evidence = inspect_supervisor_conflict()
    if evidence.is_conflict:
        fix = _supervisor_conflict_fix(evidence)
        return make_result(
            SUPERVISOR_CONFLICT_CHECK,
            "fail",
            f"macOS supervisor conflict: {_supervisor_conflict_fail_reason(evidence)} "
            f"({evidence.detail})",
            fix,
        )
    unknown_axes = evidence.unknown_axes
    if unknown_axes or evidence.foreign.is_incomplete:
        return make_result(
            SUPERVISOR_CONFLICT_CHECK,
            "warn",
            (
                "couldn't fully determine macOS supervisor topology; "
                f"{_supervisor_topology_unknown_reason(unknown_axes, evidence)} "
                f"({evidence.detail})"
            ),
        )
    return make_result(
        SUPERVISOR_CONFLICT_CHECK,
        "ok",
        f"no macOS supervisor conflict ({evidence.detail})",
    )


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


def _latest_legacy_backup(install_guard: object, binary: str) -> Path | None:
    matches = list(install_guard._legacy_backup_dir().glob(f"{binary}.old-symlink-*"))
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


# EXAMINE ONLY: detect + report; alias repair is owned by setup's wrapper step
# (install_guard.provision_wrappers) — never mutate aliases here.
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
        backup = _latest_legacy_backup(install_guard, binary)
        if backup is not None:
            return make_result(
                check,
                "warn",
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
    if state is foreign and install_guard.is_app_owned_child_launcher(alias, binary):
        return make_result(
            check,
            "ok",
            f"~/.local/bin/{binary} is an app-owned child launcher for this runtime",
        )

    if state in {cross_repo, dangling, foreign} and other is not None:
        tag = _recognized_legacy_target(other)
        if tag is not None:
            return make_result(
                check,
                "warn",
                f"~/.local/bin/{binary} is a legacy {tag} install ({other})",
                f"another sol/journal CLI is installed at ~/.local/bin/{binary}; run `journal setup` to repair",
            )

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
        "warn",
        fail_detail,
        f"another sol/journal CLI is installed at ~/.local/bin/{binary}; run `journal setup` to repair",
    )


def _discover_project_sources(repo_root: Path) -> list[Path]:
    # Knowingly duplicated with core/crates/solstone-core-sol/src/skills.rs
    # until doctor is ported out of Python.
    sources = []
    for name in _ROUTER_SKILL_NAMES:
        source = repo_root / "solstone" / "talent" / name
        skill_file = source / "SKILL.md"
        if not skill_file.is_file():
            raise FileNotFoundError(f"expected project skill at {skill_file}")
        sources.append(source)
    return sorted(sources)


def _skill_state_problem_detail(
    skills_dir: Path, expected_sources: dict[str, Path]
) -> list[str]:
    problems: list[str] = []
    expected_names = set(expected_sources)

    for name, source in sorted(expected_sources.items()):
        link = skills_dir / name
        if not link.is_symlink():
            problems.append(f"missing router {name} at {link}")
            continue
        target_text = os.readlink(link)
        target_path = (skills_dir / target_text).resolve(strict=False)
        if target_path != source.resolve(strict=False):
            problems.append(f"foreign router {name} at {link} -> {target_text}")

    for link in sorted(skills_dir.iterdir()):
        if link.name in expected_names or not link.is_symlink():
            continue
        problems.append(f"stale skill link {link.name} at {link}")

    return problems


def skill_state_check(args: Args) -> CheckResult:
    del args
    check = SKILL_STATE_CHECK
    if is_packaged_install():
        return make_result(
            check,
            "skip",
            "project skill links are a source-checkout concept",
        )

    journal_text, _source = get_journal_info()
    journal_path = Path(journal_text)
    if not journal_path.exists():
        return make_result(check, "skip", "no local journal")

    try:
        sources = _discover_project_sources(ROOT)
    except Exception as exc:
        return make_result(check, "skip", f"project skill sources unavailable: {exc}")

    expected_sources = {source.name: source for source in sources}
    skill_dirs = [
        journal_path / ".claude" / "skills",
        journal_path / ".agents" / "skills",
    ]
    existing_dirs = [path for path in skill_dirs if path.is_dir()]
    if not existing_dirs:
        return make_result(check, "skip", "no installed project skill dirs")

    problems: list[str] = []
    for skills_dir in existing_dirs:
        problems.extend(_skill_state_problem_detail(skills_dir, expected_sources))

    if not problems:
        names = ", ".join(
            name for name in _ROUTER_SKILL_NAMES if name in expected_sources
        )
        return make_result(
            check,
            "ok",
            f"router skills {names} are installed and current",
        )

    fix = (
        f"repair {', '.join(str(path) for path in existing_dirs)}: run `journal setup` "
        f"or `sol skills install --project {journal_path} --agent all`"
    )
    return make_result(check, "warn", "; ".join(problems), fix)


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
            _LAUNCHD_STALE_PLIST_REPAIR_FIX,
        )
    program_arguments = data.get("ProgramArguments")
    if not isinstance(program_arguments, list) or not program_arguments:
        return make_result(
            check,
            "fail",
            "plist is missing ProgramArguments[0]",
            _LAUNCHD_STALE_PLIST_REPAIR_FIX,
        )
    executable = Path(str(program_arguments[0]))
    if not executable.exists():
        return make_result(
            check,
            "fail",
            f"plist points to missing executable: {executable}",
            _LAUNCHD_STALE_PLIST_REPAIR_FIX,
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


def journal_caught_up_check(args: Args) -> CheckResult:
    del args
    check = JOURNAL_CAUGHT_UP_CHECK
    try:
        from solstone.think.pipeline_health import (
            BACKLOG_STATE_COMPLETE,
            BACKLOG_STATE_UNKNOWN,
            read_backlog_view,
        )

        view = read_backlog_view()
    except Exception as exc:
        return make_result(
            check,
            "warn",
            f"couldn't fully determine — backlog read failed: {type(exc).__name__}: {exc}",
            _CAUGHT_UP_CANT_TELL_FIX,
        )

    unknown_days = sum(1 for day in view.days if day.state == BACKLOG_STATE_UNKNOWN)
    if view.errors or unknown_days:
        return make_result(
            check,
            "warn",
            f"couldn't fully determine — {unknown_days} day(s) unknown",
            _CAUGHT_UP_CANT_TELL_FIX,
        )

    capped_complete_days = sum(
        1
        for day in view.days
        if day.state == BACKLOG_STATE_COMPLETE and day.capped_daily_unit_count > 0
    )
    if view.pending_days == 0 and view.stuck_days == 0 and capped_complete_days > 0:
        return make_result(
            check,
            "ok",
            f"caught up; {capped_complete_days} day(s) completed with capped daily unit(s)",
        )

    if view.pending_days == 0 and view.stuck_days == 0:
        return make_result(check, "ok", "caught up")

    detail = f"{view.pending_days} day(s) pending, {view.stuck_days} day(s) stuck"
    if view.oldest_pending_day:
        detail += f"; oldest outstanding {view.oldest_pending_day}"
    return make_result(check, "warn", detail, _CAUGHT_UP_BACKLOG_FIX)


def journal_maint_tasks_check(args: Args) -> CheckResult:
    del args
    check = JOURNAL_MAINT_TASKS_CHECK
    path_text, _source = get_journal_info()
    journal = Path(path_text)
    if not journal.is_dir():
        return make_result(check, "skip", "no local journal")

    tasks = maint.list_tasks(journal)
    failed = [task for task in tasks if task.get("status") == "failed"]
    if failed:
        detail = "failed maint task(s): " + ", ".join(
            f"{task['qualified_name']} (exit {task.get('exit_code', 'unknown')})"
            for task in failed
        )
        return make_result(check, "fail", detail, _MAINT_TASK_FIX)

    current_ms = now_ms()
    stale = [
        task
        for task in tasks
        if task.get("status") == "in_progress"
        and isinstance(task.get("ran_ts"), int)
        and current_ms - task["ran_ts"] > _MAINT_STALE_MS
    ]
    if stale:
        detail = "started, no exit: " + ", ".join(
            str(task["qualified_name"]) for task in stale
        )
        return make_result(check, "warn", detail, _MAINT_TASK_FIX)

    indeterminate = [
        task
        for task in tasks
        if task.get("status") == "in_progress"
        and not isinstance(task.get("ran_ts"), int)
    ]
    if indeterminate:
        detail = "couldn't fully determine — maint state unreadable: " + ", ".join(
            str(task["qualified_name"]) for task in indeterminate
        )
        return make_result(check, "warn", detail, _MAINT_TASK_FIX)

    return make_result(check, "ok", "no unresolved maint tasks")


def task_pace_check(args: Args) -> CheckResult:
    del args
    check = TASK_PACE_CHECK
    status = fetch_supervisor_status()
    if status is None:
        return make_result(check, "skip", "supervisor status unavailable")
    tasks = status.get("tasks") or []
    slow = [t for t in tasks if t.get("slow") or t.get("stuck")]
    if not slow:
        return make_result(check, "ok", "tasks on pace")
    names = ", ".join(
        f"{t.get('name', '?')} "
        f"({t.get('duration_seconds', 0)}s of {t.get('max_runtime_seconds', '?')}s cap)"
        for t in slow
    )
    return make_result(check, "warn", f"running long: {names}", _TASK_PACE_FIX)


def brain_check(args: Args) -> CheckResult:
    from solstone.think.brain_health import build_brain_snapshot

    del args
    check = BRAIN_CHECK
    try:
        snapshot = build_brain_snapshot(datetime.now(timezone.utc), surface="cli")
    except Exception as exc:
        return make_result(check, "warn", f"unknown: {exc}")
    state = snapshot["state"]
    headline = snapshot["headline"]
    reason = snapshot["reason_text"]
    component = snapshot["failing_component"] or "none"
    age = snapshot["evidence"]["age_text"] or "unknown"
    detail = f"{headline}; state={state}; reason={reason}; component={component}; evidence_age={age}"
    if state in {"ready", "checking"}:
        return make_result(check, "ok", detail)
    return make_result(check, "warn", detail)


def _capture_health_observer_summary(observers: list[dict]) -> str:
    summaries = []
    for observer in observers[:3]:
        status = observer.get("status", "unknown")
        summaries.append(f"{observer.get('name', 'unknown')}={status}")
    if len(observers) > 3:
        summaries.append(f"+{len(observers) - 3} more")
    return ", ".join(summaries) if summaries else "none"


def capture_health_check(args: Args) -> CheckResult:
    del args
    check = CAPTURE_HEALTH_CHECK
    result = get_capture_health()
    status = result["status"]
    if status == "active":
        return make_result(
            check,
            "ok",
            "rollup=active; observers reaching the journal",
        )
    if status in {"stale", "offline", "degraded"}:
        detail = (
            f"rollup={status}; observers: "
            f"{_capture_health_observer_summary(result['observers'])}"
        )
        return make_result(check, "warn", truncate(detail, 400), _CAPTURE_HEALTH_FIX)
    if status == "no_observers":
        return make_result(
            check,
            "skip",
            "rollup=no_observers; no registered observers",
        )
    return make_result(
        check,
        "skip",
        f"rollup={status}; observer records unavailable",
    )


def observer_binding_check(args: Args) -> CheckResult:
    del args
    result = get_capture_health()
    observers = result.get("observers", [])
    total = len(observers)
    unbound = [
        observer
        for observer in observers
        if observer.get("device_binding_kind") is None
    ]
    if not unbound:
        return make_result(
            OBSERVER_BINDING_CHECK,
            "ok",
            f"active observer records={total}; unbound=0",
        )
    names = ", ".join(str(observer.get("name", "unknown")) for observer in unbound)
    return make_result(
        OBSERVER_BINDING_CHECK,
        "ok",
        f"active observer records={total}; unbound={len(unbound)}; streams={names}",
    )


def _observer_delivery_stall_clause(
    observer: dict,
    facts: dict,
) -> str:
    name = facts["name"]
    last_seen_minutes = facts["last_seen_age_ms"] // 60_000
    last_segment_minutes = facts["last_segment_received_age_ms"] // 60_000
    clause = (
        f"observer {name} is reaching the journal; last reach "
        f"{last_seen_minutes}m ago, last upload landed {last_segment_minutes}m ago"
    )

    stats = observer.get("stats")
    duplicates = stats.get("duplicates_rejected") if isinstance(stats, dict) else None
    if (
        isinstance(duplicates, int)
        and not isinstance(duplicates, bool)
        and duplicates > 0
    ):
        return (
            f"{clause}; prior duplicate responses={duplicates}, so repeated uploads "
            "may be landing without a newer upload"
        )

    health = observer.get("health")
    beacon = health.get("beacon") if isinstance(health, dict) else None
    if isinstance(beacon, dict):
        pending = beacon.get("pending_queue_depth")
        if isinstance(pending, int) and not isinstance(pending, bool):
            return (
                f"{clause}; pending queue depth {pending}, so uploads may not be "
                "landing"
            )

    return f"{clause}; uploads may not be landing"


def observer_delivery_stall_check(args: Args) -> CheckResult:
    del args
    check = OBSERVER_DELIVERY_STALL_CHECK
    try:
        from solstone.apps.observer.utils import (
            get_delivery_divergence,
            list_observers,
        )

        observers = list_observers()
    except Exception as exc:
        return make_result(
            check,
            "skip",
            f"observer records unavailable: {type(exc).__name__}: {exc}",
        )
    enabled = [
        observer
        for observer in observers
        if not observer.get("revoked", False) and observer.get("enabled", True)
    ]
    if not enabled:
        return make_result(check, "skip", "no registered observers")

    current_ms = now_ms()
    facts_by_observer = [
        (
            observer,
            get_delivery_divergence(
                observer,
                now_ms=current_ms,
                reachable_within_ms=_STALE_MS,
            ),
        )
        for observer in enabled
    ]
    assessed = [
        (observer, facts)
        for observer, facts in facts_by_observer
        if isinstance(facts, dict)
    ]
    failing = [
        (observer, facts)
        for observer, facts in assessed
        if facts["last_segment_received_age_ms"] > _OBSERVER_DELIVERY_STALL_MS
    ]
    if not failing:
        if assessed:
            return make_result(check, "ok", "every observer is delivering")
        return make_result(
            check,
            "ok",
            "delivery could not be assessed for any observer",
        )

    detail = "; ".join(
        _observer_delivery_stall_clause(observer, facts) for observer, facts in failing
    )
    return make_result(
        check,
        "warn",
        truncate(detail, 400),
        _OBSERVER_DELIVERY_STALL_FIX,
    )


def observer_ingest_health_check(args: Args) -> CheckResult:
    del args
    check = OBSERVER_INGEST_HEALTH_CHECK
    try:
        from solstone.apps.observer.utils import (
            get_active_ingest_rejection,
            list_observers,
        )

        observers = list_observers()
    except Exception as exc:
        return make_result(
            check,
            "skip",
            f"observer records unavailable: {type(exc).__name__}: {exc}",
        )
    enabled = [
        observer
        for observer in observers
        if not observer.get("revoked", False) and observer.get("enabled", True)
    ]
    if not enabled:
        return make_result(check, "skip", "no registered observers")
    failing = [
        (observer, get_active_ingest_rejection(observer)) for observer in enabled
    ]
    failing = [(observer, rejection) for (observer, rejection) in failing if rejection]
    if not failing:
        return make_result(check, "ok", "no observers failing ingest")
    clauses = []
    for observer, rejection in failing:
        version = rejection.get("version")
        vtext = f"v{version}" if version else "version unknown"
        first_ts = rejection.get("first_ts")
        when = (
            time.strftime("%Y-%m-%d", time.gmtime(first_ts / 1000))
            if isinstance(first_ts, (int, float))
            else "unknown"
        )
        clauses.append(
            f"observer {observer.get('name', 'unknown')} ({vtext}) failing ingest: "
            f"{rejection.get('summary', '')}, "
            f"{rejection.get('active_count', 0)}x since {when}"
        )
    detail = "; ".join(clauses)
    return make_result(check, "warn", detail[:400], _OBSERVER_INGEST_FIX)


def orphan_segment_pdf_check(args: Args) -> CheckResult:
    """Warn when a raw PDF original has no document transcript beside it.

    For the owner, this means the original is present in the journal as raw
    media but the readable document transcript is missing, so catchup cannot
    use that PDF's text until the maintenance task migrates or rebuilds it.
    """
    del args
    check = ORPHAN_SEGMENT_PDF_CHECK
    journal_text, _source = get_journal_info()
    chronicle = Path(journal_text) / "chronicle"
    if not chronicle.is_dir():
        return make_result(check, "skip", "chronicle directory unavailable")

    orphan_paths: list[str] = []
    for pdf_path in sorted(chronicle.glob("*/*/*/*")):
        if not pdf_path.is_file() or pdf_path.suffix.lower() not in PDF_EXTENSIONS:
            continue
        if any(pdf_path.parent.glob("*_transcript.md")):
            continue
        try:
            display_path = pdf_path.relative_to(Path(journal_text)).as_posix()
        except ValueError:
            display_path = str(pdf_path)
        orphan_paths.append(display_path)

    if not orphan_paths:
        return make_result(
            check,
            "ok",
            "all raw PDF originals have readable document transcripts",
        )

    paths_text = truncate(", ".join(orphan_paths), 360)
    detail = (
        f"{len(orphan_paths)} raw PDF original(s) without a readable document "
        f"transcript: {paths_text}"
    )
    return make_result(check, "warn", detail, _ORPHAN_SEGMENT_PDF_FIX)


def _resolve_configured_backend() -> str | None:
    """Read transcribe.backend without materializing the journal directory."""
    path_text, _source = get_journal_info()
    config_path = Path(path_text) / "config" / "journal.json"
    config = _read_existing_journal_config(config_path)
    if config is None:
        return None
    backend = config.get("transcribe", {}).get("backend")
    return backend if isinstance(backend, str) else None


def _parakeet_cpp_ready_result(check: Check) -> CheckResult:
    os_name, arch = parakeet_readiness._platform_info()
    if os_name != "linux":
        return make_result(check, "skip", "parakeet-cpp is only supported on Linux")
    path_text, _source = get_journal_info()
    cache_root = parakeet_readiness.parakeet_cpp_cache_root(Path(path_text))
    try:
        artifact_key = parakeet_readiness.parakeet_cpp_artifact_key(os_name, arch)
    except RuntimeError as exc:
        return make_result(check, "warn", str(exc), _PARAKEET_CPP_INSTALL_FIX)
    try:
        paths = parakeet_readiness.check_parakeet_cpp_files(cache_root, artifact_key)
    except RuntimeError as exc:
        return make_result(check, "warn", str(exc), _PARAKEET_CPP_INSTALL_FIX)
    binary_probe = parakeet_readiness.probe_parakeet_cpp_binary(paths["binary_cpu"])
    if not binary_probe.runnable:
        if binary_probe.reason_code == parakeet_readiness.OPENMP_RUNTIME_UNAVAILABLE:
            return make_result(
                check,
                "warn",
                "parakeet-cpp cannot start: OpenMP runtime unavailable (libgomp.so.1)",
                parakeet_readiness.openmp_runtime_install_guidance(),
            )
        return make_result(
            check,
            "warn",
            "parakeet-cpp binary cannot start",
            _PARAKEET_CPP_INSTALL_FIX,
        )
    from solstone.think.providers import parakeet_server

    state, error = parakeet_server.probe_state()
    if state != parakeet_server.STATE_READY:
        return make_result(
            check,
            "warn",
            f"parakeet-server not reachable: {error}",
            _PARAKEET_CPP_START_FIX,
        )
    return make_result(
        check, "ok", "parakeet-cpp ready (binaries + model installed, server reachable)"
    )


def default_stt_ready_check(args: Args) -> CheckResult:
    del args
    check = DEFAULT_STT_READY_CHECK
    try:
        backend = _resolve_configured_backend()
    except CorruptConfigError as exc:
        return make_result(check, "fail", str(exc), fix=_CORRUPT_CONFIG_FIX)
    if backend and backend != "parakeet":
        return make_result(
            check,
            "skip",
            f"configured backend is {backend}; parakeet readiness not applicable",
        )

    os_name, arch = parakeet_readiness._platform_info()
    if os_name == "linux" and arch == "x86_64":
        return _parakeet_cpp_ready_result(check)
    if os_name == "darwin" and arch == "arm64":
        try:
            ready_cache = parakeet_readiness._check_parakeet_ready(
                os_name,
                arch,
                "coreml",
                parakeet_readiness._sentinel_path("coreml"),
            )
        except RuntimeError as exc:
            return make_result(check, "warn", str(exc), _COREML_MODEL_FIX)
        return make_result(check, "ok", f"parakeet model ready at {ready_cache}")
    return make_result(check, "skip", "parakeet not supported on this platform")


def parakeet_cpp_stt_ready_check(args: Args) -> CheckResult:
    del args
    check = PARAKEET_CPP_STT_READY_CHECK
    try:
        backend = _resolve_configured_backend()
    except CorruptConfigError as exc:
        return make_result(check, "fail", str(exc), fix=_CORRUPT_CONFIG_FIX)
    if backend != "parakeet-cpp":
        return make_result(
            check,
            "skip",
            "configured backend is not parakeet-cpp; check not applicable",
        )
    return _parakeet_cpp_ready_result(check)


def speakers_analyze_installation_check(args: Args) -> CheckResult:
    del args
    from solstone.think.speakers_analyze_installation import (
        check_speakers_analyze_installation,
        speakers_analyze_repair_text,
    )

    check = SPEAKERS_ANALYZE_INSTALLATION_CHECK
    result = check_speakers_analyze_installation()
    if result.ok:
        return make_result(check, "ok", "speakers-analyze installation ready")
    return make_result(check, "fail", result.message, speakers_analyze_repair_text())


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
    (JOURNAL_LEAF_EXCLUSIVITY_CHECK, journal_leaf_exclusivity_check),
    (JOURNAL_PACKAGE_VERSION_CHECK, journal_package_version_check),
    (RETIRED_HOST_SHIM_CHECK, retired_host_shim_check),
    (LOCAL_BIN_SOL_REACHABLE_CHECK, local_bin_sol_reachable_check),
    (STALE_ALIAS_CHECK, partial(stale_alias_symlink_check, binary="sol")),
    (SKILL_STATE_CHECK, skill_state_check),
]

JOURNAL_CHECKS: list[tuple[Check, Runner]] = [
    (JOURNAL_LEAF_EXCLUSIVITY_CHECK, journal_leaf_exclusivity_check),
    (JOURNAL_PACKAGE_VERSION_CHECK, journal_package_version_check),
    (RETIRED_HOST_SHIM_CHECK, retired_host_shim_check),
    (HOST_DEPENDENCIES_CHECK, host_dependencies_check),
    (DISK_SPACE_CHECK, disk_space_check),
    (CONFIG_DIR_READABLE_CHECK, config_dir_readable_check),
    (JOURNAL_DIR_WRITABLE_CHECK, journal_dir_writable_journal),
    (SUPERVISOR_CONFLICT_CHECK, supervisor_conflict_check),
    (SERVICE_IDENTITY_CHECK, service_identity_check),
    (SERVICE_RUNNING_CHECK, service_running_check),
    (JOURNAL_SYNC_CHECK, journal_sync_check),
    (JOURNAL_CAUGHT_UP_CHECK, journal_caught_up_check),
    (JOURNAL_MAINT_TASKS_CHECK, journal_maint_tasks_check),
    (TASK_PACE_CHECK, task_pace_check),
    (BRAIN_CHECK, brain_check),
    (CAPTURE_HEALTH_CHECK, capture_health_check),
    (OBSERVER_BINDING_CHECK, observer_binding_check),
    (OBSERVER_DELIVERY_STALL_CHECK, observer_delivery_stall_check),
    (OBSERVER_INGEST_HEALTH_CHECK, observer_ingest_health_check),
    (ORPHAN_SEGMENT_PDF_CHECK, orphan_segment_pdf_check),
    (STALE_ALIAS_CHECK, partial(stale_alias_symlink_check, binary="journal")),
    (LAUNCHD_STALE_PLIST_CHECK, launchd_stale_plist_check),
    (DEFAULT_STT_READY_CHECK, default_stt_ready_check),
    (PARAKEET_CPP_STT_READY_CHECK, parakeet_cpp_stt_ready_check),
    (SPEAKERS_ANALYZE_INSTALLATION_CHECK, speakers_analyze_installation_check),
    (SKILL_STATE_CHECK, skill_state_check),
    *FEATURE_CHECKS.values(),
]

READINESS_CHECKS: list[tuple[Check, Runner]] = [
    (PYTHON_VERSION_CHECK, python_sanity_check),
    (SOL_IMPORTABLE_CHECK, sol_importable_check),
    (LOCAL_BIN_SOL_REACHABLE_CHECK, local_bin_sol_reachable_check),
    (STALE_ALIAS_CHECK, partial(stale_alias_symlink_check, binary="sol")),
    (DISK_SPACE_CHECK, disk_space_check),
    (JOURNAL_DIR_WRITABLE_CHECK, journal_dir_writable_readiness),
]

JOURNAL_READINESS_CHECKS: list[tuple[Check, Runner]] = [
    (HOST_DEPENDENCIES_CHECK, host_dependencies_check),
    *READINESS_CHECKS,
    (DEFAULT_STT_READY_CHECK, default_stt_ready_check),
    (PARAKEET_CPP_STT_READY_CHECK, parakeet_cpp_stt_ready_check),
    (SPEAKERS_ANALYZE_INSTALLATION_CHECK, speakers_analyze_installation_check),
    *FEATURE_CHECKS.values(),
]

_ALL_CHECKS = (
    UNIVERSAL_CHECKS + JOURNAL_CHECKS + READINESS_CHECKS + JOURNAL_READINESS_CHECKS
)
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
        if sys.argv[0] == "journal doctor":
            return JOURNAL_READINESS_CHECKS
        return READINESS_CHECKS
    if sys.argv[0] == "journal doctor":
        return JOURNAL_CHECKS
    return UNIVERSAL_CHECKS


def _fix_mentions_unsafe_service_action(fix: str) -> bool:
    return any(action in fix for action in _UNSAFE_SERVICE_ACTIONS)


def _apply_supervisor_conflict_fix_policy(
    results: list[CheckResult],
) -> list[CheckResult]:
    conflict = next(
        (result for result in results if result.name == SUPERVISOR_CONFLICT_CHECK.name),
        None,
    )
    if conflict is None or conflict.status not in {"fail", "warn"}:
        return results

    conflict_execution_error = has_execution_error(conflict)
    updated: list[CheckResult] = []
    for result in results:
        if result.name == SUPERVISOR_CONFLICT_CHECK.name or not result.fix:
            updated.append(result)
            continue
        if conflict.status == "fail" and not conflict_execution_error:
            pointer = _SUPERVISOR_CONFLICT_FIX_POINTER_TEMPLATE.format(fix=conflict.fix)
            updated.append(replace(result, fix=pointer))
            continue
        if _fix_mentions_unsafe_service_action(result.fix):
            updated.append(replace(result, fix=_SUPERVISOR_TOPOLOGY_WARN_POINTER))
            continue
        updated.append(result)
    return updated


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
        return [run_check(check, runner, args)]

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
        results.append(run_check(check, func, args))
    return _apply_supervisor_conflict_fix_policy(results)


def print_result_line(result: CheckResult) -> None:
    label = status_label(result)
    print(f"  {label} {result.name} — {result.detail}")
    if result.fix:
        print(f"    → {result.fix}")


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
        f"{summary['skipped']} skipped, "
        f"{summary['errors']} errors"
    )


def emit_json(results: Sequence[CheckResult]) -> None:
    payload = {
        "checks": [check_result_to_json_dict(result) for result in results],
        "summary": summary_counts(results),
    }
    print(json.dumps(payload))


def solstone_version() -> str:
    try:
        return _pkg_version("solstone")
    except PackageNotFoundError:
        return "0.0.0+source"


def jsonl_summary_status(results: Sequence[CheckResult]) -> str:
    if results_failed(results):
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
            execution_error=(
                result.execution_error.to_dict()
                if has_execution_error(result)
                else None
            ),
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
    return 1 if results_failed(results) else 0
