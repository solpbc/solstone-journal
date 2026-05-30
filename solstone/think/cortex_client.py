# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Cortex client for managing AI talent requests."""

import json
import logging
import threading
from pathlib import Path
from typing import Any, Dict, Optional

from solstone.think.callosum import CallosumConnection, callosum_send_classified
from solstone.think.utils import get_journal, now_ms

logger = logging.getLogger(__name__)


class CortexSpawnUnavailable(Exception):
    """Raised when a Cortex spawn request cannot reach Callosum."""

    def __init__(self, detail: str = "") -> None:
        super().__init__("cortex spawn unavailable")
        self.detail = detail


# Module-level state for monotonic timestamp generation
_last_ts = 0


def _find_use_file(talents_dir: Path, use_id: str) -> tuple[Path | None, str]:
    """Find a use log file in per-talent subdirectories.

    Returns:
        Tuple of (file_path, status) where status is
        "completed", "running", or "not_found".
    """
    for match in talents_dir.glob(f"*/{use_id}.jsonl"):
        return match, "completed"
    for match in talents_dir.glob(f"*/{use_id}_active.jsonl"):
        return match, "running"
    return None, "not_found"


def cortex_request(
    prompt: str,
    name: str,
    provider: Optional[str] = None,
    config: Optional[Dict[str, Any]] = None,
    use_id: Optional[str] = None,
) -> str | None:
    """Create a Cortex talent request via Callosum broadcast.

    Args:
        prompt: The task or question for the talent
        name: Talent name - system (e.g., "chat") or app-qualified (e.g., "entities:entity_assist")
        provider: AI provider - openai, google, or anthropic
        config: Provider-specific configuration (model, max_output_tokens, thinking_budget, etc.)
        use_id: Optional pre-reserved use_id. When omitted, a unique timestamp is allocated.

    Returns:
        Use ID (timestamp-based string), or None if the Callosum send failed.
    """
    # Get journal path (for use_id uniqueness check)
    journal_path = get_journal()

    # Create talents directory if it doesn't exist
    talents_dir = Path(journal_path) / "talents"
    talents_dir.mkdir(parents=True, exist_ok=True)

    # Generate monotonic timestamp in milliseconds, ensuring uniqueness
    global _last_ts
    if use_id is None:
        ts = now_ms()

        if ts <= _last_ts:
            ts = _last_ts + 1

        _last_ts = ts
        use_id = str(ts)
    else:
        if not use_id.isdigit():
            raise ValueError("use_id must be a millisecond timestamp string")
        ts = int(use_id)
        if ts > _last_ts:
            _last_ts = ts

    # Build request object
    request = {
        "event": "request",
        "ts": ts,
        "use_id": use_id,
        "prompt": prompt,
        "provider": provider,
        "name": name,
    }

    # Add optional fields
    if config:
        if not isinstance(config, dict):
            raise ValueError("config must be a dictionary")
        # Merge config overrides directly into the request for a flat schema
        request.update(config)

    # Broadcast request to Callosum via classified send.
    # Remove "event" from request dict to avoid conflict with the send signature.
    request_fields = {k: v for k, v in request.items() if k != "event"}
    unavailable_detail = callosum_send_classified("cortex", "request", **request_fields)

    if unavailable_detail:
        logger.info("Failed to send cortex request for talent '%s'", name)
        raise CortexSpawnUnavailable(detail=unavailable_detail)

    return use_id


def get_use_log_status(use_id: str) -> str:
    """Get the status of a specific use from its log file.

    Args:
        use_id: The use ID (timestamp)

    Returns:
        "completed" - Use finished (*.jsonl exists)
        "running" - Use still active (*_active.jsonl exists)
        "not_found" - No use file exists
    """
    talents_dir = Path(get_journal()) / "talents"
    _, status = _find_use_file(talents_dir, use_id)
    return status


