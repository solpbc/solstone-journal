# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI commands for transcript browsing.

Provides human-friendly CLI access to transcript operations, paralleling the
transcript helper functions in ``think.cluster`` but optimized for terminal use.

Auto-discovered by ``think.call`` and mounted as ``sol call transcripts ...``.
"""

import typer

from solstone.think.cluster import (
    cluster,
    cluster_period,
    cluster_range,
    cluster_scan,
    cluster_segments,
    cluster_span,
    scan_day,
)
from solstone.think.data_state import DataState
from solstone.think.utils import (
    day_dirs,
    get_sol_stream,
    resolve_sol_day,
    resolve_sol_segment,
    truncated_echo,
)

app = typer.Typer(help="Transcript browsing.")


def _pending_slot_range(start: str) -> tuple[str, str]:
    hour_s, minute_s = start.split(":")
    hour = int(hour_s)
    minute = int(minute_s)
    slot_minute = minute - (minute % 15)
    end_hour = hour
    end_minute = slot_minute + 15
    if end_minute >= 60:
        end_hour = (end_hour + 1) % 24
        end_minute -= 60
    return f"{hour:02d}:{slot_minute:02d}", f"{end_hour:02d}:{end_minute:02d}"


def _format_pending_scan_note(starts: list[str]) -> str:
    count = len(starts)
    noun = "segment" if count == 1 else "segments"
    return f"{count} {noun} pending at {', '.join(starts)}"


def _slot_overlaps_range(slot: tuple[str, str], range_: tuple[str, str]) -> bool:
    def _to_min(hhmm: str) -> int:
        hour_s, minute_s = hhmm.split(":")
        return int(hour_s) * 60 + int(minute_s)

    slot_start, slot_end = (_to_min(slot[0]), _to_min(slot[1]))
    range_start, range_end = (_to_min(range_[0]), _to_min(range_[1]))
    return slot_start < range_end and slot_end > range_start


@app.command("scan")
def scan(
    day: str | None = typer.Argument(
        default=None, help="Day YYYYMMDD (default: SOL_DAY env)."
    ),
) -> None:
    """List transcript coverage ranges for a day."""
    day = resolve_sol_day(day)
    transcript_ranges, screen_ranges, segments = scan_day(day)
    pending_by_slot: dict[tuple[str, str], list[str]] = {}
    for segment in segments:
        if segment.get("data_state", {}).get("audio") != DataState.PENDING.value:
            continue
        slot = _pending_slot_range(segment["start"])
        pending_by_slot.setdefault(slot, []).append(segment["start"])
    for starts in pending_by_slot.values():
        starts.sort()

    typer.echo("Transcripts:")
    if transcript_ranges:
        for start, end in transcript_ranges:
            starts = [
                pending_start
                for slot, slot_starts in pending_by_slot.items()
                if _slot_overlaps_range(slot, (start, end))
                for pending_start in slot_starts
            ]
            line = f"  {start} - {end}"
            if starts:
                starts.sort()
                line += f" ({_format_pending_scan_note(starts)})"
            typer.echo(line)
    else:
        typer.echo("  (none)")

    typer.echo("Percepts:")
    if screen_ranges:
        for start, end in screen_ranges:
            typer.echo(f"  {start} - {end}")
    else:
        typer.echo("  (none)")


@app.command("segments")
def segments(
    day: str | None = typer.Argument(
        default=None, help="Day YYYYMMDD (default: SOL_DAY env)."
    ),
) -> None:
    """List recording segments for a day."""
    day = resolve_sol_day(day)
    segment_list = cluster_segments(day)
    if not segment_list:
        typer.echo("No segments.")
        return

    for segment in segment_list:
        key = segment.get("key", "")
        start = segment.get("start", "")
        end = segment.get("end", "")
        types = ", ".join(segment.get("types", []))
        typer.echo(f"{key}  {start} - {end}  [{types}]")


@app.command("read")
def read(
    day: str | None = typer.Argument(
        default=None, help="Day YYYYMMDD (default: SOL_DAY env)."
    ),
    start: str | None = typer.Option(None, "--start", help="Start time (HHMMSS)."),
    length: int | None = typer.Option(None, "--length", help="Length in minutes."),
    segment: str | None = typer.Option(
        None, "--segment", help="Segment key (HHMMSS_LEN, default: SOL_SEGMENT env)."
    ),
    segments: str | None = typer.Option(
        None, "--segments", help="Comma-separated segment keys for a span."
    ),
    stream: str | None = typer.Option(
        None, "--stream", help="Stream name (default: SOL_STREAM env)."
    ),
    full: bool = typer.Option(
        False, "--full", help="Include transcripts, screen, and agents."
    ),
    raw: bool = typer.Option(
        False, "--raw", help="Include transcripts and screen only."
    ),
    transcripts: bool = typer.Option(
        False, "--transcripts", help="Include transcript content."
    ),
    audio: bool = typer.Option(
        False, "--audio", help="Alias for --transcripts.", hidden=True
    ),
    percepts: bool = typer.Option(False, "--percepts", help="Include screen percepts."),
    screen: bool = typer.Option(
        False, "--screen", help="Alias for --percepts.", hidden=True
    ),
    agents: bool = typer.Option(False, "--agents", help="Include agent outputs."),
    max_bytes: int = typer.Option(
        16384, "--max", help="Max output bytes (0 = unlimited)."
    ),
) -> None:
    """Read transcript content for a day, segment, or time range."""
    day = resolve_sol_day(day)
    segment = resolve_sol_segment(segment)
    stream = stream or get_sol_stream()
    # --audio is an alias for --transcripts, --screen is an alias for --percepts
    transcripts = transcripts or audio
    percepts = percepts or screen

    if full and raw:
        typer.echo("Error: Cannot use --full and --raw together.", err=True)
        raise typer.Exit(1)

    if (full or raw) and (transcripts or percepts or agents):
        typer.echo(
            "Error: Cannot mix --full/--raw with individual source flags.", err=True
        )
        raise typer.Exit(1)

    if full:
        sources: dict[str, bool] = {
            "transcripts": True,
            "percepts": True,
            "agents": True,
        }
    elif raw:
        sources = {"transcripts": True, "percepts": True, "agents": False}
    elif transcripts or percepts or agents:
        sources = {"transcripts": transcripts, "percepts": percepts, "agents": agents}
    else:
        sources = {"transcripts": True, "percepts": False, "agents": True}

    # Validate mutually exclusive selection modes
    mode_count = sum(
        [
            segment is not None,
            segments is not None,
            start is not None or length is not None,
        ]
    )
    if mode_count > 1:
        typer.echo(
            "Error: Cannot mix --segment, --segments, and --start/--length.",
            err=True,
        )
        raise typer.Exit(1)

    if (start is not None) != (length is not None):
        typer.echo("Error: --start and --length must be used together.", err=True)
        raise typer.Exit(1)

    if start is not None and length is not None:
        from datetime import datetime, timedelta

        start_dt = datetime.strptime(start, "%H%M%S")
        end_dt = start_dt + timedelta(minutes=length)
        markdown = cluster_range(day, start, end_dt.strftime("%H%M%S"), sources)
    elif segments is not None:
        span = [s.strip() for s in segments.split(",") if s.strip()]
        markdown, _counts = cluster_span(day, span, sources, stream=stream)
    elif segment is not None:
        markdown, _counts = cluster_period(day, segment, sources, stream=stream)
    else:
        markdown, _counts = cluster(day, sources)

    truncated_echo(markdown, max_bytes)


@app.command("stats")
def stats(month: str = typer.Argument(help="Month (YYYYMM).")) -> None:
    """Show daily transcript coverage counts for a month."""
    days = sorted(day for day in day_dirs().keys() if day.startswith(month))

    days_with_data = 0
    for day in days:
        transcript_ranges, screen_ranges = cluster_scan(day)
        if transcript_ranges or screen_ranges:
            days_with_data += 1
            typer.echo(
                f"{day}  transcripts:{len(transcript_ranges)} percepts:{len(screen_ranges)}"
            )

    if not days_with_data:
        typer.echo(f"No data for {month}.")
        return

    typer.echo("")
    typer.echo(f"Total: {days_with_data} days with data")
