# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Stdlib-only install-readiness checks for `make preflight`.

This battery can run before `.venv` or `uv` exist. It composes the stdlib-only
checks from `solstone.think.probe`.

Exit code `0` means no blocker-severity check failed and no check raised during
execution; exit code `1` means at least one blocker-severity check failed or any
check raised during execution.
"""

from __future__ import annotations

import argparse
import json
from dataclasses import dataclass
from typing import Callable, Sequence

from solstone.think.probe import (
    CONFIG_DIR_READABLE_CHECK,
    DISK_SPACE_CHECK,
    LOCAL_BIN_SOL_REACHABLE_CHECK,
    PYTHON_VERSION_CHECK,
    SOLSTONE_CORE_NATIVE_BUILD_DEPENDENCIES_CHECK,
    SOLSTONE_CORE_RUST_TOOLCHAIN_CHECK,
    UV_INSTALLED_CHECK,
    VENV_CONSISTENT_CHECK,
    Check,
    CheckResult,
    check_result_to_json_dict,
    config_dir_readable_check,
    disk_space_check,
    local_bin_sol_reachable_check,
    make_result,
    platform_tag,
    python_version_check,
    results_failed,
    run_check,
    solstone_core_native_build_dependencies_check,
    solstone_core_rust_toolchain_check,
    status_label,
    summary_counts,
    uv_installed_check,
    venv_consistent_check,
)


@dataclass(frozen=True)
class Args:
    verbose: bool
    json: bool


CHECKS: list[tuple[Check, Callable[[Args], CheckResult]]] = [
    (PYTHON_VERSION_CHECK, python_version_check),
    (UV_INSTALLED_CHECK, uv_installed_check),
    (SOLSTONE_CORE_RUST_TOOLCHAIN_CHECK, solstone_core_rust_toolchain_check),
    (
        SOLSTONE_CORE_NATIVE_BUILD_DEPENDENCIES_CHECK,
        solstone_core_native_build_dependencies_check,
    ),
    (VENV_CONSISTENT_CHECK, venv_consistent_check),
    (LOCAL_BIN_SOL_REACHABLE_CHECK, local_bin_sol_reachable_check),
    (DISK_SPACE_CHECK, disk_space_check),
    (CONFIG_DIR_READABLE_CHECK, config_dir_readable_check),
]

CHECK_MAP = {check.name: check for check, _func in CHECKS}


def parse_args(argv: Sequence[str] | None = None) -> Args:
    parser = argparse.ArgumentParser(
        description="Run stdlib-only install-readiness checks for solstone.",
    )
    parser.add_argument(
        "--verbose", action="store_true", help="print every check result"
    )
    parser.add_argument("--json", action="store_true", help="emit JSON instead of text")
    namespace = parser.parse_args(argv)
    return Args(
        verbose=namespace.verbose,
        json=namespace.json,
    )


def run_checks(args: Args) -> list[CheckResult]:
    current_platform = platform_tag()
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
        results.append(run_check(check, func, args))
    return results


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
        "preflight: "
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


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    results = run_checks(args)
    if args.json:
        emit_json(results)
    else:
        emit_text(results, verbose=args.verbose)
    return 1 if results_failed(results) else 0
