# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Stdlib-only install-readiness checks for `make preflight`.

This battery can run before `.venv` or `uv` exist. It composes the stdlib-only
checks from `solstone.think.probe`.

Exit code `0` means no blocker-severity check failed; exit code `1` means at
least one blocker-severity check failed.
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
    UV_INSTALLED_CHECK,
    VENV_CONSISTENT_CHECK,
    Check,
    CheckResult,
    config_dir_readable_check,
    disk_space_check,
    local_bin_sol_reachable_check,
    make_result,
    platform_tag,
    python_version_check,
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
        "preflight: "
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


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    results = run_checks(args)
    if args.json:
        emit_json(results)
    else:
        emit_text(results, verbose=args.verbose)
    blocker_failed = any(
        result.severity == "blocker" and result.status == "fail" for result in results
    )
    return 1 if blocker_failed else 0
