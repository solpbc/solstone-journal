// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function() {
  const STORAGE_KEY = 'solstone:diagnostic-console';
  const CONSOLE_MAX_ENTRIES = 200;
  const STORAGE_VERSION = 1;

  window.convey = window.convey || {};

  let entries = [];
  let unreadCursor = 0;
  let idCounter = 0;
  let flushVersion = 0;
  let activeFilter = 'all';
  let drawerOpen = false;
  let drawerBound = false;

  function copyText(key, fallback) {
    return window.CONVEY_COPY?.[key] || fallback || key;
  }

  function reportingEnabled() {
    return !(window.CONVEY_SETTINGS && window.CONVEY_SETTINGS.reportingEnabled === false);
  }

  function escapeHtml(value) {
    return String(value ?? '')
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function errorToPlain(error, seen) {
    const plain = {
      message: error.message || String(error),
      stack: error.stack || ''
    };
    Object.keys(error).forEach(key => {
      const value = normalizeForJson(error[key], seen);
      if (value !== undefined) {
        plain[key] = value;
      }
    });
    return roundTrip(plain);
  }

  function roundTrip(value) {
    try {
      const json = JSON.stringify(value);
      if (json === undefined) {
        return undefined;
      }
      return JSON.parse(json);
    } catch (_) {
      return undefined;
    }
  }

  function normalizeForJson(value, seen = new WeakSet()) {
    if (value instanceof Error) {
      return errorToPlain(value, seen);
    }
    if (value && typeof value === 'object') {
      if (seen.has(value)) {
        return undefined;
      }
      seen.add(value);
      if (Array.isArray(value)) {
        const items = value
          .map(item => normalizeForJson(item, seen))
          .filter(item => item !== undefined);
        seen.delete(value);
        return roundTrip(items);
      }
      const plain = {};
      Object.keys(value).forEach(key => {
        const item = normalizeForJson(value[key], seen);
        if (item !== undefined) {
          plain[key] = item;
        }
      });
      seen.delete(value);
      return roundTrip(plain);
    }
    return roundTrip(value);
  }

  function safeContext(value) {
    if (value && typeof value === 'object' && !(value instanceof Error) && !Array.isArray(value)) {
      const plain = {};
      Object.keys(value).forEach(key => {
        const item = value[key] instanceof Error
          ? errorToPlain(value[key], new WeakSet())
          : roundTrip(value[key]);
        if (item !== undefined) {
          plain[key] = item;
        }
      });
      return plain;
    }
    const normalized = normalizeForJson(value);
    return normalized === undefined ? {} : normalized;
  }

  function cloneEntry(entry) {
    return safeContext(entry);
  }

  function normalizeSeverity(value) {
    return ['error', 'warning', 'info'].includes(value) ? value : 'error';
  }

  function normalizeEntry(entry) {
    const source = typeof entry?.source === 'string' && entry.source.trim()
      ? entry.source.trim()
      : 'manual';
    const ts = Number.isFinite(Number(entry?.ts)) ? Number(entry.ts) : Date.now();
    const id = typeof entry?.id === 'string' && entry.id.trim()
      ? entry.id.trim()
      : `dc-${ts}-${idCounter++}`;
    return {
      id,
      ts,
      severity: normalizeSeverity(entry?.severity || entry?.category),
      source,
      summary: String(entry?.summary || copyText('CONSOLE_HEADING', 'system messages')),
      detail: safeContext(entry?.detail || {})
    };
  }

  function storageWrite(payload) {
    try {
      window.sessionStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
    } catch (_) {
      // Session diagnostics are best-effort; storage failures must not recurse into logError.
    }
  }

  function storageRemove() {
    try {
      window.sessionStorage.removeItem(STORAGE_KEY);
    } catch (_) {
      // Best-effort cleanup only.
    }
  }

  function loadEntries() {
    try {
      const raw = window.sessionStorage.getItem(STORAGE_KEY);
      if (!raw) {
        return;
      }
      const parsed = JSON.parse(raw);
      if (!parsed || parsed.version !== STORAGE_VERSION || !Array.isArray(parsed.entries)) {
        entries = [];
        unreadCursor = 0;
        return;
      }
      entries = parsed.entries.map(normalizeEntry).slice(-CONSOLE_MAX_ENTRIES);
      unreadCursor = Math.max(0, Math.min(Number(parsed.unreadCursor) || 0, entries.length));
      idCounter = entries.length;
    } catch (_) {
      entries = [];
      unreadCursor = 0;
    }
  }

  function flushNow() {
    storageWrite({
      version: STORAGE_VERSION,
      unreadCursor,
      entries
    });
  }

  function scheduleFlush() {
    flushVersion += 1;
    const version = flushVersion;
    Promise.resolve().then(() => {
      if (version === flushVersion) {
        flushNow();
      }
    });
  }

  function dispatchUpdate() {
    try {
      window.dispatchEvent(new CustomEvent('diagnostic-console-updated'));
    } catch (_) {
      // Event dispatch is advisory.
    }
  }

  function countBySeverity(severity) {
    if (severity === 'all') {
      return entries.length;
    }
    return entries.filter(entry => entry.severity === severity).length;
  }

  function filteredEntries(filter = {}) {
    const severity = filter.severity;
    const source = filter.source;
    return entries.filter(entry => {
      if (severity && severity !== 'all' && entry.severity !== severity) {
        return false;
      }
      if (source && entry.source !== source) {
        return false;
      }
      return true;
    });
  }

  function formatTime(ts) {
    const date = new Date(ts);
    if (Number.isNaN(date.getTime())) {
      return '';
    }
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }

  function formatDateTime(ts) {
    const date = new Date(ts);
    if (Number.isNaN(date.getTime())) {
      return '';
    }
    return date.toLocaleString();
  }

  function severityIcon(severity) {
    if (severity === 'warning') {
      return '!';
    }
    if (severity === 'info') {
      return 'i';
    }
    return 'x';
  }

  function formatValue(value) {
    if (value === undefined || value === null || value === '') {
      return '';
    }
    if (typeof value === 'object') {
      try {
        return JSON.stringify(value);
      } catch (_) {
        return String(value);
      }
    }
    return String(value);
  }

  function detailRow(label, value) {
    const formatted = formatValue(value);
    if (!formatted) {
      return '';
    }
    return `<div><span>${escapeHtml(label)}:</span> ${escapeHtml(formatted)}</div>`;
  }

  function renderApiDetail(apiError) {
    const lines = [];
    if (apiError.status || apiError.statusText || apiError.url) {
      lines.push(`<div>HTTP ${escapeHtml(apiError.status || '')} ${escapeHtml(apiError.statusText || '')} · ${escapeHtml(apiError.url || '')}</div>`);
    }
    lines.push(detailRow('Server reason', apiError.rawDetail || apiError.serverMessage));
    lines.push(detailRow('time', formatDateTime(apiError.timestamp)));
    lines.push(detailRow('reference', apiError.correlationId));
    lines.push(detailRow('reason code', apiError.reasonCode));
    return lines.filter(Boolean).join('');
  }

  function renderJsDetail(detail) {
    const messageLabel = copyText('CONSOLE_DETAIL_MESSAGE_LABEL', 'message');
    const stackLabel = copyText('CONSOLE_DETAIL_STACK_LABEL', 'stack');
    const locationLabel = copyText('CONSOLE_DETAIL_LOCATION_LABEL', 'location');
    const lines = [];
    lines.push(detailRow(messageLabel, detail.message));
    if (detail.filename || detail.lineno || detail.colno) {
      const location = `${detail.filename || ''}${detail.lineno ? `:${detail.lineno}` : ''}${detail.colno ? `:${detail.colno}` : ''}`;
      lines.push(detailRow(locationLabel, location));
    }
    if (detail.context && Object.keys(detail.context).length) {
      lines.push(detailRow('context', detail.context));
    }
    if (detail.stack) {
      lines.push(`<div><span>${escapeHtml(stackLabel)}:</span><pre>${escapeHtml(detail.stack)}</pre></div>`);
    }
    return lines.filter(Boolean).join('');
  }

  function renderEntryDetail(entry) {
    const detail = entry.detail || {};
    if (detail.apiError) {
      return renderApiDetail(detail.apiError);
    }
    return renderJsDetail(detail);
  }

  function renderEntry(entry) {
    const sendButton = reportingEnabled()
      ? `<button type="button" data-diagnostic-send="${escapeHtml(entry.id)}">${escapeHtml(copyText('CONSOLE_ACTION_SEND', 'Send'))}</button>`
      : '';
    return `<details class="diagnostic-console-entry" data-entry-id="${escapeHtml(entry.id)}">
      <summary>
        <span class="diagnostic-console-entry-time">${escapeHtml(formatTime(entry.ts))}</span>
        <span class="diagnostic-console-entry-icon diagnostic-console-entry-icon--${escapeHtml(entry.severity)}">${escapeHtml(severityIcon(entry.severity))}</span>
        <span class="diagnostic-console-entry-source">${escapeHtml(entry.source)}</span>
        <span class="diagnostic-console-entry-summary">${escapeHtml(entry.summary)}</span>
        <span class="diagnostic-console-entry-toggle">${escapeHtml(copyText('CONSOLE_ACTION_SHOW_DETAILS', 'show details'))}</span>
        ${sendButton}
      </summary>
      <div class="diagnostic-console-entry-detail">${renderEntryDetail(entry)}</div>
    </details>`;
  }

  function visibleEntries() {
    return filteredEntries({ severity: activeFilter === 'all' ? null : activeFilter });
  }

  function renderDrawer() {
    const root = document.getElementById('diagnostic-console');
    if (!root) {
      return;
    }
    const list = document.getElementById('diagnostic-console-list');
    const empty = document.getElementById('diagnostic-console-empty');
    const clear = root.querySelector('[data-diagnostic-action="clear"]');
    const sendAll = root.querySelector('[data-diagnostic-action="send-all"]');
    const reportingOff = root.querySelector('[data-diagnostic-reporting-off]');
    const canReport = reportingEnabled();

    root.querySelectorAll('[data-diagnostic-count]').forEach(node => {
      const severity = node.getAttribute('data-diagnostic-count');
      node.textContent = String(countBySeverity(severity));
    });
    root.querySelectorAll('[data-diagnostic-filter]').forEach(button => {
      button.setAttribute('aria-selected', button.getAttribute('data-diagnostic-filter') === activeFilter ? 'true' : 'false');
    });

    const currentEntries = visibleEntries().slice().reverse();
    if (list) {
      list.innerHTML = currentEntries.map(renderEntry).join('');
    }
    if (empty) {
      empty.hidden = currentEntries.length !== 0;
    }
    // "clear" empties the whole session, not just the visible filter, so it is
    // live whenever anything is held — and dead when nothing is.
    if (clear) {
      clear.disabled = entries.length === 0;
    }
    if (sendAll) {
      sendAll.hidden = !canReport;
      sendAll.disabled = currentEntries.length === 0;
    }
    if (reportingOff) {
      reportingOff.hidden = canReport;
    }
  }

  function openDrawer() {
    const root = document.getElementById('diagnostic-console');
    if (!root) {
      return;
    }
    drawerOpen = true;
    root.classList.add('visible');
    root.setAttribute('aria-hidden', 'false');
    renderDrawer();
    api.markAllRead();
    root.focus();
  }

  function closeDrawer() {
    const root = document.getElementById('diagnostic-console');
    if (!root) {
      return;
    }
    drawerOpen = false;
    root.classList.remove('visible');
    root.setAttribute('aria-hidden', 'true');
  }

  function sendEntry(entryId) {
    if (!reportingEnabled() || !window.convey?.reportError) {
      return;
    }
    const entry = entries.find(item => item.id === entryId);
    if (!entry) {
      return;
    }
    window.convey.reportError({
      source: 'console-entry',
      heading: entry.summary,
      apiError: entry.detail?.apiError || null,
      consoleEntries: api.neighborhood(entry.id, 5),
      customDetail: ''
    });
  }

  function sendAllVisible() {
    if (!reportingEnabled() || !window.convey?.reportError) {
      return;
    }
    const currentEntries = visibleEntries().map(cloneEntry);
    const template = copyText('CONSOLE_SNAPSHOT_HEADING', 'snapshot of {count} system messages');
    window.convey.reportError({
      source: 'manual',
      heading: template.replace('{count}', String(currentEntries.length)),
      apiError: null,
      consoleEntries: currentEntries,
      customDetail: ''
    });
  }

  function bindDrawer() {
    if (drawerBound) {
      return;
    }
    const root = document.getElementById('diagnostic-console');
    if (!root) {
      return;
    }
    drawerBound = true;
    root.addEventListener('click', event => {
      const target = event.target instanceof Element ? event.target : null;
      if (!target) {
        return;
      }
      const action = target.closest('[data-diagnostic-action]')?.getAttribute('data-diagnostic-action');
      if (action === 'close') {
        closeDrawer();
        return;
      }
      if (action === 'clear') {
        api.clear();
        return;
      }
      if (action === 'send-all') {
        sendAllVisible();
        return;
      }
      const filter = target.closest('[data-diagnostic-filter]')?.getAttribute('data-diagnostic-filter');
      if (filter) {
        activeFilter = filter;
        renderDrawer();
        return;
      }
      const sendId = target.closest('[data-diagnostic-send]')?.getAttribute('data-diagnostic-send');
      if (sendId) {
        event.preventDefault();
        event.stopPropagation();
        sendEntry(sendId);
      }
    });

    document.addEventListener('keydown', event => {
      if (event.key === 'Escape' && drawerOpen) {
        closeDrawer();
      }
    });

    document.addEventListener('click', event => {
      const target = event.target instanceof Element ? event.target : null;
      if (
        drawerOpen
        && root
        && target
        && !root.contains(target)
        && !target.closest('#status-pane-console-link')
      ) {
        closeDrawer();
      }
    });
  }

  const api = {
    push(entry) {
      const normalized = normalizeEntry(entry);
      entries.push(normalized);
      if (entries.length > CONSOLE_MAX_ENTRIES) {
        const removed = entries.length - CONSOLE_MAX_ENTRIES;
        entries = entries.slice(removed);
        unreadCursor = Math.max(0, unreadCursor - removed);
      }
      unreadCursor = Math.min(unreadCursor, entries.length);
      scheduleFlush();
      renderDrawer();
      dispatchUpdate();
      return cloneEntry(normalized);
    },

    list(filter = {}) {
      return filteredEntries(filter).map(cloneEntry);
    },

    clear() {
      entries = [];
      unreadCursor = 0;
      flushVersion += 1;
      storageRemove();
      renderDrawer();
      dispatchUpdate();
    },

    markAllRead() {
      unreadCursor = entries.length;
      scheduleFlush();
      dispatchUpdate();
    },

    unreadCount() {
      return Math.max(0, entries.length - unreadCursor);
    },

    neighborhood(entryId, span = 5) {
      const index = entries.findIndex(entry => entry.id === entryId);
      if (index === -1) {
        return [];
      }
      const width = Math.max(0, Number(span) || 0);
      return entries.slice(Math.max(0, index - width), index + width + 1).map(cloneEntry);
    },

    open: openDrawer,
    close: closeDrawer
  };

  window.convey.diagnosticConsole = api;

  loadEntries();

  window.whenShellReady(() => {
    bindDrawer();
    renderDrawer();
    dispatchUpdate();
  });
})();
