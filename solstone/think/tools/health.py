# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import dataclasses
import json
import json as jsonlib
from datetime import datetime, timedelta
from typing import Optional

import typer

from solstone.think.pipeline_health import summarize_pipeline_day
from solstone.think.surfaces import health as health_surface
from solstone.think.surfaces.types import HealthReport
from solstone.think.utils import require_solstone

app = typer.Typer(
    help="Health: journal-data trust signals (for infrastructure/service liveness, use `sol health`).",
    no_args_is_help=True,
)


@app.callback()
def callback() -> None:
    require_solstone()


def _echo_json(payload: object) -> None:
    typer.echo(jsonlib.dumps(payload, indent=2, sort_keys=False))


def _render_summary(report: HealthReport) -> None:
    typer.echo(f"Range: {report.range[0]} -> {report.range[1]}")
    typer.echo("Capture")
    typer.echo(f"  hours_with_capture: {report.capture_health.hours_with_capture}")
    typer.echo(f"  hours_total: {report.capture_health.hours_total}")
    typer.echo(f"  coverage_ratio: {report.capture_health.coverage_ratio}")
    typer.echo(
        "  facets_with_recent_capture: "
        + ", ".join(report.capture_health.facets_with_recent_capture)
    )
    typer.echo(
        "  facets_silent_24h: " + ", ".join(report.capture_health.facets_silent_24h)
    )
    typer.echo(f"  last_segment_at: {report.capture_health.last_segment_at}")
    typer.echo("Synthesis")
    typer.echo(f"  activities_count: {report.synthesis_health.activities_count}")
    typer.echo(
        "  activities_with_participation: "
        + str(report.synthesis_health.activities_with_participation)
    )
    typer.echo(
        f"  activities_with_story: {report.synthesis_health.activities_with_story}"
    )
    typer.echo(
        f"  activities_user_edited: {report.synthesis_health.activities_user_edited}"
    )
    typer.echo(
        "  activities_anticipated_unfilled: "
        + str(report.synthesis_health.activities_anticipated_unfilled)
    )
    typer.echo(
        "  talent_run_failures_24h: "
        + str(report.synthesis_health.talent_run_failures_24h)
    )
    typer.echo(
        "  indexer_last_rebuild_at: "
        + str(report.synthesis_health.indexer_last_rebuild_at)
    )
    backlog = report.segment_backlog
    n = backlog.not_thought
    m = backlog.days_with_backlog
    seg_word = "segment" if n == 1 else "segments"
    day_word = "day" if m == 1 else "days"
    if backlog.errors and n > 0:
        typer.echo(
            f"  at least {n} {seg_word} across {m} {day_word} "
            "awaiting thinking (status incomplete)"
        )
    elif backlog.errors:
        typer.echo("  Segment thinking status unavailable")
    elif n > 0:
        typer.echo(f"  {n} {seg_word} across {m} {day_word} awaiting thinking")
    typer.echo("Consumer Signals")
    typer.echo(
        f"  ledger_open_items_total: {report.consumer_signal.ledger_open_items_total}"
    )
    typer.echo(
        f"  ledger_stale_items_count: {report.consumer_signal.ledger_stale_items_count}"
    )
    typer.echo(
        f"  profile_entities_total: {report.consumer_signal.profile_entities_total}"
    )
    typer.echo("Notes")
    if not report.notes:
        typer.echo("  none")
        return
    for note in report.notes:
        typer.echo(f"  [{note.severity}] {note.category}: {note.message}")


def _render_full(report: HealthReport) -> None:
    _render_summary(report)
    if not report.capture_health.facets_silent_24h:
        return

    typer.echo("Silent Facet Detail")
    for facet in report.capture_health.facets_silent_24h:
        matching = [
            note
            for note in report.notes
            if note.category == "capture" and note.message.startswith(f"{facet}:")
        ]
        if not matching:
            continue
        for note in matching:
            typer.echo(f"  {facet}: [{note.severity}] {note.message}")


@app.command("summary")
def summary(
    day: str | None = typer.Option(None, "--day"),
    json: bool = typer.Option(False, "--json"),
) -> None:
    """Summarize journal-data trust signals for one day."""
    try:
        report = health_surface.summary(day=day)
    except ValueError as exc:
        raise typer.BadParameter(str(exc)) from exc
    if json:
        _echo_json(dataclasses.asdict(report))
        return
    _render_summary(report)


@app.command("full")
def full(
    day: str | None = typer.Option(None, "--day"),
    json: bool = typer.Option(False, "--json"),
) -> None:
    """Render the full journal-data trust report for one day."""
    try:
        report = health_surface.full(day=day)
    except ValueError as exc:
        raise typer.BadParameter(str(exc)) from exc
    if json:
        _echo_json(dataclasses.asdict(report))
        return
    _render_full(report)


@app.command("for-range")
def for_range(
    day_from: str | None = typer.Option(None, "--day-from"),
    day_to: str | None = typer.Option(None, "--day-to"),
    json: bool = typer.Option(False, "--json"),
) -> None:
    """Render the journal-data trust report for an inclusive day range."""
    try:
        report = health_surface.for_range(day_from=day_from, day_to=day_to)
    except ValueError as exc:
        raise typer.BadParameter(str(exc)) from exc
    if json:
        _echo_json(dataclasses.asdict(report))
        return
    _render_full(report)


@app.command(
    "pipeline",
    help="Thin wrapper around think.pipeline_health. For journal-data trust checks use `summary` / `full` / `for-range`.",
)
def pipeline(
    day: Optional[str] = typer.Option(
        None, "--day", help="Day to summarize (YYYYMMDD)."
    ),
    yesterday: bool = typer.Option(
        False, "--yesterday", help="Summarize yesterday's pipeline."
    ),
) -> None:
    """Summarize think pipeline health for one day."""
    if day is not None and yesterday:
        typer.echo("--day and --yesterday are mutually exclusive", err=True)
        raise typer.Exit(1)

    if day is not None:
        target = day
    elif yesterday:
        target = (datetime.now() - timedelta(days=1)).strftime("%Y%m%d")
    else:
        target = datetime.now().strftime("%Y%m%d")

    summary = summarize_pipeline_day(target)
    typer.echo(json.dumps(summary, indent=2, sort_keys=False))
