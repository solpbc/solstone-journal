// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const TALENT_LABELS = {
    "read": {
      "running": "reading your journal…",
      "finished": "read your journal",
      "errored": "couldn't finish reading your journal"
    },
    "exec": {
      "running": "making that change…",
      "finished": "made the change",
      "errored": "couldn't finish the change"
    },
    "support": {
      "running": "reaching solstone support…",
      "finished": "reached solstone support",
      "errored": "couldn't reach solstone support"
    }
  };

  function talentLabel(target, status) {
    const row = TALENT_LABELS[target];
    if (!row || !(status in row)) {
      throw new Error("no chat talent label for target=" + target + " status=" + status);
    }
    return row[status];
  }

  window.solChatCopy = {
    talentLabel,
    CHAT_JOBS_INDICATOR_SINGULAR: "sol is running 1 job",
    CHAT_JOBS_INDICATOR_PLURAL_FORMAT: "sol is running {count} jobs",
    CHAT_QUEUE_DEPTH_CAP_MESSAGE: "Give sol a moment to catch up — you have 10 messages waiting.",
    CHAT_TALENT_QUEUED_LABEL: "waiting to start…",
    CHAT_DISPATCH_ORIGIN_PREFIX: "in reply to:",
    CHAT_LIVENESS_THINKING: "sol is thinking…",
    CHAT_LIVENESS_TASK_FORMAT: "{label} {task}",
    CHAT_LIVENESS_SUPPORT: "reaching solstone support on your behalf…",
    CHAT_CAPACITY_SUPPORT_ROUTE_FROM: "sol",
    CHAT_CAPACITY_SUPPORT_ROUTE_TO: "solstone support",
    CHAT_CAPACITY_SUPPORT_SUB: "reaching out on your behalf · nothing leaves without your ok",
    CHAT_OFFER_SUPPORT_YES: "yes, get support",
    CHAT_OFFER_SUPPORT_NO: "not now",
    CHAT_OFFER_SUPPORT_FALLBACK: "want me to bring in solstone support?",
    CHAT_DRAFT_SUBMIT: "send to solstone support",
    CHAT_DRAFT_CANCEL: "cancel",
    CHAT_DRAFT_HEADER: "review before this goes to solstone support",
    CHAT_DRAFT_KIND_CREATE: "new support request",
    CHAT_DRAFT_KIND_FEEDBACK: "send feedback",
    CHAT_DRAFT_KIND_REPLY: "reply",
    CHAT_DRAFT_KIND_ATTACH: "attach a file",
    CHAT_DRAFT_KIND_CLOSE: "close this ticket",
    CHAT_DRAFT_KIND_RESOLVED: "confirm this is resolved",
    CHAT_DRAFT_KIND_STILL_NEED_HELP: "still need help",
    CHAT_DRAFT_TICKET_FORMAT: "ticket #{ticket_id}",
    CHAT_DRAFT_DIAGNOSTICS_TITLE: "what's included with this request",
    CHAT_DRAFT_DIAGNOSTICS_NOTE: "these exact values go to solstone support with your request. nothing else leaves this machine.",
    CHAT_DRAFT_ATTACH_NOTE: "the contents of this file go to solstone support. nothing else leaves this machine.",
    CHAT_DRAFT_CLOSE_NOTE: "confirming closes this ticket. it leaves solstone support's open list, and only a minimal closed record is kept. this can't be undone from here.",
    CHAT_DRAFT_RESOLVED_NOTE: "confirming accepts the proposed resolution and closes this ticket. it leaves solstone support's open list, and only a minimal closed record is kept. this can't be undone from here.",
    CHAT_DRAFT_STILL_NEED_HELP_NOTE: "confirming tells solstone support the proposed resolution didn't work, cancels the pending close, and keeps this ticket open.",
    CHAT_DRAFT_FLOOR: "nothing is sent until you choose",
    CHAT_DRAFT_NAME_ATTACHED_YES: "name attached: yes",
    CHAT_DRAFT_NAME_ATTACHED_NO: "name attached: no",
    CHAT_RESULT_VIEW_IN_SUPPORT: "view in support →",
    CHAT_RESULT_DRAFT_SUBMITTED: "sent to solstone support",
    CHAT_RESULT_DRAFT_CANCELLED: "draft cancelled",
    CHAT_RESULT_DRAFT_NOT_FOUND: "I couldn't find that support draft.",
    CHAT_CLOSER_LOOP_EXHAUSTED_PREFIX: "Here's what I have so far:",
    CHAT_CLOSER_DIFFERENT_ANGLE_SUFFIX: "Want me to try a different angle?",
    CHAT_CLOSER_TALENT_ERRORED_FORMAT: "I couldn't finish that lookup — {reason}. Want to try a different angle, or rephrase the question?",
    CHAT_CLOSER_TALENT_ERRORED_GENERIC: "I couldn't finish that lookup. Want to try a different angle, or rephrase the question?",
    CHAT_CLOSER_SUPPORT_SEND_FAILED: "I couldn't finish reaching solstone support, so nothing was sent. Want me to try again?",
    CHAT_THINKING_EXPANDER_LABEL: "show thinking",
    CHAT_THINKING_COLLAPSER_LABEL: "hide thinking",
    CHAT_ERROR_DETAIL_EXPANDER_LABEL: "show details",
    CHAT_ERROR_DETAIL_COLLAPSER_LABEL: "hide details",
    CHAT_THINKING_SETTING_LABEL: "thinking surfaces",
    CHAT_THINKING_OPT_ON_TAP: "show on tap",
    CHAT_THINKING_OPT_ALWAYS: "always show",
    CHAT_THINKING_OPT_NEVER: "never show",
    CHAT_THINKING_SETTING_HELP: "sol does some thinking before replying. choose how much you want to see.",
  };
})();
