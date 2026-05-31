# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI for viewing service health logs.

Usage:
    journal health logs                    Show last 5 lines from each service
    journal health logs -c 20              Show last 20 lines from each service
    journal health logs -f                 Follow all logs for new output
    journal health logs --since 30m        Lines from last 30 minutes
    journal health logs --service observer Only show observer logs
    journal health logs --grep "error"     Lines matching regex "error"
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import time
from datetime import datetime, timedelta
from pathlib import Path
from typing import NamedTuple

from solstone.think.utils import day_path, get_journal, setup_cli

_DIM = "\033[2m"
_RESET = "\033[0m"


class LogLine(NamedTuple):
    timestamp: datetime
    service: str
    stream: str
    message: str
    raw: str


def _service_header(service: str, use_color: bool) -> str:
    header = f"── {service} ──"
    if use_color:
        return f"{_DIM}{header}{_RESET}"
    return header


def parse_log_line(line: str) -> LogLine | None:
    stripped = line.rstrip()
    if len(stripped) < 20:
        return None

    try:
        timestamp = datetime.fromisoformat(stripped[0:19])
    except ValueError:
        return None

    open_idx = stripped.find("[", 19)
    if open_idx == -1:
        return None

    close_idx = stripped.find("]", open_idx + 1)
    if close_idx == -1:
        return None

    bracket = stripped[open_idx + 1 : close_idx]
    parts = bracket.rsplit(":", 1)
    if len(parts) != 2:
        return None
    service, stream = parts

    message_start = close_idx + 2
    message = stripped[message_start:] if message_start <= len(stripped) else ""

    return LogLine(timestamp, service, stream, message, stripped)


def parse_since(spec: str) -> datetime:
    spec = spec.strip()
    match = re.fullmatch(r"(\d+)([mhd])", spec)
    if match:
        amount = int(match.group(1))
        unit = match.group(2)
        unit_map = {"m": "minutes", "h": "hours", "d": "days"}
        return datetime.now() - timedelta(**{unit_map[unit]: amount})

    spec_upper = spec.upper()
    formats = ["%I:%M%p", "%I%p", "%H:%M"]
    for fmt in formats:
        try:
            parsed = datetime.strptime(spec_upper, fmt)
            now = datetime.now()
            return parsed.replace(year=now.year, month=now.month, day=now.day)
        except ValueError:
            continue

    raise argparse.ArgumentTypeError(
        f"Invalid time: {spec!r}. Use e.g., 30m, 2h, 1d, 4pm, 16:00"
    )


def compile_grep(pattern: str) -> re.Pattern[str]:
    try:
        return re.compile(pattern)
    except re.error as error:
        raise argparse.ArgumentTypeError(
            f"Invalid Python regex: {pattern!r}: {error}"
        ) from error


def get_today_health_dir() -> Path | None:
    health_dir = day_path(create=False) / "health"
    return health_dir if health_dir.is_dir() else None


def get_day_log_files(health_dir: Path) -> list[Path]:
    return sorted(p for p in health_dir.glob("*.log") if p.is_symlink())


def tail_lines(path: Path, n: int) -> list[str]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
        return lines[-n:] if n else lines
    except OSError:
        return []


def tail_lines_large(path: Path, n: int) -> list[str]:
    try:
        with path.open("rb") as f:
            f.seek(0, 2)
            size = f.tell()
            if size == 0:
                return []
            chunk_size = 65536
            lines: list[str] = []
            remaining = size
            while remaining > 0 and len(lines) < n + 1:
                read_size = min(chunk_size, remaining)
                remaining -= read_size
                f.seek(remaining)
                chunk = f.read(read_size).decode("utf-8", errors="replace")
                lines = chunk.splitlines() + lines
            return lines[-n:]
    except OSError:
        return []


def _matches_filters(line: LogLine, args: argparse.Namespace) -> bool:
    if args.since and line.timestamp < args.since:
        return False
    if args.service and line.service != args.service:
        return False
    if args.grep and not args.grep.search(line.raw):
        return False
    return True


