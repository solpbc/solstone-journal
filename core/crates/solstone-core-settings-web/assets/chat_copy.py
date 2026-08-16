# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Owner-facing copy for the chat surface (apps/chat + convey chat-bar)."""

# fmt: off
# Owner-language talent labels.
TALENT_LABEL_READ_RUNNING = "reading your journal…"
TALENT_LABEL_READ_FINISHED = "read your journal"
TALENT_LABEL_READ_ERRORED = "couldn't finish reading your journal"
TALENT_LABEL_EXEC_RUNNING = "making that change…"
TALENT_LABEL_EXEC_FINISHED = "made the change"
TALENT_LABEL_EXEC_ERRORED = "couldn't finish the change"
TALENT_LABEL_SUPPORT_RUNNING = "reaching solstone support…"
TALENT_LABEL_SUPPORT_FINISHED = "reached solstone support"
TALENT_LABEL_SUPPORT_ERRORED = "couldn't reach solstone support"

# T1.4 — active/queued job indicators
CHAT_JOBS_INDICATOR_SINGULAR = "sol is running 1 job"
CHAT_JOBS_INDICATOR_PLURAL_FORMAT = "sol is running {count} jobs"
CHAT_QUEUE_DEPTH_CAP_MESSAGE = "Give sol a moment to catch up — you have 10 messages waiting."
CHAT_TALENT_QUEUED_LABEL = "waiting to start…"
CHAT_DISPATCH_ORIGIN_PREFIX = "in reply to:"

# T1.1 — liveness placeholder bubble
CHAT_LIVENESS_THINKING = "sol is thinking…"
CHAT_LIVENESS_TASK_FORMAT = "{label} {task}"

# T2.2 — closer framing (CPO LOCKED)
CHAT_CLOSER_LOOP_EXHAUSTED_PREFIX = "Here's what I have so far:"
CHAT_CLOSER_DIFFERENT_ANGLE_SUFFIX = "Want me to try a different angle?"
CHAT_CLOSER_TALENT_ERRORED_FORMAT = "I couldn't finish that lookup — {reason}. Want to try a different angle, or rephrase the question?"
CHAT_CLOSER_TALENT_ERRORED_GENERIC = "I couldn't finish that lookup. Want to try a different angle, or rephrase the question?"
# Deterministic support-send-failure closer (backend-selected on an outbound
# talent_errored carrying a runtime-failure reason_code). Brand: "solstone support".
CHAT_CLOSER_SUPPORT_SEND_FAILED = "I couldn't finish reaching solstone support, so nothing was sent. Want me to try again?"

# T2.4 — thinking summary surfaces (CPO LOCKED)
CHAT_THINKING_EXPANDER_LABEL = "show thinking"
CHAT_THINKING_COLLAPSER_LABEL = "hide thinking"
CHAT_ERROR_DETAIL_EXPANDER_LABEL = "show details"
CHAT_ERROR_DETAIL_COLLAPSER_LABEL = "hide details"
CHAT_THINKING_SETTING_LABEL = "thinking surfaces"
CHAT_THINKING_OPT_ON_TAP = "show on tap"
CHAT_THINKING_OPT_ALWAYS = "always show"
CHAT_THINKING_OPT_NEVER = "never show"
CHAT_THINKING_SETTING_HELP = "sol does some thinking before replying. choose how much you want to see."

# Deterministic support-offer gate (backend-emitted; rides the sol_message text).
# Brand rule: "solstone support", never "sol pbc".
CHAT_OFFER_SUPPORT_PROMPT = "Sounds like something's not working — want me to bring in solstone support?"
CHAT_OFFER_SUPPORT_DECLINE = "Okay — I'll keep this local. Tell me if you'd like me to bring in solstone support after all."
# Deterministic support-draft-ready marker (backend-emitted; rides the sol_message
# text on a clean support talent_finished with a pending draft). Brand: "solstone
# support". Backend-only — no chat_copy.js twin; the C1 draft-review card renders
# this as its lead line via the sol_message text.
CHAT_SUPPORT_DRAFT_READY = "Here's the support request I put together — look it over before anything goes to solstone support."
# Deterministic support-draft submit/cancel results.
# Backend-only — no chat_copy.js twin.
CHAT_SUPPORT_SUBMIT_FILED_FORMAT = "I sent that to solstone support as ticket #{ticket_id}."
CHAT_SUPPORT_ATTACH_FILED_FORMAT = "I added that to solstone support ticket #{ticket_id}."
CHAT_SUPPORT_SUBMIT_FAILED = "I couldn't finish reaching solstone support, so nothing was sent. Want me to try again?"
CHAT_SUPPORT_SUBMIT_AMBIGUOUS = "I couldn't confirm whether solstone support received that. Check with solstone support before resending so we don't file it twice."
CHAT_SUPPORT_DRAFT_CANCELLED = "Okay — nothing was sent to solstone support."
CHAT_SUPPORT_CLOSE_SUBMITTED = "I closed that support ticket. Closing it removes it from solstone support's open list; only a minimal closed record is kept."
CHAT_SUPPORT_RESOLVED_SUBMITTED = "I accepted the proposed resolution and closed that support ticket. It is no longer in solstone support's open list; only a minimal closed record is kept."
CHAT_SUPPORT_STILL_NEED_HELP_SUBMITTED = "I let solstone support know you still need help. The proposed close was cancelled, and the ticket stays open."
CHAT_SUPPORT_IN_PROGRESS = "I'm still working on the last support request for this — try again in a moment."
CHAT_SUPPORT_RECONSENT_NEEDED = "solstone support's terms changed; want me to try again?"
# Deterministic deferred-mode honest-degradation message (backend-emitted; rides
# the sol_message/finish text when an otherwise-empty answer is really "captured
# but not yet analyzed"). Backend-only — no chat_copy.js twin. Substance locked
# (wording refinable).
CHAT_DEFERRED_NOT_ANALYZED = "Today's segments aren't analyzed yet — they'll process during your deferred window."
CHAT_THINKING_ENGINE_NOT_CHOSEN = "no thinking engine is chosen yet. choose one in thinking so i can answer from your journal."
# fmt: on

from typing import Literal

_TALENT_LABELS: dict[tuple[str, str], str] = {
    ("read", "running"): TALENT_LABEL_READ_RUNNING,
    ("read", "finished"): TALENT_LABEL_READ_FINISHED,
    ("read", "errored"): TALENT_LABEL_READ_ERRORED,
    ("exec", "running"): TALENT_LABEL_EXEC_RUNNING,
    ("exec", "finished"): TALENT_LABEL_EXEC_FINISHED,
    ("exec", "errored"): TALENT_LABEL_EXEC_ERRORED,
    ("support", "running"): TALENT_LABEL_SUPPORT_RUNNING,
    ("support", "finished"): TALENT_LABEL_SUPPORT_FINISHED,
    ("support", "errored"): TALENT_LABEL_SUPPORT_ERRORED,
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
