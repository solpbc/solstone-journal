// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const TALENT_LABELS = {
    "exec": {
      "running": "Looking in your journal…",
      "finished": "Looked in your journal",
      "errored": "Couldn't finish looking in your journal"
    },
    "reflection": {
      "running": "Reflecting…",
      "finished": "Reflected",
      "errored": "Couldn't finish reflecting"
    }
  };

  function talentLabel(target, status) {
    const row = TALENT_LABELS[target];
    if (!row || !(status in row)) {
      throw new Error("no chat talent label for target=" + target + " status=" + status);
    }
    return row[status];
  }

  const CHAT_ERROR_RETRY_EXCERPT_LIMIT = 60;

  function chatErrorRetryExcerpt(text) {
    const source = String(text == null ? "" : text);
    if (source.length <= CHAT_ERROR_RETRY_EXCERPT_LIMIT) return source;
    return source.slice(0, CHAT_ERROR_RETRY_EXCERPT_LIMIT) + "…";
  }

  window.solChatCopy = {
    talentLabel,
    CHAT_QUEUE_INDICATOR_SINGULAR: "1 message waiting",
    CHAT_QUEUE_INDICATOR_PLURAL_FORMAT: "{count} messages waiting",
    CHAT_QUEUE_DEPTH_CAP_MESSAGE: "Give sol a moment to catch up — you have 10 messages waiting.",
    CHAT_LIVENESS_THINKING: "Sol is thinking…",
    CHAT_LIVENESS_TASK_FORMAT: "{label} {task}",
    CHAT_ERROR_RETRY_LABEL: "Try again",
    CHAT_ERROR_RETRY_ARIA_FORMAT: "Try again — re-send: {excerpt}",
    CHAT_CLOSER_LOOP_EXHAUSTED_PREFIX: "Here's what I have so far:",
    CHAT_CLOSER_DIFFERENT_ANGLE_SUFFIX: "Want me to try a different angle?",
    CHAT_CLOSER_TALENT_ERRORED_FORMAT: "I couldn't finish that lookup — {reason}. Want to try a different angle, or rephrase the question?",
    CHAT_CLOSER_TALENT_ERRORED_GENERIC: "I couldn't finish that lookup. Want to try a different angle, or rephrase the question?",
    chatErrorRetryExcerpt
  };
})();
