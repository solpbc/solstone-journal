# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Observer app event handlers for observer segment processing state."""

import logging

from solstone.apps.events import EventContext, on_event
from solstone.think.utils import now_ms

from .utils import (
    append_history_record,
    find_observer_by_name,
    increment_stat,
    observer_filename_prefix,
)

logger = logging.getLogger(__name__)


@on_event("observe", "observed")
def handle_observed(ctx: EventContext) -> None:
    """Track observe.observed events for observer-originated segments.

    When a segment from an observer completes processing, append
    an 'observed' record to that observer's sync history. This enables
    observers to verify end-to-end success via the segments API.
    """
    observer_name = ctx.msg.get("observer")
    if not observer_name:
        return  # Not an observer segment

    segment = ctx.msg.get("segment")
    day = ctx.msg.get("day")
    if not segment or not day:
        logger.warning(
            f"observe.observed missing segment/day for observer {observer_name}"
        )
        return

    # Find observer by name to get key prefix
    observer = find_observer_by_name(observer_name)
    if not observer:
        logger.debug(f"Observer not found for observed event: {observer_name}")
        return

    try:
        key_prefix = observer_filename_prefix(observer)
    except ValueError:
        return

    # Append observed record to history
    record = {
        "ts": now_ms(),
        "type": "observed",
        "segment": segment,
    }
    append_history_record(key_prefix, day, record)

    # Update stats
    increment_stat(key_prefix, "segments_observed")

    logger.debug(
        f"Recorded observed status for observer {observer_name}: {day}/{segment}"
    )


@on_event("observe", "transferred")
def handle_transferred(ctx: EventContext) -> None:
    """Handle observe.transferred events for transfer-originated segments.

    When a transferred segment is received, append a 'transferred' record
    to the observer's sync history, increment stats, and queue an indexer
    rescan to pick up the new content.
    """
    observer_name = ctx.msg.get("observer")
    if not observer_name:
        return

    segment = ctx.msg.get("segment")
    day = ctx.msg.get("day")
    if not segment or not day:
        logger.warning(
            f"observe.transferred missing segment/day for observer {observer_name}"
        )
        return

    observer = find_observer_by_name(observer_name)
    if not observer:
        logger.debug(f"Observer not found for transferred event: {observer_name}")
        return

    try:
        key_prefix = observer_filename_prefix(observer)
    except ValueError:
        return

    record = {
        "ts": now_ms(),
        "type": "transferred",
        "segment": segment,
    }
    append_history_record(key_prefix, day, record)

    increment_stat(key_prefix, "segments_transferred")

    # Queue indexer rescan to pick up transferred content
    from solstone.think.callosum import callosum_send

    callosum_send("supervisor", "request", cmd=["journal", "indexer", "--rescan"])

    logger.debug(
        f"Recorded transferred status for observer {observer_name}: {day}/{segment}"
    )
