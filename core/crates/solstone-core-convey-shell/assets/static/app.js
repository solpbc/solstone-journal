// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/**
 * App System JavaScript
 * Shared shell UI behavior.
 */


function copyToClipboard(text) {
  if (navigator.clipboard && navigator.clipboard.writeText) {
    return navigator.clipboard.writeText(text);
  }
  const textarea = document.createElement('textarea');
  textarea.value = text;
  textarea.style.position = 'fixed';
  textarea.style.opacity = '0';
  document.body.appendChild(textarea);
  textarea.select();
  document.execCommand('copy');
  document.body.removeChild(textarea);
  return Promise.resolve();
}

window.convey = window.convey || {};
window.convey.copyToClipboard = copyToClipboard;

const REPORT_KEY_CAP = 100;
const reportContexts = new Map();
let reportKeyCounter = 0;

function reportingEnabled() {
  return !(window.CONVEY_SETTINGS && window.CONVEY_SETTINGS.reportingEnabled === false);
}

function captureReportContext({ heading, apiError, customDetail }) {
  const key = `rk-${reportKeyCounter}`;
  reportKeyCounter += 1;
  if (reportContexts.size >= REPORT_KEY_CAP) {
    reportContexts.delete(reportContexts.keys().next().value);
  }
  reportContexts.set(key, {
    heading,
    apiError: apiError || null,
    customDetail: customDetail || ''
  });
  return key;
}

/**
 * Shared loading / empty / error surface-state renderer.
 * Examples: SurfaceState.loading({ text: 'Loading…' }), SurfaceState.empty({ icon: window.ConveyIcons.svg('search'), heading: 'No results' }), SurfaceState.error({ heading: 'Request failed', retry: true }).
 * Load order: call only after DOMContentLoaded or from later event/callback code.
 */
