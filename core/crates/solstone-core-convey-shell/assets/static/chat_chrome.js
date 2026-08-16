// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function() {
  function deriveAppName() {
    const parts = window.location.pathname.split('/');
    return parts[1] === 'app' && parts[2] ? decodeURIComponent(parts[2]) : '';
  }

  const APP_NAME = deriveAppName();
  const appBar = document.getElementById('appBar');
  const statusWrap = document.getElementById('chatBarStatus');
  const solPingDot = document.getElementById('chatBarSolPingDot');
  const statusText = document.getElementById('chatBarStatusText');
  const queueStatus = document.getElementById('chatBarQueue');
  const capacityEl = document.getElementById('chatBarCapacity');
  const offerEl = document.getElementById('chatBarOffer');
  const offerTextEl = offerEl ? offerEl.querySelector('.offer-text') : null;
  const offerYesBtn = document.getElementById('chatBarOfferYes');
  const offerNoBtn = document.getElementById('chatBarOfferNo');
  const draftEl = document.getElementById('chatBarDraft');
  const resultEl = document.getElementById('chatBarResult');
  const draftCardEl = draftEl ? draftEl.querySelector('.chat-bar-draft-card') : null;
  const draftTitleEl = draftEl ? draftEl.querySelector('.chat-bar-draft-title') : null;
  const draftRouteFromEl = draftEl ? draftEl.querySelector('.chat-bar-draft-route-from') : null;
  const draftRouteToEl = draftEl ? draftEl.querySelector('.chat-bar-draft-route-to') : null;
  const draftBodyEl = draftEl ? draftEl.querySelector('.chat-bar-draft-body') : null;
  const draftFloorEl = draftEl ? draftEl.querySelector('.chat-bar-draft-floor') : null;
  const draftSubmitBtn = document.getElementById('chatBarDraftSubmit');
  const draftCancelBtn = document.getElementById('chatBarDraftCancel');
  const solPingDismiss = document.getElementById('chatBarSolPingDismiss');
  const talentsTray = document.getElementById('chatBarTalents');
  const form = document.getElementById('chatBarForm');
  const input = document.getElementById('chatBarInput');
  const sendBtn = document.getElementById('chatBarSend');
  const modal = document.getElementById('talentViewModal');
  const modalPanel = modal ? modal.querySelector('.talent-view-panel') : null;
  const modalTitle = document.getElementById('talentViewTitle');
  const modalStatus = document.getElementById('talentViewStatus');
  const modalTimeline = document.getElementById('talentViewTimeline');
  const talentState = new Map();
  const queuedJobs = new Map();
  const LEGACY_KEYS = ['solstone:' + 'conversationState', 'solstone:' + 'chatBarState'];
  const NEEDS_YOU_PENDING_KEY = 'solstone:needs-you-pending-prompt';
  const SOL_INITIATED = window.SOL_INITIATED || {};
  const SOL_PING_OFFLINE_DELAY_MS = 30000;
  let trayPage = 0;
  let pendingSend = false;
  let currentDraft = null;
  let modalUseId = null;
  let modalPollTimer = null;
  let modalChatCleanup = null;
  let modalTrigger = null;
  let solRequestState = null;
  const chatBarPendingPlaceholders = [];
  let solPingPulseTimer = null;
  let solPingDisconnectedAt = null;
  let solPingOfflineTimer = null;
  let solPingConnectionUnsub = null;
  let statusErrorActive = false;
  let supportCapacityActive = false;

  function escapeHtml(value) {
    return String(value || '')
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function formatEventTime(ts) {
    if (typeof ts !== 'number') return '';
    try {
      return new Date(ts).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
    } catch (_err) {
      return '';
    }
  }

  function describeValue(value) {
    if (value == null) return '';
    if (typeof value === 'string') return value;
    try {
      return JSON.stringify(value, null, 2);
    } catch (_err) {
      return String(value);
    }
  }

  function clipTitle(text) {
    var value = String(text || '').trim();
    if (!value) return '';
    return value.length > 120 ? value.slice(0, 117) + '...' : value;
  }

  function resizeComposer() {
    if (!input) return;
    input.style.height = 'auto';
    input.style.height = Math.min(120, input.scrollHeight) + 'px';
  }

  function setStatus(text, title, action) {
    if (!statusText || !statusWrap) return;
    var label = String(text || '').trim();
    var tooltip = String(title || label).trim();
    statusText.textContent = label;
    statusText.title = tooltip;
    statusWrap.title = tooltip;
    var actionEl = statusWrap.querySelector('.chat-bar-status-action');
    if (action && action.label && action.href) {
      if (!actionEl) {
        actionEl = document.createElement('a');
        actionEl.className = 'chat-bar-status-action';
        actionEl.addEventListener('click', function(event) {
          event.stopPropagation();
        });
        statusText.insertAdjacentElement('afterend', actionEl);
      }
      actionEl.textContent = action.label;
      actionEl.href = action.href;
    } else if (actionEl) {
      actionEl.remove();
    }
  }

  function statusTitleFor(msg) {
    return msg.origin && msg.origin.ask
      ? window.solChatCopy.CHAT_DISPATCH_ORIGIN_PREFIX + ' ' + msg.origin.ask
      : msg.notes || msg.text || '';
  }

  function renderJobsIndicator() {
    if (!queueStatus) return;
    var runningCount = 0;
    for (const entry of talentState.values()) {
      if (entry && entry.status === 'running') runningCount += 1;
    }
    var count = runningCount + queuedJobs.size;
    if (count < 1) {
      queueStatus.textContent = '';
      queueStatus.hidden = true;
      return;
    }
    queueStatus.textContent = count === 1
      ? window.solChatCopy.CHAT_JOBS_INDICATOR_SINGULAR
      : window.solChatCopy.CHAT_JOBS_INDICATOR_PLURAL_FORMAT.replace('{count}', String(count));
    queueStatus.hidden = false;
  }

  function showQueueCapMessage() {
    if (!queueStatus) return;
    queueStatus.textContent = window.solChatCopy.CHAT_QUEUE_DEPTH_CAP_MESSAGE;
    queueStatus.hidden = false;
  }

  function setPendingState(active) {
    pendingSend = !!active;
  }

  function disableComposer() {
    pendingSend = true;
    if (input) input.disabled = true;
    if (sendBtn) sendBtn.disabled = true;
  }

  function clearPendingLivenessStatus() {
    if (chatBarPendingPlaceholders.length > 0) chatBarPendingPlaceholders.shift();
    if (statusWrap) {
      statusWrap.classList.remove('chat-bar-status--thinking');
      statusWrap.classList.remove('chat-bar-status--error');
      statusWrap.removeAttribute('role');
      statusWrap.removeAttribute('tabindex');
    }
    statusErrorActive = false;
  }

  function isSupportTalentRunning() {
    for (const entry of talentState.values()) {
      if (entry && entry.name === 'support' && entry.status === 'running') return true;
    }
    return false;
  }

  function populateSupportCapacityCopy() {
    if (!capacityEl) return;
    var from = capacityEl.querySelector('.cap-from');
    var to = capacityEl.querySelector('.cap-to');
    var sub = capacityEl.querySelector('.chat-bar-capacity-sub');
    if (from) from.textContent = window.solChatCopy.CHAT_CAPACITY_SUPPORT_ROUTE_FROM;
    if (to) to.textContent = window.solChatCopy.CHAT_CAPACITY_SUPPORT_ROUTE_TO;
    if (sub) sub.textContent = window.solChatCopy.CHAT_CAPACITY_SUPPORT_SUB;
  }

  function enterSupportCapacity() {
    supportCapacityActive = true;
    if (appBar) appBar.classList.add('app-bar--support');
    if (capacityEl) capacityEl.hidden = false;
    var live = window.solChatCopy.CHAT_LIVENESS_SUPPORT;
    setStatus(live, live);
    if (statusWrap) {
      statusWrap.classList.add('chat-bar-status--thinking');
      statusWrap.classList.remove('chat-bar-status--error');
      statusWrap.removeAttribute('role');
      statusWrap.removeAttribute('tabindex');
    }
    statusErrorActive = false;
  }

  function exitSupportCapacity() {
    supportCapacityActive = false;
    if (appBar) appBar.classList.remove('app-bar--support');
    if (capacityEl) capacityEl.hidden = true;
  }

  function populateSupportOfferCopy() {
    if (offerYesBtn) offerYesBtn.textContent = window.solChatCopy.CHAT_OFFER_SUPPORT_YES;
    if (offerNoBtn) offerNoBtn.textContent = window.solChatCopy.CHAT_OFFER_SUPPORT_NO;
  }

  function populateSupportDraftCopy() {
    if (draftSubmitBtn) draftSubmitBtn.textContent = window.solChatCopy.CHAT_DRAFT_SUBMIT;
    if (draftCancelBtn) draftCancelBtn.textContent = window.solChatCopy.CHAT_DRAFT_CANCEL;
    if (draftCardEl) draftCardEl.setAttribute('aria-label', 'support draft for review');
    if (draftTitleEl) draftTitleEl.textContent = window.solChatCopy.CHAT_DRAFT_HEADER;
    if (draftRouteFromEl) draftRouteFromEl.textContent = window.solChatCopy.CHAT_CAPACITY_SUPPORT_ROUTE_FROM;
    if (draftRouteToEl) draftRouteToEl.textContent = window.solChatCopy.CHAT_CAPACITY_SUPPORT_ROUTE_TO;
    if (draftFloorEl) draftFloorEl.textContent = window.solChatCopy.CHAT_DRAFT_FLOOR;
  }

  function showSupportOffer(text) {
    if (!offerEl) return;
    hideSupportResult();
    if (offerTextEl) offerTextEl.textContent = text || window.solChatCopy.CHAT_OFFER_SUPPORT_FALLBACK;
    offerEl.hidden = false;
    setStatus('', '');
  }

  function hideSupportOffer() {
    if (!offerEl) return;
    offerEl.hidden = true;
  }

  const FIELD_LABELS = {
    subject: "subject",
    description: "what's happening",
    body: "your feedback",
    content: "your message",
    filename: "file",
    content_type: "type",
    byte_size: "size",
    severity: "severity",
    category: "area",
    product: "product"
  };

  const DIAG_KEY_LABELS = {
    version: "journal version",
    revision: "build",
    platform: "system",
    services: "services",
    recent_errors: "recent errors",
    config: "configuration",
    provider_readiness: "ai providers"
  };

  const SUPPORT_OUTCOMES = {
    "support draft submitted": { state: 'submitted', icon: '✓' },
    "support draft failed": { state: 'failed', icon: '!' },
    "support draft ambiguous": { state: 'ambiguous', icon: '?' },
    "support draft in_progress": { state: 'in_progress', icon: '…' },
    "support draft re_consent_required": { state: 're_consent_required', icon: '!' },
    "support draft cancelled": { state: 'cancelled', icon: '–' }
  };

  function formatAttachmentSize(size) {
    const bytes = Number(size) || 0;
    if (bytes >= 1024 * 1024) return (bytes / 1024 / 1024).toFixed(1) + ' MB';
    if (bytes >= 1024) return String(Math.round(bytes / 1024)) + ' KB';
    return String(bytes) + ' bytes';
  }

  function appendDraftFieldRow(parent, label, value) {
    const row = document.createElement('div');
    row.className = 'chat-bar-draft-field';
    const lab = document.createElement('span');
    lab.className = 'chat-bar-draft-label';
    lab.textContent = label;
    const val = document.createElement('span');
    val.className = 'chat-bar-draft-fieldval';
    val.textContent = String(value ?? '—');
    row.appendChild(lab);
    row.appendChild(val);
    parent.appendChild(row);
  }

  function appendDraftKind(parent, kindLabel, ticketId) {
    const row = document.createElement('div');
    row.className = 'chat-bar-draft-kindrow';
    const kind = document.createElement('span');
    kind.className = 'chat-bar-draft-kind';
    kind.textContent = kindLabel;
    row.appendChild(kind);
    if (ticketId !== undefined && ticketId !== null && String(ticketId).trim()) {
      const ticket = document.createElement('span');
      ticket.className = 'chat-bar-draft-ticket';
      ticket.textContent = window.solChatCopy.CHAT_DRAFT_TICKET_FORMAT.replace('{ticket_id}', String(ticketId));
      row.appendChild(ticket);
    }
    parent.appendChild(row);
  }

  function appendDraftFieldIfPresent(parent, payload, key, value) {
    if (!(key in payload)) return;
    appendDraftFieldRow(parent, FIELD_LABELS[key], value === undefined ? payload[key] : value);
  }

  function appendDraftMetaPill(parent, label, value) {
    const pill = document.createElement('span');
    pill.className = 'chat-bar-draft-pill';
    pill.textContent = label + ': ' + String(value ?? '—');
    parent.appendChild(pill);
  }

  function appendDraftMetaRow(parent, payload, keys) {
    const row = document.createElement('div');
    row.className = 'chat-bar-draft-meta';
    keys.forEach(function(key) {
      if (!(key in payload)) return;
      appendDraftMetaPill(row, FIELD_LABELS[key], payload[key]);
    });
    parent.appendChild(row);
    return row;
  }

  function appendNameAttachedPill(parent, payload) {
    const pill = document.createElement('span');
    pill.className = 'chat-bar-draft-pill';
    pill.textContent = payload.anonymous === true
      ? window.solChatCopy.CHAT_DRAFT_NAME_ATTACHED_NO
      : window.solChatCopy.CHAT_DRAFT_NAME_ATTACHED_YES;
    parent.appendChild(pill);
  }

  function renderCreateDraftBody(parent, payload, diagnostics) {
    appendDraftKind(parent, window.solChatCopy.CHAT_DRAFT_KIND_CREATE);
    appendDraftFieldIfPresent(parent, payload, 'subject');
    appendDraftFieldIfPresent(parent, payload, 'description');
    const meta = appendDraftMetaRow(parent, payload, ['severity', 'category', 'product']);
    appendNameAttachedPill(meta, payload);
    renderDiagnosticsBlock(parent, diagnostics);
  }

  function renderFeedbackDraftBody(parent, payload, diagnostics) {
    appendDraftKind(parent, window.solChatCopy.CHAT_DRAFT_KIND_FEEDBACK);
    appendDraftFieldIfPresent(parent, payload, 'body');
    const meta = appendDraftMetaRow(parent, payload, ['product']);
    appendNameAttachedPill(meta, payload);
    renderDiagnosticsBlock(parent, diagnostics);
  }

  function renderReplyDraftBody(parent, payload) {
    appendDraftKind(parent, window.solChatCopy.CHAT_DRAFT_KIND_REPLY, payload.ticket_id);
    appendDraftFieldIfPresent(parent, payload, 'content');
  }

  function renderAttachDraftBody(parent, payload) {
    appendDraftKind(parent, window.solChatCopy.CHAT_DRAFT_KIND_ATTACH, payload.ticket_id);
    appendDraftFieldIfPresent(parent, payload, 'filename');
    const meta = appendDraftMetaRow(parent, payload, ['content_type']);
    if ('byte_size' in payload) appendDraftMetaPill(meta, FIELD_LABELS.byte_size, formatAttachmentSize(payload.byte_size));
    const note = document.createElement('div');
    note.className = 'chat-bar-draft-attach-note';
    note.textContent = window.solChatCopy.CHAT_DRAFT_ATTACH_NOTE;
    parent.appendChild(note);
  }

  function renderLifecycleDraftBody(parent, payload, kind, noteText) {
    appendDraftKind(parent, kind, payload.ticket_id);
    const note = document.createElement('div');
    note.className = 'chat-bar-draft-attach-note';
    note.textContent = noteText;
    parent.appendChild(note);
  }

  function renderDraftBody(parent, draft) {
    const payload = draft.payload || {};
    if (draft.verb === 'create') {
      renderCreateDraftBody(parent, payload, draft.diagnostics_snapshot);
    } else if (draft.verb === 'feedback') {
      renderFeedbackDraftBody(parent, payload, draft.diagnostics_snapshot);
    } else if (draft.verb === 'reply') {
      renderReplyDraftBody(parent, payload);
    } else if (draft.verb === 'attach') {
      renderAttachDraftBody(parent, payload);
    } else if (draft.verb === 'close') {
      renderLifecycleDraftBody(parent, payload, window.solChatCopy.CHAT_DRAFT_KIND_CLOSE, window.solChatCopy.CHAT_DRAFT_CLOSE_NOTE);
    } else if (draft.verb === 'resolved') {
      renderLifecycleDraftBody(parent, payload, window.solChatCopy.CHAT_DRAFT_KIND_RESOLVED, window.solChatCopy.CHAT_DRAFT_RESOLVED_NOTE);
    } else if (draft.verb === 'still_need_help') {
      renderLifecycleDraftBody(parent, payload, window.solChatCopy.CHAT_DRAFT_KIND_STILL_NEED_HELP, window.solChatCopy.CHAT_DRAFT_STILL_NEED_HELP_NOTE);
    }
  }

  function renderDiagnosticsBlock(parent, snapshot) {
    const section = document.createElement('div');
    section.className = 'chat-bar-draft-diagnostics';
    const title = document.createElement('div');
    title.className = 'chat-bar-draft-diagnostics-title';
    title.textContent = window.solChatCopy.CHAT_DRAFT_DIAGNOSTICS_TITLE;
    const note = document.createElement('div');
    note.className = 'chat-bar-draft-diagnostics-note';
    note.textContent = window.solChatCopy.CHAT_DRAFT_DIAGNOSTICS_NOTE;
    const rows = document.createElement('div');
    rows.className = 'chat-bar-draft-diagnostics-rows';
    if (snapshot && typeof snapshot === 'object' && !Array.isArray(snapshot)) {
      Object.keys(snapshot).forEach(function(key) {
        appendDiagnosticsRow(rows, key, snapshot[key]);
      });
    }
    section.appendChild(title);
    section.appendChild(note);
    section.appendChild(rows);
    parent.appendChild(section);
  }

  function appendDiagnosticsRow(parent, key, value) {
    const label = DIAG_KEY_LABELS[key] || key;
    const row = document.createElement('div');
    row.className = Array.isArray(value)
      ? 'chat-bar-draft-diagnostic-row chat-bar-draft-diagnostic-row--array'
      : 'chat-bar-draft-diagnostic-row';
    if (!Array.isArray(value)) {
      const labelEl = document.createElement('span');
      labelEl.className = 'chat-bar-draft-label';
      labelEl.textContent = label;
      row.appendChild(labelEl);
    }
    const valueEl = document.createElement('div');
    valueEl.className = 'chat-bar-draft-fieldval';
    valueEl.appendChild(renderDiagnosticsValue(value, label));
    row.appendChild(valueEl);
    parent.appendChild(row);
  }

  function appendDiagnosticSubRow(parent, key, value) {
    const row = document.createElement('div');
    row.className = 'chat-bar-draft-diagnostic-subrow';
    const label = document.createElement('span');
    label.className = 'chat-bar-draft-label';
    label.textContent = key;
    const valueEl = document.createElement('div');
    valueEl.className = 'chat-bar-draft-fieldval';
    valueEl.appendChild(renderDiagnosticsValue(value, key));
    row.appendChild(label);
    row.appendChild(valueEl);
    parent.appendChild(row);
  }

  function renderDiagnosticsValue(value, labelText) {
    if (Array.isArray(value)) {
      return renderDiagnosticsArray(value, labelText);
    }
    if (value === null || typeof value !== 'object') {
      const span = document.createElement('span');
      span.className = 'chat-bar-draft-value';
      span.textContent = value === null ? '—' : String(value);
      return span;
    }
    const keys = Object.keys(value);
    if (keys.length === 0) {
      const span = document.createElement('span');
      span.className = 'chat-bar-draft-value';
      span.textContent = '(none)';
      return span;
    }
    const group = document.createElement('div');
    group.className = 'chat-bar-draft-diagnostic-subrows';
    keys.forEach(function(key) {
      appendDiagnosticSubRow(group, key, value[key]);
    });
    return group;
  }

  function renderDiagnosticsArray(items, labelText) {
    const wrapper = document.createElement('div');
    wrapper.className = 'chat-bar-draft-diagnostic-array';
    const expanded = items.length <= 5;
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'chat-bar-draft-diagnostic-toggle';
    button.setAttribute('aria-expanded', expanded ? 'true' : 'false');
    button.textContent = String(labelText || '') + ' (' + String(items.length) + ')';
    const content = document.createElement('div');
    content.className = 'chat-bar-draft-diagnostic-array-items';
    content.hidden = !expanded;
    if (items.length === 0) {
      const none = document.createElement('span');
      none.className = 'chat-bar-draft-value';
      none.textContent = '(none)';
      content.appendChild(none);
    } else {
      items.forEach(function(item) {
        const row = document.createElement('div');
        row.className = 'chat-bar-draft-diagnostic-array-item';
        row.appendChild(renderDiagnosticsValue(item, labelText));
        content.appendChild(row);
      });
    }
    button.addEventListener('click', function() {
      const nextExpanded = button.getAttribute('aria-expanded') !== 'true';
      button.setAttribute('aria-expanded', nextExpanded ? 'true' : 'false');
      content.hidden = !nextExpanded;
    });
    wrapper.appendChild(button);
    wrapper.appendChild(content);
    return wrapper;
  }

  function renderSupportOutcome(msg) {
    if (!msg || !resultEl) return false;
    const outcome = SUPPORT_OUTCOMES[msg.notes || ''];
    if (!outcome) return false;
    resultEl.replaceChildren();
    const strip = document.createElement('div');
    strip.className = 'chat-bar-result-strip chat-bar-result-strip--' + outcome.state;
    const icon = document.createElement('span');
    icon.className = 'chat-bar-result-icon';
    icon.setAttribute('aria-hidden', 'true');
    icon.textContent = outcome.icon;
    const text = document.createElement('span');
    text.className = 'chat-bar-result-text';
    text.textContent = msg.text || '';
    strip.appendChild(icon);
    strip.appendChild(text);
    if (outcome.state === 'submitted') {
      const link = document.createElement('a');
      link.className = 'chat-bar-result-action';
      link.href = '/app/support/';
      link.textContent = window.solChatCopy.CHAT_RESULT_VIEW_IN_SUPPORT;
      strip.appendChild(link);
    }
    resultEl.appendChild(strip);
    resultEl.hidden = false;
    if (outcome.state === 'submitted' || outcome.state === 'cancelled') {
      hideSupportDraft();
    } else {
      reenableSupportDraft();
    }
    setStatus('', '');
    return true;
  }

  function hideSupportResult() {
    if (!resultEl) return;
    resultEl.replaceChildren();
    resultEl.hidden = true;
  }

  function showSupportDraft(draft) {
    if (!draftEl || !draft) return;
    currentDraft = draft;
    reenableSupportDraft();
    hideSupportResult();
    if (draftBodyEl) {
      draftBodyEl.replaceChildren();
      renderDraftBody(draftBodyEl, draft);
    }
    draftEl.hidden = false;
    setStatus('', '');
  }

  function hideSupportDraft() {
    if (!draftEl) return;
    draftEl.hidden = true;
    currentDraft = null;
    reenableSupportDraft();
  }

  function reenableSupportDraft() {
    if (draftSubmitBtn) draftSubmitBtn.disabled = false;
    if (draftCancelBtn) draftCancelBtn.disabled = false;
  }

  function getTalentEntries() {
    return Array.from(talentState.values()).sort(function(a, b) {
      var aActive = a.status === 'running' ? 1 : 0;
      var bActive = b.status === 'running' ? 1 : 0;
      if (aActive !== bActive) return bActive - aActive;
      return (b.updatedAt || 0) - (a.updatedAt || 0);
    });
  }

  function upsertTalent(entry) {
    if (!entry || !entry.useId) return;
    var current = talentState.get(entry.useId) || {};
    talentState.set(entry.useId, {
      useId: entry.useId,
      name: entry.name || current.name || '',
      task: entry.task || current.task || '',
      status: entry.status || current.status || 'running',
      updatedAt: entry.updatedAt || Date.now()
    });
    renderTalentTray();
  }

  function removeTalent(useId) {
    talentState.delete(useId);
    renderTalentTray();
  }

  function renderTalentChip(entry) {
    var button = document.createElement('button');
    var talentLabel = window.solChatCopy.talentLabel(entry.name, entry.status);
    var taskLabel = clipTitle(entry.task || '');
    var label = taskLabel ? talentLabel + ': ' + taskLabel : talentLabel;
    button.type = 'button';
    button.className = 'chat-bar-talent';
    button.dataset.useId = entry.useId;
    button.dataset.status = entry.status;
    button.title = label;
    button.setAttribute('aria-label', 'talent: ' + (label || entry.useId) + '; status: ' + entry.status);
    button.innerHTML = '<span class="chat-bar-talent-dot" aria-hidden="true"></span>';
    button.addEventListener('click', function() {
      window.openTalentView(entry.useId, { live: entry.status === 'running' });
    });
    return button;
  }

  function renderTalentTray() {
    if (!talentsTray) return;
    talentsTray.innerHTML = '';
    var entries = getTalentEntries();
    if (!entries.length) {
      talentsTray.hidden = true;
      trayPage = 0;
      return;
    }

    talentsTray.hidden = false;
    var pageSize = entries.length > 8 ? 7 : 8;
    var pageCount = entries.length > 8 ? Math.ceil(entries.length / pageSize) : 1;
    if (trayPage >= pageCount) trayPage = 0;

    var start = trayPage * pageSize;
    var visible = entries.slice(start, start + pageSize);
    visible.forEach(function(entry) {
      talentsTray.appendChild(renderTalentChip(entry));
    });

    if (entries.length > 8) {
      var hiddenCount = entries.length - visible.length;
      var overflow = document.createElement('button');
      overflow.type = 'button';
      overflow.className = 'chat-bar-talent chat-bar-talent--overflow';
      overflow.textContent = '+' + hiddenCount;
      overflow.setAttribute('aria-label', 'show more talents');
      overflow.addEventListener('click', function() {
        trayPage = (trayPage + 1) % pageCount;
        renderTalentTray();
      });
      talentsTray.appendChild(overflow);
    }
  }

  function stopTalentLiveUpdates() {
    if (modalPollTimer) {
      window.clearTimeout(modalPollTimer);
      modalPollTimer = null;
    }
    if (modalChatCleanup) {
      modalChatCleanup();
      modalChatCleanup = null;
    }
  }

  function getModalFocusables() {
    if (!modalPanel) return [];
    return Array.from(
      modalPanel.querySelectorAll('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])')
    ).filter(function(element) {
      return !element.disabled && !element.hidden && element.offsetParent !== null;
    });
  }

  function handleModalKeys(event) {
    if (!modal || modal.hidden) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      hideTalentView();
      return;
    }
    if (event.key !== 'Tab') return;

    var focusables = getModalFocusables();
    if (!focusables.length) return;
    var first = focusables[0];
    var last = focusables[focusables.length - 1];

    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  function hideTalentView() {
    if (!modal) return;
    stopTalentLiveUpdates();
    modal.hidden = true;
    modal.setAttribute('aria-hidden', 'true');
    modalUseId = null;
    document.removeEventListener('keydown', handleModalKeys);
    if (modalTrigger && typeof modalTrigger.focus === 'function') {
      modalTrigger.focus();
    }
    modalTrigger = null;
  }

  function showTalentView() {
    if (!modal) return;
    modal.hidden = false;
    modal.setAttribute('aria-hidden', 'false');
    document.removeEventListener('keydown', handleModalKeys);
    document.addEventListener('keydown', handleModalKeys);
    var closeButton = modal.querySelector('.talent-view-close');
    if (closeButton) closeButton.focus();
  }

  function createTimelineCard(label, ts, className) {
    var card = document.createElement('article');
    card.className = 'talent-view-event' + (className ? ' ' + className : '');

    var header = document.createElement('div');
    header.className = 'talent-view-event-header';

    var title = document.createElement('span');
    title.className = 'talent-view-event-label';
    title.textContent = label;

    var time = document.createElement('span');
    time.className = 'talent-view-event-time';
    time.textContent = formatEventTime(ts);

    header.appendChild(title);
    header.appendChild(time);
    card.appendChild(header);
    return card;
  }

  function renderTalentEvent(event) {
    var eventName = String(event.event || '').trim();
    var card;
    var body;
    var details;
    var summary;
    var pre;

    if (eventName === 'thinking') {
      card = createTimelineCard('thinking', event.ts, 'talent-view-event--thinking');
      body = document.createElement('div');
      body.className = 'talent-view-thinking';
      body.innerHTML = '<em>' + escapeHtml(event.content || '') + '</em>';
      card.appendChild(body);
      return card;
    }

    if (eventName === 'tool_start' || eventName === 'tool_end') {
      card = createTimelineCard(eventName === 'tool_start' ? 'tool start' : 'tool result', event.ts, 'talent-view-event--tool');
      details = document.createElement('details');
      summary = document.createElement('summary');
      summary.textContent = (event.tool || 'tool') + (event.call_id ? ' (' + event.call_id + ')' : '');
      details.appendChild(summary);
      pre = document.createElement('pre');
      pre.className = 'talent-view-pre';
      pre.textContent = eventName === 'tool_start' ? describeValue(event.args) : describeValue(event.result);
      details.appendChild(pre);
      card.appendChild(details);
      return card;
    }

    if (eventName === 'finish') {
      card = createTimelineCard('finished', event.ts, 'talent-view-event--finish');
      body = document.createElement('div');
      body.className = 'talent-view-markdown';
      body.innerHTML = window.AppServices.renderMarkdown(event.summary || event.result || '');
      card.appendChild(body);
      return card;
    }

    if (eventName === 'error') {
      card = createTimelineCard('error', event.ts, 'talent-view-event--error');
      pre = document.createElement('pre');
      pre.className = 'talent-view-pre';
      pre.textContent = event.error || '';
      card.appendChild(pre);
      return card;
    }

    card = createTimelineCard(eventName || 'event', event.ts, '');
    body = document.createElement('div');
    body.className = 'talent-view-info';
    body.textContent = event.talent || event.model || event.provider || describeValue(event);
    card.appendChild(body);
    return card;
  }

  function renderTalentView(data) {
    if (!modalTitle || !modalStatus || !modalTimeline) return;
    modalTitle.textContent = data.task || 'talent run';
    modalTitle.title = data.task || '';
    modalStatus.textContent = data.status || '';
    modalStatus.dataset.status = data.status || '';
    modalTimeline.innerHTML = '';

    if (!Array.isArray(data.events) || !data.events.length) {
      var empty = document.createElement('p');
      empty.className = 'talent-view-empty';
      empty.textContent = 'no events yet.';
      modalTimeline.appendChild(empty);
      return;
    }

    data.events.forEach(function(event) {
      modalTimeline.appendChild(renderTalentEvent(event));
    });
  }

  function renderTalentViewError(message) {
    if (!modalTitle || !modalStatus || !modalTimeline) return;
    modalTitle.textContent = 'talent run';
    modalStatus.textContent = 'errored';
    modalStatus.dataset.status = 'errored';
    modalTimeline.innerHTML = '';
    var card = createTimelineCard('error', null, 'talent-view-event--error');
    var pre = document.createElement('pre');
    pre.className = 'talent-view-pre';
    pre.textContent = message;
    card.appendChild(pre);
    modalTimeline.appendChild(card);
  }

  async function fetchTalentView(useId) {
    var response = await fetch('/api/chat/talent-log/' + encodeURIComponent(useId));
    if (!response.ok) {
      var body = {};
      try {
        body = await response.json();
      } catch (_err) {
        // Fall back to generic talent-run copy when the error body is not JSON.
      }
      throw new Error(body.error || 'unable to load talent run');
    }
    return response.json();
  }

  function scheduleTalentPoll(useId) {
    if (modalUseId !== useId) return;
    modalPollTimer = window.setTimeout(function() {
      modalPollTimer = null;
      refreshTalentView(useId);
    }, 2000);
  }

  function attachTalentViewChat(useId) {
    if (modalChatCleanup || !window.appEvents) return;
    modalChatCleanup = window.appEvents.listen('chat', function(msg) {
      if (String(msg.use_id || '') !== useId) return;
      var eventName = String(msg.event || msg.kind || '');
      if (eventName === 'talent_finished' || eventName === 'talent_errored') {
        refreshTalentView(useId);
      }
    });
  }

  async function refreshTalentView(useId) {
    if (!useId || modalUseId !== useId) return;
    try {
      var data = await fetchTalentView(useId);
      if (modalUseId !== useId) return;
      renderTalentView(data);
      if (data.status === 'running') {
        stopTalentLiveUpdates();
        attachTalentViewChat(useId);
        scheduleTalentPoll(useId);
      } else {
        stopTalentLiveUpdates();
      }
    } catch (err) {
      stopTalentLiveUpdates();
      renderTalentViewError(err && err.message ? err.message : 'unable to load talent run');
    }
  }

  window.openTalentView = function(useId, _opts) {
    if (!modal || !useId) return;
    modalTrigger = document.activeElement;
    modalUseId = String(useId);
    stopTalentLiveUpdates();
    modalStatus.textContent = '';
    modalStatus.dataset.status = '';
    modalTitle.textContent = 'loading...';
    modalTimeline.innerHTML = '';
    showTalentView();
    refreshTalentView(modalUseId);
  };

  function getClockNow() {
    // test-only clock override; do not use in production code paths.
    if (typeof window.__solChatTestClock === 'function') return window.__solChatTestClock();
    return Date.now();
  }

  function formatChatDay(ts) {
    var date = new Date(typeof ts === 'number' ? ts : Date.now());
    var year = date.getFullYear();
    var month = String(date.getMonth() + 1).padStart(2, '0');
    var day = String(date.getDate()).padStart(2, '0');
    return String(year) + month + day;
  }

  function restoreDefaultPlaceholder() {
    if (!input) return;
    input.placeholder = input.dataset.defaultPlaceholder || '';
  }

  function clearSolPingTimers() {
    if (solPingPulseTimer) {
      window.clearTimeout(solPingPulseTimer);
      solPingPulseTimer = null;
    }
    if (solPingOfflineTimer) {
      window.clearTimeout(solPingOfflineTimer);
      solPingOfflineTimer = null;
    }
  }

  function applySolPingOfflineClass() {
    if (!solRequestState || !solPingDot || solPingDisconnectedAt === null) return;
    if (getClockNow() - solPingDisconnectedAt < SOL_PING_OFFLINE_DELAY_MS) return;
    solPingDot.classList.add('offline');
    solPingDot.title = SOL_INITIATED.SOL_PINGED_OFFLINE_TOOLTIP || '';
  }

  function scheduleSolPingOfflineTimer() {
    if (!solRequestState || solPingDisconnectedAt === null) return;
    if (solPingOfflineTimer) window.clearTimeout(solPingOfflineTimer);
    var elapsed = Math.max(0, getClockNow() - solPingDisconnectedAt);
    var delay = Math.max(0, SOL_PING_OFFLINE_DELAY_MS - elapsed);
    solPingOfflineTimer = window.setTimeout(function() {
      solPingOfflineTimer = null;
      applySolPingOfflineClass();
    }, delay);
  }

  function clearSolPingOfflineState() {
    solPingDisconnectedAt = null;
    if (solPingOfflineTimer) {
      window.clearTimeout(solPingOfflineTimer);
      solPingOfflineTimer = null;
    }
    if (solPingDot) {
      solPingDot.classList.remove('offline');
      solPingDot.removeAttribute('title');
    }
  }

  function handleSolPingConnectionState(state) {
    if (!solRequestState) return;
    if (state && state.connected === false) {
      if (solPingDisconnectedAt === null) solPingDisconnectedAt = getClockNow();
      scheduleSolPingOfflineTimer();
      return;
    }
    if (state && state.connected === true) {
      clearSolPingOfflineState();
    }
  }

  function attachSolPingConnectionListener() {
    if (solPingConnectionUnsub || !window.appEvents || typeof window.appEvents.onConnectionState !== 'function') return;
    solPingConnectionUnsub = window.appEvents.onConnectionState(handleSolPingConnectionState);
  }

  function detachSolPingConnectionListener() {
    if (!solPingConnectionUnsub) return;
    solPingConnectionUnsub();
    solPingConnectionUnsub = null;
  }

  function renderSolPing(state) {
    if (!state || !state.request_id || !statusText || !statusWrap) return;
    solRequestState = {
      request_id: String(state.request_id || ''),
      summary: String(state.summary || ''),
      ts: state.ts,
      event_index: state.event_index,
      day: String(state.day || formatChatDay(state.ts))
    };
    clearSolPingTimers();
    clearSolPingOfflineState();
    statusWrap.classList.add('chat-bar-status--sol-ping');
    statusWrap.setAttribute('role', 'button');
    statusWrap.tabIndex = 0;
    statusText.textContent = solRequestState.summary;
    statusText.title = solRequestState.summary;
    statusWrap.title = solRequestState.summary;
    if (input && solRequestState.summary) input.placeholder = solRequestState.summary;
    if (solPingDot) {
      solPingDot.hidden = false;
      solPingDot.classList.remove('offline');
      solPingDot.classList.add('pulsing');
      solPingDot.removeAttribute('title');
    }
    if (solPingDismiss) solPingDismiss.hidden = false;
    solPingPulseTimer = window.setTimeout(function() {
      solPingPulseTimer = null;
      if (solPingDot) solPingDot.classList.remove('pulsing');
    }, SOL_PING_OFFLINE_DELAY_MS);
    attachSolPingConnectionListener();
  }

  function clearSolPing() {
    solRequestState = null;
    clearSolPingTimers();
    clearSolPingOfflineState();
    detachSolPingConnectionListener();
    if (solPingDot) {
      solPingDot.hidden = true;
      solPingDot.classList.remove('pulsing');
    }
    if (solPingDismiss) solPingDismiss.hidden = true;
    if (statusWrap) {
      statusWrap.classList.remove('chat-bar-status--sol-ping');
      statusWrap.removeAttribute('role');
      statusWrap.removeAttribute('tabindex');
    }
    if (statusText) {
      statusText.textContent = '';
      statusText.removeAttribute('title');
    }
    if (statusWrap) statusWrap.removeAttribute('title');
    restoreDefaultPlaceholder();
  }

  function solPingApiJson(path, requestId) {
    if (!window.apiJson || !requestId) return Promise.resolve(null);
    return window.apiJson(path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ request_id: requestId })
    }).catch(function(err) {
      if (window.logError) window.logError(err, { context: 'sol-chat-request' });
      return null;
    });
  }

  function solPingEndpoint(action) {
    return '/api/chat/' + SOL_INITIATED.KIND_SOL_CHAT_REQUEST + '/' + action;
  }

  function openSolConversation() {
    if (!solRequestState) return;
    var target = '/app/chat/' + encodeURIComponent(solRequestState.day || '');
    var hasIndex = solRequestState.event_index !== null && solRequestState.event_index !== undefined;
    var anchor = hasIndex ? '#event-' + solRequestState.event_index : '';
    if (window.location.pathname === target) {
      if (hasIndex) {
        var targetNode = document.getElementById('event-' + solRequestState.event_index);
        if (targetNode && typeof targetNode.scrollIntoView === 'function') {
          targetNode.scrollIntoView({ behavior: 'smooth' });
        }
      }
      if (input) input.focus();
      solPingApiJson(solPingEndpoint('open'), solRequestState.request_id);
      return;
    }
    window.location.href = target + anchor;
    solPingApiJson(solPingEndpoint('open'), solRequestState.request_id);
  }

  window.recordOwnerChatOpen = function(requestId) {
    solPingApiJson(solPingEndpoint('open'), requestId);
  };

  function clearConversationSuggestion() {
    if (!input) return;
    delete input.dataset.suggestion;
    restoreDefaultPlaceholder();
  }

  function acceptConversationSuggestion() {
    if (!input || !input.dataset.suggestion || input.value !== '') return false;
    input.value = input.dataset.suggestion;
    clearConversationSuggestion();
    resizeComposer();
    return true;
  }

  function handleComposerKeydown(event) {
    if (event.isComposing === true || event.keyCode === 229) return;
    if (event.key === 'Enter' && event.shiftKey) return;
    if (event.key === 'Enter') {
      event.preventDefault();
      if (form && typeof form.requestSubmit === 'function') {
        form.requestSubmit();
      } else if (form) {
        form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
      }
      return;
    }
    if (!input || !input.dataset.suggestion || input.value !== '') return;
    if (event.key === 'Tab' && !event.shiftKey) {
      event.preventDefault();
      acceptConversationSuggestion();
      return;
    }
    if (event.key === 'ArrowRight' && input.selectionStart === 0) {
      event.preventDefault();
      acceptConversationSuggestion();
    }
  }

  function handleSuggestionInput() {
    if (!input || !input.dataset.suggestion || input.value === '') return;
    clearConversationSuggestion();
  }

  window.fillChat = function(text) {
    if (!input) return;
    input.focus();
    input.value = text;
    clearConversationSuggestion();
    resizeComposer();
    if (typeof input.setSelectionRange === 'function') {
      var end = input.value.length;
      input.setSelectionRange(end, end);
    }
  };

  window.suggestChat = function(text) {
    if (!input) return;
    if (!input.dataset.defaultPlaceholder) input.dataset.defaultPlaceholder = input.placeholder || '';
    input.dataset.suggestion = text;
    input.placeholder = text;
    input.focus();
  };

  (function consumePendingNeedsYouPrompt() {
    try {
      var raw = sessionStorage.getItem(NEEDS_YOU_PENDING_KEY);
      if (!raw) return;
      sessionStorage.removeItem(NEEDS_YOU_PENDING_KEY);
      var data = JSON.parse(raw);
      if (data && typeof data.prompt === 'string') {
        window.__needsYouPendingPrompt = data.prompt;
        window.__needsYouPendingSource = {
          kind: 'needs_you',
          item_text: typeof data.item_text === 'string' ? data.item_text : ''
        };
        window.fillChat(data.prompt);
      }
    } catch (_err) {}
  })();

  window.openConversation = function(options) {
    var opts = options || {};
    if (opts.openOn !== 'chat-request') return;
    openSolConversation();
  };

  function handleChatEvent(msg) {
    var eventName = String(msg.event || msg.kind || '');
    if (eventName === SOL_INITIATED.KIND_SOL_CHAT_REQUEST) {
      renderSolPing({
        request_id: msg.request_id,
        summary: msg.summary,
        ts: msg.ts,
        event_index: msg.event_index,
        day: msg.day
      });
      return;
    }
    if (
      eventName === SOL_INITIATED.KIND_SOL_CHAT_REQUEST_SUPERSEDED
      || eventName === SOL_INITIATED.KIND_OWNER_CHAT_OPEN
      || eventName === SOL_INITIATED.KIND_OWNER_CHAT_DISMISSED
    ) {
      if (solRequestState && solRequestState.request_id === String(msg.request_id || '')) {
        clearSolPing();
      }
      return;
    }
    if (eventName === 'owner_message') {
      hideSupportOffer();
      hideSupportDraft();
      if (supportCapacityActive && !isSupportTalentRunning()) exitSupportCapacity();
      if (!solRequestState) {
        chatBarPendingPlaceholders.push({ ownerText: msg.text || '', ts: msg.ts || Date.now() });
        setStatus(window.solChatCopy.CHAT_LIVENESS_THINKING, window.solChatCopy.CHAT_LIVENESS_THINKING);
        if (statusWrap) {
          statusWrap.classList.add('chat-bar-status--thinking');
          statusWrap.classList.remove('chat-bar-status--error');
          statusWrap.removeAttribute('role');
          statusWrap.removeAttribute('tabindex');
        }
        statusErrorActive = false;
      }
      return;
    }
    if (eventName === 'sol_message') {
      if (solRequestState) return;
      clearPendingLivenessStatus();
      if (msg.draft) {
        hideSupportOffer();
        showSupportDraft(msg.draft);
      } else if (msg.offer && msg.offer.kind === 'support') {
        hideSupportDraft();
        showSupportOffer(msg.text || '');
      } else {
        hideSupportOffer();
        if (!renderSupportOutcome(msg)) {
          hideSupportDraft();
          hideSupportResult();
          setStatus(msg.text || '', statusTitleFor(msg));
        }
      }
      if (supportCapacityActive && !isSupportTalentRunning()) exitSupportCapacity();
      return;
    }
    if (eventName === 'talent_queued') {
      var queuedUseId = String(msg.use_id || '');
      if (queuedUseId) {
        queuedJobs.set(queuedUseId, {
          name: msg.name || '',
          task: msg.task || ''
        });
        renderJobsIndicator();
      }
      return;
    }
    if (eventName === 'talent_spawned') {
      upsertTalent({
        useId: String(msg.use_id || ''),
        name: msg.name || '',
        task: msg.task || '',
        status: 'running',
        updatedAt: msg.started_at || msg.ts || Date.now()
      });
      queuedJobs.delete(String(msg.use_id || ''));
      renderJobsIndicator();
      if (String(msg.name || '') === 'support') {
        enterSupportCapacity();
        return;
      }
      if (!solRequestState && chatBarPendingPlaceholders.length > 0) {
        const task = String(msg.task || '').trim();
        if (task) {
          try {
            const label = window.solChatCopy.talentLabel(String(msg.name || ''), 'running');
            const composed = window.solChatCopy.CHAT_LIVENESS_TASK_FORMAT
              .replace('{label}', label)
              .replace('{task}', task);
            setStatus(composed, composed);
          } catch (_err) {
            /* unknown target — leave phase-1 text in place */
          }
        }
      }
      return;
    }
    if (eventName === 'talent_finished') {
      removeTalent(String(msg.use_id || ''));
      queuedJobs.delete(String(msg.use_id || ''));
      renderJobsIndicator();
      if (modalUseId && modalUseId === String(msg.use_id || '')) {
        refreshTalentView(modalUseId);
      }
      if (!solRequestState && chatBarPendingPlaceholders.length > 0) {
        clearPendingLivenessStatus();
        setStatus('', '');
      }
      return;
    }
    if (eventName === 'talent_errored') {
      removeTalent(String(msg.use_id || ''));
      queuedJobs.delete(String(msg.use_id || ''));
      renderJobsIndicator();
      if (modalUseId && modalUseId === String(msg.use_id || '')) {
        refreshTalentView(modalUseId);
      }
      if (!solRequestState && chatBarPendingPlaceholders.length > 0) {
        clearPendingLivenessStatus();
        setStatus('', '');
      }
      return;
    }
    if (eventName === 'chat_error') {
      if (solRequestState) return;
      if (chatBarPendingPlaceholders.length > 0) chatBarPendingPlaceholders.shift();
      var renderedReason = window.renderChatReason(msg.reason, msg.provider || '');
      if (statusWrap) {
        statusWrap.classList.remove('chat-bar-status--thinking');
        statusWrap.classList.add('chat-bar-status--error');
        statusWrap.setAttribute('role', 'button');
        statusWrap.tabIndex = 0;
      }
      statusErrorActive = true;
      setStatus(renderedReason.message, renderedReason.message, renderedReason.action);
    }
  }

  async function hydrateChatBar() {
    if (!appBar) return;
    populateSupportCapacityCopy();
    populateSupportOfferCopy();
    populateSupportDraftCopy();
    if (window.solChatBarSeed) {
      renderSolPing(window.solChatBarSeed);
    } else {
      restoreDefaultPlaceholder();
    }
    try {
      var data = await window.apiJson('/api/chat/session');
      if (data.latest_sol_message && !solRequestState) {
        var msg = data.latest_sol_message;
        if (msg.draft) {
          hideSupportOffer();
          showSupportDraft(msg.draft);
        } else if (msg.offer && msg.offer.kind === 'support') {
          hideSupportDraft();
          showSupportOffer(msg.text || '');
        } else {
          hideSupportOffer();
          if (!renderSupportOutcome(msg)) {
            hideSupportDraft();
            hideSupportResult();
            setStatus(msg.text || '', statusTitleFor(msg));
          }
        }
      }
      (data.active_talents || []).forEach(function(talent) {
        upsertTalent({
          useId: String(talent.use_id || ''),
          name: talent.name || '',
          task: talent.task || '',
          status: 'running',
          updatedAt: talent.started_at || Date.now()
        });
      });
      (data.queued_talents || []).forEach(function(talent) {
        var queuedUseId = String(talent.use_id || '');
        if (!queuedUseId) return;
        queuedJobs.set(queuedUseId, {
          name: talent.name || '',
          task: talent.task || ''
        });
      });
      renderJobsIndicator();
    } catch (err) {
      disableComposer();
      var hydrateError = `Couldn't load recent chat session. ${window.CONVEY_COPY.RELOAD_HINT}`;
      setStatus(
        hydrateError,
        hydrateError
      );
      window.logError(err, { context: 'chat-hydrate' });
    }
  }

  async function postChatMessage(message) {
    setPendingState(true);
    try {
      var body = {
        message: message,
        app: APP_NAME,
        path: window.location.pathname,
        facet: window.selectedFacet || null
      };
      if (
        window.__needsYouPendingSource
        && window.__needsYouPendingPrompt === message
      ) {
        body.source = window.__needsYouPendingSource;
      }
      var response = await fetch('/api/chat', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body)
      });
      if (!response.ok) {
        var errorBody = {};
        try {
          errorBody = await response.json();
        } catch (_jsonErr) {}
        if (response.status === 429 && errorBody.reason_code === 'chat_queue_full') {
          showQueueCapMessage();
          return false;
        }
        throw new Error('request failed');
      }
      window.__needsYouPendingSource = null;
      window.__needsYouPendingPrompt = null;
      return true;
    } catch (_err) {
      var renderedReason = window.renderChatReason('unknown', '');
      setStatus(renderedReason.message, renderedReason.message, renderedReason.action);
      return false;
    } finally {
      setPendingState(false);
    }
  }

  async function handleSubmit(event) {
    event.preventDefault();
    if (!input || !sendBtn || pendingSend) return;
    var message = input.value.trim();
    if (!message) return;

    var ok = await postChatMessage(message);
    if (ok) {
      input.value = '';
      resizeComposer();
    }
    input.focus();
  }

  function sendOfferResponse(message) {
    if (pendingSend) return;
    hideSupportOffer();
    postChatMessage(message);
  }

  function initChatChrome() {
    try {
      LEGACY_KEYS.forEach(function(key) { localStorage.removeItem(key); });
    } catch (_err) {
      // Legacy cleanup is best-effort; failing closed leaves current keys untouched.
    }

    if (modal) {
      modal.addEventListener('click', function(event) {
        if (event.target.closest('[data-action="close"]')) {
          hideTalentView();
        }
      });
    }

    if (input) {
      if (!input.dataset.defaultPlaceholder) input.dataset.defaultPlaceholder = input.placeholder || '';
      restoreDefaultPlaceholder();
      resizeComposer();
      input.addEventListener('input', resizeComposer);
      input.addEventListener('input', handleSuggestionInput);
      input.addEventListener('keydown', handleComposerKeydown);
    }

    if (form) {
      form.addEventListener('submit', handleSubmit);
    }

    if (statusWrap) {
      statusWrap.addEventListener('click', function(event) {
        if (event.target.closest('button, a')) return;
        if (statusErrorActive) {
          window.location.href = '/app/chat/';
          return;
        }
        if (solRequestState) {
          window.openConversation({ prompt: null, openOn: 'chat-request' });
        }
      });
      statusWrap.addEventListener('keydown', function(event) {
        if (!solRequestState && !statusErrorActive) return;
        if (event.target.closest('button, a')) return;
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        if (statusErrorActive) {
          window.location.href = '/app/chat/';
          return;
        }
        if (solRequestState) {
          window.openConversation({ prompt: null, openOn: 'chat-request' });
        }
      });
    }

    if (solPingDismiss) {
      solPingDismiss.addEventListener('click', function(event) {
        event.stopPropagation();
        if (!solRequestState) return;
        solPingApiJson(solPingEndpoint('dismissed'), solRequestState.request_id);
      });
    }

    if (offerYesBtn) {
      offerYesBtn.addEventListener('click', function() {
        sendOfferResponse(window.solChatCopy.CHAT_OFFER_SUPPORT_YES);
      });
    }
    if (offerNoBtn) {
      offerNoBtn.addEventListener('click', function() {
        if (pendingSend) return;
        hideSupportOffer();
        if (!window.apiJson) return;
        window.apiJson('/api/chat/offer/decline', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: '{}'
        }).catch(function(err) {
          if (window.logError) window.logError(err, { context: 'chat-offer-decline' });
          return null;
        });
      });
    }

    function submitDraftAction(endpoint) {
      if (pendingSend) return;
      if (!currentDraft || !currentDraft.draft_id) return;
      const draftId = String(currentDraft.draft_id);
      if (draftSubmitBtn) draftSubmitBtn.disabled = true;
      if (draftCancelBtn) draftCancelBtn.disabled = true;
      if (!window.apiJson) return;
      window.apiJson(endpoint, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ draft_id: draftId })
      }).then(function(resp) {
        const outcome = resp && resp.outcome;
        if (outcome === 'not_found' || outcome === 'already_submitted' || outcome === 'superseded') {
          hideSupportDraft();
        }
        return resp;
      }).catch(function(err) {
        if (window.logError) window.logError(err, { context: 'chat-support-draft' });
        if (draftSubmitBtn) draftSubmitBtn.disabled = false;
        if (draftCancelBtn) draftCancelBtn.disabled = false;
        return null;
      });
    }
    if (draftSubmitBtn) {
      draftSubmitBtn.addEventListener('click', function() {
        submitDraftAction('/api/chat/support/draft/confirm');
      });
    }
    if (draftCancelBtn) {
      draftCancelBtn.addEventListener('click', function() {
        submitDraftAction('/api/chat/support/draft/cancel');
      });
    }

    if (window.appEvents) {
      window.appEvents.listen('chat', handleChatEvent);
    }

    window.addEventListener('beforeunload', function() {
      clearSolPingTimers();
      detachSolPingConnectionListener();
    });

    hydrateChatBar();
  }

  window.whenShellReady(initChatChrome);
})();
