# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI commands for managing user-defined routines.

Top-level ``journal routines`` command.
"""

import json
import sys
import uuid
from datetime import datetime
from datetime import timezone as dt_tz
from pathlib import Path
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

import frontmatter
import typer

from solstone.think.routines import _run_routine, cron_matches, get_config, save_config
from solstone.think.utils import get_journal, get_project_root, require_solstone

app = typer.Typer(help="Manage custom routines.")


@app.callback()
def _require_up() -> None:
    require_solstone()


def _resolve_id(config: dict[str, dict], prefix: str) -> str:
    """Resolve a routine by UUID prefix or exact name (case-insensitive)."""
    matches = sorted(
        rid for rid in config if not rid.startswith("_") and rid.startswith(prefix)
    )
    if len(matches) == 1:
        return matches[0]
    if len(matches) > 1:
        typer.echo(f"Error: routine id '{prefix}' is ambiguous.", err=True)
        raise typer.Exit(1)

    lower = prefix.lower()
    name_matches = sorted(
        rid
        for rid, routine in config.items()
        if routine.get("id") and routine.get("name", "").lower() == lower
    )
    if not name_matches:
        typer.echo(f"Error: routine '{prefix}' not found.", err=True)
        raise typer.Exit(1)
    if len(name_matches) > 1:
        typer.echo(f"Error: routine name '{prefix}' is ambiguous.", err=True)
        raise typer.Exit(1)
    return name_matches[0]


def _format_last_run(value: str | None) -> str:
    if not value:
        return "never"
    try:
        return datetime.fromisoformat(value).strftime("%Y-%m-%d %H:%M")
    except ValueError:
        return value


def _validate_timezone(name: str) -> None:
    try:
        ZoneInfo(name)
    except ZoneInfoNotFoundError:
        typer.echo(f"Error: invalid timezone: {name}", err=True)
        raise typer.Exit(1)


def _parse_enabled(value: str) -> bool:
    """Parse a CLI boolean value for routine enablement."""
    normalized = value.strip().lower()
    if normalized in {"true", "1", "yes", "on"}:
        return True
    if normalized in {"false", "0", "no", "off"}:
        return False
    typer.echo("Error: enabled must be true or false.", err=True)
    raise typer.Exit(1)


def _templates_dir() -> Path:
    """Resolve the routines templates directory."""
    return Path(get_project_root()) / "routines" / "templates"


def _load_template(name: str) -> tuple[dict, str]:
    """Load a template by name. Returns (metadata, instruction_body)."""
    path = _templates_dir() / f"{name}.md"
    if not path.is_file():
        typer.echo(f"Error: template '{name}' not found.", err=True)
        raise typer.Exit(1)
    post = frontmatter.load(path)
    return dict(post.metadata), post.content.strip()


def _format_cadence(cadence: object) -> str:
    """Format a cadence value for display."""
    return str(cadence)


def _validate_routine_cadence(cadence: object) -> None:
    """Validate a cadence value accepted by routine config."""
    if isinstance(cadence, str):
        try:
            cron_matches(cadence, datetime.now())
        except ValueError as exc:
            typer.echo(f"Error: invalid cadence: {exc}", err=True)
            raise typer.Exit(1)
        return

    # Keep this cadence-object validation in sync with think.routines.check().
    if isinstance(cadence, dict):
        if "type" not in cadence:
            typer.echo("Error: invalid cadence: missing 'type' field", err=True)
            raise typer.Exit(1)

        cadence_type = cadence["type"]
        if not isinstance(cadence_type, str):
            typer.echo("Error: invalid cadence: 'type' must be a string", err=True)
            raise typer.Exit(1)

        if cadence_type == "activity-anticipation":
            if "offset_minutes" in cadence:
                try:
                    int(cadence["offset_minutes"])
                except (TypeError, ValueError):
                    typer.echo(
                        "Error: invalid cadence: offset_minutes must be an integer",
                        err=True,
                    )
                    raise typer.Exit(1)
            return

        typer.echo(
            f"Error: invalid cadence: unsupported cadence type {cadence_type!r}",
            err=True,
        )
        raise typer.Exit(1)

    typer.echo(
        "Error: invalid cadence: must be a cron string or cadence object", err=True
    )
    raise typer.Exit(1)


@app.command("list")
def list_routines() -> None:
    """List all routines."""
    config = get_config()
    routines = {k: v for k, v in config.items() if v.get("id")}
    if not routines:
        typer.echo("No routines configured.")
        return

    for routine in routines.values():
        routine_id = routine.get("id", "")
        enabled_marker = "on" if routine.get("enabled") else "off"
        resume_date = routine.get("resume_date")
        if not routine.get("enabled") and resume_date:
            enabled_marker = f"off (resumes {resume_date})"
        cadence_display = _format_cadence(routine.get("cadence", ""))
        last_run_display = _format_last_run(routine.get("last_run"))
        name = routine.get("name", "")
        typer.echo(
            f"{routine_id[:8]}  {enabled_marker:<25}  {cadence_display:<20}  {last_run_display:<20}  {name}"
        )


@app.command()
def templates() -> None:
    """List available routine templates."""
    tpl_dir = _templates_dir()
    if not tpl_dir.is_dir():
        typer.echo("No templates directory found.")
        return
    found = False
    for path in sorted(tpl_dir.glob("*.md")):
        post = frontmatter.load(path)
        desc = post.metadata.get("description", "")
        typer.echo(f"{path.stem:<25}  {desc}")
        found = True
    if not found:
        typer.echo("No templates found.")


@app.command()
def create(
    name: str = typer.Option(None, help="Routine name"),
    instruction: str = typer.Option(None, help="Natural-language instruction"),
    cadence: str = typer.Option(None, help="Cron expression (5-field)"),
    tz: str = typer.Option("", "--timezone", help="IANA timezone"),
    facets: str = typer.Option("", help="Comma-separated facet names"),
    template: str = typer.Option("", help="Template name"),
) -> None:
    """Create a routine."""
    metadata: dict = {}
    template_body = ""
    if template:
        metadata, template_body = _load_template(template)
        name = name or metadata.get("name", template)
        instruction = instruction or template_body
        if cadence is None:
            cadence = metadata.get("default_cadence")
        if not tz:
            tz = str(metadata.get("default_timezone", "UTC"))
        if not facets:
            default_facets = metadata.get("default_facets", [])
            if isinstance(default_facets, list):
                facets = ",".join(str(facet) for facet in default_facets)

    if name is None:
        typer.echo("Error: routine name is required.", err=True)
        raise typer.Exit(1)
    if instruction is None:
        typer.echo("Error: instruction is required.", err=True)
        raise typer.Exit(1)
    if cadence is None:
        typer.echo("Error: cadence is required.", err=True)
        raise typer.Exit(1)

    _validate_routine_cadence(cadence)
    if not tz:
        tz = "UTC"
    _validate_timezone(tz)

    routine_id = str(uuid.uuid4())
    routine = {
        "id": routine_id,
        "name": name,
        "instruction": instruction,
        "cadence": cadence,
        "timezone": tz,
        "facets": [f.strip() for f in facets.split(",") if f.strip()],
        "enabled": True,
        "created": datetime.now(dt_tz.utc).isoformat(),
        "last_run": None,
        "template": template or None,
        "notify": False,
    }

    config = get_config()
    config[routine_id] = routine
    save_config(config)
    typer.echo(f'Created routine {routine_id[:8]} "{name}"')


@app.command()
def edit(
    routine_id: str = typer.Argument(help="Routine ID (or prefix)"),
    name: str | None = typer.Option(None, help="New name"),
    instruction: str | None = typer.Option(None, help="New instruction"),
    cadence: str | None = typer.Option(None, help="New cron expression"),
    tz: str | None = typer.Option(None, "--timezone", help="New timezone"),
    enabled: str | None = typer.Option(None, help="Enable or disable"),
    resume_date: str | None = typer.Option(
        None, "--resume-date", help="ISO date (YYYY-MM-DD) to auto-resume"
    ),
    facets: str | None = typer.Option(None, help="Comma-separated facet names"),
    template: str | None = typer.Option(None, help="Template name"),
) -> None:
    """Edit a routine."""
    config = get_config()
    full_id = _resolve_id(config, routine_id)
    routine = config[full_id]

    if cadence is not None:
        try:
            cron_matches(cadence, datetime.now())
        except ValueError as exc:
            typer.echo(f"Error: invalid cadence: {exc}", err=True)
            raise typer.Exit(1)
        routine["cadence"] = cadence
    if name is not None:
        routine["name"] = name
    if instruction is not None:
        routine["instruction"] = instruction
    if tz is not None:
        _validate_timezone(tz)
        routine["timezone"] = tz
    enabled_value: bool | None = None
    if enabled is not None:
        enabled_value = _parse_enabled(enabled)
        routine["enabled"] = enabled_value
    if enabled_value is True:
        routine.pop("resume_date", None)
    if resume_date is not None:
        if resume_date == "":
            routine.pop("resume_date", None)
        else:
            try:
                datetime.strptime(resume_date, "%Y-%m-%d")
            except ValueError:
                typer.echo("Error: resume-date must be YYYY-MM-DD format.", err=True)
                raise typer.Exit(1)
            routine["resume_date"] = resume_date
    if facets is not None:
        routine["facets"] = [f.strip() for f in facets.split(",") if f.strip()]
    if template is not None:
        routine["template"] = template or None

    config[full_id] = routine
    save_config(config)
    typer.echo(f'Updated routine {full_id[:8]} "{routine.get("name", "")}"')


@app.command()
def delete(routine_id: str = typer.Argument(help="Routine ID (or prefix)")) -> None:
    """Delete a routine."""
    config = get_config()
    full_id = _resolve_id(config, routine_id)
    routine = config.pop(full_id)

    template_name = routine.get("template")
    if template_name:
        meta = config.get("_meta", {})
        suggestions = meta.get("suggestions", {})
        entry = suggestions.get(template_name)
        if entry and entry.get("response") == "accepted":
            entry["trigger_count"] = 0
            entry["first_trigger"] = None
            entry["last_trigger"] = None
            entry["trigger_data"] = {}
            entry["response"] = None
            entry["suggested"] = False

    save_config(config)
    typer.echo(f'Deleted routine {full_id[:8]} "{routine.get("name", "")}"')


@app.command()
def run(routine_id: str = typer.Argument(help="Routine ID (or prefix)")) -> None:
    """Run a routine immediately."""
    config = get_config()
    full_id = _resolve_id(config, routine_id)
    routine = config[full_id]
    typer.echo(f'Running routine "{routine.get("name", "")}"...')
    _run_routine(routine)
    typer.echo("Done.")


@app.command()
def output(
    routine_id: str = typer.Argument(help="Routine ID (or prefix)"),
    date: str | None = typer.Option(None, help="Date (YYYY-MM-DD) to show output for"),
) -> None:
    """Print routine output (most recent, or for a specific date)."""
    config = get_config()
    full_id = _resolve_id(config, routine_id)
    output_dir = Path(get_journal()) / "routines" / full_id
    if not output_dir.exists():
        typer.echo("No output yet.")
        return
    if date is not None:
        date_prefix = date.replace("-", "")
        matches = sorted(
            output_dir.glob(f"{date_prefix}*.md"),
            key=lambda path: (len(path.stem), path.stem),
        )
        if not matches:
            typer.echo("No output for that date.")
            return
        sys.stdout.write(matches[-1].read_text(encoding="utf-8"))
    else:
        outputs = sorted(output_dir.glob("*.md"), reverse=True)
        if not outputs:
            typer.echo("No output yet.")
            return
        sys.stdout.write(outputs[0].read_text(encoding="utf-8"))


@app.command()
def suggestions(
    enable: bool | None = typer.Option(
        None, "--enable/--disable", help="Toggle suggestions"
    ),
) -> None:
    """Manage routine suggestions."""
    config = get_config()
    meta = config.setdefault("_meta", {})
    if enable is not None:
        meta["suggestions_enabled"] = enable
        save_config(config)
        state = "enabled" if enable else "disabled"
        typer.echo(f"Routine suggestions {state}.")
    else:
        current = meta.get("suggestions_enabled", True)
        state = "enabled" if current else "disabled"
        typer.echo(f"Routine suggestions are {state}.")


@app.command("suggest-respond")
def suggest_respond(
    template: str = typer.Argument(help="Template name"),
    accepted: bool = typer.Option(False, "--accepted", help="Accept suggestion"),
    declined: bool = typer.Option(False, "--declined", help="Decline suggestion"),
) -> None:
    """Record response to a routine suggestion."""
    if accepted == declined:
        typer.echo(
            "Error: exactly one of --accepted or --declined is required.", err=True
        )
        raise typer.Exit(code=1)

    config = get_config()
    meta = config.setdefault("_meta", {})
    suggestions = meta.get("suggestions", {})

    if template not in suggestions:
        typer.echo(f"Error: no suggestion state for template '{template}'.", err=True)
        raise typer.Exit(code=1)

    from datetime import date

    today = date.today().isoformat()
    entry = suggestions[template]

    if accepted:
        entry["response"] = "accepted"
    else:
        entry["response"] = "declined"

    entry["suggested"] = True
    entry["last_suggestion_date"] = today
    meta["last_suggestion_date"] = today

    save_config(config)
    action = "accepted" if accepted else "declined"
    typer.echo(f"Suggestion for '{template}' {action}.")


@app.command("suggest-state")
def suggest_state() -> None:
    """Show suggestion state for all templates."""
    config = get_config()
    meta = config.get("_meta", {})
    suggestions = meta.get("suggestions", {})
    typer.echo(json.dumps(suggestions, indent=2))


def main() -> None:
    app()