window.SurfaceState = (() => {
  const HEADING_LEVELS = new Set(['h1', 'h2', 'h3', 'h4', 'h5', 'h6']);
  const ERROR_ICON = window.ConveyIcons.svg('triangle-alert');
  const STRIP_LAST_KNOWN = /\s*[—-]\s*showing last known state\.?\s*$/i;

  function escapeHtml(value) {
    return String(value ?? '')
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function normalizeHeadingLevel(level) {
    return HEADING_LEVELS.has(level) ? level : 'h2';
  }

  function hasValue(value) {
    return value !== undefined && value !== null && value !== '';
  }

  function formatDetailTimestamp(timestamp) {
    if (!hasValue(timestamp)) {
      return '';
    }
    const date = new Date(timestamp);
    if (Number.isNaN(date.getTime())) {
      return '';
    }
    return date.toLocaleString();
  }

  function renderErrorActions({ retry, retryLabel, secondary, reportable, heading, apiError }) {
    const parts = [];
    if (retry) {
      parts.push(`<button type="button" class="surface-state-retry">${escapeHtml(retryLabel)}</button>`);
    }
    if (secondary && hasValue(secondary.label)) {
      if (hasValue(secondary.href)) {
        parts.push(`<a class="surface-state-secondary" href="${escapeHtml(secondary.href)}">${escapeHtml(secondary.label)}</a>`);
      } else {
        parts.push(`<button type="button" class="surface-state-secondary">${escapeHtml(secondary.label)}</button>`);
      }
    }
    if (reportable && reportingEnabled()) {
      const reportKey = captureReportContext({ heading, apiError, customDetail: '' });
      const label = window.CONVEY_COPY.REPORT_BUTTON_LABEL;
      parts.push(`<button type="button" class="surface-state-report" data-report-key="${escapeHtml(reportKey)}">${escapeHtml(label)}</button>`);
    }
    return parts.length ? `<div class="surface-state-action-row">${parts.join('')}</div>` : '';
  }

  function renderErrorDetail(detail, serverMessage) {
    if (!detail) {
      return '';
    }

    const lines = [];
    if (hasValue(detail.status) && hasValue(detail.statusText) && hasValue(detail.url)) {
      lines.push(`<div>HTTP ${escapeHtml(detail.status)} ${escapeHtml(detail.statusText)} · ${escapeHtml(detail.url)}</div>`);
    }

    const reason = hasValue(detail.rawDetail)
      ? detail.rawDetail
      : (hasValue(detail.serverMessage) ? detail.serverMessage : serverMessage);
    if (hasValue(reason)) {
      lines.push(`<div>Server reason: ${escapeHtml(reason)}</div>`);
    }

    const timestamp = formatDetailTimestamp(detail.timestamp);
    if (timestamp) {
      lines.push(`<div>time: ${escapeHtml(timestamp)}</div>`);
    }

    if (hasValue(detail.correlationId)) {
      const correlationId = String(detail.correlationId);
      lines.push(
        `<div>reference: <button type="button" class="surface-state-copy-reference" data-copy-value="${escapeHtml(correlationId)}">`
        + `${escapeHtml(correlationId)} <span class="surface-state-copy-affordance">(click to copy)</span>`
        + `</button></div>`
      );
    }

    if (hasValue(detail.reasonCode)) {
      lines.push(`<div>reason code: ${escapeHtml(detail.reasonCode)}</div>`);
    }

    if (!lines.length) {
      return '';
    }
    return `<details class="surface-state-detail"><summary>show details</summary>${lines.join('')}</details>`;
  }

  document.addEventListener('click', event => {
    const target = event.target instanceof Element ? event.target : null;
    const trigger = target ? target.closest('.surface-state-copy-reference') : null;
    if (!trigger) {
      return;
    }
    const value = trigger.getAttribute('data-copy-value') || '';
    if (!value) {
      return;
    }
    copyToClipboard(value).then(() => {
      const affordance = trigger.querySelector('.surface-state-copy-affordance');
      if (affordance) {
        affordance.textContent = '(copied)';
      }
    }).catch(error => {
      if (window.logError) {
        window.logError(error, { context: 'surface-state copy reference failed' });
      }
    });
  });

  document.addEventListener('click', event => {
    const target = event.target instanceof Element ? event.target : null;
    const trigger = target ? target.closest('.surface-state-report') : null;
    if (!trigger) {
      return;
    }
    const key = trigger.getAttribute('data-report-key') || '';
    const context = reportContexts.get(key) || {
      heading: window.CONVEY_COPY.REPORT_DEFAULT_SUBJECT,
      apiError: null,
      customDetail: ''
    };
    if (window.convey && typeof window.convey.reportError === 'function') {
      window.convey.reportError({
        source: 'auto',
        heading: context.heading,
        apiError: context.apiError,
        customDetail: context.customDetail
      });
    } else if (window.logError) {
      window.logError(new Error('report-error handler unavailable'), { context: 'surface-state report failed' });
    }
  });

  function render(kind, {
    icon = '',
    heading = '',
    desc = '',
    action = '',
    headingLevel = 'h2',
    role = ''
  } = {}) {
    const tag = normalizeHeadingLevel(headingLevel);
    const roleAttr = role ? ` role="${role}"` : '';

    return `<div class="surface-state surface-state--${kind}"${roleAttr}>`
      + `${icon ? `<div class="surface-state-icon" aria-hidden="true">${icon}</div>` : ''}`
      + `${heading ? `<${tag} class="surface-state-heading">${escapeHtml(heading)}</${tag}>` : ''}`
      + `${desc ? `<p class="surface-state-desc">${escapeHtml(desc)}</p>` : ''}`
      + `${action ? `<div class="surface-state-action">${action}</div>` : ''}`
      + `</div>`;
  }

  function stripLastKnownFromHeading(errorHtml) {
    const template = document.createElement('template');
    template.innerHTML = errorHtml;
    const headingEl = template.content.querySelector('.surface-state-heading');
    if (headingEl) {
      headingEl.textContent = headingEl.textContent.replace(STRIP_LAST_KNOWN, '');
    }
    return template.innerHTML;
  }

  return {
    loading({ text = '' } = {}) {
      return `<div class="surface-state surface-state--loading" role="status" aria-busy="true">`
        + `<div class="surface-state-spinner" aria-hidden="true"></div>`
        + `${text ? `<span class="surface-state-text" data-role="loading-status">${escapeHtml(text)}</span>` : ''}`
        + `</div>`;
    },

    empty(options = {}) {
      return render('empty', options);
    },

    error({
      heading = 'Couldn\'t load this section',
      desc = window.CONVEY_COPY?.RELOAD_HINT || 'reload to try again.',
      serverMessage = '',
      retry = false,
      retryLabel = 'Try again',
      secondary = null,
      detail = null,
      reportable = true,
      headingLevel = 'h2'
    } = {}) {
      const tag = normalizeHeadingLevel(headingLevel);
      return `<div class="surface-state surface-state--error" role="alert">`
        + `<div class="surface-state-icon" aria-hidden="true">${ERROR_ICON}</div>`
        + `<${tag} class="surface-state-heading">${escapeHtml(heading)}</${tag}>`
        + `<p class="surface-state-desc">${escapeHtml(desc)}</p>`
        + `${serverMessage ? `<p class="surface-state-server-message">${escapeHtml(serverMessage)}</p>` : ''}`
        + renderErrorActions({ retry, retryLabel, secondary, reportable, heading, apiError: detail })
        + renderErrorDetail(detail, serverMessage)
        + `</div>`;
    },

    /**
     * Replace an initial loading scaffold or append a singleton refresh error beside it.
     * Prevents the apps/entities anti-pattern where an `.error-message` is stuffed inside
     * the loading scaffold (`apps/entities/workspace.html:2671-2674`).
     * On first-paint, strips a trailing `— showing last known state` tail from the
     * rendered heading so callers can pass the same heading on first-paint and refresh
     * paths without leaking refresh-only language to first-paint owners.
     *
     * @param {string} containerId
     * @param {string} errorHtml
     * @returns {HTMLElement|null}
     */
    replaceLoading(containerId, errorHtml) {
      const container = document.getElementById(containerId);
      if (!container) {
        return null;
      }

      const isFirstPaint = container.querySelector('.surface-state--loading');
      if (isFirstPaint) {
        container.innerHTML = stripLastKnownFromHeading(errorHtml);
        return container;
      }

      const parent = container.parentElement;
      if (parent) {
        Array.from(parent.children).forEach(child => {
          if (child !== container && child.classList.contains('surface-state-refresh-error')) {
            child.remove();
          }
        });
      }

      if (container.nextElementSibling?.classList.contains('surface-state-refresh-error')) {
        container.nextElementSibling.remove();
      }

      const wrapper = document.createElement('div');
      wrapper.className = 'surface-state-refresh-error';
      wrapper.innerHTML = errorHtml;
      container.insertAdjacentElement('afterend', wrapper);
      return container;
    }
  };
})();

/**
 * Translate a raw browser network-failure message (e.g. "Failed to fetch",
 * "NetworkError when attempting to fetch resource", "Load failed") into a
 * plain-language reason. A background task's own errors (a real HTTP status,
 * a server-provided message) pass through unchanged.
 */
function toOwnerFacingTaskError(message) {
  if (/failed to fetch|networkerror|load failed|err_/i.test(String(message || ''))) {
    return "couldn't reach your journal. some background updates are paused.";
  }
  return message;
}

/**
 * App Services Framework
 * Global API for apps to register background services, update badges, and show notifications
 */
window.AppServices = {
  services: {},
  _tasks: {},

  /**
   * Register an app background service
   * @param {string} appName - Name of the app
   * @param {object} service - Service object with initialize() method
   */
  register(appName, service) {
    this.services[appName] = service;
    if (service.initialize) {
      try {
        service.initialize();
      } catch (err) {
        console.error(`[AppServices] Failed to initialize ${appName} service:`, err);
      }
    }
  },

  markBackgroundFailing(_appName, _error) {},

  registerTask(appName, taskName, {
    run,
    intervalMs,
    onSuccess,
    onError,
    failuresBeforeFailing = 3
  }) {
    if (typeof run !== 'function') {
      throw new Error('AppServices.registerTask requires a run() function');
    }

    if (!this._tasks[appName]) {
      this._tasks[appName] = {};
    }

    const health = {
      disabled: false,
      failing: false,
      lastError: '',
      lastRunAt: null,
      lastSuccessAt: null,
      consecutiveFailures: 0,
      intervalId: null
    };
    this._tasks[appName][taskName] = health;

    const apiJsonForTask = (url, opts) => window.apiJson(url, { ...(opts || {}), noAuthRedirect: true });

    const runTask = async () => {
      health.lastRunAt = Date.now();

      try {
        const result = await run({ apiJson: apiJsonForTask });
        health.disabled = false;
        health.lastError = '';
        health.lastSuccessAt = Date.now();
        health.consecutiveFailures = 0;
        if (health.failing) {
          health.failing = false;
        }
        if (typeof onSuccess === 'function') {
          onSuccess(result);
        }
        return result;
      } catch (error) {
        const message = error?.message || 'Request failed';
        health.lastError = message;

        if (error instanceof window.ApiError && error.status === 403) {
          health.disabled = true;
          health.failing = false;
          if (health.intervalId) {
            window.clearInterval(health.intervalId);
            health.intervalId = null;
          }
          if (typeof onError === 'function') {
            onError(error);
          }
          return undefined;
        }

        health.disabled = false;
        health.consecutiveFailures += 1;
        if (typeof onError === 'function') {
          onError(error);
        }

        if (health.consecutiveFailures >= failuresBeforeFailing && !health.failing) {
          health.failing = true;
          this.notifications.show({
            app: 'system',
            title: `${String(appName).toLowerCase()} background task`,
            message: toOwnerFacingTaskError(message),
            dismissible: true,
            autoDismiss: false,
            buttons: [
              {
                label: 'Try now',
                onClick: () => runNow(),
                dismiss: false
              },
              {
                label: 'Disable',
                onClick: () => {
                  health.disabled = true;
                  if (health.intervalId) {
                    window.clearInterval(health.intervalId);
                    health.intervalId = null;
                  }
                }
              }
            ]
          });
        }

        throw error;
      }
    };

    const stop = () => {
      if (health.intervalId) {
        window.clearInterval(health.intervalId);
        health.intervalId = null;
      }
    };

    const runNow = () => runTask();
    const ignoreTaskRejection = () => {
      // runTask already updates task health and owner-visible failure state.
    };

    if (Number.isFinite(intervalMs) && intervalMs > 0) {
      health.intervalId = window.setInterval(() => {
        runTask().catch(ignoreTaskRejection);
      }, intervalMs);
    }

    runTask().catch(ignoreTaskRejection);

    return {
      stop,
      runNow,
      getHealth() {
        return { ...health };
      }
    };
  },

  getTaskHealth(appName) {
    return { ...(this._tasks[appName] || {}) };
  },

  /**
   * Notification system
   */
  notifications: {
    _stack: [],
    _history: JSON.parse(localStorage.getItem('solstone:notification_history') || '[]'),
    _nextId: 1,
    _container: null,
    _dismissTimers: {},
    _defaultIconName: 'mailbox',
    _iconSvgByName: Object.freeze({
      'trash-2': window.ConveyIcons.svg('trash-2'),
      'undo-2': window.ConveyIcons.svg('undo-2'),
      'timer': window.ConveyIcons.svg('timer'),
      'triangle-alert': window.ConveyIcons.svg('triangle-alert'),
      'mailbox': window.ConveyIcons.svg('mailbox'),
      'life-buoy': window.ConveyIcons.svg('life-buoy'),
      'bot': window.ConveyIcons.svg('bot'),
      'circle-x': window.ConveyIcons.svg('circle-x'),
      'circle-check': window.ConveyIcons.svg('circle-check'),
      'refresh-cw': window.ConveyIcons.svg('refresh-cw'),
      'check': window.ConveyIcons.svg('check'),
      'mic-vocal': window.ConveyIcons.svg('mic-vocal'),
      'eye': window.ConveyIcons.svg('eye')
    }),
    _legacyIconNameByGlyph: Object.freeze({
      '🗑️': 'trash-2',
      '↩️': 'undo-2',
      '⏱️': 'timer',
      '⚠️': 'triangle-alert',
      '📬': 'mailbox',
      '🛟': 'life-buoy',
      '🤖': 'bot',
      '❌': 'circle-x',
      '✅': 'circle-check',
      '🔄': 'refresh-cw',
      '✓': 'check',
      '🎙️': 'mic-vocal',
      '👁️': 'eye'
    }),

    _iconNameFor(value) {
      if (value === undefined || value === null || value === '') {
        return this._defaultIconName;
      }
      const text = String(value);
      if (Object.prototype.hasOwnProperty.call(this._iconSvgByName, text)) {
        return text;
      }
      return this._legacyIconNameByGlyph[text] || null;
    },

    _normalizeIconName(value) {
      const name = this._iconNameFor(value);
      if (name) {
        return name;
      }
      console.warn('[Notifications] unsupported icon value:', String(value));
      return this._defaultIconName;
    },

    _resolveIcon(value) {
      return this._iconSvgByName[this._iconNameFor(value) || this._defaultIconName];
    },

    /**
     * Show a persistent notification card
     * @param {object} options - {app, icon, title, message, action, dismissible, badge, autoDismiss, buttons, key, work_key}
     * @returns {number} Notification ID
     */
    show(options) {
      const action = window.AppServices.sameOriginPath(options.action);
      const autoDismiss = Number.isFinite(options.autoDismiss) && options.autoDismiss > 0
        ? options.autoDismiss : null;
      const key = options.key ? String(options.key) : null;
      const workKey = options.work_key ? String(options.work_key) : null;
      const buttons = this._normalizeButtons(options.buttons);
      const hasIcon = Object.prototype.hasOwnProperty.call(options, 'icon');
      const normalizedIcon = hasIcon ? this._normalizeIconName(options.icon) : this._defaultIconName;

      if (key) {
        const existing = this._stack.find(n => n.key === key);
        if (existing) {
          if (!existing._workKeys) existing._workKeys = new Set();
          if (workKey) existing._workKeys.add(workKey);
          existing.count = existing._workKeys.size || 1;
          existing.lastSeen = Date.now();
          existing.app = options.app || existing.app;
          if (hasIcon) existing.icon = normalizedIcon;
          existing.title = options.title || existing.title;
          existing.message = options.message || '';
          existing.action = action;
          existing.dismissible = options.dismissible !== false;
          existing.badge = options.badge || this._countBadge(existing.count);
          existing.autoDismiss = autoDismiss;
          existing.buttons = buttons;
          this._render();
          return existing.id;
        }
      }

      const timestamp = Date.now();
      const notif = {
        id: this._nextId++,
        app: options.app || 'system',
        icon: normalizedIcon,
        title: options.title || 'Notification',
        message: options.message || '',
        action,
        dismissible: options.dismissible !== false,
        badge: options.badge || null,
        timestamp,
        lastSeen: timestamp,
        autoDismiss,
        buttons
      };

      if (key) {
        notif.key = key;
        notif._workKeys = new Set(workKey ? [workKey] : []);
        notif.count = notif._workKeys.size || 1;
        notif.badge = options.badge || this._countBadge(notif.count);
      }

      this._stack.push(notif);
      this._addToHistory(notif);
      this._render();

      // Browser notification if permitted
      if ('Notification' in window && Notification.permission === 'granted') {
        new Notification(notif.title, {
          body: notif.message,
          tag: `${notif.app}-${notif.id}`
        });
      }

      // Auto-dismiss timer
      if (notif.autoDismiss) {
        this._startDismissTimer(notif);
      }

      return notif.id;
    },

    /**
     * Dismiss a specific notification
     * @param {number} id - Notification ID
     */
    dismiss(id) {
      this._clearDismissTimer(id);
      this._stack = this._stack.filter(n => n.id !== id);
      this._render();
    },

    /**
     * Dismiss all notifications for an app
     * @param {string} appName - App name
     */
    dismissApp(appName) {
      this._stack.filter(n => n.app === appName).forEach(n => this._clearDismissTimer(n.id));
      this._stack = this._stack.filter(n => n.app !== appName);
      this._render();
    },

    /**
     * Dismiss all notifications
     */
    dismissAll() {
      Object.keys(this._dismissTimers).forEach(id => this._clearDismissTimer(id));
      this._stack = [];
      this._render();
    },

    _startDismissTimer(notif) {
      // Clear any existing timer for this notification
      if (this._dismissTimers[notif.id]) {
        clearTimeout(this._dismissTimers[notif.id]);
      }
      this._dismissTimers[notif.id] = setTimeout(() => {
        delete this._dismissTimers[notif.id];
        this.dismiss(notif.id);
      }, notif.autoDismiss);

      // Reset the progress bar animation
      const card = this._container && this._container.querySelector(`.notification-card[data-id="${notif.id}"]`);
      if (card) {
        const bar = card.querySelector('.notification-countdown');
        if (bar) {
          bar.style.animation = 'none';
          // Force reflow to restart animation
          bar.offsetHeight;
          bar.style.animation = '';
          bar.style.animationDuration = notif.autoDismiss + 'ms';
        }
      }
    },

    _pauseDismissTimer(id) {
      if (this._dismissTimers[id]) {
        clearTimeout(this._dismissTimers[id]);
        delete this._dismissTimers[id];
      }
      const card = this._container && this._container.querySelector(`.notification-card[data-id="${id}"]`);
      if (card) {
        const bar = card.querySelector('.notification-countdown');
        if (bar) {
          bar.style.animationPlayState = 'paused';
        }
      }
    },

    _clearDismissTimer(id) {
      if (this._dismissTimers[id]) {
        clearTimeout(this._dismissTimers[id]);
        delete this._dismissTimers[id];
      }
    },

    _normalizeButtons(buttons) {
      return Array.isArray(buttons)
        ? buttons
            .filter(button => button && button.label)
            .map(button => ({
              label: String(button.label),
              onClick: typeof button.onClick === 'function' ? button.onClick : null,
              dismiss: button.dismiss !== false
            }))
        : [];
    },

    _countBadge(count) {
      return count > 1 ? `${count} segments` : null;
    },

    /**
     * Get count of active notifications
     * @returns {number}
     */
    count() {
      // Keyed notifications are deduped in _stack, so length is active groups
      // plus non-keyed cards rather than raw repeated events.
      return this._stack.length;
    },

    /**
     * Update existing notification
     * @param {number} id - Notification ID
     * @param {object} options - Fields to update
     */
    update(id, options) {
      const notif = this._stack.find(n => n.id === id);
      if (!notif) return;

      Object.assign(notif, options);
      notif.action = window.AppServices.sameOriginPath(notif.action);
      if (!Number.isFinite(notif.autoDismiss) || notif.autoDismiss <= 0) notif.autoDismiss = null;
      this._render();
    },

    /**
     * Get notification history (most recent first)
     * @returns {Array} Array of notification objects
     */
    getHistory() {
      return [...this._history].reverse();
    },

    /**
     * Add notification to history and persist
     * @private
     */
    _addToHistory(notif) {
      // Store minimal data for history (exclude runtime fields)
      const historyEntry = {
        app: notif.app,
        icon: notif.icon,
        title: notif.title,
        message: notif.message,
        action: notif.action,
        timestamp: notif.timestamp
      };

      this._history.push(historyEntry);

      // Cap at 10 items
      if (this._history.length > 10) {
        this._history = this._history.slice(-10);
      }

      // Persist to localStorage
      try {
        localStorage.setItem('solstone:notification_history', JSON.stringify(this._history));
      } catch (e) {
        // localStorage may be full or disabled
        console.warn('[Notifications] Failed to persist history:', e);
      }
    },

    /**
     * Render notification cards
     * @private
     */
    _render() {
      if (!this._container) {
        this._container = document.getElementById('notification-center');
        if (!this._container) return;
      }

      // Limit to 5 most recent
      const visible = this._stack.slice(-5);
      const visibleIds = visible.map(n => n.id);

      // Get existing card IDs
      const existingCards = Array.from(this._container.querySelectorAll('.notification-card'));
      const existingIds = existingCards.map(card => parseInt(card.getAttribute('data-id')));

      // Remove cards that are no longer in visible stack
      existingCards.forEach(card => {
        const id = parseInt(card.getAttribute('data-id'));
        if (!visibleIds.includes(id) && !card.classList.contains('notification-card--dismissing')) {
          card.classList.add('notification-card--dismissing');
          const onEnd = () => card.remove();
          card.addEventListener('transitionend', onEnd, { once: true });
          setTimeout(onEnd, 250);
        }
      });

      // Add or update cards
      visible.forEach(n => {
        let card = this._container.querySelector(`.notification-card[data-id="${n.id}"]`);

        if (!card) {
          // New card - create and animate
          card = this._createCard(n);
          this._container.appendChild(card);
        } else {
          // Existing card - update content (no animation)
          this._updateCard(card, n);
        }
      });

      // Start timestamp updater if not already running
      if (visible.length > 0 && !this._updateInterval) {
        this._updateInterval = setInterval(() => this._updateTimestamps(), 60000);
      } else if (visible.length === 0 && this._updateInterval) {
        clearInterval(this._updateInterval);
        this._updateInterval = null;
      }

    },

    /**
     * Attach click handler to notification card
     * @private
     */
	    _attachClickHandler(card, n) {
	      if (!n.action) return;

	      card.onclick = (e) => {
	        // Ignore clicks on controls inside the card
	        if (e.target.closest('.notification-close, .notification-action')) {
	          return;
	        }

        // Prevent default for anchor tags
        if (card.tagName === 'A') {
          e.preventDefault();
        }

        // Navigate to the path
        window.location.href = n.action;
	      };
	    },

	    _syncButtons(card, n) {
	      const footer = card.querySelector('.notification-footer');
	      if (!footer) return;

	      let actionsEl = footer.querySelector('.notification-actions');
	      if (!n.buttons || n.buttons.length === 0) {
	        if (actionsEl) actionsEl.remove();
	        return;
	      }

	      if (!actionsEl) {
	        actionsEl = document.createElement('div');
	        actionsEl.className = 'notification-actions';
	        footer.appendChild(actionsEl);
	      }

	      actionsEl.replaceChildren();
	      n.buttons.forEach((button, idx) => {
	        const buttonEl = document.createElement('button');
	        buttonEl.type = 'button';
	        buttonEl.className = 'notification-action';
	        buttonEl.dataset.btn = String(idx);
	        buttonEl.textContent = button.label;
	        actionsEl.appendChild(buttonEl);
	      });

	      actionsEl.querySelectorAll('.notification-action').forEach((buttonEl) => {
	        buttonEl.onclick = (event) => {
	          event.preventDefault();
	          event.stopPropagation();
	          const button = n.buttons[Number(buttonEl.dataset.btn)];
	          if (!button) return;
	          if (button.onClick) {
	            button.onClick(n);
	          }
	          if (button.dismiss !== false) {
	            this.dismiss(n.id);
	          }
	        };
	      });
	    },

	    /**
	     * Create a new notification card element
	     * @private
     */
    _createCard(n) {
      // Use anchor tag for semantic HTML when action exists
      const card = document.createElement(n.action ? 'a' : 'div');
      card.className = 'notification-card';
      card.setAttribute('data-id', n.id);
      card.setAttribute('data-app', n.app);

      if (n.action) {
        card.href = n.action;
      }

      if (n.autoDismiss) {
        card.setAttribute('tabindex', '0');
      }

      const relativeTime = this._getRelativeTime(n.lastSeen || n.timestamp);
      card.innerHTML = `
        <div class="notification-header">
          <span class="notification-app-icon icon-slot" aria-hidden="true">${this._resolveIcon(n.icon)}</span>
          <span class="notification-app-name">${window.AppServices.escapeHtml(n.app)}</span>
          ${n.dismissible ? `<button class="notification-close" onclick="event.preventDefault(); event.stopPropagation(); window.AppServices.notifications.dismiss(${n.id});">×</button>` : ''}
        </div>
        <div class="notification-body">
          <div class="notification-title">${window.AppServices.escapeHtml(n.title)}</div>
          ${n.message ? `<div class="notification-message">${window.AppServices.escapeHtml(n.message)}</div>` : ''}
          ${n.badge ? `<span class="notification-badge">${window.AppServices.escapeHtml(n.badge)}</span>` : ''}
	        </div>
	        <div class="notification-footer">
	          <span class="notification-time">${relativeTime}</span>
	        </div>
	        ${n.autoDismiss ? `<div class="notification-countdown" style="animation-duration: ${n.autoDismiss}ms"></div>` : ''}
	      `;
	      this._syncButtons(card, n);

	      // Attach click handler
	      this._attachClickHandler(card, n);

      if (n.autoDismiss) {
        const self = this;
        card.addEventListener('mouseenter', () => self._pauseDismissTimer(n.id));
        card.addEventListener('focusin', () => self._pauseDismissTimer(n.id));
        card.addEventListener('mouseleave', () => {
          if (card.matches(':focus-within')) return;
          const notif = self._stack.find(s => s.id === n.id);
          if (notif) self._startDismissTimer(notif);
        });
        card.addEventListener('focusout', (e) => {
          if (!card.contains(e.relatedTarget) && !card.matches(':hover')) {
            const notif = self._stack.find(s => s.id === n.id);
            if (notif) self._startDismissTimer(notif);
          }
        });
      }

      return card;
    },

    /**
     * Update existing notification card content
     * @private
     */
    _updateCard(card, n) {
      const iconEl = card.querySelector('.notification-app-icon');
      if (iconEl) {
        iconEl.innerHTML = this._resolveIcon(n.icon);
      }

      // Update title
      const titleEl = card.querySelector('.notification-title');
      if (titleEl) {
        titleEl.textContent = n.title;
      }

      // Update message
      const messageEl = card.querySelector('.notification-message');
      if (n.message) {
        if (messageEl) {
          messageEl.textContent = n.message;
        } else {
          const bodyEl = card.querySelector('.notification-body');
          const newMessage = document.createElement('div');
          newMessage.className = 'notification-message';
          newMessage.textContent = n.message;
          bodyEl.insertBefore(newMessage, bodyEl.querySelector('.notification-badge'));
        }
      } else if (messageEl) {
        messageEl.remove();
      }

      // Update badge
      const badgeEl = card.querySelector('.notification-badge');
      if (n.badge) {
        if (badgeEl) {
          badgeEl.textContent = n.badge;
        } else {
          const bodyEl = card.querySelector('.notification-body');
          const newBadge = document.createElement('span');
          newBadge.className = 'notification-badge';
          newBadge.textContent = n.badge;
          bodyEl.appendChild(newBadge);
        }
      } else if (badgeEl) {
        badgeEl.remove();
      }

      // Update time
	      const timeEl = card.querySelector('.notification-time');
	      if (timeEl) {
	        timeEl.textContent = this._getRelativeTime(n.lastSeen || n.timestamp);
	      }
	      this._syncButtons(card, n);

      // Update action.
	      if (n.action) {
        if (card.tagName === 'A') {
          card.href = n.action;
        }

        // Recreate click handler with the new action.
        this._attachClickHandler(card, n);
      } else {
        card.style.cursor = 'default';
        card.removeAttribute('href');
        card.onclick = null;
      }
    },

    /**
     * Update timestamps on visible notifications
     * @private
     */
    _updateTimestamps() {
      const cards = this._container?.querySelectorAll('.notification-card');
      if (!cards) return;

      cards.forEach(card => {
        const id = parseInt(card.getAttribute('data-id'));
        const notif = this._stack.find(n => n.id === id);
        if (notif) {
          const timeEl = card.querySelector('.notification-time');
          if (timeEl) {
            timeEl.textContent = this._getRelativeTime(notif.timestamp);
          }
        }
      });
    },

    /**
     * Get relative time string
     * @private
     */
    _getRelativeTime(timestamp) {
      const seconds = Math.floor((Date.now() - timestamp) / 1000);
      if (seconds < 60) return 'now';
      const minutes = Math.floor(seconds / 60);
      if (minutes < 60) return `${minutes}m`;
      const hours = Math.floor(minutes / 60);
      if (hours < 24) return `${hours}h`;
      const days = Math.floor(hours / 24);
      return `${days}d`;
    }
  },

  quietNotifs: (() => {
    let stored;
    try { stored = JSON.parse(localStorage.getItem('solstone:quiet_notifs') || '[]'); }
    catch(e) { stored = []; }
    return {
      _notifs: stored,
      _unviewed: stored.length,
      _nextId: stored.length ? Math.max(...stored.map(n => n.id || 0)) + 1 : 1,

      add({ source, message, ts }) {
        const notif = { id: this._nextId++, source, message: message || '', ts: ts || Date.now() };
        this._notifs.push(notif);
        if (this._notifs.length > 20) this._notifs.shift();
        this._unviewed++;
        this._persist();
        this._updateBadge();
      },

      markViewed() {
        this._unviewed = 0;
        this._updateBadge();
      },

      unviewedCount() {
        return this._unviewed;
      },

      getAll() {
        return [...this._notifs].reverse();
      },

      _persist() {
        try {
          localStorage.setItem('solstone:quiet_notifs', JSON.stringify(this._notifs));
        } catch(e) {}
      },

      _updateBadge() {
        window.updateStatusLabel?.();
        const badge = document.getElementById('quiet-notif-badge');
        if (!badge) return;
        if (this._unviewed > 0) {
          badge.textContent = String(this._unviewed);
          badge.style.display = 'flex';
        } else {
          badge.style.display = 'none';
        }
      }
    };
  })(),

  /**
   * Request browser notification permission
   * @returns {Promise<string>} Permission state
   */
  async requestNotificationPermission() {
    if ('Notification' in window && Notification.permission === 'default') {
      return await Notification.requestPermission();
    }
    return Notification.permission;
  },

  sameOriginPath(value) {
    if (typeof value !== 'string' || !value.startsWith('/')) return null;
    try {
      const url = new URL(value, window.location.origin);
      return url.origin === window.location.origin && !url.pathname.startsWith('//')
        ? url.pathname + url.search + url.hash : null;
    } catch (_) {
      return null;
    }
  },

  /**
   * Escape a value for safe interpolation into HTML. DOM-based (routes
   * through textContent/innerHTML). Nullish-safe: null/undefined become ''.
   */
  escapeHtml(value) {
    const div = document.createElement('div');
    div.textContent = String(value ?? '');
    return div.innerHTML;
  },

  /**
   * Render user-supplied markdown into sanitized HTML. Calls marked + DOMPurify.
   * Throws if `marked` or `DOMPurify` isn't loaded (shell is broken; fail loudly).
   */
  renderMarkdown(raw) {
    return DOMPurify.sanitize(marked.parse(String(raw || ''), { breaks: true, gfm: true }));
  },

  /**
   * Badge system for apps.
   */
  badges: {
    /**
     * App badge state for background producers.
     */
    app: {
      _data: {},  // {appName: count}

      /**
       * Set badge count for an app
       * @param {string} appName - Name of the app
       * @param {number} count - Badge count (0 or falsy to hide)
       */
      set(appName, count) {
        if (count && count > 0) {
          this._data[appName] = count;
        } else {
          delete this._data[appName];
        }
      },

      /**
       * Clear badge for an app
       * @param {string} appName - Name of the app
       */
      clear(appName) {
        delete this._data[appName];
      },

      /**
       * Get badge count for an app
       * @param {string} appName - Name of the app
       * @returns {number} Badge count (0 if not set)
       */
      get(appName) {
        return this._data[appName] || 0;
      },
    }
  }
};
