// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function() {
  const REQUEST_TIMEOUT_MS = 12000;
  const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;
  const MAX_ATTACHMENT_FILES = 5;
  const ALLOWED_SUFFIXES = [
    '.png', '.jpg', '.jpeg', '.gif', '.webp', '.svg', '.pdf',
    '.txt', '.csv', '.html', '.md', '.xml', '.json'
  ];
  const CONTENT_TYPES = {
    '.png': 'image/png',
    '.jpg': 'image/jpeg',
    '.jpeg': 'image/jpeg',
    '.gif': 'image/gif',
    '.webp': 'image/webp',
    '.svg': 'image/svg+xml',
    '.pdf': 'application/pdf',
    '.txt': 'text/plain',
    '.csv': 'text/csv',
    '.html': 'text/html',
    '.md': 'text/markdown',
    '.xml': 'text/xml',
    '.json': 'application/json'
  };
  const FEEDBACK_PORTAL = {
    subject: 'Feedback',
    severity: 'low',
    category: 'feedback'
  };
  const FIELD_LABELS = {
    subject: 'subject',
    description: "what's happening",
    body: 'your feedback',
    content: 'your message',
    user_email: 'your email',
    filename: 'file',
    content_type: 'type',
    byte_size: 'size',
    severity: 'severity',
    category: 'area',
    product: 'product'
  };

  const defaultSetTimer = (fn, ms) => setTimeout(fn, ms);
  const defaultClearTimer = id => clearTimeout(id);
  let _setTimer = defaultSetTimer;
  let _clearTimer = defaultClearTimer;

  let reviewInFlight = false;
  let currentAttempt = null;
  let pendingReplyFiles = [];
  let ticketListDeps = null;
  let replyOnSubmitted = null;

  function __setTimers(set, clear) {
    if (!set && !clear) {
      _setTimer = defaultSetTimer;
      _clearTimer = defaultClearTimer;
      return;
    }
    _setTimer = set || defaultSetTimer;
    _clearTimer = clear || defaultClearTimer;
  }

  function fetchWithTimeout(url, opts) {
    const controller = new AbortController();
    let timerId;
    const timeout = new Promise(function(_resolve, reject) {
      timerId = _setTimer(function() {
        controller.abort();
        reject(new Error('timed out'));
      }, REQUEST_TIMEOUT_MS);
    });
    const fetchOpts = Object.assign({}, opts || {}, {signal: controller.signal});
    return Promise.race([window.fetch(url, fetchOpts), timeout]).finally(function() {
      if (timerId !== undefined) _clearTimer(timerId);
    });
  }

  function esc(s) {
    const el = document.createElement('span');
    el.textContent = s;
    return el.innerHTML;
  }

  function attr(s) {
    return esc(s).replace(/"/g, '&quot;').replace(/'/g, '&#39;');
  }

  function formatSize(bytes) {
    if (bytes >= 1024 * 1024) return (bytes / 1024 / 1024).toFixed(1) + ' MB';
    if (bytes >= 1024) return Math.round(bytes / 1024) + ' KB';
    return bytes + ' bytes';
  }

  function isReviewInFlight() {
    return reviewInFlight;
  }

  function suffixOf(name) {
    const text = String(name || '');
    const index = text.lastIndexOf('.');
    if (index < 0) return '';
    return text.slice(index).toLowerCase();
  }

  function contentTypeFor(suffix) {
    return CONTENT_TYPES[suffix] || '';
  }

  function validateFiles(files) {
    if (files.length > MAX_ATTACHMENT_FILES) return 'max 5 files per upload.';
    for (let i = 0; i < files.length; i++) {
      const file = files[i];
      if (file.size > MAX_ATTACHMENT_BYTES) return file.name + ' exceeds 10 MB limit.';
      const suffix = suffixOf(file.name);
      if (ALLOWED_SUFFIXES.indexOf(suffix) === -1) {
        return 'unsupported file type: ' + (suffix || '(none)');
      }
    }
    return null;
  }

  function reportStatus(el, msg, type) {
    if (!el) return;
    el.textContent = msg || '';
    el.className = 'support-status-msg' + (type ? ' ' + type : '');
  }

  function setComposeDisabled(disabled) {
    ['create-submit', 'feedback-submit', 'reply-submit', 'attach-only-submit'].forEach(function(id) {
      const el = document.getElementById(id);
      if (el) el.disabled = disabled;
    });
  }

  async function readJsonResponse(resp) {
    let body = {};
    try {
      body = await resp.json();
    } catch (err) {
      body = {};
    }
    if (!resp.ok) {
      return {
        ok: false,
        error: body.error || ("request failed (HTTP " + resp.status + ")"),
        reason_code: body.reason_code || null,
        status: resp.status,
        body: body
      };
    }
    return {ok: true, status: resp.status, body: body};
  }

  async function postDraftJson(url, payload) {
    const resp = await fetchWithTimeout(url, {
      method: 'POST',
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify(payload)
    });
    return readJsonResponse(resp);
  }

  async function cancelItems(items) {
    let last = {ok: true, body: {}};
    for (let i = 0; i < items.length; i++) {
      try {
        last = await postDraftJson('/app/support/api/draft/cancel', {draft_id: items[i].draftId});
      } catch (err) {
        last = {ok: false, body: {}, error: err && err.message};
      }
    }
    return last;
  }

  async function fetchDiagnosticsSnapshot() {
    const resp = await fetchWithTimeout('/app/support/api/diagnostics');
    let body = null;
    try {
      body = await resp.json();
    } catch (err) {
      throw new Error("couldn't load diagnostics");
    }
    if (!resp.ok) {
      throw new Error((body && body.error) || "couldn't load diagnostics");
    }
    return body;
  }

  function reportingEnabled() {
    return !(window.CONVEY_SETTINGS && window.CONVEY_SETTINGS.reportingEnabled === false);
  }

  function showReview() {
    const review = document.getElementById('support-review');
    const main = document.getElementById('support-main');
    if (review) review.hidden = false;
    if (main) main.classList.add('support-main--reviewing');
  }

  function hideReview() {
    const review = document.getElementById('support-review');
    const main = document.getElementById('support-main');
    if (review) review.hidden = true;
    const body = document.getElementById('support-review-body');
    if (body) {
      body.replaceChildren();
      body.removeAttribute('data-rendered');
    }
    const send = document.getElementById('support-review-send');
    if (send) send.disabled = true;
    const status = document.getElementById('support-review-status');
    if (status) {
      status.textContent = '';
      status.className = 'support-status-msg';
    }
    if (main) main.classList.remove('support-main--reviewing');
  }

  function finishAttempt(message, type) {
    const statusEl = currentAttempt && currentAttempt.composeStatusEl;
    const kind = currentAttempt && currentAttempt.kind;
    const submitted = currentAttempt && currentAttempt.items &&
      currentAttempt.items.some(function(item) { return item.confirmed; });
    hideReview();
    reviewInFlight = false;
    setComposeDisabled(false);
    const reopen = replyOnSubmitted;
    currentAttempt = null;
    if (statusEl) reportStatus(statusEl, message, type || 'success');
    if (submitted && (kind === 'create' || kind === 'feedback') && ticketListDeps) {
      loadTickets(ticketListDeps);
    }
    if (submitted && (kind === 'reply' || kind === 'files-only') && typeof reopen === 'function') {
      reopen();
    }
  }

  function appendKind(parent, kindLabel, ticketId) {
    const row = document.createElement('div');
    row.className = 'support-review-kindrow';
    const kind = document.createElement('span');
    kind.className = 'support-review-kind';
    kind.textContent = kindLabel;
    row.appendChild(kind);
    if (ticketId !== undefined && ticketId !== null && String(ticketId).trim()) {
      const ticket = document.createElement('span');
      ticket.className = 'support-review-ticket';
      ticket.textContent = 'ticket #' + String(ticketId);
      row.appendChild(ticket);
    }
    parent.appendChild(row);
  }

  function appendField(parent, label, value) {
    const row = document.createElement('div');
    row.className = 'support-review-field';
    const lab = document.createElement('span');
    lab.className = 'support-review-label';
    lab.textContent = label;
    const val = document.createElement('span');
    val.className = 'support-review-fieldval';
    val.textContent = String(value);
    row.appendChild(lab);
    row.appendChild(val);
    parent.appendChild(row);
  }

  function appendPill(parent, label, value) {
    const pill = document.createElement('span');
    pill.className = 'support-review-pill';
    pill.textContent = label + ': ' + String(value);
    parent.appendChild(pill);
  }

  function appendNameAttached(parent, payload) {
    const pill = document.createElement('span');
    pill.className = 'support-review-pill';
    pill.textContent = payload.anonymous === true ? 'name attached: no' : 'name attached: yes';
    parent.appendChild(pill);
  }

  function appendPortalConstantPills(parent) {
    appendPill(parent, 'subject', FEEDBACK_PORTAL.subject);
    appendPill(parent, 'severity', FEEDBACK_PORTAL.severity);
    appendPill(parent, 'area', FEEDBACK_PORTAL.category);
  }

  function diagnosticLabel(key) {
    return key === 'version' ? 'journal version' : key;
  }

  function renderDiagnosticsValue(value, labelText) {
    if (Array.isArray(value)) return renderDiagnosticsArray(value, labelText);
    if (value === null || typeof value !== 'object') {
      const span = document.createElement('span');
      span.className = 'support-review-value';
      span.textContent = value === null ? '—' : String(value);
      return span;
    }
    const keys = Object.keys(value);
    if (keys.length === 0) {
      const span = document.createElement('span');
      span.className = 'support-review-value';
      span.textContent = '(none)';
      return span;
    }
    const group = document.createElement('div');
    group.className = 'support-review-diagnostic-subrows';
    keys.forEach(function(key) {
      appendDiagnosticSubRow(group, key, value[key]);
    });
    return group;
  }

  function renderDiagnosticsArray(items, labelText) {
    const wrapper = document.createElement('div');
    const expanded = items.length <= 5;
    const button = document.createElement('button');
    const content = document.createElement('div');
    wrapper.className = 'support-review-diagnostic-array';
    button.type = 'button';
    button.className = 'support-review-diagnostic-toggle';
    button.setAttribute('aria-expanded', expanded ? 'true' : 'false');
    button.textContent = String(labelText || '') + ' (' + String(items.length) + ')';
    content.className = 'support-review-diagnostic-array-items';
    content.hidden = !expanded;
    if (items.length === 0) {
      const none = document.createElement('span');
      none.textContent = '(none)';
      content.appendChild(none);
    } else {
      items.forEach(function(item) {
        const row = document.createElement('div');
        row.className = 'support-review-diagnostic-array-item';
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

  function appendDiagnosticSubRow(parent, key, value) {
    const row = document.createElement('div');
    row.className = 'support-review-diagnostic-subrow';
    const label = document.createElement('span');
    label.className = 'support-review-label';
    label.textContent = key;
    const valueEl = document.createElement('div');
    valueEl.className = 'support-review-fieldval';
    valueEl.appendChild(renderDiagnosticsValue(value, key));
    row.appendChild(label);
    row.appendChild(valueEl);
    parent.appendChild(row);
  }

  function appendDiagnosticsRow(parent, key, value) {
    const label = diagnosticLabel(key);
    const row = document.createElement('div');
    row.className = Array.isArray(value)
      ? 'support-review-diagnostic-row support-review-diagnostic-row--array'
      : 'support-review-diagnostic-row';
    if (!Array.isArray(value)) {
      const labelEl = document.createElement('span');
      labelEl.className = 'support-review-label';
      labelEl.textContent = label;
      row.appendChild(labelEl);
    }
    const valueEl = document.createElement('div');
    valueEl.className = 'support-review-fieldval';
    valueEl.appendChild(renderDiagnosticsValue(value, label));
    row.appendChild(valueEl);
    parent.appendChild(row);
  }

  function renderDiagnosticsBlock(parent, snapshot) {
    const section = document.createElement('div');
    section.className = 'support-review-diagnostics';
    const title = document.createElement('div');
    title.className = 'support-review-diagnostics-title';
    title.textContent = "what's included with this request";
    section.appendChild(title);
    if (snapshot === null || snapshot === undefined) {
      const omitted = document.createElement('div');
      omitted.className = 'support-review-omitted';
      omitted.textContent = 'diagnostic context is not included. you can turn diagnostic reports on in settings.';
      section.appendChild(omitted);
      parent.appendChild(section);
      return;
    }
    const note = document.createElement('div');
    note.className = 'support-review-diagnostics-note';
    note.textContent = 'these exact values go to solstone support with your request. nothing else leaves this machine.';
    section.appendChild(note);
    const rows = document.createElement('div');
    rows.className = 'support-review-diagnostics-rows';
    if (snapshot && typeof snapshot === 'object' && !Array.isArray(snapshot)) {
      Object.keys(snapshot).forEach(function(key) {
        appendDiagnosticsRow(rows, key, snapshot[key]);
      });
    }
    section.appendChild(rows);
    parent.appendChild(section);
  }

  function renderCreateCard(parent, payload, snapshot) {
    appendKind(parent, 'new support request');
    appendField(parent, FIELD_LABELS.subject, payload.subject);
    appendField(parent, FIELD_LABELS.description, payload.description);
    if ('user_email' in payload) appendField(parent, FIELD_LABELS.user_email, payload.user_email);
    const meta = document.createElement('div');
    meta.className = 'support-review-meta';
    appendPill(meta, FIELD_LABELS.severity, payload.severity);
    if ('category' in payload) appendPill(meta, FIELD_LABELS.category, payload.category);
    appendPill(meta, FIELD_LABELS.product, payload.product);
    appendNameAttached(meta, payload);
    parent.appendChild(meta);
    renderDiagnosticsBlock(parent, snapshot);
  }

  function renderFeedbackCard(parent, payload, snapshot) {
    appendKind(parent, 'send feedback');
    appendField(parent, FIELD_LABELS.body, payload.body);
    if ('user_email' in payload) appendField(parent, FIELD_LABELS.user_email, payload.user_email);
    const meta = document.createElement('div');
    meta.className = 'support-review-meta';
    appendPill(meta, FIELD_LABELS.product, payload.product);
    appendNameAttached(meta, payload);
    appendPortalConstantPills(meta);
    parent.appendChild(meta);
    renderDiagnosticsBlock(parent, snapshot);
  }

  function renderReplyCard(parent, payload) {
    appendKind(parent, 'reply', payload.ticket_id);
    appendField(parent, FIELD_LABELS.content, payload.content);
  }

  function renderAttachCard(parent, payload) {
    appendKind(parent, 'attach a file', payload.ticket_id);
    appendField(parent, FIELD_LABELS.filename, payload.filename);
    const meta = document.createElement('div');
    meta.className = 'support-review-meta';
    appendPill(meta, FIELD_LABELS.content_type, payload.content_type);
    appendPill(meta, FIELD_LABELS.byte_size, formatSize(payload.byte_size));
    parent.appendChild(meta);
    const note = document.createElement('div');
    note.className = 'support-review-attach-note';
    note.textContent = 'the contents of this file go to solstone support. nothing else leaves this machine.';
    parent.appendChild(note);
  }

  function renderReview(attempt) {
    const body = document.getElementById('support-review-body');
    const send = document.getElementById('support-review-send');
    if (!body) return;
    body.replaceChildren();
    body.removeAttribute('data-rendered');
    if (send) send.disabled = true;
    attempt.items.forEach(function(item) {
      if (item.verb === 'create') renderCreateCard(body, item.payload, attempt.snapshot);
      else if (item.verb === 'feedback') renderFeedbackCard(body, item.payload, attempt.snapshot);
      else if (item.verb === 'reply') renderReplyCard(body, item.payload);
      else if (item.verb === 'attach') renderAttachCard(body, item.payload);
    });
    body.setAttribute('data-rendered', 'true');
    if (send) send.disabled = false;
  }

  async function captureJson(verb, payload, snapshot) {
    const result = await postDraftJson('/app/support/api/draft', {
      verb: verb,
      payload: payload,
      diagnostics_snapshot: snapshot
    });
    if (!result.ok) return result;
    return {ok: true, draftId: result.body.draft_id};
  }

  async function captureAttach(ticketId, file) {
    const form = new FormData();
    form.append('verb', 'attach');
    form.append('ticket_id', String(ticketId));
    form.append('file', file);
    const resp = await fetchWithTimeout('/app/support/api/draft', {method: 'POST', body: form});
    const result = await readJsonResponse(resp);
    if (!result.ok) return result;
    return {ok: true, draftId: result.body.draft_id};
  }

  function failBeforeReview(statusEl, message) {
    reportStatus(statusEl, message, 'error');
    reviewInFlight = false;
    setComposeDisabled(false);
    currentAttempt = null;
    hideReview();
  }

  async function startReview(attempt, deps) {
    deps = deps || {};
    const statusEl = deps.statusEl;
    if (reviewInFlight) return;
    const files = attempt.files || [];
    const fileError = validateFiles(files);
    if (fileError) {
      reportStatus(statusEl, fileError, 'error');
      return;
    }

    reviewInFlight = true;
    setComposeDisabled(true);
    const items = [];
    try {
      let snapshot = null;
      if (attempt.kind === 'create' || attempt.kind === 'feedback') {
        if (reportingEnabled()) {
          snapshot = await fetchDiagnosticsSnapshot();
        } else {
          snapshot = null;
        }
      }

      if (attempt.kind === 'create' || attempt.kind === 'feedback' || attempt.kind === 'reply') {
        const verb = attempt.kind === 'reply' ? 'reply' : attempt.kind;
        const captureSnapshot = (attempt.kind === 'create' || attempt.kind === 'feedback') ? snapshot : null;
        const captured = await captureJson(verb, attempt.payload, captureSnapshot);
        if (!captured.ok) {
          failBeforeReview(statusEl, captured.error);
          return;
        }
        items.push({
          draftId: captured.draftId,
          verb: verb,
          payload: attempt.payload,
          filename: null,
          confirmed: false
        });
      }

      for (let i = 0; i < files.length; i++) {
        const file = files[i];
        const captured = await captureAttach(attempt.ticketId, file);
        if (!captured.ok) {
          try {
            await cancelItems(items);
          } catch (cancelErr) { /* still release the guard */ }
          failBeforeReview(statusEl, captured.error);
          return;
        }
        const suffix = suffixOf(file.name);
        items.push({
          draftId: captured.draftId,
          verb: 'attach',
          payload: {
            ticket_id: attempt.ticketId,
            filename: file.name,
            content_type: contentTypeFor(suffix),
            byte_size: file.size
          },
          filename: file.name,
          confirmed: false
        });
      }

      currentAttempt = {
        kind: attempt.kind,
        composeStatusEl: statusEl,
        snapshot: (attempt.kind === 'create' || attempt.kind === 'feedback') ? snapshot : null,
        items: items,
        ticketId: attempt.ticketId
      };
      renderReview(currentAttempt);
      showReview();
    } catch (err) {
      try {
        await cancelItems(items);
      } catch (cancelErr) { /* still release the guard */ }
      failBeforeReview(statusEl, (err && err.message) || "couldn't capture");
    }
  }

  async function confirmReview() {
    if (!currentAttempt || !currentAttempt.items.length) return;
    const body = document.getElementById('support-review-body');
    if (!body || body.getAttribute('data-rendered') !== 'true') return;
    const sendBtn = document.getElementById('support-review-send');
    const discardBtn = document.getElementById('support-review-discard');
    const reviewStatus = document.getElementById('support-review-status');
    if (sendBtn) sendBtn.disabled = true;
    if (discardBtn) discardBtn.disabled = true;

    for (let i = 0; i < currentAttempt.items.length; i++) {
      const item = currentAttempt.items[i];
      if (item.confirmed) continue;
      let result;
      try {
        result = await postDraftJson('/app/support/api/draft/confirm', {draft_id: item.draftId});
      } catch (err) {
        reportStatus(reviewStatus, (err && err.message) || "couldn't send", 'error');
        if (sendBtn) sendBtn.disabled = false;
        if (discardBtn) discardBtn.disabled = false;
        return;
      }
      if (!result.ok) {
        const replyLeft = currentAttempt.items.some(function(entry) {
          return entry.verb === 'reply' && entry.confirmed;
        });
        if (replyLeft && item.verb === 'attach') {
          reportStatus(reviewStatus, 'the reply left. ' + item.filename + ' did not.', 'error');
        } else {
          reportStatus(reviewStatus, result.error, 'error');
        }
        if (sendBtn) sendBtn.disabled = false;
        if (discardBtn) discardBtn.disabled = false;
        return;
      }
      const outcome = result.body.outcome;
      if (outcome === 'not_found') {
        const leftover = currentAttempt.items.filter(function(entry) { return !entry.confirmed; });
        const hadConfirmed = currentAttempt.items.some(function(entry) { return entry.confirmed; });
        try {
          await cancelItems(leftover);
        } catch (cancelErr) { /* still finish */ }
        if (!hadConfirmed) {
          finishAttempt('that draft is gone. nothing was sent.');
          return;
        }
        const names = leftover.map(function(entry) { return entry.filename; }).filter(Boolean);
        const didNot = names.length ? names.join(', ') : 'that file';
        finishAttempt('the reply left. ' + didNot + ' did not.', 'error');
        return;
      }
      item.confirmed = true;
    }
    finishAttempt('sent to solstone support');
  }

  async function discardReview() {
    if (!currentAttempt) return;
    const sendBtn = document.getElementById('support-review-send');
    const discardBtn = document.getElementById('support-review-discard');
    if (sendBtn) sendBtn.disabled = true;
    if (discardBtn) discardBtn.disabled = true;
    const hadConfirmed = currentAttempt.items.some(function(item) { return item.confirmed; });
    const leftover = currentAttempt.items.filter(function(item) { return !item.confirmed; });
    const reviewStatus = document.getElementById('support-review-status');
    const partial = hadConfirmed && reviewStatus ? reviewStatus.textContent : '';
    let last = {ok: true, body: {}};
    try {
      last = await cancelItems(leftover);
    } catch (err) {
      last = {ok: false, body: {}, error: err && err.message};
    }
    if (hadConfirmed) {
      finishAttempt(partial || 'sent to solstone support', partial ? 'error' : 'success');
      return;
    }
    if (last.ok && last.body && last.body.outcome === 'not_found') {
      finishAttempt('that draft is gone. nothing was sent.');
      return;
    }
    finishAttempt('discarded. nothing was sent.');
  }

  function readEmail(anonymousEl, emailInput, emailError) {
    if (anonymousEl.checked) return {ok: true, email: null};
    const email = (emailInput.value || '').trim();
    if (!email) {
      emailError.textContent = 'enter an email or check submit anonymously';
      emailInput.focus();
      return {ok: false};
    }
    if (!emailInput.checkValidity()) {
      emailError.textContent = "that doesn't look like a valid email";
      emailInput.focus();
      return {ok: false};
    }
    return {ok: true, email: email};
  }

  function bindAnonymousToggle(anonymousEl, emailRow, emailInput, emailError) {
    if (!anonymousEl) return;
    anonymousEl.addEventListener('change', function() {
      if (anonymousEl.checked) {
        emailRow.hidden = true;
        emailInput.value = '';
        emailError.textContent = '';
      } else {
        emailRow.hidden = false;
        emailInput.focus();
      }
    });
  }

  function bindCtrlEnter(form) {
    if (!form) return;
    form.addEventListener('keydown', function(e) {
      if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
        e.preventDefault();
        form.requestSubmit();
      }
    });
  }

  function bindCreateForm() {
    const form = document.getElementById('create-form');
    if (!form) return;
    const subjectEl = document.getElementById('create-subject');
    const descriptionEl = document.getElementById('create-description');
    const subjectError = document.getElementById('create-subject-error');
    const descriptionError = document.getElementById('create-description-error');
    const severityEl = document.getElementById('create-severity');
    const categoryEl = document.getElementById('create-category');
    const anonymousEl = document.getElementById('create-anonymous');
    const emailRow = document.getElementById('create-email-row');
    const emailInput = document.getElementById('create-email');
    const emailError = document.getElementById('create-email-error');
    bindAnonymousToggle(anonymousEl, emailRow, emailInput, emailError);
    form.addEventListener('submit', function(e) {
      e.preventDefault();
      const subject = (subjectEl.value || '').trim();
      const description = (descriptionEl.value || '').trim();
      if (!subject) {
        subjectError.style.display = 'block';
        subjectEl.focus();
        return;
      }
      subjectError.style.display = 'none';
      if (!description) {
        descriptionError.style.display = 'block';
        descriptionEl.focus();
        return;
      }
      descriptionError.style.display = 'none';
      emailError.textContent = '';
      const email = readEmail(anonymousEl, emailInput, emailError);
      if (!email.ok) return;
      const payload = {
        subject: subject,
        description: description,
        product: 'solstone',
        severity: (severityEl.value || '').trim() || 'medium',
        anonymous: anonymousEl.checked
      };
      const category = (categoryEl.value || '').trim();
      if (category) payload.category = category;
      if (!anonymousEl.checked) payload.user_email = email.email;
      startReview({kind: 'create', payload: payload}, {statusEl: document.getElementById('create-status')});
    });
    subjectEl.addEventListener('input', function() { subjectError.style.display = 'none'; });
    descriptionEl.addEventListener('input', function() { descriptionError.style.display = 'none'; });
    bindCtrlEnter(form);
  }

  function bindFeedbackForm() {
    const form = document.getElementById('feedback-form');
    if (!form) return;
    const textEl = document.getElementById('feedback-text');
    const errorEl = document.getElementById('feedback-error');
    const anonymousEl = document.getElementById('feedback-anonymous');
    const emailRow = document.getElementById('feedback-email-row');
    const emailInput = document.getElementById('feedback-email');
    const emailError = document.getElementById('feedback-email-error');
    bindAnonymousToggle(anonymousEl, emailRow, emailInput, emailError);
    form.addEventListener('submit', function(e) {
      e.preventDefault();
      const text = (textEl.value || '').trim();
      if (!text) {
        errorEl.textContent = 'please write something';
        errorEl.style.display = 'block';
        textEl.focus();
        return;
      }
      errorEl.style.display = 'none';
      emailError.textContent = '';
      const email = readEmail(anonymousEl, emailInput, emailError);
      if (!email.ok) return;
      const payload = {body: text, product: 'solstone', anonymous: anonymousEl.checked};
      if (!anonymousEl.checked) payload.user_email = email.email;
      startReview({kind: 'feedback', payload: payload}, {statusEl: document.getElementById('feedback-status')});
    });
    textEl.addEventListener('input', function() { errorEl.style.display = 'none'; });
    emailInput.addEventListener('input', function() { emailError.textContent = ''; });
    bindCtrlEnter(form);
  }

  function renderPendingFiles() {
    const fileList = document.getElementById('attach-file-list');
    const attachOnlyBtn = document.getElementById('attach-only-submit');
    if (!fileList) return;
    if (!pendingReplyFiles.length) {
      fileList.innerHTML = '';
      if (attachOnlyBtn) attachOnlyBtn.style.display = 'none';
      return;
    }
    if (attachOnlyBtn) attachOnlyBtn.style.display = '';
    fileList.innerHTML = pendingReplyFiles.map(function(file, i) {
      return '<div class="support-file-entry"><span>\u{1F4CE} ' + esc(file.name) +
        ' (' + formatSize(file.size) + ')</span><button class="remove-file" data-idx="' + i +
        '" type="button">\u00d7</button></div>';
    }).join('');
    fileList.querySelectorAll('.remove-file').forEach(function(btn) {
      btn.addEventListener('click', function() {
        pendingReplyFiles.splice(parseInt(btn.dataset.idx, 10), 1);
        renderPendingFiles();
      });
    });
  }

  function addPendingFiles(newFiles, statusEl) {
    const incoming = Array.prototype.slice.call(newFiles || []);
    for (let i = 0; i < incoming.length; i++) {
      const file = incoming[i];
      const probe = pendingReplyFiles.concat([file]);
      const error = validateFiles(probe);
      if (error) {
        reportStatus(statusEl, error, 'error');
        if (file.size > MAX_ATTACHMENT_BYTES || ALLOWED_SUFFIXES.indexOf(suffixOf(file.name)) === -1) {
          continue;
        }
        break;
      }
      if (!pendingReplyFiles.some(function(existing) {
        return existing.name === file.name && existing.size === file.size;
      })) {
        pendingReplyFiles.push(file);
      }
    }
    renderPendingFiles();
  }

  function bindReplyForm(ticketId, deps) {
    deps = deps || {};
    pendingReplyFiles = [];
    replyOnSubmitted = typeof deps.onSubmitted === 'function' ? deps.onSubmitted : null;
    const form = document.getElementById('reply-form');
    if (!form) return;
    const textEl = document.getElementById('reply-text');
    const errorEl = document.getElementById('reply-error');
    const zone = document.getElementById('attach-zone');
    const fileInput = document.getElementById('attach-input');
    const attachOnlyBtn = document.getElementById('attach-only-submit');
    const statusEl = document.getElementById('reply-status');

    if (fileInput) fileInput.accept = ALLOWED_SUFFIXES.join(',');

    function beginReply(kind) {
      const text = (textEl.value || '').trim();
      const files = pendingReplyFiles.slice();
      if (kind === 'reply' && !text && !files.length) {
        errorEl.style.display = 'block';
        textEl.focus();
        return;
      }
      if (kind === 'files-only' && !files.length) return;
      errorEl.style.display = 'none';
      const attempt = kind === 'files-only' || (!text && files.length)
        ? {kind: 'files-only', ticketId: ticketId, files: files}
        : {kind: 'reply', ticketId: ticketId, payload: {ticket_id: ticketId, content: text}, files: files};
      startReview(attempt, {statusEl: statusEl});
    }

    form.addEventListener('submit', function(e) {
      e.preventDefault();
      beginReply('reply');
    });
    if (textEl) {
      textEl.addEventListener('input', function() { errorEl.style.display = 'none'; });
      bindCtrlEnter(form);
    }
    if (attachOnlyBtn) {
      attachOnlyBtn.addEventListener('click', function() {
        beginReply('files-only');
      });
    }
    if (zone) {
      zone.addEventListener('click', function() { if (fileInput) fileInput.click(); });
      zone.addEventListener('keydown', function(e) {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          if (fileInput) fileInput.click();
        }
      });
      zone.addEventListener('dragover', function(e) { e.preventDefault(); zone.classList.add('dragover'); });
      zone.addEventListener('dragleave', function() { zone.classList.remove('dragover'); });
      zone.addEventListener('drop', function(e) {
        e.preventDefault();
        zone.classList.remove('dragover');
        if (e.dataTransfer.files.length) addPendingFiles(e.dataTransfer.files, statusEl);
      });
    }
    if (fileInput) {
      fileInput.addEventListener('change', function() {
        if (fileInput.files.length) addPendingFiles(fileInput.files, statusEl);
        fileInput.value = '';
      });
    }
    renderPendingFiles();
  }

  function runDiagnostics() {
    const btn = document.getElementById('diagnostics-btn');
    const loading = document.getElementById('diagnostics-loading');
    const results = document.getElementById('diagnostics-results');
    const errorEl = document.getElementById('diagnostics-error');

    btn.disabled = true;
    results.style.display = 'none';
    results.innerHTML = '';
    errorEl.style.display = 'none';
    errorEl.innerHTML = '';
    loading.style.display = '';

    fetchWithTimeout('/app/support/api/diagnostics')
      .then(function(r) {
        if (!r.ok) throw new Error('HTTP ' + r.status);
        return r.json();
      })
      .then(function(data) {
        loading.style.display = 'none';
        renderDiagnostics(data, results);
        results.style.display = '';
        btn.disabled = false;
      })
      .catch(function(err) {
        loading.style.display = 'none';
        errorEl.innerHTML = 'couldn\'t run diagnostics: ' + esc(err.message) +
          '<br><span style="color:#666;">try again in a moment.</span>';
        errorEl.style.display = '';
        btn.disabled = false;
      });
  }

  function renderDiagnostics(data, container) {
    var html = '';

    var services = data.services || {};
    var serviceNames = Object.keys(services);
    var runningCount = serviceNames.filter(function(k) { return services[k] === 'running'; }).length;
    var errorCount = (data.recent_errors || []).length;
    html += '<div class="support-diagnostics-summary">' +
      esc(runningCount + ' of ' + serviceNames.length + ' services running') +
      (errorCount > 0 ? ', ' + esc(errorCount + ' recent error' + (errorCount !== 1 ? 's' : '')) : ', no recent errors') +
      '</div>';

    var versionPlatformRows = '';
    if (data.version) {
      versionPlatformRows += '<div class="support-diagnostics-row"><span class="support-diagnostics-label">version</span><span>' + esc(String(data.version)) + '</span></div>';
    }
    if (data.platform) {
      var p = data.platform;
      if (p.system) versionPlatformRows += '<div class="support-diagnostics-row"><span class="support-diagnostics-label">system</span><span>' + esc(String(p.system)) + ' ' + esc(String(p.release || '')) + '</span></div>';
      if (p.machine) versionPlatformRows += '<div class="support-diagnostics-row"><span class="support-diagnostics-label">machine</span><span>' + esc(String(p.machine)) + '</span></div>';
      if (p.python) versionPlatformRows += '<div class="support-diagnostics-row"><span class="support-diagnostics-label">python</span><span>' + esc(String(p.python)) + '</span></div>';
    }
    if (versionPlatformRows) {
      html += '<details class="support-diagnostics-section"><summary>version &amp; platform</summary>' +
        '<div class="support-diagnostics-body">' + versionPlatformRows + '</div></details>';
    }

    if (serviceNames.length > 0) {
      var svcRows = '';
      serviceNames.forEach(function(name) {
        var status = services[name] || 'unknown';
        var dotClass = status === 'running' ? 'support-diagnostics-running' : status === 'stopped' ? 'support-diagnostics-stopped' : 'support-diagnostics-unknown';
        svcRows += '<div class="support-diagnostics-row"><span class="support-diagnostics-label">' + esc(String(name)) + '</span>' +
          '<span><span class="support-diagnostics-dot ' + dotClass + '"></span>' + esc(String(status)) + '</span></div>';
      });
      html += '<details class="support-diagnostics-section"><summary>services</summary>' +
        '<div class="support-diagnostics-body">' + svcRows + '</div></details>';
    }

    if (data.recent_errors && data.recent_errors.length > 0) {
      var errRows = '';
      data.recent_errors.forEach(function(e) {
        var t = e.time ? (e.time_approximate ? '~' + e.time : e.time) : '';
        errRows += '<div style="padding:0.2rem 0;">' +
          (t ? '<span style="color:#888;">' + esc(String(t)) + '</span> ' : '') +
          '<strong>' + esc(String(e.service || 'unknown')) + '</strong>: ' +
          esc(String(e.message || '')) + '</div>';
      });
      html += '<details class="support-diagnostics-section" open><summary>recent errors (' + esc(String(data.recent_errors.length)) + ')</summary>' +
        '<div class="support-diagnostics-body">' + errRows + '</div></details>';
    }

    if (data.config && Object.keys(data.config).length > 0) {
      var cfgRows = '';
      Object.keys(data.config).forEach(function(key) {
        cfgRows += '<div class="support-diagnostics-row"><span class="support-diagnostics-label">' + esc(String(key)) + '</span>' +
          '<span>' + esc(String(data.config[key])) + '</span></div>';
      });
      html += '<details class="support-diagnostics-section"><summary>config</summary>' +
        '<div class="support-diagnostics-body">' + cfgRows + '</div></details>';
    }

    if (!versionPlatformRows && serviceNames.length === 0 && (!data.recent_errors || data.recent_errors.length === 0) && (!data.config || Object.keys(data.config).length === 0)) {
      html += '<div style="font-size:0.85rem;color:#666;">no diagnostic data available.</div>';
    }

    container.innerHTML = html;
  }

  async function loadTickets(deps) {
    if (deps) ticketListDeps = deps;
    const resolved = deps || ticketListDeps || {openTicket: function() {}, activateTab: function() {}};
    const list = document.getElementById('tickets-list');
    list.innerHTML = window.SurfaceState.loading({ text: 'checking for tickets' });
    try {
      const resp = await fetchWithTimeout('/app/support/api/tickets');
      if (!resp.ok) {
        if (resp.status === 403) {
          document.getElementById('support-main').style.display = 'none';
          document.getElementById('support-disabled').style.display = '';
          return;
        }
        throw new Error('Failed to load tickets');
      }
      const tickets = await resp.json();

      if (!tickets.length) {
        list.innerHTML = window.SurfaceState.empty({
          icon: window.ConveyIcons.svg('life-buoy'),
          heading: 'no tickets yet. that\'s a good thing',
          desc: 'start a support request on this tab if something comes up'
        });
        return;
      }

      list.innerHTML = tickets.map(t => {
        const ticketId = String(t.id || '');
        const statusClass = 'support-status-' + (t.status || 'open').replace(/[^a-z-]/g, '');
        const createdAt = t.created_at
          ? `${window.relativeTime(Date.now() - new Date(t.created_at + (t.created_at.includes('Z') ? '' : 'Z')).getTime())} ago`
          : '';
        return `<div class="support-ticket" data-id="${attr(ticketId)}" tabindex="0" role="button">
          <div class="support-ticket-header">
            <span class="support-ticket-subject">${esc(t.subject || 'untitled')}</span>
            <span class="support-status ${statusClass}">${esc(t.status || 'open')}</span>
          </div>
          <div class="support-ticket-meta">
            <span class="support-ticket-id">#${esc(ticketId)}</span> &middot;
            ${esc(t.product || '')} &middot;
            ${createdAt}
          </div>
        </div>`;
      }).join('');

      list.querySelectorAll('.support-ticket').forEach(el => {
        el.addEventListener('click', () => resolved.openTicket(parseInt(el.dataset.id)));
      });
      list.querySelectorAll('.support-ticket').forEach(el => {
        el.addEventListener('keydown', e => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            resolved.openTicket(parseInt(el.dataset.id));
          }
        });
      });
      const count = tickets.length;
      let badge = document.getElementById('tab-tickets-badge');
      if (!badge) {
        badge = document.createElement('span');
        badge.id = 'tab-tickets-badge';
        badge.className = 'support-tab-badge';
        document.getElementById('tab-tickets').appendChild(badge);
      }
      badge.textContent = count;
      const activeTab = document.querySelector('.support-nav button.active');
      const show = activeTab && activeTab.dataset.section !== 'tickets';
      badge.style.opacity = show ? '1' : '0';
      badge.style.pointerEvents = show ? 'auto' : 'none';
    } catch (e) {
      list.innerHTML = window.SurfaceState.error({
        heading: 'unable to load tickets',
        desc: 'check your connection and try refreshing',
        retry: true,
        retryLabel: 'try again',
        detail: e
      });
      const retryBtn = list.querySelector('.surface-state-retry');
      if (retryBtn) retryBtn.addEventListener('click', () => loadTickets(resolved));
    }
  }

  document.addEventListener('click', function(e) {
    const target = e.target instanceof Element ? e.target : null;
    if (!target) return;
    if (target.closest('#support-review-send')) {
      e.preventDefault();
      confirmReview();
    } else if (target.closest('#support-review-discard')) {
      e.preventDefault();
      discardReview();
    }
  });

  window.SupportUI = {
    REQUEST_TIMEOUT_MS,
    fetchWithTimeout,
    runDiagnostics,
    renderDiagnostics,
    loadTickets,
    __setTimers,
    startReview,
    bindCreateForm,
    bindFeedbackForm,
    bindReplyForm,
    confirmReview,
    discardReview,
    isReviewInFlight,
    esc,
    formatSize,
    ALLOWED_SUFFIXES
  };
})();
