// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/**
 * Callosum SSE Bridge
 *
 * Connects to /sse/events and broadcasts Callosum events to registered listeners.
 * Provides window.appEvents API for subscribing to events by tract.
 */
(function(){
  const DISCONNECT_FIRST_PHASE_MS = 5000;
  const DISCONNECT_SECOND_PHASE_MS = 30000;
  const listeners = {};
  const parseErrorHandlers = new Set();
  const connectionStateHandlers = new Set();
  let eventSource;
  let statusIcon = null;

  // Connection metrics
  let connectedAt = null;
  let lastMessageAt = null;
  let connectionState = 'disconnected';
  let lastCaptureStatus = null;
  let lastRenderedVariant = null;
  let lastRenderedConnecting = null;
  let disconnectTimerId = null;
  let disconnectCardId = null;
  let disconnectSecondPhaseTimerId = null;
  let firstDisconnectAt = null;
  let disconnectSecondPhase = false;
  let reconnectAttempts = [];

  function getTractListeners(tract) {
    if (!listeners[tract]) {
      listeners[tract] = [];
    }
    return listeners[tract];
  }

  function notifyParseError(error, rawPayload) {
    parseErrorHandlers.forEach(handler => {
      try {
        handler(error, rawPayload);
      } catch (handlerError) {
        if (typeof window.logError === 'function') {
          window.logError(handlerError, { context: 'sse-parse-handler' });
        }
      }
    });

    if (typeof window.logError === 'function') {
      window.logError(error, { context: 'sse-parse' });
    }
  }

  function notifyConnectionState() {
    const payload = { connected: connectionState === 'connected', state: connectionState };
    connectionStateHandlers.forEach(handler => {
      try {
        handler(payload);
      } catch (handlerError) {
        if (typeof window.logError === 'function') {
          window.logError(handlerError, { context: 'sse-connection-handler' });
        }
      }
    });
  }

  function createPendingController(options) {
    const pending = new Map();
    const hasTimeout = Number.isFinite(options.timeout) && options.timeout > 0;
    const onTimeout = typeof options.onTimeout === 'function' ? options.onTimeout : null;

    return {
      track(correlationId) {
        if (!hasTimeout || !onTimeout || correlationId == null) {
          return correlationId;
        }
        this.clear(correlationId);
        const timeoutId = window.setTimeout(() => {
          pending.delete(correlationId);
          onTimeout(correlationId);
        }, options.timeout);
        pending.set(correlationId, timeoutId);
        return correlationId;
      },

      clear(correlationId) {
        if (correlationId == null) {
          return;
        }
        const timeoutId = pending.get(correlationId);
        if (timeoutId) {
          window.clearTimeout(timeoutId);
          pending.delete(correlationId);
        }
      },

      clearAll() {
        pending.forEach(timeoutId => window.clearTimeout(timeoutId));
        pending.clear();
      }
    };
  }

  function getCorrelationId(msg, correlationKey) {
    if (!msg) {
      return undefined;
    }
    if (typeof correlationKey === 'function') {
      return correlationKey(msg);
    }
    return msg[correlationKey || 'use_id'];
  }

  function validateSchema(msg, schema) {
    if (!schema) {
      return;
    }
    if (Array.isArray(schema)) {
      const missing = schema.filter(key => msg == null || msg[key] === undefined);
      if (missing.length > 0) {
        throw new Error(`Missing required SSE field(s): ${missing.join(', ')}`);
      }
      return;
    }
    if (typeof schema === 'function') {
      const result = schema(msg);
      if (result === false) {
        throw new Error('SSE schema validation failed');
      }
    }
  }

  function createListenerRecord(fn, options) {
    return {
      fn,
      options,
      pending: createPendingController(options)
    };
  }

  function addListenerRecord(tract, record) {
    getTractListeners(tract).push(record);
  }

  function removeListenerRecord(tract, record) {
    if (!listeners[tract]) {
      return;
    }
    record.pending.clearAll();
    listeners[tract] = listeners[tract].filter(candidate => candidate !== record);
  }

  function dispatchToRecords(tract, msg) {
    const records = listeners[tract];
    if (!records || records.length === 0) {
      return;
    }

    records.slice().forEach(record => {
      try {
        const correlationId = getCorrelationId(msg, record.options.correlationKey);
        if (correlationId != null) {
          record.pending.clear(correlationId);
        }

        validateSchema(msg, record.options.schema);
        record.fn(msg);
      } catch (err) {
        if (record.options.schema) {
          if (typeof record.options.onDrop === 'function') {
            try {
              record.options.onDrop(msg, err);
            } catch (dropError) {
              if (typeof window.logError === 'function') {
                window.logError(dropError, { context: 'sse-drop' });
              }
            }
          }
          notifyParseError(err, msg);
          return;
        }

        console.error(`[SSE] Error in ${tract} listener:`, err);
      }
    });
  }

  // STATUS_MAP is JSON-shaped on purpose because a --lib contract test in this
  // crate extracts and asserts it.
  const STATUS_MAP = [
    {"ws": "connecting", "capture": "*", "unviewed": "*", "variant": "mark-connecting", "label": "connecting"},
    {"ws": "connected", "capture": null, "unviewed": "*", "variant": "mark-connecting", "label": "connecting"},
    {"ws": "disconnected", "capture": "*", "unviewed": "*", "variant": "mark-offline", "label": "disconnected"},
    {"ws": "*", "capture": "offline", "unviewed": "*", "variant": "mark-offline", "label": "devices offline"},
    {"ws": "*", "capture": "degraded", "unviewed": "*", "variant": "mark-attention", "label": "a device needs attention"},
    {"ws": "*", "capture": "*", "unviewed": true, "variant": "mark-attention", "label": "attention"},
    {"ws": "*", "capture": "stale", "unviewed": "*", "variant": "mark-attention", "label": "a device hasn't reached your journal"},
    {"ws": "*", "capture": "no_observers", "unviewed": "*", "variant": "mark-paused", "label": "no devices connected"},
    {"ws": "*", "capture": "active", "unviewed": "*", "variant": "mark", "label": "going in"},
    {"ws": "*", "capture": "*", "unviewed": "*", "variant": "mark-offline", "label": "can't confirm"}
  ];

  function statusFieldMatches(expected, actual) {
    return expected === '*' || expected === actual;
  }

  function statusUnviewedMatches(expected, actual) {
    return expected === '*' || (expected === true && !!actual);
  }

  function deriveStatusMark(wsState, captureStatus, hasUnviewedNotifs) {
    for (const row of STATUS_MAP) {
      if (!statusFieldMatches(row.ws, wsState)) {
        continue;
      }
      if (!statusFieldMatches(row.capture, captureStatus)) {
        continue;
      }
      if (!statusUnviewedMatches(row.unviewed, hasUnviewedNotifs)) {
        continue;
      }
      return {
        variant: row.variant,
        connecting: row.variant === 'mark-connecting',
        label: row.label
      };
    }
    throw new Error('STATUS_MAP has no matching row');
  }

  function renderStatusMark() {
    if (!statusIcon) {
      statusIcon = document.querySelector('#status-instrument .status-icon');
    }
    if (!statusIcon) return;

    const badge = statusIcon.querySelector('#quiet-notif-badge');
    const hasUnviewed = !!badge && badge.style.display !== 'none';
    const mark = deriveStatusMark(connectionState, lastCaptureStatus, hasUnviewed);
    if (window.appEvents) {
      window.appEvents.statusLabel = mark.label;
    }

    if (mark.variant === lastRenderedVariant && mark.connecting === lastRenderedConnecting) {
      window.updateStatusLabel?.();
      return;
    }

    let img = statusIcon.querySelector('img.status-indicator');
    if (!img) {
      img = document.createElement('img');
      img.className = 'status-indicator';
      img.alt = '';
      img.setAttribute('aria-hidden', 'true');
      img.width = 22;
      img.height = 22;
      statusIcon.insertBefore(img, badge || statusIcon.firstChild);
    }

    const reduced = window.matchMedia?.('(prefers-reduced-motion: reduce)')?.matches;
    const stem = mark.connecting && !reduced ? 'mark-connecting-animated' : mark.variant;
    img.src = '/static/sol-status/' + stem + '.svg';
    img.classList.toggle('status-indicator--connecting', mark.connecting);
    lastRenderedVariant = mark.variant;
    lastRenderedConnecting = mark.connecting;
    window.updateStatusLabel?.();
  }

  function updateStatusIcon(state) {
    if (!statusIcon) {
      statusIcon = document.querySelector('#status-instrument .status-icon');
    }

    const previousState = connectionState;
    connectionState = state;
    if (previousState !== state) {
      notifyConnectionState();
    }
    renderStatusMark();
  }

	  function reconnectAttemptDetails() {
	    if (!reconnectAttempts.length) return 'no reconnect attempts recorded yet.';
	    return 'recent attempts: ' + reconnectAttempts
	      .map(ts => new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' }))
	      .join(', ');
	  }

	  function enterDisconnectSecondPhase() {
	    if (
	      disconnectSecondPhase
	      || disconnectCardId === null
	      || firstDisconnectAt === null
	      || Date.now() - firstDisconnectAt < DISCONNECT_SECOND_PHASE_MS
	    ) {
	      return;
	    }
	    disconnectSecondPhase = true;
	    window.AppServices?.notifications?.update(disconnectCardId, {
	      title: 'connection lost',
	      message: 'last reconnect attempt failed.',
	      buttons: [
	        {
	          label: 'Reconnect now',
	          onClick: () => connect(),
	          dismiss: false
	        },
	        {
	          label: 'Show details',
	          onClick: (notification) => {
	            window.AppServices?.notifications?.update(notification.id, {
	              message: 'last reconnect attempt failed. ' + reconnectAttemptDetails()
	            });
	          },
	          dismiss: false
	        }
	      ]
	    });
	  }

	  function connect() {
	    if (eventSource) {
	      eventSource.close();
	    }
	    updateStatusIcon('connecting');
	    eventSource = new EventSource('/sse/events');

    eventSource.onopen = () => {
      connectedAt = Date.now();
      updateStatusIcon('connected');

	      if (disconnectTimerId) {
	        clearTimeout(disconnectTimerId);
	        disconnectTimerId = null;
	      }
	      if (disconnectSecondPhaseTimerId) {
	        clearTimeout(disconnectSecondPhaseTimerId);
	        disconnectSecondPhaseTimerId = null;
	      }
	      firstDisconnectAt = null;
	      disconnectSecondPhase = false;
	      reconnectAttempts = [];

      if (disconnectCardId !== null) {
        window.AppServices?.notifications?.dismiss(disconnectCardId);
        const reconnectedId = window.AppServices?.notifications?.show({
          app: 'system',
          icon: 'check',
          title: 'reconnected',
          message: 'all features restored',
          dismissible: true
        });
        if (reconnectedId != null) {
          setTimeout(() => window.AppServices?.notifications?.dismiss(reconnectedId), 3000);
        }
        disconnectCardId = null;
      }

      console.debug('[SSE] Connected to /sse/events');
    };

	    eventSource.onerror = err => {
	      connectedAt = null;
	      updateStatusIcon('disconnected');
	      if (firstDisconnectAt === null) {
	        firstDisconnectAt = Date.now();
	        disconnectSecondPhase = false;
	      }
	      reconnectAttempts.push(Date.now());
	      reconnectAttempts = reconnectAttempts.slice(-5);

	      if (!disconnectTimerId && disconnectCardId === null) {
	        disconnectTimerId = setTimeout(() => {
	          disconnectTimerId = null;
	          const id = window.AppServices?.notifications?.show({
            app: 'system',
            icon: 'triangle-alert',
            title: 'connection lost',
            message: 'reconnecting. some features may be delayed',
            dismissible: false
          });
	          if (id != null) {
	            disconnectCardId = id;
	          }
	        }, DISCONNECT_FIRST_PHASE_MS);
	      }

	      if (!disconnectSecondPhaseTimerId) {
	        disconnectSecondPhaseTimerId = setTimeout(() => {
	          disconnectSecondPhaseTimerId = null;
	          enterDisconnectSecondPhase();
	        }, DISCONNECT_SECOND_PHASE_MS);
	      }
	      enterDisconnectSecondPhase();

	      console.error('[SSE] Error:', err);
	    };

    eventSource.onmessage = event => {
      lastMessageAt = Date.now();

      let msg;
      try {
        msg = JSON.parse(event.data);
      } catch (err) {
        console.warn('[SSE] Failed to parse message:', err);
        notifyParseError(err, event.data);
        return;
      }

      const tract = msg.tract;
      if (tract) {
        dispatchToRecords(tract, msg);
      }
      dispatchToRecords('*', msg);
    };
  }

  window.renderStatusMark = renderStatusMark;

  window.appEvents = {
    statusLabel: 'connecting',

    setCaptureStatus(status) {
      lastCaptureStatus = status ?? null;
      renderStatusMark();
    },

    /**
     * Listen for events from a specific tract or all events.
     *
     * @param {string} tract - Tract name ('cortex', 'observe', 'indexer', etc.) or '*' for all
     * @param {function|object} optionsOrFn - Callback or options object
     * @param {function} [fn] - Callback when using the `(tract, options, fn)` overload
     * @returns {function} Cleanup function with `.pending.track(correlationId)` and `.pending.clear(correlationId)`
     *
     * @example
     * const cleanup = window.appEvents.listen('importer', {
     *   schema: ['event', 'use_id'],
     *   timeout: 15000,
     *   onTimeout(useId) {
     *     console.warn('Importer request timed out:', useId);
     *   }
     * }, (msg) => {
     *   console.log('Importer event:', msg.event);
     * });
     * cleanup.pending.track('abc123');
     */
    listen(tract, optionsOrFn, fn) {
      const hasOptions = typeof optionsOrFn === 'object' && optionsOrFn !== null && typeof fn === 'function';
      const options = hasOptions ? optionsOrFn : {};
      const handler = hasOptions ? fn : optionsOrFn;

      if (typeof handler !== 'function') {
        throw new Error('appEvents.listen requires a callback');
      }

      const record = createListenerRecord(handler, {
        correlationKey: hasOptions ? options.correlationKey || 'use_id' : 'use_id',
        onDrop: hasOptions ? options.onDrop : null,
        onTimeout: hasOptions ? options.onTimeout : null,
        schema: hasOptions ? options.schema : null,
        timeout: hasOptions ? options.timeout : null
      });
      addListenerRecord(tract, record);

      const cleanup = () => {
        removeListenerRecord(tract, record);
      };
      cleanup.pending = record.pending;
      return cleanup;
    },

    unlisten(tract, fn) {
      if (!listeners[tract]) {
        return;
      }
      listeners[tract].slice().forEach(record => {
        if (record.fn === fn) {
          removeListenerRecord(tract, record);
        }
      });
    },

    onParseError(fn) {
      if (typeof fn !== 'function') {
        throw new Error('appEvents.onParseError requires a callback');
      }
      parseErrorHandlers.add(fn);
      return () => {
        parseErrorHandlers.delete(fn);
      };
    },

    onConnectionState(fn) {
      if (typeof fn !== 'function') {
        throw new Error('appEvents.onConnectionState requires a callback');
      }
      connectionStateHandlers.add(fn);
      fn({ connected: connectionState === 'connected', state: connectionState });
      return () => {
        connectionStateHandlers.delete(fn);
      };
    },

    getMetrics() {
      const now = Date.now();
      return {
        connected: connectionState === 'connected',
        state: connectionState,
        uptimeMs: connectedAt ? now - connectedAt : 0,
        lastMessageMs: lastMessageAt ? now - lastMessageAt : null,
        lastMessageAt: lastMessageAt,
        connectedAt: connectedAt
      };
    }
  };

  addListenerRecord('notification', createListenerRecord(function(msg) {
    window.AppServices?.notifications?.show(msg);
  }, {}));

  addListenerRecord('navigate', createListenerRecord(function(msg) {
    if (msg.facet && !msg.path) {
      window.selectFacet && window.selectFacet(msg.facet);
    } else if (msg.path) {
      if (msg.facet) {
        var expires = new Date();
        expires.setFullYear(expires.getFullYear() + 1);
        document.cookie = 'selectedFacet=' + msg.facet + '; expires=' + expires.toUTCString() + '; path=/; SameSite=Lax';
      }
      window.location.href = msg.path;
    }
  }, {}));

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', connect);
  } else {
    connect();
  }
})();
