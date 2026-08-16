// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function() {
  const savedControlValues = new WeakMap();

  class ApiError extends Error {
    constructor({
      status,
      statusText,
      serverMessage,
      url,
      cause,
      correlationId = '',
      timestamp = null,
      reasonCode = null,
      rawDetail = null,
      payload = null
    }) {
      super(serverMessage);
      this.name = 'ApiError';
      this.status = status;
      this.statusText = statusText;
      this.serverMessage = serverMessage;
      this.url = url;
      this.correlationId = correlationId || '';
      this.timestamp = timestamp ?? null;
      if (cause) {
        this.cause = cause;
      }
      this.reasonCode = reasonCode;
      this.rawDetail = rawDetail;
      this.payload = payload;
    }
  }

  function normalizeRequestOptions(opts) {
    const source = opts || {};
    const fetchOptions = { credentials: 'same-origin' };

    Object.keys(source).forEach(key => {
      if (key === 'headers' || key === 'noAuthRedirect') return;
      fetchOptions[key] = source[key];
    });

    if (source.headers !== undefined) {
      fetchOptions.headers = new Headers(source.headers);
    }

    return {
      fetchOptions,
      noAuthRedirect: !!source.noAuthRedirect
    };
  }

  function parseJsonPayload(text) {
    if (text === '') {
      return { ok: true, payload: {} };
    }
    try {
      return { ok: true, payload: JSON.parse(text) };
    } catch (error) {
      return { ok: false, error };
    }
  }

  function getEscapeHtml() {
    if (window.AppServices && typeof window.AppServices.escapeHtml === 'function') {
      return window.AppServices.escapeHtml;
    }
    return function escapeHtml(value) {
      const div = document.createElement('div');
      div.textContent = String(value ?? '');
      return div.innerHTML;
    };
  }

  function readControlValue(el) {
    if (el instanceof HTMLInputElement) {
      const type = (el.type || '').toLowerCase();
      if (type === 'checkbox' || type === 'radio') {
        return el.checked;
      }
      return el.value;
    }
    if (el instanceof HTMLSelectElement || el instanceof HTMLTextAreaElement) {
      return el.value;
    }
    if ('value' in el) {
      return el.value;
    }
    return undefined;
  }

  function writeControlValue(el, value) {
    if (el instanceof HTMLInputElement) {
      const type = (el.type || '').toLowerCase();
      if (type === 'checkbox' || type === 'radio') {
        el.checked = !!value;
        return;
      }
      el.value = value ?? '';
      return;
    }
    if (el instanceof HTMLSelectElement || el instanceof HTMLTextAreaElement) {
      el.value = value ?? '';
      return;
    }
    if ('value' in el) {
      el.value = value ?? '';
    }
  }

  function getInitialControlValue(el) {
    if (el instanceof HTMLInputElement) {
      const type = (el.type || '').toLowerCase();
      if (type === 'checkbox' || type === 'radio') {
        return el.defaultChecked;
      }
      return el.defaultValue ?? el.value;
    }
    if (el instanceof HTMLSelectElement || el instanceof HTMLTextAreaElement) {
      return el.defaultValue ?? el.value;
    }
    if ('defaultValue' in el) {
      return el.defaultValue ?? el.value;
    }
    return readControlValue(el);
  }

  function pushApiErrorToConsole(apiError, url, fetchOptions) {
    const diagnosticConsole = window.convey?.diagnosticConsole;
    if (!diagnosticConsole || typeof diagnosticConsole.push !== 'function') {
      return;
    }
    const method = String(fetchOptions?.method || 'GET').toUpperCase();
    diagnosticConsole.push({
      severity: 'error',
      source: 'api',
      summary: `${method} ${apiError.status} ${url}`,
      detail: {
        apiError: {
          status: apiError.status,
          statusText: apiError.statusText,
          serverMessage: apiError.serverMessage,
          url: apiError.url,
          correlationId: apiError.correlationId,
          timestamp: apiError.timestamp,
          reasonCode: apiError.reasonCode,
          rawDetail: apiError.rawDetail
        }
      }
    });
  }

  function getExistingControlError(el, errorHost) {
    if (errorHost) {
      return errorHost.querySelector('[data-control-save-error]');
    }
    const sibling = el.nextElementSibling;
    if (sibling && sibling.matches('[data-control-save-error]')) {
      return sibling;
    }
    return null;
  }

  function clearControlError(el, errorHost) {
    const existing = getExistingControlError(el, errorHost);
    if (existing) {
      existing.remove();
    }
  }

  function renderControlError(el, errorHost, message) {
    const escaped = getEscapeHtml()(message);
    const html = `<span class="control-save-error" role="alert" data-control-save-error>${escaped}</span>`;
    const existing = getExistingControlError(el, errorHost);
    if (existing) {
      existing.remove();
    }
    if (errorHost) {
      errorHost.insertAdjacentHTML('beforeend', html);
      return;
    }
    el.insertAdjacentHTML('afterend', html);
  }

  async function apiJson(url, opts) {
    const { fetchOptions, noAuthRedirect } = normalizeRequestOptions(opts);
    const response = await fetch(url, fetchOptions);

    if ((response.status === 401 || response.status === 403) && !noAuthRedirect) {
      const correlationId = response?.headers?.get('X-Solstone-Request-Id') || '';
      const timestamp = Date.now();
      window.location.href = '/';
      const apiError = new ApiError({
        status: response.status,
        statusText: response.statusText,
        serverMessage: 'Authentication required',
        url: url,
        correlationId,
        timestamp
      });
      pushApiErrorToConsole(apiError, url, fetchOptions);
      throw apiError;
    }

    const text = await response.text();

    if (!response.ok) {
      let payload = null;
      if (text !== '') {
        const parsed = parseJsonPayload(text);
        payload = parsed.ok ? parsed.payload : null;
      }
      const serverMessage = payload?.error
        ?? payload?.message
        ?? `Request failed (HTTP ${response.status})`;
      const correlationId = response.headers.get('X-Solstone-Request-Id') || '';
      const timestamp = Date.now();
      const reasonCode = payload?.reason_code ?? null;
      const rawDetail = payload?.detail ?? null;
      const apiError = new ApiError({
        status: response.status,
        statusText: response.statusText,
        serverMessage,
        url: url,
        correlationId,
        timestamp,
        reasonCode,
        rawDetail,
        payload
      });
      pushApiErrorToConsole(apiError, url, fetchOptions);
      throw apiError;
    }

    const parsed = parseJsonPayload(text);
    if (parsed.ok) {
      return parsed.payload;
    }

    const correlationId = response.headers.get('X-Solstone-Request-Id') || '';
    const timestamp = Date.now();
    const apiError = new ApiError({
      status: response.status,
      statusText: response.statusText,
      serverMessage: 'Malformed server response',
      url: url,
      cause: 'parse',
      correlationId,
      timestamp
    });
    pushApiErrorToConsole(apiError, url, fetchOptions);
    throw apiError;
  }

  function saveControl({
    el,
    fetchArgs,
    revertOnError = true,
    onSuccess,
    onError,
    errorHost,
    readValue,
    writeValue
  }) {
    if (!el) {
      throw new Error('saveControl requires an element');
    }

    const hasCustomReader = typeof readValue === 'function';
    const reader = hasCustomReader ? readValue : readControlValue;
    const writer = typeof writeValue === 'function' ? writeValue : writeControlValue;
    const previousValue = hasCustomReader
      ? reader(el)
      : (savedControlValues.has(el) ? savedControlValues.get(el) : getInitialControlValue(el));

    const promise = (async () => {
      try {
        const result = Array.isArray(fetchArgs)
          ? await apiJson(fetchArgs[0], fetchArgs[1])
          : await fetchArgs();
        savedControlValues.set(el, readControlValue(el));
        clearControlError(el, errorHost);
        if (typeof onSuccess === 'function') {
          onSuccess(result);
        }
        return result;
      } catch (error) {
        if (revertOnError !== false) {
          writer(el, previousValue);
        }
        const serverMessage = error instanceof ApiError
          ? error.serverMessage
          : (error && error.message ? error.message : 'Request failed');
        renderControlError(el, errorHost, serverMessage);
        if (typeof onError === 'function') {
          onError(error);
        }
        throw error;
      }
    })();

    return promise;
  }

  window.ApiError = ApiError;
  window.apiJson = apiJson;
  window.saveControl = saveControl;
})();
