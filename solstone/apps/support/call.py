# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI commands for solstone support.

Auto-discovered by ``think.call`` and mounted as ``sol call support ...``.

Subcommands provide full access to the support portal: registration, KB search,
ticket management, feedback, announcements, and local diagnostics.
"""

from __future__ import annotations

import json
from pathlib import Path

import typer

app = typer.Typer(help="Support tools — file tickets, search KB, give feedback.")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _json_out(data: object) -> None:
    """Pretty-print JSON to stdout."""
    typer.echo(json.dumps(data, indent=2, default=str))


def _check_enabled() -> None:
    """Exit early if support is disabled in settings."""
    from solstone.apps.support.portal import is_enabled

    if not is_enabled():
        typer.echo("Support agent is disabled in settings.", err=True)
        raise typer.Exit(1)


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------


@app.command("register")
def register() -> None:
    """(Re-)register with the support portal."""
    _check_enabled()
    from solstone.apps.support.portal import get_client

    client = get_client()
    result = client.register()
    typer.echo(f"Registered as: {result.get('handle', '?')}")


@app.command("search")
def search(
    query: str = typer.Argument(..., help="Search query for KB articles."),
) -> None:
    """Search knowledge base articles."""
    _check_enabled()
    from solstone.apps.support.tools import support_search

    articles = support_search(query)
    if not articles:
        typer.echo("No articles found.")
        return

    for a in articles:
        typer.echo(f"  [{a.get('slug', '?')}] {a.get('title', 'Untitled')}")
    typer.echo(
        f"\n{len(articles)} article(s) found. Use `sol call support article <slug>` to read."
    )


@app.command("article")
def article(
    slug: str = typer.Argument(..., help="Article slug."),
    as_json: bool = typer.Option(False, "--json", help="Output raw JSON."),
) -> None:
    """Read a KB article."""
    _check_enabled()
    from solstone.apps.support.tools import support_article

    try:
        data = support_article(slug)
    except Exception as exc:
        typer.echo(f"Error: {exc}", err=True)
        raise typer.Exit(1) from None

    if as_json:
        _json_out(data)
    else:
        typer.echo(f"# {data.get('title', 'Untitled')}\n")
        typer.echo(data.get("content", "(no content)"))


@app.command("create")
def create(
    subject: str = typer.Option(..., "--subject", "-s", help="Ticket subject."),
    description: str = typer.Option(
        ..., "--description", "-d", help="Ticket description."
    ),
    product: str = typer.Option("solstone", "--product", "-p", help="Product name."),
    severity: str = typer.Option(
        "medium", "--severity", help="low, medium, high, critical."
    ),
    category: str | None = typer.Option(
        None, "--category", help="bug, feature, question, account."
    ),
    skip_kb: bool = typer.Option(
        False, "--skip-kb", help="Skip KB search before filing."
    ),
    yes: bool = typer.Option(False, "--yes", "-y", help="Skip confirmation prompt."),
    anonymous: bool = typer.Option(
        False, "--anonymous", help="Strip installation identifiers."
    ),
) -> None:
    """File a support ticket (KB-first flow with consent gate)."""
    _check_enabled()
    from solstone.apps.support.diagnostics import collect_all
    from solstone.apps.support.tools import support_create, support_search

    # Step 1: KB-first — search before filing
    if not skip_kb:
        typer.echo("Searching knowledge base...")
        articles = support_search(subject)
        if articles:
            typer.echo(f"\nFound {len(articles)} related article(s):")
            for a in articles:
                typer.echo(f"  [{a.get('slug', '?')}] {a.get('title', '')}")
            typer.echo(
                "\nThese may answer your question. "
                "Use `sol call support article <slug>` to read."
            )
            if not yes:
                proceed = typer.confirm("Still want to file a ticket?")
                if not proceed:
                    typer.echo("Cancelled.")
                    return

    # Step 2: Collect diagnostics
    diagnostics = collect_all()

    # Step 3: Present draft for review (consent gate)
    typer.echo("\n--- Ticket Draft ---")
    typer.echo(f"Subject:     {subject}")
    typer.echo(f"Product:     {product}")
    typer.echo(f"Severity:    {severity}")
    if category:
        typer.echo(f"Category:    {category}")
    typer.echo(f"Description: {description}")
    typer.echo(f"\nDiagnostic data ({len(json.dumps(diagnostics))} bytes):")
    typer.echo(json.dumps(diagnostics, indent=2, default=str))
    typer.echo("--- End Draft ---\n")

    if not yes:
        approved = typer.confirm("Submit this ticket?")
        if not approved:
            typer.echo("Cancelled — nothing was sent.")
            return

    # Step 4: Submit
    try:
        result = support_create(
            subject=subject,
            description=description,
            product=product,
            severity=severity,
            category=category,
            user_context=diagnostics,
            auto_context=False,
            anonymous=anonymous,
        )
        typer.echo(f"Ticket created: #{result.get('id', '?')}")
    except Exception as exc:
        typer.echo(f"Error submitting ticket: {exc}", err=True)
        raise typer.Exit(1) from None


@app.command("list")
def list_tickets(
    status: str | None = typer.Option(None, "--status", help="Filter by status."),
    as_json: bool = typer.Option(False, "--json", help="Output raw JSON."),
) -> None:
    """List your support tickets."""
    _check_enabled()
    from solstone.apps.support.tools import support_list

    tickets = support_list(status=status)
    if as_json:
        _json_out(tickets)
        return

    if not tickets:
        typer.echo("No tickets found.")
        return

    for t in tickets:
        status_str = t.get("status", "?")
        typer.echo(
            f"  #{t.get('id', '?'):>4}  [{status_str:<12}] {t.get('subject', 'Untitled')}"
        )
    typer.echo(f"\n{len(tickets)} ticket(s).")


@app.command("show")
def show(
    ticket_id: int = typer.Argument(..., help="Ticket ID."),
    as_json: bool = typer.Option(False, "--json", help="Output raw JSON."),
) -> None:
    """View a ticket with its message thread."""
    _check_enabled()
    from solstone.apps.support.tools import support_check

    try:
        data = support_check(ticket_id)
    except Exception as exc:
        typer.echo(f"Error: {exc}", err=True)
        raise typer.Exit(1) from None

    if as_json:
        _json_out(data)
        return

    typer.echo(f"# Ticket #{data.get('id', '?')}: {data.get('subject', '')}")
    typer.echo(
        f"Status: {data.get('status', '?')}  |  Severity: {data.get('severity', '?')}"
    )
    typer.echo(f"Created: {data.get('created_at', '?')}")
    typer.echo(f"\n{data.get('description', '')}")

    messages = data.get("messages", [])
    if messages:
        typer.echo(f"\n--- {len(messages)} message(s) ---")
        for msg in messages:
            handle = msg.get("handle", "?")
            typer.echo(f"\n[{handle}] {msg.get('created_at', '')}")
            typer.echo(msg.get("content", ""))
            attachments = msg.get("attachments", [])
            if attachments:
                for att in attachments:
                    size = att.get("size_bytes", 0)
                    if size >= 1024 * 1024:
                        size_str = f"{size / 1024 / 1024:.1f} MB"
                    elif size >= 1024:
                        size_str = f"{size / 1024:.0f} KB"
                    else:
                        size_str = f"{size} bytes"
                    typer.echo(f"  📎 {att.get('filename', '?')} ({size_str})")


@app.command("reply")
def reply(
    ticket_id: int = typer.Argument(..., help="Ticket ID."),
    body: str = typer.Option(..., "--body", "-b", help="Reply content."),
    yes: bool = typer.Option(False, "--yes", "-y", help="Skip confirmation."),
) -> None:
    """Reply to a ticket."""
    _check_enabled()
    from solstone.apps.support.tools import support_reply

    if not yes:
        typer.echo(f"Reply to ticket #{ticket_id}:\n{body}\n")
        if not typer.confirm("Send this reply?"):
            typer.echo("Cancelled.")
            return

    try:
        support_reply(ticket_id, body)
        typer.echo(f"Reply sent to ticket #{ticket_id}.")
    except Exception as exc:
        typer.echo(f"Error: {exc}", err=True)
        raise typer.Exit(1) from None


@app.command("attach")
def attach(
    ticket_id: int = typer.Argument(..., help="Ticket ID to attach files to."),
    files: list[Path] = typer.Argument(..., help="File(s) to attach."),
    yes: bool = typer.Option(False, "--yes", "-y", help="Skip confirmation."),
) -> None:
    """Attach file(s) to a ticket."""
    _check_enabled()
    from solstone.apps.support.portal import PortalClient
    from solstone.apps.support.tools import support_attach

    # Validate files up front
    for f in files:
        if not f.is_file():
            typer.echo(f"Error: file not found: {f}", err=True)
            raise typer.Exit(1)

    if len(files) > PortalClient.MAX_ATTACHMENTS_PER_MESSAGE:
        typer.echo(
            f"Error: max {PortalClient.MAX_ATTACHMENTS_PER_MESSAGE} files per upload.",
            err=True,
        )
        raise typer.Exit(1)

    # Consent gate — show what will be uploaded
    typer.echo(f"\n--- Attachment Review (ticket #{ticket_id}) ---")
    for f in files:
        size = f.stat().st_size
        if size >= 1024 * 1024:
            size_str = f"{size / 1024 / 1024:.1f} MB"
        elif size >= 1024:
            size_str = f"{size / 1024:.0f} KB"
        else:
            size_str = f"{size} bytes"
        typer.echo(f"  {f.name}  ({size_str})")
    typer.echo("--- End Review ---\n")

    if not yes:
        approved = typer.confirm("Upload these files?")
        if not approved:
            typer.echo("Cancelled — nothing was sent.")
            return

    for f in files:
        try:
            result = support_attach(ticket_id, str(f))
            typer.echo(f"Attached: {f.name} (id: {result.get('id', '?')})")
        except ValueError as exc:
            typer.echo(f"Skipped {f.name}: {exc}", err=True)
        except Exception as exc:
            typer.echo(f"Error uploading {f.name}: {exc}", err=True)
            raise typer.Exit(1) from None


@app.command("feedback")
def feedback(
    body: str = typer.Option(..., "--body", "-b", help="Your feedback."),
    product: str = typer.Option("solstone", "--product", "-p", help="Product name."),
    anonymous: bool = typer.Option(False, "--anonymous", help="Submit anonymously."),
    yes: bool = typer.Option(False, "--yes", "-y", help="Skip confirmation."),
) -> None:
    """Submit feedback (lower friction than a full ticket)."""
    _check_enabled()
    from solstone.apps.support.tools import support_feedback

    if not yes:
        typer.echo(f"Feedback:\n{body}\n")
        anon_note = " (anonymous)" if anonymous else ""
        if not typer.confirm(f"Submit this feedback{anon_note}?"):
            typer.echo("Cancelled.")
            return

    try:
        result = support_feedback(body=body, product=product, anonymous=anonymous)
        typer.echo(f"Feedback submitted: #{result.get('id', '?')}")
    except Exception as exc:
        typer.echo(f"Error: {exc}", err=True)
        raise typer.Exit(1) from None


@app.command("announcements")
def announcements(
    as_json: bool = typer.Option(False, "--json", help="Output raw JSON."),
) -> None:
    """Check for product updates and known issues."""
    _check_enabled()
    from solstone.apps.support.tools import support_announcements

    items = support_announcements()
    if as_json:
        _json_out(items)
        return

    if not items:
        typer.echo("No active announcements.")
        return

    for a in items:
        icon = {"known-issue": "⚠️", "maintenance": "🔧"}.get(a.get("type", ""), "📢")
        typer.echo(f"  {icon} {a.get('title', 'Untitled')}")
        if a.get("content"):
            typer.echo(f"     {a['content'][:120]}")
    typer.echo(f"\n{len(items)} announcement(s).")


@app.command("diagnose")
def diagnose(
    as_json: bool = typer.Option(False, "--json", help="Output raw JSON."),
) -> None:
    """Run local diagnostics (no network)."""
    from solstone.apps.support.tools import support_diagnose

    data = support_diagnose()
    if as_json:
        _json_out(data)
    else:
        typer.echo("# Local Diagnostics\n")
        typer.echo(f"Version:  {data.get('version', 'unknown')}")
        plat = data.get("platform", {})
        typer.echo(
            f"Platform: {plat.get('system', '?')} {plat.get('release', '')} "
            f"({plat.get('machine', '')})"
        )
        typer.echo(f"Python:   {plat.get('python', '?')}")

        services = data.get("services", {})
        if services:
            typer.echo("\nServices:")
            for name, status in sorted(services.items()):
                icon = "✓" if status == "running" else "✗"
                typer.echo(f"  {icon} {name}: {status}")

        errors = data.get("recent_errors", [])
        if errors:
            typer.echo(f"\nRecent errors ({len(errors)}):")
            for e in errors:
                t = e.get("time", "")
                if t and e.get("time_approximate"):
                    t = "~" + t
                prefix = (t + " ") if t else ""
                typer.echo(
                    f"  {prefix}[{e.get('service', '?')}] {e.get('message', '')[:100]}"
                )
        else:
            typer.echo("\nNo recent errors.")
