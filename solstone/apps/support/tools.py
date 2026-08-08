# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Support tool functions for agent workflows.

Each function provides a discrete capability that both the talent agent
(via ``sol call support``) and the convey routes can use.  All outbound
operations are **consent-gated** — they return a draft for review rather
than submitting directly.
"""

from __future__ import annotations

import logging
from typing import Any

logger = logging.getLogger(__name__)


def support_diagnose() -> dict[str, Any]:
    """Run local diagnostics — no network.

    Returns a dict of system state suitable for the ``user_context`` field.
    """
    from solstone.apps.support.diagnostics import collect_all

    return collect_all()


def support_search(query: str, portal_url: str | None = None) -> list[dict[str, Any]]:
    """Search the knowledge base for articles matching *query*."""
    from solstone.apps.support.portal import get_client

    client = get_client(portal_url=portal_url)
    return client.search_articles(query=query)


def support_article(slug: str, portal_url: str | None = None) -> dict[str, Any]:
    """Read a single KB article by slug."""
    from solstone.apps.support.portal import get_client

    client = get_client(portal_url=portal_url)
    return client.get_article(slug)


def support_create(
    *,
    subject: str,
    description: str,
    product: str = "solstone",
    severity: str = "medium",
    category: str | None = None,
    user_email: str | None = None,
    user_context: dict | None = None,
    auto_context: bool = True,
    portal_url: str | None = None,
    anonymous: bool = False,
    action_id: str,
) -> dict[str, Any]:
    """Create a support ticket.

    If *auto_context* is True (default), diagnostic data is collected
    and merged into *user_context*.
    """
    from solstone.apps.support.portal import get_client

    if auto_context:
        from solstone.apps.support.diagnostics import collect_all

        diag = collect_all()
        if user_context:
            diag.update(user_context)
        user_context = diag

    client = get_client(portal_url=portal_url, anonymous=anonymous)
    return client.create_ticket(
        product=product,
        subject=subject,
        description=description,
        severity=severity,
        category=category,
        user_email=user_email,
        user_context=user_context,
        action_id=action_id,
    )


def support_feedback(
    *,
    body: str,
    product: str = "solstone",
    portal_url: str | None = None,
    anonymous: bool = False,
    user_email: str | None = None,
    action_id: str,
) -> dict[str, object]:
    """Submit feedback (lower-friction path).

    Feedback is a ticket with ``category="feedback"`` and low severity.
    """
    from solstone.apps.support.portal import get_client

    client = get_client(portal_url=portal_url, anonymous=anonymous)
    return client.submit_feedback(
        body=body,
        product=product,
        user_email=user_email,
        action_id=action_id,
    )


def support_list(
    *,
    status: str | None = None,
    portal_url: str | None = None,
) -> list[dict[str, Any]]:
    """List the user's tickets."""
    from solstone.apps.support.portal import get_client

    client = get_client(portal_url=portal_url)
    return client.list_tickets(status=status)


def support_check(
    ticket_id: int,
    portal_url: str | None = None,
) -> dict[str, Any]:
    """Check status of a specific ticket (with message thread)."""
    from solstone.apps.support.portal import get_client

    client = get_client(portal_url=portal_url)
    return client.get_ticket(ticket_id)


def support_history(
    *,
    cursor: str | None = None,
    portal_url: str | None = None,
) -> dict[str, Any]:
    """List closed-ticket history using the server's opaque cursor."""
    from solstone.apps.support.portal import get_client

    client = get_client(portal_url=portal_url)
    return client.list_closed_history(cursor=cursor)


def support_close(
    ticket_id: int,
    *,
    action_id: str,
    portal_url: str | None = None,
) -> dict[str, Any]:
    """Close a support ticket."""
    from solstone.apps.support.portal import get_client

    client = get_client(portal_url=portal_url)
    return client.close_ticket(ticket_id, action_id=action_id)


def support_resolved(
    ticket_id: int,
    *,
    action_id: str,
    portal_url: str | None = None,
) -> dict[str, Any]:
    """Confirm a proposed ticket resolution."""
    from solstone.apps.support.portal import get_client

    client = get_client(portal_url=portal_url)
    return client.confirm_resolution(ticket_id, action_id=action_id)


def support_still_need_help(
    ticket_id: int,
    *,
    action_id: str,
    portal_url: str | None = None,
) -> dict[str, Any]:
    """Reject a proposed ticket resolution."""
    from solstone.apps.support.portal import get_client

    client = get_client(portal_url=portal_url)
    return client.still_need_help(ticket_id, action_id=action_id)


def support_reply(
    ticket_id: int,
    content: str,
    portal_url: str | None = None,
    *,
    action_id: str,
) -> dict[str, Any]:
    """Reply to a ticket."""
    from solstone.apps.support.portal import get_client

    client = get_client(portal_url=portal_url)
    return client.reply_to_ticket(ticket_id, content, action_id=action_id)


def support_attach(
    ticket_id: int,
    file_path: str,
    *,
    action_id: str,
    index: int = 0,
    filename: str | None = None,
    portal_url: str | None = None,
) -> dict[str, Any]:
    """Attach a file to an existing ticket.

    Returns the attachment metadata from the portal.
    """
    from pathlib import Path

    from solstone.apps.support.portal import get_client

    client = get_client(portal_url=portal_url)
    return client.attach_file(
        ticket_id,
        Path(file_path),
        action_id=action_id,
        index=index,
        filename=filename,
    )


def support_announcements(
    portal_url: str | None = None,
) -> list[dict[str, Any]]:
    """List active announcements."""
    from solstone.apps.support.portal import get_client

    client = get_client(portal_url=portal_url)
    return client.list_announcements()
