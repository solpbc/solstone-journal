# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI commands for todo management.

Auto-discovered by ``think.call`` and mounted as ``sol call todos ...``.
"""

import json
import subprocess
from datetime import datetime
from pathlib import Path

import typer

from solstone.apps.todos import todo
from solstone.think.facets import log_call_action
from solstone.think.utils import get_journal, require_solstone

app = typer.Typer(help="Todo checklist management.")


@app.callback()
def _require_up() -> None:
    require_solstone()


def _print_day_facet(day: str, facet: str) -> bool:
    """Print todos for a single day+facet. Returns True if any items exist."""
    checklist = todo.TodoChecklist.load(day, facet)
    if not checklist.items:
        return False
    typer.echo(checklist.display())
    return True


def _validate_facet_or_exit(facet: str, label: str) -> None:
    """Exit if the facet directory does not exist."""
    facet_path = Path(get_journal()) / "facets" / facet
    if not facet_path.is_dir():
        typer.echo(
            f"Error: Facet '{facet}' ({label}) does not exist.",
            err=True,
        )
        raise typer.Exit(1)


@app.command("list")
def list_todos(
    day: str | None = typer.Argument(
        None, help="Journal day in YYYYMMDD format (or set SOL_DAY)."
    ),
    facet: str | None = typer.Option(
        None, "--facet", "-f", help="Facet name. Omit to show all facets."
    ),
    to: str | None = typer.Option(
        None, "--to", help="End day for range query (YYYYMMDD, inclusive)."
    ),
) -> None:
    """Show the todo checklist for a day (or date range)."""
    from solstone.think.utils import (
        get_journal,
        get_sol_facet,
        resolve_sol_day_or_today,
    )

    get_journal()
    day = resolve_sol_day_or_today(day)
    if facet is None:
        facet = get_sol_facet()

    if to is not None and to < day:
        typer.echo(f"Error: --to ({to}) must not be before day ({day})", err=True)
        raise typer.Exit(1)

    # Range query
    if to is not None and to != day:
        # Use all facets for range — get_facets_with_todos only checks the start day
        from solstone.think.facets import get_facets

        facets = [facet] if facet else sorted(get_facets())

        for f in facets:
            days_with_todos = todo.get_todo_days_in_range(f, day, to)
            if not days_with_todos:
                continue
            if len(facets) > 1:
                typer.echo(f"## {f}")
            for day_str in days_with_todos:
                checklist = todo.TodoChecklist.load(day_str, f)
                if checklist.items:
                    typer.echo(f"### {day_str}")
                    typer.echo(checklist.display())
                    typer.echo()
        return

    # Single day
    facets = [facet] if facet else todo.get_facets_with_todos(day)

    if not facets:
        typer.echo(f"No todos found for {day}.")
        return

    if len(facets) == 1:
        if not _print_day_facet(day, facets[0]):
            typer.echo(f"No todos found for {day}.")
        return

    for f in facets:
        typer.echo(f"## {f}")
        _print_day_facet(day, f)
        typer.echo()


@app.command("add")
def add_todo(
    text: str = typer.Argument(help="Todo item text."),
    day: str | None = typer.Option(
        None, "--day", "-d", help="Journal day in YYYYMMDD format (or set SOL_DAY)."
    ),
    facet: str | None = typer.Option(
        None, "--facet", "-f", help="Facet name (or set SOL_FACET)."
    ),
    nudge: str | None = typer.Option(
        None,
        "--nudge",
        "-n",
        help="Nudge time: HH:MM, now, tomorrow HH:MM, or YYYYMMDDTHH:MM.",
    ),
    force: bool = typer.Option(
        False, "--force", help="Skip duplicate check and add anyway."
    ),
) -> None:
    """Add a new todo item."""
    from datetime import datetime

    from solstone.think.utils import get_journal, resolve_sol_day, resolve_sol_facet

    get_journal()
    day = resolve_sol_day(day)
    facet = resolve_sol_facet(facet)

    try:
        datetime.strptime(day, "%Y%m%d")
    except ValueError:
        typer.echo(f"Error: invalid day format '{day}'", err=True)
        raise typer.Exit(1)

    # Parse nudge if provided
    parsed_nudge: str | None = None
    if nudge is not None:
        try:
            parsed_nudge = todo.parse_nudge(nudge, day)
        except ValueError as exc:
            typer.echo(f"Error: {exc}", err=True)
            raise typer.Exit(1) from None

    # Cross-facet duplicate check
    if not force:
        matches = todo.find_cross_facet_matches(text, day, exclude_facet=facet)
        if matches:
            typer.echo(f"Duplicate detected for: {text}", err=True)
            for match in matches:
                typer.echo(
                    f"  [{match['score']:.0f}%] {match['facet']}/{match['day']} "
                    f"line {match['line']}: {match['text']}",
                    err=True,
                )
            typer.echo("Use --force to add anyway.", err=True)
            raise typer.Exit(1)

    try:

        def _add(checklist: todo.TodoChecklist) -> todo.TodoChecklist:
            checklist.append_entry(text, nudge=parsed_nudge)
            return checklist

        checklist = todo.TodoChecklist.locked_modify(day, facet, _add)
        item = checklist.items[-1]
        log_call_action(
            facet=facet,
            action="todo_add",
            params={"line_number": item.index, "text": item.text},
            day=day,
        )
        typer.echo(checklist.display())
    except todo.TodoEmptyTextError:
        typer.echo("Error: todo text cannot be empty", err=True)
        raise typer.Exit(1)


@app.command("done")
def done_todo(
    line_number: int = typer.Argument(help="1-based line number of the todo."),
    day: str | None = typer.Option(
        None, "--day", "-d", help="Journal day in YYYYMMDD format (or set SOL_DAY)."
    ),
    facet: str | None = typer.Option(
        None, "--facet", "-f", help="Facet name (or set SOL_FACET)."
    ),
) -> None:
    """Mark a todo item as done."""
    from solstone.think.utils import get_journal, resolve_sol_day, resolve_sol_facet

    get_journal()
    day = resolve_sol_day(day)
    facet = resolve_sol_facet(facet)

    try:

        def _done(checklist: todo.TodoChecklist) -> tuple:
            item = checklist.mark_done(line_number)
            return checklist, item

        checklist, item = todo.TodoChecklist.locked_modify(day, facet, _done)
        log_call_action(
            facet=facet,
            action="todo_done",
            params={"line_number": line_number, "text": item.text},
            day=day,
        )
        typer.echo(checklist.display())
    except FileNotFoundError:
        typer.echo(f"Error: no todos found for facet '{facet}' on {day}", err=True)
        raise typer.Exit(1)
    except IndexError as exc:
        typer.echo(f"Error: {exc}", err=True)
        raise typer.Exit(1)


@app.command("cancel")
def cancel_todo(
    line_number: int = typer.Argument(help="1-based line number of the todo."),
    day: str | None = typer.Option(
        None, "--day", "-d", help="Journal day in YYYYMMDD format (or set SOL_DAY)."
    ),
    facet: str | None = typer.Option(
        None, "--facet", "-f", help="Facet name (or set SOL_FACET)."
    ),
) -> None:
    """Cancel a todo item."""
    from solstone.think.utils import get_journal, resolve_sol_day, resolve_sol_facet

    get_journal()
    day = resolve_sol_day(day)
    facet = resolve_sol_facet(facet)

    try:

        def _cancel(checklist: todo.TodoChecklist) -> tuple:
            item = checklist.cancel_entry(line_number)
            return checklist, item

        checklist, item = todo.TodoChecklist.locked_modify(day, facet, _cancel)
        log_call_action(
            facet=facet,
            action="todo_cancel",
            params={"line_number": line_number, "text": item.text},
            day=day,
        )
        typer.echo(checklist.display())
    except FileNotFoundError:
        typer.echo(f"Error: no todos found for facet '{facet}' on {day}", err=True)
        raise typer.Exit(1)
    except IndexError as exc:
        typer.echo(f"Error: {exc}", err=True)
        raise typer.Exit(1)


@app.command("move")
def move_todo(
    line_number: int = typer.Argument(
        help="Line number of the todo to move (1-indexed)."
    ),
    day: str = typer.Option(..., "--day", help="Day in YYYYMMDD format."),
    from_facet: str = typer.Option(..., "--from", help="Source facet."),
    to_facet: str = typer.Option(..., "--to", help="Destination facet."),
    consent: bool = typer.Option(
        False,
        "--consent",
        help="Assert that explicit user approval was obtained before calling this command (agent audit trail).",
    ),
) -> None:
    """Move an open todo from one facet to another."""
    from datetime import datetime

    _validate_facet_or_exit(from_facet, "--from")
    _validate_facet_or_exit(to_facet, "--to")

    try:
        datetime.strptime(day, "%Y%m%d")
    except ValueError:
        typer.echo(
            f"Error: Invalid day format '{day}', expected YYYYMMDD.",
            err=True,
        )
        raise typer.Exit(1)

    try:
        source_checklist = todo.TodoChecklist.load(day, from_facet)
        if not source_checklist.exists:
            raise FileNotFoundError()
        todo.validate_line_number(line_number, len(source_checklist.items))
        item = source_checklist.items[line_number - 1]
        if item.completed:
            raise todo.TodoError("Cannot move a completed todo.")
        if item.cancelled:
            raise todo.TodoError("Cannot move an already cancelled todo.")
    except FileNotFoundError:
        typer.echo(
            f"Error: No todos found for day {day} in facet '{from_facet}'.",
            err=True,
        )
        raise typer.Exit(1)
    except IndexError as exc:
        typer.echo(f"Error: {exc}", err=True)
        raise typer.Exit(1)
    except todo.TodoError as exc:
        typer.echo(f"Error: {exc}", err=True)
        raise typer.Exit(1)

    try:

        def _append_dest(
            checklist: todo.TodoChecklist,
        ) -> tuple[todo.TodoChecklist, todo.TodoItem]:
            new_item = checklist.append_entry(
                item.text,
                item.nudge,
                created_at=item.created_at,
            )
            return checklist, new_item

        _, new_item = todo.TodoChecklist.locked_modify(day, to_facet, _append_dest)
    except Exception as exc:
        typer.echo(
            f"Error: Failed to append to destination facet '{to_facet}': {exc}. Source todo is unchanged.",
            err=True,
        )
        raise typer.Exit(1)

    try:

        def _cancel_source(
            checklist: todo.TodoChecklist,
        ) -> tuple[todo.TodoChecklist, todo.TodoItem]:
            todo.validate_line_number(line_number, len(checklist.items))
            current_item = checklist.items[line_number - 1]
            if current_item.completed:
                raise todo.TodoError("Cannot move a completed todo.")
            if current_item.cancelled:
                raise todo.TodoError("Cannot move an already cancelled todo.")
            cancelled_item = checklist.cancel_entry(
                line_number,
                cancelled_reason="moved_to_facet",
                moved_to=to_facet,
            )
            return checklist, cancelled_item

        _, item = todo.TodoChecklist.locked_modify(day, from_facet, _cancel_source)
    except (FileNotFoundError, IndexError, todo.TodoError):
        typer.echo(
            f"Warning: Item was appended to '{to_facet}' but could not cancel source in '{from_facet}'. Cancel it manually with: sol call todos cancel {line_number} --day {day} --facet {from_facet}",
            err=True,
        )
        raise typer.Exit(1)

    params_out: dict[str, object] = {
        "moved_from": from_facet,
        "moved_to": to_facet,
        "line_number": line_number,
        "text": item.text,
    }
    params_in: dict[str, object] = {
        "moved_from": from_facet,
        "moved_to": to_facet,
        "line_number": new_item.index,
        "text": new_item.text,
    }
    if consent:
        params_out["consent"] = True
        params_in["consent"] = True
    log_call_action(facet=from_facet, action="todo_move_out", params=params_out)
    log_call_action(facet=to_facet, action="todo_move_in", params=params_in)
    typer.echo(
        f"Moved todo {line_number} ('{item.text}') from '{from_facet}' to '{to_facet}'."
    )


@app.command("upcoming")
def upcoming_todos(
    limit: int = typer.Option(20, "--limit", "-l", help="Maximum number of todos."),
    facet: str | None = typer.Option(
        None, "--facet", "-f", help="Facet name. Omit to show all facets."
    ),
) -> None:
    """Show upcoming todos across future days."""
    from solstone.think.utils import get_journal, get_sol_facet

    get_journal()
    if facet is None:
        facet = get_sol_facet()

    result = todo.upcoming(limit=limit, facet=facet)
    typer.echo(result)


def _due_nudges(
    facet: str | None,
) -> list[tuple[str, todo.TodoChecklist, todo.TodoItem]]:
    """Return due, unnotified nudges for today without mutating state."""
    from solstone.think.utils import get_journal, resolve_sol_facet

    journal = get_journal()
    today = datetime.now().strftime("%Y%m%d")
    now_str = datetime.now().strftime("%Y%m%dT%H:%M")

    facets_dir = Path(journal) / "facets"
    if not facets_dir.is_dir():
        return []

    if facet is not None:
        facet = resolve_sol_facet(facet)
        facet_names = [facet]
    else:
        facet_names = [d.name for d in facets_dir.iterdir() if d.is_dir()]

    due: list[tuple[str, todo.TodoChecklist, todo.TodoItem]] = []
    for facet_name in facet_names:
        checklist = todo.TodoChecklist.load(today, facet_name)
        if not checklist.exists:
            continue
        for item in checklist.items:
            if (
                item.nudge
                and item.nudge <= now_str
                and not item.notified
                and not item.completed
                and not item.cancelled
            ):
                due.append((facet_name, checklist, item))
    return due


@app.command("list-nudges-due")
def list_nudges_due(
    facet: str | None = typer.Option(
        None,
        "--facet",
        "-f",
        help="Facet name (or set SOL_FACET). Omit to check all facets.",
    ),
    json_output: bool = typer.Option(False, "--json", help="Output as JSON."),
) -> None:
    """List due, unnotified todo nudges."""
    due = _due_nudges(facet)
    now = datetime.now()
    if json_output:
        payload = [
            {
                "day": datetime.now().strftime("%Y%m%d"),
                "facet": facet_name,
                "index": item.index,
                "text": item.text,
                "nudge": item.nudge,
                "nudge_display": todo.format_nudge(item.nudge or "", now=now),
            }
            for facet_name, _checklist, item in due
        ]
        typer.echo(json.dumps(payload, ensure_ascii=False, indent=2))
        return

    if not due:
        typer.echo("No nudges due.")
        return

    grouped: dict[str, list[todo.TodoItem]] = {}
    for facet_name, _checklist, item in due:
        grouped.setdefault(facet_name, []).append(item)

    if len(grouped) == 1:
        items = next(iter(grouped.values()))
        for item in items:
            typer.echo(f"{item.index}: {item.display_line()}")
        return

    for facet_name, items in grouped.items():
        typer.echo(f"## {facet_name}")
        for item in items:
            typer.echo(f"{item.index}: {item.display_line()}")
        typer.echo()


@app.command("dispatch-nudges")
def dispatch_nudges(
    facet: str | None = typer.Option(
        None,
        "--facet",
        "-f",
        help="Facet name (or set SOL_FACET). Omit to check all facets.",
    ),
) -> None:
    """Dispatch due, unnotified todo nudges."""
    due = _due_nudges(facet)
    today = datetime.now().strftime("%Y%m%d")
    modified_checklists: dict[str, todo.TodoChecklist] = {}
    dispatched = 0

    for facet_name, checklist, item in due:
        try:
            subprocess.run(
                [
                    "sol",
                    "notify",
                    item.text,
                    "--title",
                    "Todo Reminder",
                    "--icon",
                    "✅",
                    "--app",
                    "todos",
                    "--facet",
                    facet_name,
                    "--action",
                    f"/app/todos/{today}",
                ],
                check=False,
                capture_output=True,
            )
        except FileNotFoundError:
            pass
        item.notified = True
        modified_checklists[facet_name] = checklist
        dispatched += 1

    for checklist in modified_checklists.values():
        checklist.save()

    typer.echo(f"dispatched {dispatched} nudge(s)")