def wait_for_uses(
    use_ids: list[str],
    timeout: int | None = 600,
) -> tuple[dict[str, str], list[str]]:
    """Wait for uses to complete via Callosum events.

    Listens for cortex.finish and cortex.error events. Sets up the event
    listener first, then does an initial file check for uses that may have
    already completed, and a final file check at timeout as a backstop for
    any missed events.

    Args:
        use_ids: List of use IDs to wait for
        timeout: Maximum wait time in seconds (default 600 = 10 minutes)

    Returns:
        Tuple of (completed, timed_out) where completed is a dict mapping
        use_id to end state ("finish" or "error"), and timed_out is a
        list of use IDs that did not complete within the timeout.
    """
    pending = set(use_ids)
    completed: dict[str, str] = {}
    lock = threading.Lock()
    all_done = threading.Event()

    def on_message(msg: dict) -> None:
        if msg.get("tract") != "cortex":
            return
        use_id = msg.get("use_id")
        if not use_id:
            return

        event_type = msg.get("event")
        if event_type in ("finish", "error"):
            with lock:
                if use_id in pending:
                    completed[use_id] = event_type
                    pending.discard(use_id)
                    if not pending:
                        all_done.set()

    # Start listener BEFORE initial check to avoid race condition
    listener = CallosumConnection()
    listener.start(callback=on_message)

    try:
        # Initial file check (with lock since callback may be running)
        with lock:
            for use_id in list(pending):
                end_state = get_use_end_state(use_id)
                if end_state in ("finish", "error"):
                    completed[use_id] = end_state
                    pending.discard(use_id)

            if not pending:
                return completed, []

        # Wait for all completions or timeout
        all_done.wait(timeout=timeout)

    finally:
        listener.stop()

    # Final file check for any remaining (backstop for missed events)
    # Listener is stopped, so no lock needed
    for use_id in list(pending):
        end_state = get_use_end_state(use_id)
        if end_state in ("finish", "error"):
            logger.info(
                f"Talent use {use_id} completion event not received but use completed"
            )
            completed[use_id] = end_state
            pending.discard(use_id)

    return completed, list(pending)


def get_use_end_state(use_id: str) -> str:
    """Get how a completed use ended (finish or error).

    Checks file contents for terminal events even if file is still _active.jsonl,
    since Callosum broadcasts happen before file rename.

    Args:
        use_id: The use ID (timestamp)

    Returns:
        "finish" - Use completed successfully
        "error" - Use ended with an error
        "running" - Use is still active (no terminal event in file)
        "unknown" - Use file not found
    """
    status = get_use_log_status(use_id)
    if status == "not_found":
        return "unknown"

    # Read events to find terminal state (even for "running" files that may
    # have finish event - Callosum broadcast happens before file rename)
    try:
        events = read_use_events(use_id)
        # Find last finish or error event
        for event in reversed(events):
            event_type = event.get("event")
            if event_type == "finish":
                return "finish"
            if event_type == "error":
                return "error"
        # No terminal event found - still running
        return "running"
    except FileNotFoundError:
        return "unknown"


def read_use_events(use_id: str) -> list[Dict[str, Any]]:
    """Read all events from a use's JSONL log file.

    Args:
        use_id: The use ID (timestamp)

    Returns:
        List of event dictionaries in chronological order

    Raises:
        FileNotFoundError: If the use log doesn't exist
    """
    talents_dir = Path(get_journal()) / "talents"
    use_file, _status = _find_use_file(talents_dir, use_id)
    if use_file is None:
        raise FileNotFoundError(f"Talent log not found: {use_id}")

    events = []
    with open(use_file, "r") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
                events.append(event)
            except json.JSONDecodeError:
                logger.debug(f"Skipping malformed JSON in {use_file}")
                continue

    return events


def read_use_provider_model(use_id: str) -> tuple[str | None, str | None]:
    """Return resolved provider/model from a use log's start event, if present."""
    try:
        events = read_use_events(use_id)
    except FileNotFoundError:
        return None, None

    provider: str | None = None
    model: str | None = None
    for event in events:
        if event.get("event") != "start":
            continue
        raw_provider = event.get("provider")
        raw_model = event.get("model")
        provider = raw_provider if isinstance(raw_provider, str) else None
        model = raw_model if isinstance(raw_model, str) else None
    return provider, model


