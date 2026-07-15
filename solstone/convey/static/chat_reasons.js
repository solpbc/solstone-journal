// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function() {
  const CHAT_REASON_DISPLAY_NAMES = Object.freeze({
    "google": "Gemini",
    "openai": "OpenAI",
    "anthropic": "Anthropic",
    "local": "Local"
  });

  const CHAT_REASONS = Object.freeze({
    "thinking_engine_not_chosen": {
      "template": "no thinking engine is chosen yet",
      "action": {"label": "Open Thinking", "href": "/app/thinking/#main"}
    },
    "provider_key_missing": {
      "template": "{provider} needs credentials before it can read your screen descriptions",
      "action": {"label": "Open Thinking", "href": "/app/thinking/#main"}
    },
    "ram_insufficient": {
      "template": "the local model needs more memory than this machine has",
      "action": {"label": "Open Local Model Setup", "href": "/app/thinking/#local-setup"}
    },
    "gpu_unavailable": {
      "template": "local models need GPU acceleration on this computer",
      "action": {"label": "Open Thinking", "href": "/app/thinking/#main"}
    },
    "gpu_probe_failed": {
      "template": "local GPU check couldn't finish",
      "action": {"label": "Open Thinking", "href": "/app/thinking/#main"}
    },
    "local_model_missing": {
      "template": "local model setup is not finished",
      "action": {"label": "Open Local Model Setup", "href": "/app/thinking/#local-setup"}
    },
    "model_missing": {
      "template": "local model setup is not finished",
      "action": {"label": "Open Local Model Setup", "href": "/app/thinking/#local-setup"}
    },
    "binary_missing": {
      "template": "local model setup is not finished",
      "action": {"label": "Open Local Model Setup", "href": "/app/thinking/#local-setup"}
    },
    "local_model_installing": {
      "template": "local model setup is finishing",
      "action": null
    },
    "local_model_loading": {
      "template": "the local model is starting up",
      "action": null
    },
    "local_model_not_ready": {
      "template": "the local model is starting up",
      "action": null
    },
    "local_server_unhealthy": {
      "template": "the local model isn't responding",
      "action": {"label": "Open Local Model Setup", "href": "/app/thinking/#local-setup"}
    },
    "local_endpoint_unreachable": {
      "template": "The inference endpoint you configured could not be reached.",
      "action": {"label": "Open Thinking", "href": "/app/thinking/#main"}
    },
    "local_endpoint_contract_failed": {
      "template": "The configured endpoint did not respond in the expected format.",
      "action": {"label": "Open Thinking", "href": "/app/thinking/#main"}
    },
    "unsupported_platform": {
      "template": "this machine is not supported for local model setup",
      "action": {"label": "Open Local Model Setup", "href": "/app/thinking/#local-setup"}
    },
    "host_unfit": {
      "template": "this computer doesn't meet the local model's requirements",
      "action": {"label": "Open Local Model Setup", "href": "/app/thinking/#local-setup"}
    },
    "unsupported_model": {
      "template": "this local model is not supported",
      "action": {"label": "Open Local Model Setup", "href": "/app/thinking/#local-setup"}
    },
    "sha256_mismatch": {
      "template": "local model setup could not be verified",
      "action": {"label": "Open Local Model Setup", "href": "/app/thinking/#local-setup"}
    },
    "archive_path_traversal": {
      "template": "local model setup could not be verified",
      "action": {"label": "Open Local Model Setup", "href": "/app/thinking/#local-setup"}
    },
    "provider_key_invalid": {
      "template": "your {provider} key didn't validate",
      "action": {"label": "Open Thinking", "href": "/app/thinking/#main"}
    },
    "provider_quota_exceeded": {
      "template": "your {provider} quota is spent",
      "action": null
    },
    "network_unreachable": {
      "template": "I couldn't reach the network",
      "action": null
    },
    "provider_response_invalid": {
      "template": "{provider}'s response didn't match the expected shape — try rephrasing or asking something more specific.",
      "action": null
    },
    "provider_unavailable": {
      "template": "{provider} is having trouble right now",
      "action": null
    },
    "chat_pipeline_unavailable": {
      "template": "the chat pipeline isn't ready yet",
      "action": null
    },
    "chat_timeout": {
      "template": "chat took too long",
      "action": null
    },
    "local_queue_timeout": {
      "template": "the local model was busy and couldn't start in time",
      "action": null
    },
    "local_capacity_exhausted": {
      "template": "the local model was busy and could not finish this request",
      "action": null
    },
    "context_window_exceeded": {
      "template": "the conversation grew too long to finish",
      "action": null
    },
    "context_budget_exceeded": {
      "template": "the request was too long for the local model",
      "action": null
    },
    "incomplete_json_length": {
      "template": "the answer ran out of room before it finished",
      "action": null
    },
    "incomplete_text_length": {
      "template": "the answer ran out of room before it finished",
      "action": null
    },
    "max_turns_exhausted": {
      "template": "this took too many steps to finish",
      "action": null
    },
    "no_output": {
      "template": "I didn't get a response",
      "action": null
    },
    "token_budget_exceeded": {
      "template": "this run reached its resource budget before finishing",
      "action": null
    },
    "wall_clock_exceeded": {
      "template": "this run took too long to finish",
      "action": null
    },
    "unknown": {
      "template": "chat had trouble",
      "action": null
    }
  });

  window.CHAT_REASON_DISPLAY_NAMES = CHAT_REASON_DISPLAY_NAMES;
  window.CHAT_REASONS = CHAT_REASONS;
  window.renderChatReason = function(code, provider) {
    const reason = CHAT_REASONS[code];
    if (!reason) {
      return {code: code, message: code, action: null};
    }
    const providerSlug = String(provider || "");
    if (code === "unknown") {
      const displayName = CHAT_REASON_DISPLAY_NAMES[providerSlug];
      const message = displayName
        ? `something went wrong with ${displayName}`
        : reason.template;
      return {code: code, message: message, action: null};
    }
    const displayName = CHAT_REASON_DISPLAY_NAMES[providerSlug] || providerSlug;
    const message = reason.template.replace(/\{provider\}/g, displayName);
    const action = reason.action
      ? {label: reason.action.label, href: reason.action.href}
      : null;
    return {code: code, message: message, action: action};
  };
})();
