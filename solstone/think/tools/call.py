# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI commands for journal search and browsing.

Provides human-friendly CLI access to journal operations, paralleling the
tool functions in ``think/tools/search.py`` and ``think/tools/facets.py`` but
optimized for terminal use.

Mounted by ``think.call`` as ``sol call journal ...``.
"""

import json
import logging
import re
import shutil
import subprocess
import sys
from dataclasses import asdict
from datetime import datetime, timezone
from pathlib import Path

import typer

from solstone.think.entities import scan_facet_relationships
from solstone.think.facets import (
    create_facet,
    delete_facet,
    facet_summary,
    get_enabled_facets,
    get_facet_news,
    get_facets,
    log_call_action,
    rename_facet,
    set_facet_muted,
    update_facet,
)
from solstone.think.importers.utils import (
    build_import_info,
    get_import_details,
    list_import_timestamps,
)
from solstone.think.indexer.journal import search_counts as search_counts_impl
from solstone.think.indexer.journal import search_journal as search_journal_impl
from solstone.think.utils import (
    day_path,
    get_journal,
    is_solstone_up,
    iter_segments,
    require_solstone,
    resolve_sol_day,
    resolve_sol_facet,
    resolve_sol_segment,
    truncated_echo,
)

app = typer.Typer(help="Journal search and browsing.")
facet_app = typer.Typer(help="Facet management.")


@app.callback()
def _require_up(ctx: typer.Context) -> None:
    if (
        ctx.invoked_subcommand == "export"
    ):  # export is read-only and must work when supervisor is down (scope §6)
        return
    require_solstone()


app.add_typer(facet_app, name="facet")
retention_app = typer.Typer(help="Media retention management.")
app.add_typer(retention_app, name="retention")


@app.command()
def search(
    query: str = typer.Argument("", help="Search query (FTS5 syntax)."),
    limit: int = typer.Option(10, "--limit", "-n", help="Max results."),
    offset: int = typer.Option(0, "--offset", help="Skip N results."),
    day: str | None = typer.Option(None, "--day", "-d", help="Filter by day YYYYMMDD."),
    day_from: str | None = typer.Option(
        None, "--day-from", help="Date range start YYYYMMDD."
    ),
    day_to: str | None = typer.Option(
        None, "--day-to", help="Date range end YYYYMMDD."
    ),
    facet: str | None = typer.Option(None, "--facet", "-f", help="Filter by facet."),
    agent: str | None = typer.Option(None, "--agent", "-a", help="Filter by agent."),
    stream: str | None = typer.Option(
        None, "--stream", help="Filter by stream (e.g. import.ics, archon)."
    ),
) -> None:
    """Search the journal index."""
    kwargs = {}
    if day is not None:
        kwargs["day"] = day
    if day_from is not None:
        kwargs["day_from"] = day_from
    if day_to is not None:
        kwargs["day_to"] = day_to
    if facet is not None:
        kwargs["facet"] = facet
    if agent is not None:
        kwargs["agent"] = agent
    if stream is not None:
        kwargs["stream"] = stream

    total, results = search_journal_impl(query, limit, offset, **kwargs)

    # Counts summary
    counts = search_counts_impl(query, **kwargs)
    typer.echo(f"{total} results")

    facet_counts = counts.get("facets", {})
    if facet_counts:
        parts = [f"{f}:{c}" for f, c in facet_counts.most_common(10)]
        typer.echo(f"Facets: {', '.join(parts)}")

    agent_counts = counts.get("agents", {})
    if agent_counts:
        parts = [f"{a}:{c}" for a, c in agent_counts.most_common(10)]
        typer.echo(f"Agents: {', '.join(parts)}")

    day_counts = counts.get("days", {})
    if day_counts:
        top_days = sorted(day_counts.items(), key=lambda x: (-x[1], x[0]))[:10]
        parts = [f"{d}:{c}" for d, c in top_days]
        typer.echo(f"Top days: {', '.join(parts)}")

    # Results
    for r in results:
        meta = r["metadata"]
        stream_tag = f" | {meta['stream']}" if meta.get("stream") else ""
        typer.echo(
            f"\n--- {meta['day']} | {meta['facet']} | {meta['agent']}{stream_tag} | {r['id']} ---"
        )
        typer.echo(r["text"].strip())


@facet_app.command()
def show(
    name: str | None = typer.Argument(
        default=None, help="Facet name (default: SOL_FACET env)."
    ),
) -> None:
    """Show facet summary."""
    name = resolve_sol_facet(name)
    try:
        summary = facet_summary(name)
    except FileNotFoundError:
        typer.echo(f"Facet '{name}' not found.", err=True)
        raise typer.Exit(1)
    typer.echo(summary)


@app.command()
def facets(
    all_: bool = typer.Option(False, "--all", help="Include muted facets."),
) -> None:
    """List facets."""
    if all_:
        all_facets = get_facets()
    else:
        all_facets = get_enabled_facets()
    if not all_facets:
        typer.echo("No facets found.")
        return
    for name, info in sorted(all_facets.items()):
        title = info.get("title", name)
        emoji = info.get("emoji", "")
        desc = info.get("description", "")
        muted = info.get("muted", False)

        parts = []
        if emoji:
            parts.append(f"{emoji} {title} ({name})")
        else:
            parts.append(f"{title} ({name})")
        if desc:
            parts.append(f": {desc}")
        if muted:
            parts.append(" [muted]")
        typer.echo(f"- {''.join(parts)}")


@facet_app.command()
def create(
    title: str = typer.Argument(help="Display title for the new facet."),
    emoji: str = typer.Option("📦", "--emoji", help="Icon emoji."),
    color: str = typer.Option("#667eea", "--color", help="Hex color."),
    description: str = typer.Option("", "--description", help="Facet description."),
    consent: bool = typer.Option(
        False,
        "--consent",
        help="Assert that explicit user approval was obtained before calling this command (agent audit trail).",
    ),
) -> None:
    """Create a new facet."""
    try:
        slug = create_facet(
            title,
            emoji=emoji,
            color=color,
            description=description,
            consent=consent,
        )
    except ValueError as e:
        typer.echo(f"Error: {e}", err=True)
        raise typer.Exit(1)
    typer.echo(f"Created facet '{slug}'.")


@facet_app.command()
def update(
    name: str = typer.Argument(help="Facet name to update."),
    title: str | None = typer.Option(None, "--title", help="New display title."),
    description: str | None = typer.Option(
        None, "--description", help="New description."
    ),
    emoji: str | None = typer.Option(None, "--emoji", help="New icon emoji."),
    color: str | None = typer.Option(None, "--color", help="New hex color."),
) -> None:
    """Update facet configuration."""
    kwargs = {}
    if title is not None:
        kwargs["title"] = title
    if description is not None:
        kwargs["description"] = description
    if emoji is not None:
        kwargs["emoji"] = emoji
    if color is not None:
        kwargs["color"] = color

    if not kwargs:
        typer.echo(
            "Error: No fields to update. Use --title, --description, --emoji, or --color.",
            err=True,
        )
        raise typer.Exit(1)

    try:
        changed = update_facet(name, **kwargs)
    except FileNotFoundError:
        typer.echo(f"Error: Facet '{name}' not found.", err=True)
        raise typer.Exit(1)
    except ValueError as e:
        typer.echo(f"Error: {e}", err=True)
        raise typer.Exit(1)

    if changed:
        fields = ", ".join(changed.keys())
        typer.echo(f"Updated {fields} for facet '{name}'.")
    else:
        typer.echo(f"No changes for facet '{name}'.")


@facet_app.command()
def rename(
    name: str = typer.Argument(help="Current facet name."),
    new_name: str = typer.Argument(help="New facet name."),
    consent: bool = typer.Option(
        False,
        "--consent",
        help="Assert that explicit user approval was obtained before calling this command (agent audit trail).",
    ),
) -> None:
    """Rename a facet."""
    try:
        rename_facet(name, new_name)
    except ValueError as e:
        typer.echo(f"Error: {e}", err=True)
        raise typer.Exit(1)
    params: dict = {"old_name": name, "new_name": new_name}
    if consent:
        params["consent"] = True
    log_call_action(facet=new_name, action="facet_rename", params=params)


@facet_app.command()
def mute(name: str = typer.Argument(help="Facet name to mute.")) -> None:
    """Mute a facet (hide from default listings)."""
    try:
        set_facet_muted(name, True)
    except FileNotFoundError:
        typer.echo(f"Error: Facet '{name}' not found.", err=True)
        raise typer.Exit(1)
    typer.echo(f"Facet '{name}' muted.")


@facet_app.command()
def unmute(name: str = typer.Argument(help="Facet name to unmute.")) -> None:
    """Unmute a facet (show in default listings)."""
    try:
        set_facet_muted(name, False)
    except FileNotFoundError:
        typer.echo(f"Error: Facet '{name}' not found.", err=True)
        raise typer.Exit(1)
    typer.echo(f"Facet '{name}' unmuted.")


@facet_app.command()
def delete(
    name: str = typer.Argument(help="Facet name to delete."),
    yes: bool = typer.Option(False, "--yes", help="Skip confirmation."),
    consent: bool = typer.Option(
        False,
        "--consent",
        help="Assert that explicit user approval was obtained before calling this command (agent audit trail).",
    ),
) -> None:
    """Delete a facet and all its data."""
    if not yes:
        typer.echo(
            f"This will permanently delete facet '{name}' and all its data "
            "(entities, todos, events, logs, news).\n"
            "Use --yes to confirm."
        )
        raise typer.Exit(1)

    try:
        delete_facet(name, consent=consent)
    except FileNotFoundError:
        typer.echo(f"Error: Facet '{name}' not found.", err=True)
        raise typer.Exit(1)
    typer.echo(f"Deleted facet '{name}'.")


@facet_app.command("merge")
def merge(
    source: str = typer.Argument(help="Source facet to merge from (will be deleted)."),
    dest: str = typer.Option(..., "--into", help="Destination facet to merge into."),
    consent: bool = typer.Option(
        False,
        "--consent",
        help="Assert that explicit user approval was obtained before calling this command (agent audit trail).",
    ),
) -> None:
    """Merge all data from SOURCE facet into DEST facet, then delete SOURCE."""
    from solstone.apps.todos import todo as todo_module
    from solstone.think.entities.observations import (
        load_observations,
        save_observations,
    )
    from solstone.think.entities.relationships import (
        load_facet_relationship,
        save_facet_relationship,
    )

    if source == dest:
        typer.echo("Error: Source and destination facets must be different.", err=True)
        raise typer.Exit(1)

    journal = Path(get_journal())
    src_path = journal / "facets" / source
    dst_path = journal / "facets" / dest

    if not src_path.is_dir():
        typer.echo(f"Error: Facet '{source}' not found.", err=True)
        raise typer.Exit(1)
    if not dst_path.is_dir():
        typer.echo(f"Error: Facet '{dest}' not found.", err=True)
        raise typer.Exit(1)

    entity_slugs = scan_facet_relationships(source)

    open_todos: list[tuple[str, int, todo_module.TodoItem]] = []
    todos_dir = src_path / "todos"
    if todos_dir.is_dir():
        for todo_file in sorted(todos_dir.glob("*.jsonl")):
            checklist = todo_module.TodoChecklist.load(todo_file.stem, source)
            for item in checklist.items:
                if not item.completed and not item.cancelled:
                    open_todos.append((todo_file.stem, item.index, item))

    news_to_copy: list[tuple[Path, Path]] = []
    src_news_dir = src_path / "news"
    dst_news_dir = dst_path / "news"
    if src_news_dir.is_dir():
        for news_file in sorted(src_news_dir.glob("*.md")):
            dest_file = dst_news_dir / news_file.name
            if not dest_file.exists():
                news_to_copy.append((news_file, dest_file))

    typer.echo(
        f"Merging '{source}' into '{dest}': "
        f"{len(entity_slugs)} entities, {len(open_todos)} open todos, "
        f"{len(news_to_copy)} news files. This cannot be undone. Proceeding..."
    )

    for entity_id in entity_slugs:
        src_dir = src_path / "entities" / entity_id
        dst_dir = dst_path / "entities" / entity_id
        if dst_dir.exists():
            src_rel = load_facet_relationship(source, entity_id)
            dst_rel = load_facet_relationship(dest, entity_id)
            if src_rel is not None or dst_rel is not None:
                merged_rel = {**(src_rel or {}), **(dst_rel or {})}
                save_facet_relationship(dest, entity_id, merged_rel)

            src_obs = load_observations(source, entity_id)
            dst_obs = load_observations(dest, entity_id)
            seen = {(o.get("content", ""), o.get("observed_at")) for o in dst_obs}
            merged_obs = list(dst_obs)
            for observation in src_obs:
                key = (
                    observation.get("content", ""),
                    observation.get("observed_at"),
                )
                if key not in seen:
                    merged_obs.append(observation)
                    seen.add(key)
            save_observations(dest, entity_id, merged_obs)
            shutil.rmtree(str(src_dir))
        else:
            dst_dir.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(str(src_dir), str(dst_dir))

    for day, line_number, item in open_todos:
        captured_item = item

        def _append_todo(
            checklist: todo_module.TodoChecklist,
        ) -> tuple[todo_module.TodoChecklist, todo_module.TodoItem]:
            new_item = checklist.append_entry(
                captured_item.text,
                captured_item.nudge,
                created_at=captured_item.created_at,
            )
            return checklist, new_item

        captured_line_number = line_number
        captured_dest = dest

        def _cancel_todo(
            checklist: todo_module.TodoChecklist,
        ) -> tuple[todo_module.TodoChecklist, todo_module.TodoItem]:
            cancelled_item = checklist.cancel_entry(
                captured_line_number,
                cancelled_reason="moved_to_facet",
                moved_to=captured_dest,
            )
            return checklist, cancelled_item

        todo_module.TodoChecklist.locked_modify(day, dest, _append_todo)
        todo_module.TodoChecklist.locked_modify(day, source, _cancel_todo)

    if news_to_copy:
        dst_news_dir.mkdir(parents=True, exist_ok=True)
    for src_file, dest_file in news_to_copy:
        shutil.copy2(src_file, dest_file)

    params: dict[str, object] = {
        "source": source,
        "dest": dest,
        "entity_count": len(entity_slugs),
        "todo_count": len(open_todos),
        "news_count": len(news_to_copy),
    }
    if consent:
        params["consent"] = True
    log_call_action(facet=None, action="facet_merge", params=params)

    delete_facet(source)

    subprocess.run(
        ["journal", "indexer", "--rescan-full"],
        check=False,
        capture_output=True,
    )

    typer.echo(f"Merged '{source}' into '{dest}'. Index rebuild started.")


@app.command()
def news(
    name: str | None = typer.Argument(
        default=None, help="Facet name (default: SOL_FACET env)."
    ),
    day: str | None = typer.Option(None, "--day", "-d", help="Specific day YYYYMMDD."),
    limit: int = typer.Option(5, "--limit", "-n", help="Max days to show."),
    cursor: str | None = typer.Option(None, "--cursor", help="Pagination cursor."),
    write: bool = typer.Option(False, "--write", "-w", help="Write news from stdin."),
) -> None:
    """Read or write facet news."""
    name = resolve_sol_facet(name)
    if write:
        day = resolve_sol_day(day)
    elif day is None:
        from solstone.think.utils import get_sol_day

        day = get_sol_day()
    if write:
        # Read markdown from stdin
        markdown = sys.stdin.read()
        if not markdown.strip():
            typer.echo("Error: no content provided on stdin.", err=True)
            raise typer.Exit(1)

        journal_path = Path(get_journal())
        facet_path = journal_path / "facets" / name
        if not facet_path.exists():
            typer.echo(f"Error: facet '{name}' not found.", err=True)
            raise typer.Exit(1)

        news_dir = facet_path / "news"
        news_dir.mkdir(exist_ok=True)
        news_file = news_dir / f"{day}.md"
        news_file.write_text(markdown, encoding="utf-8")
        typer.echo(f"News for {day} saved to {name}.")
        return

    result = get_facet_news(name, cursor=cursor, limit=limit, day=day)
    days = result.get("days", [])
    if not days:
        typer.echo("No news found.")
        return
    for entry in days:
        typer.echo(entry.get("raw_content", ""))


@app.command()
def agents(
    day: str | None = typer.Argument(
        default=None, help="Day YYYYMMDD (default: SOL_DAY env)."
    ),
    segment: str | None = typer.Option(
        None,
        "--segment",
        "-s",
        help="Segment key (HHMMSS_LEN, default: SOL_SEGMENT env).",
    ),
) -> None:
    """List available agent outputs for a day."""
    day = resolve_sol_day(day)
    segment = resolve_sol_segment(segment)
    day_dir = day_path(day, create=False)

    if not day_dir.is_dir():
        typer.echo(f"No data for {day}.")
        return

    if segment:
        # List outputs in a specific segment directory
        seg_path = day_dir / segment / "talents"
        if not seg_path.is_dir():
            typer.echo(f"Segment {segment} not found for {day}.")
            return
        _list_outputs(seg_path, f"Segment {segment}")
        return

    # List daily agent outputs
    agents_path = day_dir / "talents"
    if agents_path.is_dir():
        _list_outputs(agents_path, "Daily agents")

    # List segments and their outputs (across all streams)
    seg_list = iter_segments(day)
    if seg_list:
        typer.echo(f"\nSegments: {len(seg_list)}")
        for stream_name, seg_key, seg_path_obj in seg_list:
            talents_dir = seg_path_obj / "talents"
            outputs = _get_output_names(talents_dir)
            label = f"  {stream_name}/{seg_key}" if stream_name else f"  {seg_key}"
            if outputs:
                typer.echo(f"{label}: {', '.join(outputs)}")
            else:
                typer.echo(f"{label}: (no outputs)")


def _get_output_names(directory: Path) -> list[str]:
    """Get sorted list of output file basenames in a directory."""
    names = []
    if not directory.is_dir():
        return names

    for f in sorted(directory.iterdir()):
        if f.is_file() and f.suffix in (".md", ".json", ".jsonl"):
            names.append(f.name)
        elif f.is_dir():
            for nested in sorted(f.iterdir()):
                if nested.is_file() and nested.suffix in (".md", ".json", ".jsonl"):
                    names.append(f"{f.name}/{nested.name}")
    return names


def _list_outputs(directory: Path, label: str) -> None:
    """Print output files in a directory."""
    outputs = _get_output_names(directory)
    if not outputs:
        typer.echo(f"{label}: (none)")
        return
    typer.echo(f"{label}:")
    for name in outputs:
        size = (directory / name).stat().st_size
        typer.echo(f"  {name} ({size:,} bytes)")


@app.command()
def read(
    agent: str = typer.Argument(help="Agent name (e.g., flow, meetings, activity)."),
    day: str | None = typer.Option(
        None, "--day", "-d", help="Day YYYYMMDD (default: SOL_DAY env)."
    ),
    segment: str | None = typer.Option(
        None,
        "--segment",
        "-s",
        help="Segment key (HHMMSS_LEN, default: SOL_SEGMENT env).",
    ),
    max_bytes: int = typer.Option(
        16384, "--max", help="Max output bytes (0 = unlimited)."
    ),
) -> None:
    """Read full content of an agent output."""
    day = resolve_sol_day(day)
    segment = resolve_sol_segment(segment)
    day_dir = day_path(day, create=False)

    if not day_dir.is_dir():
        typer.echo(f"No data for {day}.", err=True)
        raise typer.Exit(1)

    if segment:
        base_dir = day_dir / segment / "talents"
    else:
        base_dir = day_dir / "talents"

    if not base_dir.is_dir():
        location = f"segment {segment}" if segment else "talents"
        typer.echo(f"No {location} directory for {day}.", err=True)
        raise typer.Exit(1)

    # Try common extensions
    for ext in (".md", ".json", ".jsonl"):
        candidate = base_dir / f"{agent}{ext}"
        if candidate.is_file():
            truncated_echo(candidate.read_text(encoding="utf-8"), max_bytes)
            return

    # List what is available
    available = _get_output_names(base_dir)
    if available:
        typer.echo(
            f"Agent '{agent}' not found. Available: {', '.join(available)}", err=True
        )
    else:
        typer.echo(f"Agent '{agent}' not found and no outputs exist.", err=True)
    raise typer.Exit(1)


# ============================================================================
# Import Commands
# ============================================================================


def _derive_status(info: dict) -> str:
    """Derive import status from info dict fields."""
    if info.get("error"):
        return "failed"
    if info.get("processed"):
        return "success"
    if info.get("task_id"):
        return "running"
    return "pending"


def _get_source_type(journal_root: Path, timestamp: str, info: dict) -> str:
    """Get source type from manifest.json or infer from mime_type."""
    manifest_path = journal_root / "imports" / timestamp / "manifest.json"
    if manifest_path.exists():
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            if manifest.get("source_type"):
                return manifest["source_type"]
        except Exception:
            pass

    # Infer from mime_type
    mime = info.get("mime_type", "")
    if "calendar" in mime or "ics" in mime:
        return "ics"
    if "zip" in mime:
        return "archive"
    if "audio" in mime:
        return "audio"
    if "text" in mime:
        return "text"
    return "unknown"


def _get_entry_count(journal_root: Path, timestamp: str, info: dict) -> int | None:
    """Get entry count from manifest.json or total_files_created."""
    manifest_path = journal_root / "imports" / timestamp / "manifest.json"
    if manifest_path.exists():
        try:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            if manifest.get("entry_count") is not None:
                return manifest["entry_count"]
        except Exception:
            pass
    count = info.get("total_files_created")
    if count is not None and count > 0:
        return count
    return None


def _match_import_id(timestamps: list[str], prefix: str) -> str | None:
    """Match a partial import ID prefix to a full timestamp."""
    matches = [t for t in timestamps if t.startswith(prefix)]
    if len(matches) == 1:
        return matches[0]
    if len(matches) > 1:
        # Check for exact match first
        if prefix in matches:
            return prefix
        typer.echo(
            f"Ambiguous prefix '{prefix}' matches {len(matches)} imports. "
            "Be more specific.",
            err=True,
        )
        raise typer.Exit(1)
    return None


@app.command(name="imports")
def imports_list(
    limit: int = typer.Option(20, "--limit", "-n", help="Max results."),
    source: str | None = typer.Option(
        None, "--source", "-s", help="Filter by source type."
    ),
    json_output: bool = typer.Option(False, "--json", help="Output as JSON."),
) -> None:
    """List recent imports with metadata."""
    journal_root = Path(get_journal())
    timestamps = list_import_timestamps(journal_root)

    if not timestamps:
        typer.echo("No imports found.")
        return

    # Reverse chronological
    timestamps.sort(reverse=True)

    # Build info for each import
    rows = []
    for ts in timestamps:
        info = build_import_info(journal_root, ts)
        source_type = _get_source_type(journal_root, ts, info)

        if source and source_type != source:
            continue

        status = _derive_status(info)
        filename = info.get("original_filename", "unknown")
        entry_count = _get_entry_count(journal_root, ts, info)

        rows.append(
            {
                "timestamp": ts,
                "status": status,
                "source_type": source_type,
                "filename": filename,
                "entry_count": entry_count,
                "error": info.get("error"),
            }
        )

        if len(rows) >= limit:
            break

    if not rows:
        typer.echo("No imports found.")
        return

    if json_output:
        typer.echo(json.dumps(rows, indent=2))
        return

    for row in rows:
        parts = [f"{row['timestamp']} [{row['status']}]"]
        parts.append(f"{row['source_type']:8s}")
        parts.append(row["filename"])
        if row["status"] == "failed" and row.get("error"):
            parts.append(f"— error: {row['error']}")
        elif row.get("entry_count") is not None:
            parts.append(f"({row['entry_count']} entries)")
        typer.echo(" ".join(parts))


@app.command(name="import")
def import_detail(
    id: str = typer.Argument(help="Import ID or prefix (e.g. 20260309_143000)."),
) -> None:
    """Show full metadata for a single import."""
    journal_root = Path(get_journal())
    timestamps = list_import_timestamps(journal_root)

    if not timestamps:
        typer.echo("No imports found.", err=True)
        raise typer.Exit(1)

    # Match partial prefix
    matched = _match_import_id(timestamps, id)
    if matched is None:
        typer.echo(f"Import '{id}' not found.", err=True)
        raise typer.Exit(1)

    info = build_import_info(journal_root, matched)
    info["status"] = _derive_status(info)
    info["source_type"] = _get_source_type(journal_root, matched, info)

    # Merge manifest and detail data
    try:
        details = get_import_details(journal_root, matched)
        if details.get("import_json"):
            info["import_metadata"] = details["import_json"]
        if details.get("imported_json"):
            info["imported_results"] = details["imported_json"]
        if details.get("segments_json"):
            info["segments"] = details["segments_json"]
    except FileNotFoundError:
        pass

    typer.echo(json.dumps(info, indent=2, default=str))


# ============================================================================
# Retention Commands
# ============================================================================


def _parse_age(value: str) -> int:
    """Parse age string like '30d' or '30' to number of days."""
    value = value.strip().lower()
    if value.endswith("d"):
        return int(value[:-1])
    return int(value)


@retention_app.command()
def purge(
    older_than: str | None = typer.Option(
        None, "--older-than", help="Age threshold (e.g. 30d, 7d)."
    ),
    stream: str | None = typer.Option(
        None, "--stream", help="Only purge from this stream."
    ),
    dry_run: bool = typer.Option(
        False, "--dry-run", help="Show what would be deleted."
    ),
) -> None:
    """Purge raw media from completed segments."""
    from solstone.think.retention import _human_bytes, load_retention_config
    from solstone.think.retention import purge as run_purge

    older_than_days = _parse_age(older_than) if older_than else None
    config = load_retention_config()

    if dry_run:
        typer.echo("DRY RUN — no files will be deleted.\n")

    result = run_purge(
        older_than_days=older_than_days,
        stream_filter=stream,
        dry_run=dry_run,
        config=config,
    )

    if result.details:
        for detail in result.details:
            typer.echo(
                f"  {detail['day']}/{detail['stream']}/{detail['segment']}: "
                f"{len(detail['files'])} files, {_human_bytes(detail['bytes_freed'])}"
            )
        typer.echo("")

    action = "Would delete" if dry_run else "Deleted"
    typer.echo(
        f"{action} {result.files_deleted} files, "
        f"freeing {_human_bytes(result.bytes_freed)}"
    )

    if result.segments_skipped_incomplete:
        typer.echo(
            f"Skipped {result.segments_skipped_incomplete} incomplete segments "
            "(processing not finished)."
        )

    if result.segments_skipped_policy:
        typer.echo(
            f"Skipped {result.segments_skipped_policy} segments "
            "(not yet eligible under retention policy)."
        )


@retention_app.command()
def config(
    mode: str | None = typer.Option(
        None, "--mode", help="Retention mode: keep, days, or processed."
    ),
    days: int | None = typer.Option(
        None, "--days", help="Days to retain (required when mode is 'days')."
    ),
    stream: str | None = typer.Option(
        None, "--stream", help="Apply to a specific stream instead of global."
    ),
    clear: bool = typer.Option(
        False, "--clear", help="Clear per-stream override (requires --stream)."
    ),
) -> None:
    """Show or update retention configuration."""
    import os

    from solstone.think.retention import load_retention_config
    from solstone.think.utils import get_config, get_journal

    if mode is None and days is None and not clear:
        cfg = load_retention_config()
        result = {
            "default": {"mode": cfg.default.mode, "days": cfg.default.days},
            "per_stream": {
                name: {"mode": policy.mode, "days": policy.days}
                for name, policy in cfg.per_stream.items()
            },
        }
        typer.echo(json.dumps(result, indent=2))
        return

    if clear:
        if not stream:
            typer.echo("--clear requires --stream", err=True)
            raise typer.Exit(1)
        if mode is not None or days is not None:
            typer.echo("--clear cannot be combined with --mode or --days", err=True)
            raise typer.Exit(1)

    if mode is not None and mode not in ("keep", "days", "processed"):
        typer.echo(f"Invalid mode: {mode}. Must be keep, days, or processed.", err=True)
        raise typer.Exit(1)

    if mode == "days" and days is None:
        typer.echo("--days is required when mode is 'days'.", err=True)
        raise typer.Exit(1)

    if days is not None and days < 1:
        typer.echo("--days must be a positive integer.", err=True)
        raise typer.Exit(1)

    journal_config = get_config()
    retention = journal_config.setdefault("retention", {})

    if clear:
        ps = retention.get("per_stream", {})
        if stream in ps:
            del ps[stream]
            if not ps:
                retention.pop("per_stream", None)
        log_call_action(
            facet=None,
            action="retention_config",
            params={"stream": stream, "clear": True},
        )
    elif stream:
        ps = retention.setdefault("per_stream", {})
        entry = ps.setdefault(stream, {})
        if mode is not None:
            entry["raw_media"] = mode
        if days is not None:
            entry["raw_media_days"] = days
        log_call_action(
            facet=None,
            action="retention_config",
            params={"stream": stream, "mode": mode, "days": days},
        )
    else:
        if mode is not None:
            retention["raw_media"] = mode
        if days is not None:
            retention["raw_media_days"] = days
        log_call_action(
            facet=None,
            action="retention_config",
            params={"mode": mode, "days": days},
        )

    config_dir = Path(get_journal()) / "config"
    config_dir.mkdir(parents=True, exist_ok=True)
    config_path = config_dir / "journal.json"

    with open(config_path, "w", encoding="utf-8") as f:
        json.dump(journal_config, f, indent=2, ensure_ascii=False)
        f.write("\n")
    os.chmod(config_path, 0o600)

    cfg = load_retention_config()
    result = {
        "default": {"mode": cfg.default.mode, "days": cfg.default.days},
        "per_stream": {
            name: {"mode": policy.mode, "days": policy.days}
            for name, policy in cfg.per_stream.items()
        },
    }
    typer.echo(json.dumps(result, indent=2))


@app.command(name="storage-summary")
def storage_summary(
    json_output: bool = typer.Option(False, "--json", help="Output as JSON."),
    check: bool = typer.Option(
        False, "--check", help="Check storage health thresholds."
    ),
) -> None:
    """Show journal storage summary."""
    from solstone.think.retention import compute_storage_summary

    summary = compute_storage_summary()

    if check:
        from solstone.think.retention import check_storage_health
        from solstone.think.utils import get_journal

        journal_path = get_journal()
        warnings = check_storage_health(summary, journal_path)

        if json_output:
            typer.echo(
                json.dumps(
                    {
                        "raw_media_bytes": summary.raw_media_bytes,
                        "derived_bytes": summary.derived_bytes,
                        "total_segments": summary.total_segments,
                        "segments_with_raw": summary.segments_with_raw,
                        "segments_purged": summary.segments_purged,
                        "warnings": warnings,
                    },
                    indent=2,
                )
            )
        else:
            typer.echo(f"Raw media:          {summary.raw_media_human}")
            typer.echo(f"AI-processed content: {summary.derived_human}")
            typer.echo(
                f"Segments: {summary.total_segments} total, "
                f"{summary.segments_with_raw} with raw media, "
                f"{summary.segments_purged} purged"
            )
            if warnings:
                typer.echo("")
                for w in warnings:
                    typer.echo(f"⚠ {w['message']}")
            else:
                typer.echo("\nAll storage thresholds OK.")
        return

    if json_output:
        typer.echo(
            json.dumps(
                {
                    "raw_media_bytes": summary.raw_media_bytes,
                    "derived_bytes": summary.derived_bytes,
                    "total_segments": summary.total_segments,
                    "segments_with_raw": summary.segments_with_raw,
                    "segments_purged": summary.segments_purged,
                },
                indent=2,
            )
        )
        return

    typer.echo(f"Raw media:          {summary.raw_media_human}")
    typer.echo(f"AI-processed content: {summary.derived_human}")
    typer.echo(
        f"Segments: {summary.total_segments} total, "
        f"{summary.segments_with_raw} with raw media, "
        f"{summary.segments_purged} purged"
    )


def _looks_like_journal_source(source_path: Path) -> bool:
    try:
        chronicle_dir = source_path / "chronicle"
        if chronicle_dir.is_dir():
            if any(
                entry.is_dir() and re.match(r"^\d{8}$", entry.name)
                for entry in chronicle_dir.iterdir()
            ):
                return True
        return any(
            entry.is_dir() and re.match(r"^\d{8}$", entry.name)
            for entry in source_path.iterdir()
        )
    except OSError:
        return False


def _merge_error_envelope(
    *,
    code: str,
    message: str,
    source: Path,
    dry_run: bool,
    details: dict[str, object] | None = None,
) -> dict[str, object]:
    return {
        "ok": False,
        "code": code,
        "message": message,
        "source": str(source),
        "dry_run": dry_run,
        "details": details or {},
    }


@app.command("export")
def journal_export(
    out: Path | None = typer.Option(
        None,
        "--out",
        help="Write the journal ZIP to PATH (default: alongside the journal root).",
    ),
    quiet: bool = typer.Option(
        False,
        "--quiet",
        help="Suppress success output; errors still print.",
    ),
) -> None:
    """Export the active journal as a portable ZIP archive."""
    from solstone.think.journal_export import (
        export_journal_archive,
        get_skipped_export_entries,
    )

    journal_root = Path(get_journal())
    if is_solstone_up():
        typer.echo(
            "warning: solstone supervisor is running; export reflects a live snapshot and may include partial writes",
            err=True,
        )

    skipped_entries = get_skipped_export_entries(journal_root)
    if skipped_entries and not quiet:
        typer.echo(
            f"advisory: skipped non-export entries: {', '.join(skipped_entries)}",
            err=True,
        )

    try:
        archive_path = export_journal_archive(journal_root, out)
    except FileNotFoundError:
        typer.echo(
            "error: active journal root is not a directory; try: verify the journal path",
            err=True,
        )
        raise typer.Exit(1)
    except OSError:
        target_path = (
            out or journal_root.parent / f"{journal_root.name}.exports"
        ).expanduser()
        advisory_parent = target_path.parent if target_path.suffix else target_path
        typer.echo(
            f"error: failed to write archive; try: check disk space and write permissions on {advisory_parent.resolve()}",
            err=True,
        )
        raise typer.Exit(1)

    if not quiet:
        typer.echo(str(archive_path))


@app.command("merge")
def journal_merge(
    source: str = typer.Argument(
        help="Path to source journal directory to merge from."
    ),
    dry_run: bool = typer.Option(
        False,
        "--dry-run",
        help="Show what would be merged without making changes.",
    ),
    json_output: bool = typer.Option(
        False,
        "--json",
        help="Output result as a single-line JSON object (suppresses normal output).",
    ),
) -> None:
    """Merge segments, entities, facets, and imports from a source journal."""
    from solstone.think.merge import DecisionLogWriteError, MergeSummary, merge_journals

    source_path = Path(source).resolve()
    target_path = Path(get_journal())

    if not source_path.is_dir():
        if json_output:
            typer.echo(
                json.dumps(
                    _merge_error_envelope(
                        code="source-not-a-directory",
                        message="Source path is not a directory.",
                        source=source_path,
                        dry_run=dry_run,
                    )
                ),
                err=True,
            )
        else:
            typer.echo(f"Error: '{source}' is not a directory.", err=True)
        raise typer.Exit(1)

    if target_path.exists() and not target_path.is_dir():
        if json_output:
            typer.echo(
                json.dumps(
                    _merge_error_envelope(
                        code="target-not-a-journal",
                        message="Target journal path is not a directory.",
                        source=source_path,
                        dry_run=dry_run,
                        details={"target": str(target_path)},
                    )
                ),
                err=True,
            )
        else:
            typer.echo(
                f"Error: journal target '{target_path}' is not a directory.", err=True
            )
        raise typer.Exit(1)

    if not _looks_like_journal_source(source_path):
        if json_output:
            typer.echo(
                json.dumps(
                    _merge_error_envelope(
                        code="source-not-a-journal",
                        message="Source path does not look like a journal directory.",
                        source=source_path,
                        dry_run=dry_run,
                    )
                ),
                err=True,
            )
        else:
            typer.echo(
                f"Error: '{source}' does not appear to be a journal (no YYYYMMDD directories found).",
                err=True,
            )
        raise typer.Exit(1)

    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    artifact_root = target_path.parent / f"{target_path.name}.merge" / run_id
    log_path = artifact_root / "decisions.jsonl"
    staging_path = artifact_root / "staging"

    root_logger = logging.getLogger()
    original_level = root_logger.level
    try:
        if json_output:
            root_logger.setLevel(logging.CRITICAL)
        summary: MergeSummary = merge_journals(
            source_path,
            target_path,
            dry_run=dry_run,
            log_path=log_path,
            staging_path=staging_path,
        )
    except DecisionLogWriteError as exc:
        if json_output:
            typer.echo(
                json.dumps(
                    _merge_error_envelope(
                        code="decision-log-write-failed",
                        message="Decision log could not be written.",
                        source=source_path,
                        dry_run=dry_run,
                        details={
                            "exception_type": type(exc).__name__,
                            "exception": repr(exc),
                        },
                    )
                ),
                err=True,
            )
            raise typer.Exit(1)
        raise
    except Exception as exc:
        if json_output:
            typer.echo(
                json.dumps(
                    _merge_error_envelope(
                        code="merge-engine-error",
                        message="Journal merge failed.",
                        source=source_path,
                        dry_run=dry_run,
                        details={
                            "exception_type": type(exc).__name__,
                            "exception": repr(exc),
                        },
                    )
                ),
                err=True,
            )
            raise typer.Exit(1)
        raise
    finally:
        if json_output:
            root_logger.setLevel(original_level)

    action = "Would merge" if dry_run else "Merged"
    if not json_output:
        typer.echo(f"\n{action}:")
        typer.echo(
            f"  Segments: {summary.segments_copied} copied, {summary.segments_skipped} skipped, {summary.segments_errored} errored"
        )
        typer.echo(
            f"  Entities: {summary.entities_created} created, {summary.entities_merged} merged, {summary.entities_staged} staged, {summary.entities_skipped} skipped"
        )
        typer.echo(
            f"  Facets: {summary.facets_created} created, {summary.facets_merged} merged"
        )
        typer.echo(
            f"  Imports: {summary.imports_copied} copied, {summary.imports_skipped} skipped"
        )

        if summary.errors:
            typer.echo(f"\n{len(summary.errors)} errors:")
            for error in summary.errors:
                typer.echo(f"  - {error}")

        if log_path.exists():
            typer.echo(f"\nDecision log: {log_path}")
        if summary.entities_staged > 0:
            typer.echo(f"Staged entities: {staging_path}")

    indexer_returncode = 0
    if not dry_run:
        indexer_result = subprocess.run(
            ["journal", "indexer", "--rescan-full"],
            check=False,
            capture_output=True,
        )
        indexer_returncode = indexer_result.returncode

        log_call_action(
            facet=None,
            action="journal_merge",
            params={
                "source": str(source_path),
                "segments_copied": summary.segments_copied,
                "entities_created": summary.entities_created,
                "entities_merged": summary.entities_merged,
                "entities_staged": summary.entities_staged,
                "facets_created": summary.facets_created,
                "facets_merged": summary.facets_merged,
                "imports_copied": summary.imports_copied,
                "errors": len(summary.errors),
            },
        )

        if not json_output:
            typer.echo("Index rebuild started.")

    if json_output:
        typer.echo(
            json.dumps(
                {
                    "ok": True,
                    "code": "ok",
                    "source": str(source_path),
                    "dry_run": dry_run,
                    "summary": asdict(summary),
                    "decision_log": str(log_path),
                    "staging_dir": str(staging_path)
                    if summary.entities_staged > 0
                    else None,
                    "indexer_returncode": indexer_returncode,
                }
            )
        )
