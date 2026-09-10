# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Command-line entry point for the local thinking install operator harness."""

from __future__ import annotations

import argparse
import signal
import sys
from pathlib import Path

from .harness import run_harness


def _parse_args(args: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="python3 -m tools.local_thinking_install",
        description=(
            "Operator test harness for local thinking installation lifecycle.\n\n"
            "Runs isolated supervisor portal instances against fresh and prior-MLX "
            "journals to test the local install admission contract via "
            "POST /app/thinking/api/local/bootstrap."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )

    parser.add_argument(
        "--candidate-dir",
        type=Path,
        default=None,
        help="Absolute directory path containing solstone-core-journal and solstone-core binaries.",
    )
    parser.add_argument(
        "--candidate-bin",
        type=Path,
        default=None,
        help="Absolute path to solstone-core-journal candidate binary (candidate-dir derived from parent).",
    )
    parser.add_argument(
        "--run-dir",
        type=Path,
        required=True,
        help="Absolute directory path for disposable test execution, logs, and receipts.",
    )
    parser.add_argument(
        "--timeout-seconds",
        type=float,
        default=60.0,
        help="Timeout in seconds for supervisor portal startup (default: 60.0s).",
    )
    parser.add_argument(
        "--install-timeout-seconds",
        type=float,
        default=3600.0,
        help="Timeout in seconds for post-admission install polling (default: 3600.0s).",
    )

    return parser.parse_args(args)


def main(argv: list[str] | None = None) -> int:
    args = _parse_args(argv or sys.argv[1:])

    if args.candidate_bin and args.candidate_dir:
        sys.stderr.write("Error: Specify either --candidate-dir or --candidate-bin, not both.\n")
        return 2

    if not args.candidate_bin and not args.candidate_dir:
        sys.stderr.write("Error: Must specify either --candidate-dir or --candidate-bin.\n")
        return 2

    candidate_dir = args.candidate_bin.parent if args.candidate_bin else args.candidate_dir
    run_dir = args.run_dir

    if not candidate_dir.is_absolute():
        sys.stderr.write(
            f"Error: Candidate path must be absolute (got '{candidate_dir}').\n"
        )
        return 2

    if not run_dir.is_absolute():
        sys.stderr.write(
            f"Error: --run-dir must be an absolute path (got '{run_dir}').\n"
        )
        return 2

    if run_dir.exists():
        sys.stderr.write("Error: --run-dir must not already exist.\n")
        return 2

    def handle_signal(signum: int, _frame: object) -> None:
        sys.stderr.write(f"\nReceived signal {signum}, aborting run...\n")
        receipt_txt = run_dir / "RECEIPT.txt"
        if run_dir.is_dir() and not receipt_txt.exists():
            receipt_txt.write_text(
                f"OVERALL OUTCOME: INTERRUPTED (signal {signum})\n", encoding="utf-8"
            )
        raise KeyboardInterrupt()

    signal.signal(signal.SIGINT, handle_signal)
    signal.signal(signal.SIGTERM, handle_signal)

    try:
        report = run_harness(
            candidate_dir=candidate_dir,
            run_dir=run_dir,
            portal_timeout_seconds=args.timeout_seconds,
            install_timeout_seconds=args.install_timeout_seconds,
        )
    except KeyboardInterrupt:
        sys.stderr.write("Harness interrupted by user/signal.\n")
        return 130
    except Exception as error:
        sys.stderr.write(f"Harness execution error: {error}\n")
        receipt_txt = run_dir / "RECEIPT.txt"
        if run_dir.is_dir() and not receipt_txt.exists():
            receipt_txt.write_text(
                f"OVERALL OUTCOME: HARNESS_INFRA ({error})\n", encoding="utf-8"
            )
        return 1

    receipt_txt = run_dir / "RECEIPT.txt"
    if receipt_txt.exists():
        sys.stdout.write(receipt_txt.read_text(encoding="utf-8"))

    return 0 if report.all_passed else 1


if __name__ == "__main__":
    sys.exit(main())
