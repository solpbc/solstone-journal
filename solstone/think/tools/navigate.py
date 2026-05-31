# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI commands for browser navigation actions.

Top-level ``journal navigate`` command.
"""

import typer

from solstone.think.utils import require_solstone

app = typer.Typer()


@app.callback(invoke_without_command=True)
def navigate(
    path: str = typer.Argument(None, help="URL path to navigate to."),
    facet: str = typer.Option(None, "--facet", "-f", help="Facet to switch to."),
) -> None:
    """Navigate the browser to a path and/or switch facet."""
    require_solstone()
    if not path and not facet:
        typer.echo("Error: provide a path and/or --facet", err=True)
        raise typer.Exit(1)

    from solstone.think.callosum import callosum_send

    fields: dict = {}
    if path is not None:
        fields["path"] = path
    if facet is not None:
        fields["facet"] = facet

    callosum_send("navigate", "request", **fields)

    parts = []
    if path:
        parts.append(path)
    if facet:
        parts.append(f"[{facet}]")
    typer.echo(f"Navigate: {' '.join(parts)}")


def main() -> None:
    app()
