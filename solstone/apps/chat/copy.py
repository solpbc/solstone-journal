# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Owner-facing copy for the chat surface (apps/chat + convey chat-bar)."""

# fmt: off
# T1.3 — owner-language talent labels (CMO subagent voice pass, 2026-05-26)
TALENT_LABEL_EXEC_RUNNING = "Looking in your journal…"
TALENT_LABEL_EXEC_FINISHED = "Looked in your journal"
TALENT_LABEL_EXEC_ERRORED = "Couldn't finish looking in your journal"
TALENT_LABEL_REFLECTION_RUNNING = "Reflecting…"
TALENT_LABEL_REFLECTION_FINISHED = "Reflected"
TALENT_LABEL_REFLECTION_ERRORED = "Couldn't finish reflecting"

# T1.4 — queue depth indicators (lowercase "sol" per system-anatomy canon)
CHAT_QUEUE_INDICATOR_SINGULAR = "1 message waiting"
CHAT_QUEUE_INDICATOR_PLURAL_FORMAT = "{count} messages waiting"
CHAT_QUEUE_DEPTH_CAP_MESSAGE = "Give sol a moment to catch up — you have 10 messages waiting."

# T1.1 — liveness placeholder bubble
CHAT_LIVENESS_THINKING = "Sol is thinking…"
CHAT_LIVENESS_TASK_FORMAT = "{label} {task}"

# T1.2 — chat error retry button
CHAT_ERROR_RETRY_LABEL = "Try again"
CHAT_ERROR_RETRY_ARIA_FORMAT = "Try again — re-send: {excerpt}"

# T2.2 — closer framing (CPO LOCKED)
CHAT_CLOSER_LOOP_EXHAUSTED_PREFIX = "Here's what I have so far:"
CHAT_CLOSER_DIFFERENT_ANGLE_SUFFIX = "Want me to try a different angle?"
CHAT_CLOSER_TALENT_ERRORED_FORMAT = "I couldn't finish that lookup — {reason}. Want to try a different angle, or rephrase the question?"
CHAT_CLOSER_TALENT_ERRORED_GENERIC = "I couldn't finish that lookup. Want to try a different angle, or rephrase the question?"
# fmt: on

from typing import Literal

_CHAT_ERROR_RETRY_EXCERPT_LIMIT = 60


def chat_error_retry_excerpt(text: str) -> str:
    """Truncate owner text for the retry button aria-label.

    Returns up to 60 source code points; appends U+2026 when truncated.
    """
    source = text or ""
    if len(source) <= _CHAT_ERROR_RETRY_EXCERPT_LIMIT:
        return source
    return source[:_CHAT_ERROR_RETRY_EXCERPT_LIMIT] + "…"


_TALENT_LABELS: dict[tuple[str, str], str] = {
    ("exec", "running"): TALENT_LABEL_EXEC_RUNNING,
    ("exec", "finished"): TALENT_LABEL_EXEC_FINISHED,
    ("exec", "errored"): TALENT_LABEL_EXEC_ERRORED,
    ("reflection", "running"): TALENT_LABEL_REFLECTION_RUNNING,
    ("reflection", "finished"): TALENT_LABEL_REFLECTION_FINISHED,
    ("reflection", "errored"): TALENT_LABEL_REFLECTION_ERRORED,
}


def talent_label_for(
    target: str, status: Literal["running", "finished", "errored"]
) -> str:
    """Return owner-facing label for (target, status). Raises ValueError on unknown."""
    try:
        return _TALENT_LABELS[(target, status)]
    except KeyError:
        raise ValueError(
            f"no chat talent label for target={target!r} status={status!r}"
        )