def collect_and_print(args: argparse.Namespace) -> None:
    has_filters = args.since or args.service or args.grep
    include_supervisor = not has_filters

    lines: list[LogLine] = []

    health_dir = get_today_health_dir()
    if health_dir:
        for log_path in get_day_log_files(health_dir):
            raw_lines = (
                tail_lines(log_path, 0) if has_filters else tail_lines(log_path, args.c)
            )
            for raw in raw_lines:
                parsed = parse_log_line(raw)
                if parsed and _matches_filters(parsed, args):
                    lines.append(parsed)

    if include_supervisor:
        journal = Path(os.path.expanduser(get_journal()))
        sup_path = journal / "health" / "supervisor.log"
        if sup_path.exists():
            raw_lines = tail_lines_large(sup_path, args.c)
            for raw in raw_lines:
                parsed = parse_log_line(raw)
                if parsed:
                    lines.append(parsed)

    lines.sort(key=lambda line: line.timestamp)
    if has_filters and args.c:
        lines = lines[-args.c :]
    use_color = sys.stdout.isatty()
    last_service = None
    for line in lines:
        if use_color and line.service != last_service:
            if last_service is not None:
                print()
            print(_service_header(line.service, use_color))
            last_service = line.service
        print(line.raw)


def follow_logs(args: argparse.Namespace) -> None:
    journal = Path(os.path.expanduser(get_journal()))
    health_dir = journal / "health"
    if not health_dir.is_dir():
        print("No health directory found.", file=sys.stderr)
        return

    last_service = None
    use_color = sys.stdout.isatty()
    tracked: dict[Path, tuple[Path | None, object]] = {}

    def open_logs() -> None:
        for log_path in sorted(health_dir.glob("*.log")):
            if log_path not in tracked:
                try:
                    resolved = log_path.resolve()
                    fh = open(resolved, "r", encoding="utf-8")
                    fh.seek(0, 2)
                    tracked[log_path] = (resolved, fh)
                except OSError:
                    pass

    open_logs()

    if not tracked:
        print("No log files found.", file=sys.stderr)
        return

    last_check = time.monotonic()
    try:
        while True:
            for symlink, (resolved, fh) in list(tracked.items()):
                line = fh.readline()
                while line:
                    line = line.rstrip("\n")
                    if line:
                        parsed = parse_log_line(line)
                        current_service = parsed.service if parsed else None
                        if (
                            use_color
                            and current_service
                            and current_service != last_service
                        ):
                            if last_service is not None:
                                print(flush=True)
                            print(
                                _service_header(current_service, use_color), flush=True
                            )
                            last_service = current_service
                        print(line, flush=True)
                    line = fh.readline()

            now = time.monotonic()
            if now - last_check >= 2.0:
                last_check = now
                for symlink in list(tracked):
                    if symlink.is_symlink():
                        new_target = symlink.resolve()
                        old_target, fh = tracked[symlink]
                        if new_target != old_target:
                            fh.close()
                            try:
                                new_fh = open(new_target, "r", encoding="utf-8")
                                tracked[symlink] = (new_target, new_fh)
                            except OSError:
                                del tracked[symlink]
                open_logs()

            time.sleep(0.2)
    except KeyboardInterrupt:
        pass
    finally:
        for _, (_, fh) in tracked.items():
            try:
                fh.close()
            except Exception:
                pass


def main() -> None:
    parser = argparse.ArgumentParser(
        description="View service health logs",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "-c",
        type=int,
        default=5,
        metavar="N",
        help="number of lines per log (default: 5)",
    )
    parser.add_argument(
        "-f",
        action="store_true",
        help="follow logs for new output",
    )
    parser.add_argument(
        "--since",
        type=parse_since,
        metavar="TIME",
        help="show lines since TIME (e.g., 30m, 2h, 4pm, 16:00)",
    )
    parser.add_argument(
        "--service",
        metavar="NAME",
        help="filter to a specific service",
    )
    parser.add_argument(
        "--grep",
        type=compile_grep,
        metavar="PATTERN",
        help="filter lines matching Python regex PATTERN",
    )
    args = setup_cli(parser)

    if args.f:
        follow_logs(args)
    else:
        collect_and_print(args)


if __name__ == "__main__":
    main()
