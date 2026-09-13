// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function() {
  const SUPPORT_URL = 'https://support.solstone.app/';
  const RECENT_LINE_LIMIT = 5;
  const RECENT_CHARACTER_LIMIT = 4000;

  window.convey = window.convey || {};

  let currentModal = null;
  let keydownHandler = null;

  function copyText(key, fallback) {
    return window.CONVEY_COPY?.[key] || fallback;
  }

  function closeModal() {
    if (keydownHandler) {
      document.removeEventListener('keydown', keydownHandler, true);
      keydownHandler = null;
    }
    currentModal?.remove();
    currentModal = null;
  }

  function currentAppAndRoute(context) {
    const path = window.solPathContext?.() || {};
    return {
      app: String(context.app || path.appName || 'journal'),
      route: String(context.route || window.location.pathname),
    };
  }

  function recentErrorLines(context) {
    const lines = (Array.isArray(context.consoleEntries) ? context.consoleEntries : [])
      .map(entry => String(entry?.detail?.message || '').trim())
      .filter(Boolean)
      .slice(-RECENT_LINE_LIMIT);
    if (lines.length === 0 && context.apiError?.message) {
      lines.push(String(context.apiError.message).trim());
    }
    if (lines.length === 0 && context.heading) {
      lines.push(String(context.heading).trim());
    }
    return lines.join('\n').slice(0, RECENT_CHARACTER_LIMIT);
  }

  async function fixedContext() {
    const response = await fetch('/app/support/api/context', {
      headers: { Accept: 'application/json' },
      cache: 'no-store',
    });
    if (!response.ok) throw new Error('local support context unavailable');
    const value = await response.json();
    return {
      version: String(value.version || ''),
      os: String(value.os || ''),
      osVersion: String(value.os_version || ''),
    };
  }

  function row(label, value) {
    const wrapper = document.createElement('div');
    const term = document.createElement('dt');
    const detail = document.createElement('dd');
    term.textContent = label;
    detail.textContent = value || '—';
    wrapper.append(term, detail);
    return wrapper;
  }

  function destination(fields, recent) {
    const fragment = new URLSearchParams();
    fragment.set('report', 'v1');
    fragment.set('app', fields.app);
    fragment.set('version', fields.version);
    fragment.set('os', fields.os);
    fragment.set('os_version', fields.osVersion);
    fragment.set('route', fields.route);
    if (fields.errorCode) fragment.set('error_code', fields.errorCode);
    if (recent) fragment.set('recent', recent);
    return `${SUPPORT_URL}#${fragment.toString()}`;
  }

  async function openModal(context) {
    closeModal();
    const platform = await fixedContext();
    const location = currentAppAndRoute(context);
    const fields = {
      ...platform,
      ...location,
      errorCode: String(context.apiError?.reasonCode || context.apiError?.status || ''),
    };

    const modal = document.createElement('div');
    modal.className = 'modal report-error-modal';
    modal.style.display = 'block';
    modal.setAttribute('role', 'dialog');
    modal.setAttribute('aria-modal', 'true');
    modal.setAttribute('aria-labelledby', 'report-error-title');

    const content = document.createElement('div');
    content.className = 'modal-content';
    modal.appendChild(content);

    const title = document.createElement('h2');
    title.id = 'report-error-title';
    title.textContent = copyText('REPORT_TITLE', 'review this report');
    content.appendChild(title);

    const note = document.createElement('p');
    note.textContent = copyText(
      'REPORT_PRIVACY_NOTE',
      'nothing leaves this journal until you send the form on the support website.'
    );
    content.appendChild(note);

    const review = document.createElement('dl');
    review.className = 'report-error-review';
    review.append(
      row(copyText('REPORT_VERSION_LABEL', 'journal version'), fields.version),
      row(copyText('REPORT_PLATFORM_LABEL', 'operating system'), `${fields.os} ${fields.osVersion}`.trim()),
      row(copyText('REPORT_APP_LABEL', 'app'), fields.app),
      row(copyText('REPORT_ROUTE_LABEL', 'route'), fields.route)
    );
    if (fields.errorCode) {
      review.append(row(copyText('REPORT_ERROR_CODE_LABEL', 'error code'), fields.errorCode));
    }
    content.appendChild(review);

    const recentLabel = document.createElement('label');
    recentLabel.setAttribute('for', 'report-error-recent');
    recentLabel.textContent = copyText('REPORT_RECENT_LABEL', 'recent error lines');
    const recent = document.createElement('textarea');
    recent.id = 'report-error-recent';
    recent.className = 'report-error-detail';
    recent.maxLength = RECENT_CHARACTER_LIMIT;
    recent.value = recentErrorLines(context);
    content.append(recentLabel, recent);

    const actions = document.createElement('div');
    actions.className = 'report-error-actions';
    const cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.textContent = copyText('REPORT_ACTION_CANCEL', 'cancel');
    const send = document.createElement('a');
    send.textContent = copyText('REPORT_ACTION_SEND', 'continue to support');
    send.href = destination(fields, recent.value.trim());
    send.target = '_blank';
    send.rel = 'noopener noreferrer';
    actions.append(cancel, send);
    content.appendChild(actions);

    cancel.addEventListener('click', closeModal);
    recent.addEventListener('input', () => {
      send.href = destination(fields, recent.value.trim());
    });
    modal.addEventListener('click', event => {
      if (event.target === modal) closeModal();
    });
    keydownHandler = event => {
      if (event.key === 'Escape') {
        event.preventDefault();
        closeModal();
      }
    };
    document.addEventListener('keydown', keydownHandler, true);
    document.body.appendChild(modal);
    currentModal = modal;
    recent.focus();
  }

  window.convey.reportError = function(context) {
    openModal(context || {}).catch(error => {
      window.logError?.(error, { context: 'local support report context' });
    });
  };
})();