def cortex_uses(
    limit: int = 10,
    offset: int = 0,
    use_type: str = "all",
    facet: Optional[str] = None,
) -> Dict[str, Any]:
    """List talent uses from the journal with pagination and filtering.

    Legacy unnamed run logs predate the chat rename and are surfaced as chat.

    Args:
        limit: Maximum number of uses to return (1-100)
        offset: Number of uses to skip
        use_type: Filter by "live", "historical", or "all"
        facet: Optional facet to filter by. If provided, only returns uses
               that were run in this facet context. None means no filtering.

    Returns:
        Dictionary with use list and pagination info
    """
    # Validate parameters
    limit = max(1, min(limit, 100))
    offset = max(0, offset)

    talents_dir = Path(get_journal()) / "talents"
    if not talents_dir.exists():
        return {
            "uses": [],
            "pagination": {
                "limit": limit,
                "offset": offset,
                "total": 0,
                "has_more": False,
            },
            "live_count": 0,
            "historical_count": 0,
        }

    # Collect all use files
    all_uses = []
    live_count = 0
    historical_count = 0

    for use_file in talents_dir.glob("*/*.jsonl"):
        # Determine status from filename
        is_active = "_active.jsonl" in use_file.name
        is_pending = "_pending.jsonl" in use_file.name

        # Skip pending files
        if is_pending:
            continue

        status = "running" if is_active else "completed"

        # Count by type
        if status == "running":
            live_count += 1
        else:
            historical_count += 1

        # Filter by requested type
        if use_type == "live" and status != "running":
            continue
        if use_type == "historical" and status != "completed":
            continue

        # Extract use ID from filename
        use_id = use_file.stem.replace("_active", "")

        # Read use file to get request info and calculate runtime
        try:
            with open(use_file, "r") as f:
                lines = f.readlines()
                if not lines:
                    continue

                # Parse first line (request)
                first_line = lines[0].strip()
                if not first_line:
                    continue

                request = json.loads(first_line)
                if request.get("event") != "request":
                    continue

                # Extract facet from request
                use_facet = request.get("facet")

                # Filter by facet if specified
                if facet is not None and use_facet != facet:
                    continue

                # Extract basic info
                use_info = {
                    "id": use_id,
                    # Legacy unnamed run logs predate the chat rename; treat them as chat.
                    "name": request.get("name", "chat"),
                    "start": request.get("ts", 0),
                    "status": status,
                    "prompt": request.get("prompt", ""),
                    "provider": request.get("provider", "openai"),
                    "facet": use_facet,
                }

                # For completed uses, find finish event to calculate runtime
                if status == "completed" and len(lines) > 1:
                    # Read last few lines to find finish event (reading backwards is more efficient)
                    for line in reversed(lines[-10:]):  # Check last 10 lines
                        line = line.strip()
                        if not line:
                            continue
                        try:
                            event = json.loads(line)
                            if event.get("event") == "finish":
                                end_ts = event.get("ts", 0)
                                if end_ts and use_info["start"]:
                                    # Calculate runtime in seconds
                                    use_info["runtime_seconds"] = (
                                        end_ts - use_info["start"]
                                    ) / 1000.0
                                break
                        except json.JSONDecodeError:
                            continue

                all_uses.append(use_info)
        except (json.JSONDecodeError, IOError):
            # Skip malformed files
            continue

    # Sort by start time (newest first)
    all_uses.sort(key=lambda x: x["start"], reverse=True)

    # Apply pagination
    total = len(all_uses)
    paginated = all_uses[offset : offset + limit]

    return {
        "uses": paginated,
        "pagination": {
            "limit": limit,
            "offset": offset,
            "total": total,
            "has_more": (offset + limit) < total,
        },
        "live_count": live_count,
        "historical_count": historical_count,
    }
