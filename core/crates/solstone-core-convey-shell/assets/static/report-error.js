// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function() {
  const MAILTO_LIMIT = 1800;
  const CONSOLE_ENTRY_LIMIT = 20000;
  const SUPPORT_EMAIL = 'support@solstone.app';
  const TICKET_URL = '/app/support/';
  const REPORT_CATEGORY = 'error_report';

  window.convey = window.convey || {};

  let currentModal = null;
  let keydownHandler = null;
  let sending = false;

  function copyText(key) {
    return window.CONVEY_COPY[key];
  }

  // Kept local: SurfaceState formats timestamps for local display; reports need ISO bundle timestamps.
  function hasValue(value) {
    return value !== undefined && value !== null && value !== '';
  }

  function logUnexpected(step, error) {
    if (window.logError) {
      window.logError(error, { context: `report-error: ${step}` });
    }
  }

  function formatTimestamp(timestamp) {
    if (!hasValue(timestamp)) {
      return '';
    }
    const date = new Date(timestamp);
    if (Number.isNaN(date.getTime())) {
      return '';
    }
    return date.toISOString();
  }

  function formatEntryValue(value) {
    if (!hasValue(value)) {
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

  function entryApiError(entry) {
    return entry?.detail?.apiError || null;
  }

  function formatConsoleEntry(entry) {
    const lines = [
      `${formatTimestamp(entry.ts)} · ${entry.severity || 'error'} · ${entry.source || 'unknown'}`,
      `summary: ${entry.summary || ''}`,
    ];
    const apiError = entryApiError(entry);
    if (apiError) {
      const bits = [];
      if (hasValue(apiError.status)) {
        bits.push(`HTTP ${apiError.status}`);
      }
      if (hasValue(apiError.url)) {
        bits.push(apiError.url);
      }
      if (hasValue(apiError.serverMessage)) {
        bits.push(apiError.serverMessage);
      }
      if (bits.length) {
        lines.push(bits.join(' · '));
      }
      if (hasValue(apiError.correlationId)) {
        lines.push(`reference: ${apiError.correlationId}`);
      }
      if (hasValue(apiError.reasonCode)) {
        lines.push(`reason code: ${apiError.reasonCode}`);
      }
    } else {
      const detail = entry?.detail || {};
      if (hasValue(detail.message)) {
        lines.push(`message: ${detail.message}`);
      }
      if (hasValue(detail.filename) || hasValue(detail.lineno)) {
        const location = `${detail.filename || ''}${detail.lineno ? `:${detail.lineno}` : ''}${detail.colno ? `:${detail.colno}` : ''}`;
        lines.push(`location: ${location}`);
      }
      if (hasValue(detail.stack)) {
        lines.push('stack:');
        String(detail.stack).split('\n').slice(0, 6).forEach(line => {
          lines.push(`  ${line}`);
        });
      }
      if (detail.context && Object.keys(detail.context).length) {
        lines.push(`context: ${formatEntryValue(detail.context)}`);
      }
    }
    return lines.slice(0, 10).join('\n');
  }

  function formatConsoleEntryLines(consoleEntries) {
    const entries = Array.isArray(consoleEntries) ? consoleEntries.filter(Boolean) : [];
    if (!entries.length) {
      return [
        copyText('REPORT_DIAGNOSTIC_CONSOLE_HEADING'),
        `  ${copyText('REPORT_NO_CONSOLE')}`,
      ];
    }

    let rendered = entries.map(formatConsoleEntry);
    let truncated = false;
    while (rendered.length > 0 && rendered.join('\n\n').length > CONSOLE_ENTRY_LIMIT) {
      rendered.shift();
      truncated = true;
    }

    const heading = copyText('CONSOLE_REPORT_HEADING').replace('{count}', String(rendered.length));
    const lines = [heading];
    if (truncated) {
      lines.push(copyText('CONSOLE_TRUNCATED'));
    }
    rendered.forEach(block => {
      lines.push('');
      lines.push(block);
    });
    return lines;
  }

  function formatDiagnosticBundle(apiError, consoleEntries) {
    const lines = [copyText('REPORT_DIAGNOSTIC_SUMMARY')];
    if (apiError) {
      if (hasValue(apiError.status)) {
        lines.push(`${copyText('REPORT_DIAGNOSTIC_HTTP_STATUS_LABEL')}: ${apiError.status}`);
      }
      if (hasValue(apiError.statusText)) {
        lines.push(`${copyText('REPORT_DIAGNOSTIC_STATUS_TEXT_LABEL')}: ${apiError.statusText}`);
      }
      if (hasValue(apiError.url)) {
        lines.push(`${copyText('REPORT_DIAGNOSTIC_REQUEST_URL_LABEL')}: ${apiError.url}`);
      }
      if (hasValue(apiError.serverMessage)) {
        lines.push(`${copyText('REPORT_DIAGNOSTIC_SERVER_REASON_LABEL')}: ${apiError.serverMessage}`);
      }
      if (hasValue(apiError.correlationId)) {
        lines.push(`${copyText('REPORT_DIAGNOSTIC_CORRELATION_ID_LABEL')}: ${apiError.correlationId}`);
      }
      if (hasValue(apiError.reasonCode)) {
        lines.push(`${copyText('REPORT_DIAGNOSTIC_REASON_CODE_LABEL')}: ${apiError.reasonCode}`);
      }
      const time = formatTimestamp(apiError.timestamp);
      if (time) {
        lines.push(`${copyText('REPORT_DIAGNOSTIC_TIME_LABEL')}: ${time}`);
      }
      if (hasValue(apiError.rawDetail)) {
        lines.push(`${copyText('REPORT_DIAGNOSTIC_RAW_DETAIL_LABEL')}: ${apiError.rawDetail}`);
      }
    }
    lines.push(...formatConsoleEntryLines(consoleEntries));
    lines.push(copyText('REPORT_DIAGNOSTIC_FOOTER'));
    return lines.join('\n');
  }

  function stripEmptyFields(data) {
    const result = {};
    Object.keys(data).forEach(key => {
      if (hasValue(data[key])) {
        result[key] = data[key];
      }
    });
    return result;
  }

  function buildUserContext(apiError) {
    return stripEmptyFields({
      url: apiError?.url || window.location.pathname,
      correlation_id: apiError?.correlationId,
      http_status: apiError?.status,
      reason_code: apiError?.reasonCode,
      time: apiError?.timestamp,
    });
  }

  function buildFormState({ subject, detail, severity, anonymous, apiError, consoleEntries }) {
    const safeSubject = String(subject || '').trim() || copyText('REPORT_DEFAULT_SUBJECT');
    const bundle = formatDiagnosticBundle(apiError, consoleEntries);
    const description = `${detail || ''}\n\n---\n\n${bundle}`;
    const subjectLabel = copyText('REPORT_SUBJECT_LABEL');
    return {
      subject: safeSubject,
      detail: detail || '',
      severity: severity || 'low',
      anonymous: Boolean(anonymous),
      apiError: apiError || null,
      consoleEntries: Array.isArray(consoleEntries) ? consoleEntries : null,
      bundle,
      description,
      fullReport: `${subjectLabel}: ${safeSubject}\n\n${description}`,
      userContext: buildUserContext(apiError),
    };
  }

  function closeModal(force = false) {
    if (sending && !force) {
      return;
    }
    if (keydownHandler) {
      document.removeEventListener('keydown', keydownHandler, true);
      keydownHandler = null;
    }
    if (currentModal) {
      currentModal.remove();
      currentModal = null;
    }
    sending = false;
  }

  function textLabel(text, input) {
    const label = document.createElement('label');
    label.textContent = text;
    if (input) {
      label.htmlFor = input.id;
    }
    return label;
  }

  function openModal(context) {
    if (currentModal && sending) {
      return;
    }
    closeModal(true);

    const subject = context.heading || copyText('REPORT_DEFAULT_SUBJECT');
    const detail = context.customDetail || '';
    const bundle = formatDiagnosticBundle(context.apiError, context.consoleEntries);

    const modal = document.createElement('div');
    modal.className = 'modal report-error-modal';
    modal.style.display = 'block';
    modal.setAttribute('role', 'dialog');
    modal.setAttribute('aria-modal', 'true');

    const content = document.createElement('div');
    content.className = 'modal-content';
    modal.appendChild(content);

    const title = document.createElement('h2');
    title.textContent = copyText('REPORT_TITLE');
    content.appendChild(title);

    const form = document.createElement('form');
    form.className = 'report-error-form';
    form.noValidate = true;
    content.appendChild(form);

    const subjectInput = document.createElement('input');
    subjectInput.id = 'report-error-subject';
    subjectInput.type = 'text';
    subjectInput.className = 'report-error-subject';
    subjectInput.value = subject;
    form.appendChild(textLabel(copyText('REPORT_SUBJECT_LABEL'), subjectInput));
    form.appendChild(subjectInput);

    const detailInput = document.createElement('textarea');
    detailInput.id = 'report-error-detail';
    detailInput.className = 'report-error-detail';
    detailInput.placeholder = copyText('REPORT_DETAIL_PLACEHOLDER');
    detailInput.value = detail;
    form.appendChild(textLabel(copyText('REPORT_DETAIL_LABEL'), detailInput));
    form.appendChild(detailInput);

    const details = document.createElement('details');
    const summary = document.createElement('summary');
    summary.textContent = copyText('REPORT_DIAGNOSTIC_SUMMARY');
    const diagnostic = document.createElement('pre');
    diagnostic.className = 'report-error-diagnostic';
    diagnostic.textContent = bundle;
    details.appendChild(summary);
    details.appendChild(diagnostic);
    form.appendChild(details);

    const severity = document.createElement('select');
    severity.id = 'report-error-severity';
    severity.className = 'report-error-severity';
    [
      ['low', copyText('REPORT_SEVERITY_LOW')],
      ['medium', copyText('REPORT_SEVERITY_MEDIUM')],
      ['high', copyText('REPORT_SEVERITY_HIGH')],
    ].forEach(([value, label]) => {
      const option = document.createElement('option');
      option.value = value;
      option.textContent = label;
      severity.appendChild(option);
    });
    severity.value = 'low';
    form.appendChild(textLabel(copyText('REPORT_SEVERITY_LABEL'), severity));
    form.appendChild(severity);

    const anonymousLabel = document.createElement('label');
    anonymousLabel.className = 'report-error-checkbox';
    const anonymous = document.createElement('input');
    anonymous.type = 'checkbox';
    anonymous.className = 'report-error-anonymous';
    anonymousLabel.appendChild(anonymous);
    anonymousLabel.appendChild(document.createTextNode(` ${copyText('REPORT_ANONYMOUS_LABEL')}`));
    form.appendChild(anonymousLabel);

    const actions = document.createElement('div');
    actions.className = 'report-error-actions';
    const cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.textContent = copyText('REPORT_ACTION_CANCEL');
    const send = document.createElement('button');
    send.type = 'submit';
    send.textContent = copyText('REPORT_ACTION_SEND');
    actions.appendChild(cancel);
    actions.appendChild(send);
    form.appendChild(actions);

    cancel.addEventListener('click', () => closeModal());
    modal.addEventListener('click', event => {
      if (event.target === modal) {
        closeModal();
      }
    });
    keydownHandler = event => {
      if (event.key === 'Escape') {
        event.preventDefault();
        closeModal();
      }
    };
    document.addEventListener('keydown', keydownHandler, true);

    form.addEventListener('submit', event => {
      event.preventDefault();
      const formState = buildFormState({
        subject: subjectInput.value,
        detail: detailInput.value,
        severity: severity.value,
        anonymous: anonymous.checked,
        apiError: context.apiError,
        consoleEntries: context.consoleEntries,
      });
      submitReport(formState, { cancel, send }).catch(error => {
        logUnexpected('submit unexpected', error);
        showFallbackView(error, formState);
      });
    });

    document.body.appendChild(modal);
    currentModal = modal;
    subjectInput.focus();
  }

  async function submitReport(formState, controls) {
    sending = true;
    controls.cancel.disabled = true;
    controls.send.disabled = true;
    controls.send.textContent = copyText('REPORT_ACTION_SENDING');

    try {
      const response = await window.apiJson('/app/support/api/tickets', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          subject: formState.subject,
          description: formState.description,
          category: REPORT_CATEGORY,
          severity: formState.severity,
          auto_context: true,
          anonymous: formState.anonymous,
          user_context: formState.userContext,
        }),
      });
      showSuccessNotification(response);
      closeModal(true);
    } catch (error) {
      logUnexpected('submit failed', error);
      showFallbackView(error, formState);
    }
  }

  function ticketIdFromResponse(response) {
    if (!response || typeof response !== 'object') {
      return null;
    }
    return hasValue(response.id) ? response.id : (hasValue(response.ticket_id) ? response.ticket_id : null);
  }

  function successMessage(ticketId) {
    if (hasValue(ticketId)) {
      return copyText('REPORT_SUCCESS_BODY').replace('{ticket_id}', ticketId);
    }
    return copyText('REPORT_SUCCESS_BODY_NO_ID');
  }

  function showSuccessNotification(response) {
    const notifications = window.AppServices?.notifications;
    if (!notifications || typeof notifications.show !== 'function') {
      return;
    }
    const ticketId = ticketIdFromResponse(response);
    const buttons = [];
    if (hasValue(ticketId)) {
      const viewLabel = copyText('REPORT_ACTION_VIEW_TICKET');
      buttons.push({
        label: `${viewLabel} #${ticketId}`,
        onClick() {
          window.location.href = TICKET_URL;
        },
      });
    }
    notifications.show({
      app: 'support',
      title: successMessage(ticketId),
      message: '',
      buttons,
      autoDismiss: 8000,
    });
  }

  function buildMailtoUrl(subject, body) {
    const prefix = `mailto:${SUPPORT_EMAIL}?subject=${encodeURIComponent(subject)}&body=`;
    let mailtoUrl = `${prefix}${encodeURIComponent(body)}`;
    if (mailtoUrl.length <= MAILTO_LIMIT) {
      return mailtoUrl;
    }

    const suffix = copyText('REPORT_MAILTO_TRUNCATION_SUFFIX');
    const overflow = mailtoUrl.length - MAILTO_LIMIT;
    const keepLength = Math.max(0, body.length - overflow - suffix.length);
    return `${prefix}${encodeURIComponent(body.slice(0, keepLength) + suffix)}`;
  }

  function showFallbackView(error, formState) {
    sending = false;
    const modal = currentModal || document.createElement('div');
    if (!currentModal) {
      modal.className = 'modal report-error-modal';
      modal.style.display = 'block';
      modal.setAttribute('role', 'dialog');
      modal.setAttribute('aria-modal', 'true');
      currentModal = modal;
      document.body.appendChild(modal);
    }

    let content = modal.querySelector('.modal-content');
    if (!content) {
      content = document.createElement('div');
      content.className = 'modal-content';
      modal.appendChild(content);
    }
    content.replaceChildren();
    content.classList.add('report-error-fallback');

    const fullReportText = copyText('REPORT_MAILTO_BODY_PREFIX') + formState.fullReport;

    const headline = document.createElement('h2');
    headline.textContent = copyText('REPORT_FALLBACK_HEADLINE');
    const body = document.createElement('p');
    body.textContent = copyText('REPORT_FALLBACK_BODY');
    content.appendChild(headline);
    content.appendChild(body);

    const failureBits = [];
    if (hasValue(error?.status)) {
      failureBits.push(`${copyText('REPORT_FAILURE_HTTP_PREFIX')} ${error.status}`);
    }
    if (hasValue(error?.correlationId)) {
      failureBits.push(`${copyText('REPORT_FAILURE_REFERENCE_PREFIX')} ${error.correlationId}`);
    }
    if (failureBits.length) {
      const failure = document.createElement('p');
      failure.className = 'report-error-failure-meta';
      failure.textContent = failureBits.join(' · ');
      content.appendChild(failure);
    }

    const textarea = document.createElement('textarea');
    textarea.className = 'report-error-fallback-report';
    textarea.readOnly = true;
    textarea.value = fullReportText;
    content.appendChild(textarea);

    if (window.convey.copyToClipboard) {
      window.convey.copyToClipboard(fullReportText).catch(copyError => {
        logUnexpected('fallback copy failed', copyError);
      });
    }

    const actions = document.createElement('div');
    actions.className = 'report-error-actions';
    const email = document.createElement('button');
    email.type = 'button';
    email.textContent = copyText('REPORT_ACTION_OPEN_EMAIL');
    email.addEventListener('click', () => {
      window.location.href = buildMailtoUrl(formState.subject, fullReportText);
    });
    const close = document.createElement('button');
    close.type = 'button';
    close.textContent = copyText('REPORT_ACTION_CLOSE');
    close.addEventListener('click', () => closeModal(true));
    actions.appendChild(email);
    actions.appendChild(close);
    content.appendChild(actions);
  }

  window.convey.reportError = function(context) {
    const safeContext = context || {};
    const normalized = {
      source: safeContext.source || 'manual',
      heading: safeContext.heading || copyText('REPORT_DEFAULT_SUBJECT'),
      apiError: safeContext.apiError || null,
      customDetail: safeContext.customDetail || '',
      consoleEntries: Array.isArray(safeContext.consoleEntries) ? safeContext.consoleEntries : null,
    };
    try {
      openModal(normalized);
    } catch (error) {
      logUnexpected('open modal failed', error);
      showFallbackView(error, buildFormState({
        subject: normalized.heading,
        detail: normalized.customDetail,
        severity: 'low',
        anonymous: false,
        apiError: normalized.apiError,
        consoleEntries: normalized.consoleEntries,
      }));
    }
  };
})();
