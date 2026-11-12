// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function() {
  const savedControlValues = new WeakMap();
  // Timers, where the host has them. Without them a read has no deadline.
  function startTimer(fn, ms) {
    return typeof setTimeout === 'function' ? setTimeout(fn, ms) : null;
  }

  function stopTimer(id) {
    if (id !== null && id !== undefined && typeof clearTimeout === 'function') clearTimeout(id);
  }

  // Every browser the dashboard supports has AbortController. Without it
  // (a bare script host), requests simply can't be cut short.
  const Abort = typeof AbortController === 'function' ? AbortController : class {
    constructor() {
      this.signal = {
        aborted: false,
        addEventListener() {},
        removeEventListener() {}
      };
    }

    abort() {
      this.signal.aborted = true;
    }
  };

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

  // Options apiJson reads itself, never passed on to fetch.
  const REQUEST_OPTION_KEYS = new Set(['headers', 'noAuthRedirect', 'scope', 'bounds', 'retry']);

  function normalizeRequestOptions(opts) {
    const source = opts || {};
    const fetchOptions = { credentials: 'same-origin' };

    Object.keys(source).forEach(key => {
      if (REQUEST_OPTION_KEYS.has(key)) return;
      fetchOptions[key] = source[key];
    });

    if (source.headers !== undefined) {
      fetchOptions.headers = new Headers(source.headers);
    }

    return {
      fetchOptions,
      noAuthRedirect: !!source.noAuthRedirect,
      scope: source.scope === 'shell' ? 'shell' : 'view',
      bounds: source.bounds,
      retry: source.retry
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
    const method = String(fetchOptions?.method || 'GET').toUpperCase();
    const entry = {
      severity: 'error',
      source: 'api',
      // Owner language on the owner's instrument; the method, status, url,
      // server message and correlation id stay in the detail below (G1-118).
      summary: window.CONVEY_COPY?.CONSOLE_SUMMARY_REQUEST_FAILED || "couldn't finish that request.",
      detail: {
        apiError: {
          method,
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
    };
    if (typeof window.convey?.keepDiagnostic === 'function') {
      window.convey.keepDiagnostic(entry);
      return;
    }
    const diagnosticConsole = window.convey?.diagnosticConsole;
    if (!diagnosticConsole || typeof diagnosticConsole.push !== 'function') {
      return;
    }
    diagnosticConsole.push(entry);
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

  function isSetupUrl(value) {
    try {
      const path = new URL(value, window.location.href).pathname;
      return path === '/init' || path.startsWith('/init/');
    } catch (_) {
      return false;
    }
  }

  // Reload the current page once per stamp key. A second attempt inside the
  // window returns false and leaves the caller on its error state, so a cause
  // a reload can't fix never turns into a loop. Without storage for the stamp
  // there is no reload at all.
  const RELOAD_ONCE_WINDOW_MS = 10000;

  function reloadOnce(stampKey) {
    const now = Date.now();
    try {
      const last = Number(window.sessionStorage.getItem(stampKey)) || 0;
      if (last > 0 && now - last < RELOAD_ONCE_WINDOW_MS) {
        return false;
      }
      window.sessionStorage.setItem(stampKey, String(now));
    } catch (_) {
      return false;
    }
    window.location.reload();
    return true;
  }

  // A phone app's in-app proxy answers 403 with this header when it refuses a
  // request itself (for example a stale capability). That is never a sign-in
  // failure, and the journal never sends it. The page reloads its current URL
  // once, so the host app can re-establish access; a second refusal inside the
  // window is left to the caller's error state instead of reloading again.
  const LOOPBACK_REFUSED_HEADER = 'X-SPL-Loopback-Refused';
  const LOOPBACK_RELOAD_STAMP_KEY = 'solstone.loopbackRefusedReloadAt';

  function isLoopbackRefusal(response) {
    return response?.status === 403
      && typeof response?.headers?.has === 'function'
      && response.headers.has(LOOPBACK_REFUSED_HEADER);
  }

  function recoverFromLoopbackRefusal() {
    return reloadOnce(LOOPBACK_RELOAD_STAMP_KEY);
  }

  // Code and styles are served under a hash of the build that made them, and
  // the journal serves only its own build's hashes. A page from an older build
  // that asks for one after an upgrade gets a 404, so it reloads once to pick
  // up the new build's page. It reloads only on that answer: a journal that is
  // restarting or out of reach answers nothing, and the page waits for it
  // rather than reloading onto nothing.
  const STALE_ASSET_RELOAD_STAMP_KEY = 'solstone.staleAssetReloadAt';
  const HASHED_ASSET_PATH = /^\/(?:static\/(?:vendor\/[^/]+\/)?|app\/[^/]+\/static\/)[0-9a-f]{16}\//;

  function isHashedAssetUrl(value) {
    try {
      const url = new URL(value, window.location.href);
      return url.origin === window.location.origin && HASHED_ASSET_PATH.test(url.pathname);
    } catch (_) {
      return false;
    }
  }

  async function recoverFromStaleAsset(value) {
    if (!isHashedAssetUrl(value)) {
      return false;
    }
    let response;
    try {
      response = await fetch(value, { method: 'HEAD', cache: 'no-store', credentials: 'same-origin' });
    } catch (_) {
      return false;
    }
    if (response?.status !== 404) {
      return false;
    }
    return reloadOnce(STALE_ASSET_RELOAD_STAMP_KEY);
  }

  // Shell scripts and stylesheets that fail after this file has run. Images
  // are left alone: a missing picture never warrants reloading the page.
  if (typeof window.addEventListener === 'function') {
    window.addEventListener('error', (event) => {
      const target = event?.target;
      if (!target || target === window) return;
      const url = target.tagName === 'SCRIPT' ? target.src
        : (target.tagName === 'LINK' ? target.href : '');
      if (url) recoverFromStaleAsset(url);
    }, true);
  }

  // Every read has two clocks, and neither is a limit on the whole read: the
  // time to its first byte, and how long its body may go with no new bytes.
  // A slow read that keeps arriving is never cut off. Each bound sits well
  // above the slowest honest read measured on a large journal, so a bound
  // only ends a read that has stopped.
  //
  // The routes below answer a request that arrives mid-run from the server's
  // next run, so their wait can reach two runs. Their first-byte bound is at
  // least twice the slowest run measured (about 21 s, cold).
  const READ_BOUNDS = Object.freeze({ firstByteMs: 30000, stallMs: 20000 });
  const COALESCED_READ_BOUNDS = Object.freeze({ firstByteMs: 60000, stallMs: 20000 });
  const COALESCED_PATHS = new Set([
    '/app/speakers/api/index',
    '/app/health/api/state',
    '/app/health/api/media-backlog',
    '/app/search/api/search',
    '/app/transcripts/api/index'
  ]);
  const COALESCED_PREFIXES = ['/app/transcripts/api/stats/'];

  function readBoundsFor(url) {
    let path = '';
    try {
      path = new URL(url, window.location.href).pathname;
    } catch (_) {
      return READ_BOUNDS;
    }
    if (COALESCED_PATHS.has(path) || COALESCED_PREFIXES.some(prefix => path.startsWith(prefix))) {
      return COALESCED_READ_BOUNDS;
    }
    return READ_BOUNDS;
  }

  // A read that passed its bound. Named like the browser's own timeout, so
  // the error state treats it as a dropped connection, not a server message.
  class ReadDeadlineError extends Error {
    constructor(kind, ms, url) {
      super(`no ${kind === 'stall' ? 'new bytes' : 'answer'} within ${ms} ms`);
      this.name = 'TimeoutError';
      this.deadline = kind;
      this.boundMs = ms;
      this.url = url;
    }
  }

  // Only a read is retried, only when the answer never came or a gateway in
  // between (the relay, a phone's proxy) said so, and only twice. A read that
  // passed its bound is not retried: the server is probably still working on
  // it, and asking again would start a second run.
  const RETRY_DELAYS_MS = [1000, 3000];
  const GATEWAY_STATUSES = new Set([502, 503, 504]);

  function isGatewayFailure(response) {
    return GATEWAY_STATUSES.has(response?.status) && !isLoopbackRefusal(response);
  }

  function isTransportFailure(error) {
    return error instanceof TypeError;
  }

  function waitFor(ms, signals) {
    return new Promise((resolve, reject) => {
      const live = signals.filter(Boolean);
      const stop = () => {
        stopTimer(timer);
        live.forEach(signal => signal.removeEventListener('abort', stop));
        reject((typeof DOMException === 'function' ? new DOMException('aborted', 'AbortError') : new Error('aborted')));
      };
      const timer = startTimer(() => {
        live.forEach(signal => signal.removeEventListener('abort', stop));
        resolve();
      }, ms);
      if (timer === null) {
        resolve();
        return;
      }
      if (live.some(signal => signal.aborted)) {
        stop();
        return;
      }
      live.forEach(signal => signal.addEventListener('abort', stop, { once: true }));
    });
  }

  function jittered(ms) {
    return Math.round(ms * (0.7 + Math.random() * 0.6));
  }

  // The page's head starts a view's first reads beside the shell's own files
  // (`window.__solEarly`). A read here takes one of those by its exact address,
  // once, instead of asking again. It takes the request whatever its age, under
  // what is left of the bound from the moment it started, so it behaves as if
  // the view had sent it then. Only an answer the ordinary path has to handle
  // itself is asked for again: a redirect, a refusal or the wrong type.
  function earlyTable() {
    const table = window.__solEarly;
    return table && table.reads ? table : null;
  }

  function takeEarly(url, init) {
    const table = earlyTable();
    if (!table) return null;
    if (String(init?.method || 'GET').toUpperCase() !== 'GET' || init?.body) return null;
    let href;
    try {
      href = new URL(url, window.location.href).href;
    } catch (_) {
      return null;
    }
    const entry = table.reads[href];
    if (!entry) return null;
    delete table.reads[href];
    return entry;
  }

  function earlyIsPlain(entry, response) {
    const type = response.headers?.get?.('content-type') || '';
    const wanted = entry.expect === 'html' ? 'text/html' : 'application/json';
    return response.ok && !response.redirected && type.includes(wanted);
  }

  function countEarly(field) {
    const stats = window.__solEarly?.stats;
    if (stats) stats[field] = (stats[field] || 0) + 1;
  }

  // Resolves as the request would. `ask` makes the ordinary request. A gateway
  // failure comes back as itself, so it spends one of the ordinary retries.
  function adoptEarly(entry, signal, ask) {
    const abort = () => entry.controller?.abort();
    if (signal.aborted) abort();
    else signal.addEventListener('abort', abort, { once: true });
    return entry.response.then((response) => {
      if (earlyIsPlain(entry, response) || isGatewayFailure(response)) {
        countEarly('adopted');
        return response;
      }
      countEarly('refetched');
      try {
        response.body?.cancel?.()?.catch?.(() => {});
      } catch (_) {}
      abort();
      return ask();
    });
  }

  function earlyFirstByteMs(entry, boundMs) {
    return Math.max(1, boundMs - Math.max(0, Date.now() - entry.startedAt));
  }

  // The mount that owns the early reads. A different view, or a later mount,
  // ends whatever hasn't been taken.
  function claimEarlyView(view) {
    const table = earlyTable();
    if (!table) return false;
    if (table.claimed || table.view !== view) {
      settleEarly();
      return false;
    }
    table.claimed = true;
    return true;
  }

  // Ends every early read nothing took. After this nothing is adopted.
  function settleEarly() {
    const table = earlyTable();
    if (!table) return;
    for (const href of Object.keys(table.reads)) {
      countEarly('unclaimed');
      table.reads[href].controller?.abort();
      delete table.reads[href];
    }
    table.reads = null;
  }

  // One attempt of one request, under both clocks. Resolves with the
  // response once its headers are in; `readText()` then reads the body under
  // the stall clock. Any signal in `signals` aborts it.
  function boundedAttempt(url, fetchOptions, bounds, signals) {
    const controller = new Abort();
    let failure = null;
    let timer = null;
    const live = signals.filter(Boolean);
    const onAbort = () => controller.abort();
    live.forEach(signal => {
      if (signal.aborted) controller.abort();
      else signal.addEventListener('abort', onAbort, { once: true });
    });
    const release = () => {
      stopTimer(timer);
      live.forEach(signal => signal.removeEventListener('abort', onAbort));
    };
    const arm = (kind, ms, delay = ms) => {
      stopTimer(timer);
      if (!ms) return;
      timer = startTimer(() => {
        failure = new ReadDeadlineError(kind, ms, url);
        controller.abort();
      }, delay);
    };
    const ask = () => {
      arm('first-byte', bounds?.firstByteMs);
      return rawFetch()(url, { ...fetchOptions, signal: controller.signal });
    };

    let started;
    const early = takeEarly(url, fetchOptions);
    if (early) {
      if (bounds?.firstByteMs) {
        arm('first-byte', bounds.firstByteMs, earlyFirstByteMs(early, bounds.firstByteMs));
      }
      started = adoptEarly(early, controller.signal, ask);
    } else {
      started = ask();
    }
    return started.then(
      (response) => {
        arm('stall', bounds?.stallMs);
        const readText = async () => {
          try {
            const reader = response.body && typeof response.body.getReader === 'function'
              && typeof TextDecoder === 'function'
              ? response.body.getReader()
              : null;
            if (!reader) {
              return await response.text();
            }
            const decoder = new TextDecoder();
            let text = '';
            for (;;) {
              const { done, value } = await reader.read();
              if (done) break;
              arm('stall', bounds?.stallMs);
              text += decoder.decode(value, { stream: true });
            }
            return text + decoder.decode();
          } catch (error) {
            throw failure || error;
          } finally {
            release();
          }
        };
        const discard = () => {
          release();
          try {
            // A body already ended by an abort refuses the cancel; that's fine.
            response.body?.cancel?.()?.catch?.(() => {});
          } catch (_) {}
        };
        return { response, readText, discard };
      },
      (error) => {
        release();
        throw failure || error;
      }
    );
  }

  // A request under the read bounds, retried as above when it is a read.
  // `signals` abort it; the caller decides what an abort means.
  async function boundedRequest(url, fetchOptions, options = {}) {
    const method = String(fetchOptions?.method || 'GET').toUpperCase();
    const bounds = options.bounds === undefined ? readBoundsFor(url) : options.bounds;
    const retriable = method === 'GET' && options.retry !== false;
    const signals = (options.signals || []).filter(Boolean);
    for (let attempt = 0; ; attempt += 1) {
      let result;
      try {
        result = await boundedAttempt(url, fetchOptions, bounds, signals);
      } catch (error) {
        if (retriable && attempt < RETRY_DELAYS_MS.length && isTransportFailure(error)
            && !signals.some(signal => signal.aborted)) {
          await waitFor(jittered(RETRY_DELAYS_MS[attempt]), signals);
          continue;
        }
        throw error;
      }
      if (retriable && attempt < RETRY_DELAYS_MS.length && isGatewayFailure(result.response)) {
        result.discard();
        await waitFor(jittered(RETRY_DELAYS_MS[attempt]), signals);
        continue;
      }
      return result;
    }
  }

  // Views also read with plain `fetch`. Those reads get the same two clocks,
  // with no retry and nothing else changed: a same-origin GET or HEAD only,
  // never the live stream, never a write.
  const nativeFetch = typeof fetch === 'function'
    ? (typeof window.fetch === 'function' ? window.fetch.bind(window) : fetch)
    : null;
  const NULL_BODY_STATUSES = new Set([101, 103, 204, 205, 304]);

  // The fetch a bounded request goes out on: the browser's own, or whatever
  // a test host put in its place after this file ran.
  function rawFetch() {
    const current = typeof fetch === 'function' ? fetch : null;
    return current && current !== boundedFetch ? current : nativeFetch;
  }

  function isRequest(input) {
    return typeof Request === 'function' && input instanceof Request;
  }

  function boundedFetchApplies(input, init) {
    const method = String(init?.method || (isRequest(input) ? input.method : 'GET')).toUpperCase();
    if (method !== 'GET' && method !== 'HEAD') return false;
    if (init?.keepalive) return false;
    let url;
    try {
      url = new URL(isRequest(input) ? input.url : String(input), window.location.href);
    } catch (_) {
      return false;
    }
    return url.origin === window.location.origin && !url.pathname.startsWith('/sse/');
  }

  // A response whose body is read through the stall clock. The fields a
  // caller reads from a fetched response stay as fetch gave them.
  function streamedResponse(attempt) {
    const { response, discard } = attempt;
    if (!response.body || NULL_BODY_STATUSES.has(response.status) || typeof ReadableStream !== 'function'
        || typeof Response !== 'function') {
      return response;
    }
    const reader = response.body.getReader();
    const stalled = attempt.stalled;
    const body = new ReadableStream({
      async pull(controller) {
        try {
          const { done, value } = await reader.read();
          if (done) {
            attempt.finish();
            controller.close();
            return;
          }
          attempt.progress();
          controller.enqueue(value);
        } catch (error) {
          attempt.finish();
          controller.error(stalled() || error);
        }
      },
      cancel(reason) {
        discard();
        return reader.cancel(reason).catch(() => {});
      }
    });
    const wrapped = new Response(body, {
      status: response.status,
      statusText: response.statusText,
      headers: response.headers
    });
    for (const field of ['url', 'redirected', 'type']) {
      Object.defineProperty(wrapped, field, { value: response[field] });
    }
    return wrapped;
  }

  function boundedFetch(input, init) {
    const fetchImpl = nativeFetch;
    if (!fetchImpl || !boundedFetchApplies(input, init)) {
      return fetchImpl ? fetchImpl(input, init) : Promise.reject(new TypeError('fetch unavailable'));
    }
    const url = isRequest(input) ? input.url : String(input);
    const bounds = readBoundsFor(url);
    const controller = new Abort();
    const callerSignal = init?.signal || (isRequest(input) ? input.signal : null);
    let failure = null;
    let timer = null;
    const finish = () => {
      stopTimer(timer);
      callerSignal?.removeEventListener('abort', onAbort);
    };
    // An aborted read is over: a body nobody reads keeps no stall clock.
    const onAbort = () => {
      controller.abort(callerSignal.reason);
      finish();
    };
    if (callerSignal) {
      if (callerSignal.aborted) controller.abort(callerSignal.reason);
      else callerSignal.addEventListener('abort', onAbort, { once: true });
    }
    const arm = (kind, ms, delay = ms) => {
      stopTimer(timer);
      timer = startTimer(() => {
        failure = new ReadDeadlineError(kind, ms, url);
        controller.abort();
      }, delay);
    };
    const ask = () => {
      arm('first-byte', bounds.firstByteMs);
      return fetchImpl(input, { ...(init || {}), signal: controller.signal });
    };
    let started;
    const early = isRequest(input) ? null : takeEarly(url, init);
    if (early) {
      arm('first-byte', bounds.firstByteMs, earlyFirstByteMs(early, bounds.firstByteMs));
      started = adoptEarly(early, controller.signal, ask);
    } else {
      started = ask();
    }
    return started.then(
      (response) => {
        arm('stall', bounds.stallMs);
        return streamedResponse({
          response,
          discard: finish,
          finish,
          progress: () => arm('stall', bounds.stallMs),
          stalled: () => failure
        });
      },
      (error) => {
        finish();
        throw failure || error;
      }
    );
  }

  if (nativeFetch && typeof window.fetch === 'function') {
    window.fetch = boundedFetch;
  }

  // Each view mount is a generation. A read a view starts belongs to the
  // generation it started in; when the next mount begins, the read is
  // aborted and its answer is never delivered, so an older view can't draw
  // onto a newer one or put its error state there. Reads the shell itself
  // makes pass `scope: 'shell'` and outlive mounts.
  let viewGeneration = new Abort();

  // Built once, while nothing is mid-move. Aborting with a fresh exception
  // records the stack of whoever aborted, and that stack keeps the function
  // that was replacing the view for as long as the signal lives.
  const ABORT_REASON = typeof DOMException === 'function'
    ? new DOMException('The view has gone', 'AbortError')
    : null;

  function beginViewGeneration() {
    if (ABORT_REASON) viewGeneration.abort(ABORT_REASON);
    else viewGeneration.abort();
    viewGeneration = new Abort();
  }

  // A fresh pending promise per call. One shared promise would keep every
  // caller waiting on it for the life of the page.
  function never() { return new Promise(() => {}); }

  async function apiJson(url, opts) {
    const { fetchOptions, noAuthRedirect, scope, bounds, retry } = normalizeRequestOptions(opts);
    const method = String(fetchOptions.method || 'GET').toUpperCase();
    const generation = method === 'GET' && scope !== 'shell' ? viewGeneration.signal : null;
    const callerSignal = fetchOptions.signal || null;
    delete fetchOptions.signal;
    // Writes take no deadline: abandoning one would leave its outcome unknown.
    const requestBounds = bounds !== undefined ? bounds : (method === 'GET' ? undefined : null);
    let request;
    try {
      request = await boundedRequest(url, fetchOptions, {
        bounds: requestBounds,
        retry,
        signals: [callerSignal, generation]
      });
    } catch (error) {
      if (generation?.aborted && !callerSignal?.aborted) return never();
      if (error instanceof ReadDeadlineError) {
        pushApiErrorToConsole(new ApiError({
          status: 0,
          statusText: '',
          serverMessage: error.message,
          url,
          cause: 'deadline',
          timestamp: Date.now()
        }), url, fetchOptions);
      }
      throw error;
    }
    if (generation?.aborted && !callerSignal?.aborted) {
      request.discard();
      return never();
    }
    const { response } = request;

    if (isLoopbackRefusal(response)) {
      request.discard();
      const apiError = new ApiError({
        status: response.status,
        statusText: response.statusText,
        serverMessage: window.CONVEY_COPY?.CONSOLE_SUMMARY_REQUEST_FAILED || "couldn't finish that request.",
        url: url,
        cause: 'loopback_refused',
        timestamp: Date.now()
      });
      pushApiErrorToConsole(apiError, url, fetchOptions);
      if (!noAuthRedirect) {
        recoverFromLoopbackRefusal();
      }
      throw apiError;
    }

    if ((response.status === 401 || response.status === 403) && !noAuthRedirect) {
      request.discard();
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

    // The session gate answers every app route with a redirect to /init until
    // setup finishes; fetch follows it and gets the setup page, not JSON.
    if (response.redirected && isSetupUrl(response.url)) {
      request.discard();
      const apiError = new ApiError({
        status: response.status,
        statusText: response.statusText,
        serverMessage: window.CONVEY_COPY?.CONSOLE_SUMMARY_REQUEST_FAILED || "couldn't finish that request.",
        url: url,
        cause: 'setup_required',
        correlationId: response.headers.get('X-Solstone-Request-Id') || '',
        timestamp: Date.now()
      });
      pushApiErrorToConsole(apiError, url, fetchOptions);
      window.location.href = response.url;
      throw apiError;
    }

    let text;
    try {
      text = await request.readText();
    } catch (error) {
      if (generation?.aborted && !callerSignal?.aborted) return never();
      if (error instanceof ReadDeadlineError) {
        pushApiErrorToConsole(new ApiError({
          status: response.status,
          statusText: response.statusText,
          serverMessage: error.message,
          url,
          cause: 'deadline',
          timestamp: Date.now()
        }), url, fetchOptions);
      }
      throw error;
    }
    if (generation?.aborted && !callerSignal?.aborted) return never();

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
  window.ReadDeadlineError = ReadDeadlineError;
  window.boundedRequest = boundedRequest;
  window.beginViewGeneration = beginViewGeneration;
  window.claimEarlyView = claimEarlyView;
  window.settleEarly = settleEarly;
  window.READ_BOUNDS = { read: READ_BOUNDS, coalesced: COALESCED_READ_BOUNDS };
  window.apiJson = apiJson;
  window.isLoopbackRefusal = isLoopbackRefusal;
  window.recoverFromLoopbackRefusal = recoverFromLoopbackRefusal;
  window.recoverFromStaleAsset = recoverFromStaleAsset;
  window.reloadOnce = reloadOnce;
  window.saveControl = saveControl;
})();
