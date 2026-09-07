// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/**
 * Error Handler for App System
 * Captures JavaScript errors and provides visual feedback
 *
 * Features:
 * - Catches window errors and unhandled promise rejections
 * - Adds error glow to status icon via .error class
 * - Routes diagnostics into the diagnostic console
 * - Provides modal for manual error display via window.showError()
 */

(function(){
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

  function normalizeContextValue(value) {
    if (value instanceof Error) {
      return roundTrip({
        message: value.message,
        stack: value.stack || ''
      });
    }
    return roundTrip(value);
  }

  function safeContext(context) {
    if (!context || typeof context !== 'object') {
      return {};
    }
    const result = {};
    Object.keys(context).forEach(key => {
      const value = normalizeContextValue(context[key]);
      if (value !== undefined) {
        result[key] = value;
      }
    });
    return result;
  }

  // The entry this message lands in is the one the owner can send on, so it
  // carries the shape of a failure and never its contents.
  const DETAIL_MESSAGE_MAX = 200;
  const DETAIL_KEYS_MAX = 12;

  function bounded(text) {
    const value = String(text);
    return value.length > DETAIL_MESSAGE_MAX ? `${value.slice(0, DETAIL_MESSAGE_MAX)}…` : value;
  }

  function nameFallback(error) {
    try {
      const name = error && typeof error.name === 'string' ? error.name : '';
      if (name) return bounded(name);
    } catch (_) {
      // an exotic reason with a throwing accessor still has to say something
    }
    return 'unknown error';
  }

  // X-01: String() on a null-prototype object throws "Cannot convert object to
  // primitive value", and it threw from inside the rejection listener, which
  // took the whole handler down with it. Nothing here may throw.
  function coerce(error) {
    try {
      const text = String(error ?? '');
      if (text) return bounded(text);
    } catch (_) {
      // fall through to the name
    }
    return nameFallback(error);
  }

  // X-03: `throw await res.json()` makes the rejection reason a parsed response
  // body, and serialising it put journal content into a diagnostic entry the
  // owner can send to support. The keys say which read failed; the values are
  // never the entry's business.
  function objectShape(error) {
    let keys = [];
    try {
      keys = Object.keys(error);
    } catch (_) {
      keys = [];
    }
    // "{}" has no shape to report, and String() on it is the "[object Object]"
    // the owner was reading before any of this.
    if (!keys.length) return nameFallback(error);
    const shown = keys.slice(0, DETAIL_KEYS_MAX).map(key => bounded(String(key)));
    const rest = keys.length - shown.length;
    return `{keys: ${shown.join(', ')}${rest > 0 ? `, +${rest} more` : ''}}`;
  }

  // X-01: a rejection reason is not always an Error. Coercing a plain object
  // with String() writes the literal "[object Object]" into the entry the
  // "show details" disclosure renders, so the owner gets a calm summary with
  // nothing behind it. Read the message, then describe the shape, then coerce.
  function messageFromError(error) {
    if (error instanceof Error) {
      // An Error's own message is the one branch that used to skip the bound,
      // and a parser or a fetch wrapper can carry a whole response body in it.
      // A reconstructed Error (`Object.create(Error.prototype)`) has no own
      // message, and `String(undefined)` wrote the literal "undefined" into the
      // entry. Coerce the absence to empty so the name path takes it. FE3 #4.
      return bounded(error.message ?? '') || coerce(error);
    }
    if (typeof error === 'string') {
      return bounded(error);
    }
    if (error && typeof error === 'object') {
      if (typeof error.message === 'string' && error.message) {
        return bounded(error.message);
      }
      return objectShape(error);
    }
    return coerce(error);
  }

  function stackFromError(error) {
    return error && typeof error.stack === 'string' ? error.stack : '';
  }

  // A page that is going away cancels its own in-flight GETs. The browser
  // reports that cancellation as an AbortError, or as a bare
  // "TypeError: Failed to fetch" with no status behind it, and every app that
  // was mid-read when the owner navigated would otherwise leave an error in
  // system messages that never happened to them.
  let unloading = false;
  let navigationGuard = null;
  function markUnloading() {
    unloading = true;
    dropDeferredAborts();
  }
  // pagehide only: a beforeunload listener costs the back/forward cache.
  window.addEventListener('pagehide', markUnloading);
  window.addEventListener('pageshow', () => {
    // a fresh load, or a document restored from the back/forward cache: either
    // way this document is live and its reads are its own again
    unloading = false;
    clearTimeout(navigationGuard);
  });

  // X-01: a same-tab link click cancels this page's in-flight GETs before
  // pagehide or any visibility change fires, so the guards above never covered
  // the most common way off a dashboard — and the owner read a phantom
  // "refreshVitals failed" on the page they had just asked for. A plain
  // same-origin anchor click, and a form submit, start the same navigation.
  function startsNavigation(event) {
    if (event.defaultPrevented) return false;
    if (typeof event.button === 'number' && event.button !== 0) return false;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return false;
    const anchor = event.target && event.target.closest ? event.target.closest('a[href]') : null;
    if (!anchor || anchor.hasAttribute('download')) return false;
    const target = (anchor.getAttribute('target') || '').trim().toLowerCase();
    if (target && target !== '_self') return false;
    let url;
    try {
      url = new URL(anchor.getAttribute('href'), document.baseURI);
    } catch (_) {
      return false;
    }
    // mailto:, javascript: and another origin all report a null/foreign origin
    if (url.origin !== window.location.origin) return false;
    // a hash on the document we are already on unloads nothing
    const here = window.location.href.split('#')[0];
    return url.href.split('#')[0] !== here;
  }

  // The flag must not latch: if a handler downstream of this one takes the
  // navigation over, or the browser declines it, this document is still live.
  function markUnloadingForNavigation() {
    markUnloading();
    clearTimeout(navigationGuard);
    navigationGuard = setTimeout(() => { unloading = false; }, 2000);
  }
  document.addEventListener('click', (e) => {
    if (startsNavigation(e)) markUnloadingForNavigation();
  });
  // A submit only unloads this document when it actually navigates it: a form
  // with target="_blank", or one whose handler preventDefaults after an await,
  // leaves the owner here, and the 2 s guard would swallow a genuine failure.
  // Same target/origin gate the click path uses.
  function submitStartsNavigation(event) {
    if (event.defaultPrevented) return false;
    const form = event.target;
    if (!form || typeof form.getAttribute !== 'function') return false;
    const target = (form.getAttribute('target') || '').trim().toLowerCase();
    if (target && target !== '_self') return false;
    let url;
    try {
      url = new URL(form.getAttribute('action') || '', document.baseURI);
    } catch (_) {
      return false;
    }
    return url.origin === window.location.origin;
  }
  document.addEventListener('submit', (e) => {
    if (submitStartsNavigation(e)) markUnloadingForNavigation();
  });
  // B22: pagehide fires on paths that never reach a bfcache restore, and the
  // flag would otherwise latch true for the life of the document and drop
  // every later cancelled-fetch-shaped error. A document that is visible again
  // is plainly not going away.
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') {
      unloading = false;
      return;
    }
    // X-01: a document that has just gone hidden is on its way somewhere. The
    // flag does not latch — the visible branch above and the 2 s guard below
    // both clear it — so a page that comes back reports its own failures again.
    markUnloadingForNavigation();
  });

  function looksLikeCancelledFetch(error) {
    if (!error) {
      return false;
    }
    const message = String(error.message || '');
    return (error instanceof TypeError || error.name === 'TypeError')
      && /failed to fetch|networkerror|load failed|network request failed/i.test(message);
  }

  function isNavigationAbort(error) {
    if (!error) {
      return false;
    }
    if (error.name === 'AbortError') {
      return true;
    }
    if (!looksLikeCancelledFetch(error)) {
      return false;
    }
    return unloading;
  }

  // The status instrument is the owner's, so it gets a sentence they can act
  // on. The developer context label and the browser's own exception text
  // ("date-nav:index: Failed to fetch") stay in the entry detail, which the
  // "show details" disclosure already renders (G1-118).
  const NETWORK_FAILURE = /failed to fetch|networkerror|load failed|network request failed/i;

  function ownerSummary(message) {
    const copy = window.CONVEY_COPY || {};
    return NETWORK_FAILURE.test(String(message || ''))
      ? (copy.CONSOLE_SUMMARY_LOAD_FAILED || "couldn't load part of this page.")
      : (copy.CONSOLE_SUMMARY_UNEXPECTED || "something on this page didn't work.");
  }

  // X-01: the guards above all key on a signal that arrives *before* the
  // rejection. An address-bar entry, a bookmark and a back button cancel this
  // page's in-flight GETs first and fire pagehide after, so the flag is still
  // false when the rejection lands and the owner reads a phantom failure on
  // the page they just left. A cancelled-fetch-shaped rejection therefore
  // waits out the navigation window before it is written down: if the document
  // is going away it never fires at all, and a genuine offline failure on a
  // page that stays is logged a second later, intact.
  function navigationWindowMs() {
    const override = Number(window.CONVEY_NAV_ABORT_WINDOW_MS);
    return Number.isFinite(override) && override >= 0 ? override : 1000;
  }
  // Keyed by its own timer so the drop path can say what it dropped: a
  // rejection that waits out the navigation window and is then discarded used
  // to leave no trace at all, which made a real read failure during a tab
  // switch indistinguishable from nothing having happened.
  const deferredAborts = new Map();
  function noteNavigationAbort(error, context) {
    if (window.console && typeof window.console.debug === 'function') {
      window.console.debug('navigation aborted a read:', error, context || '');
    }
  }
  function dropDeferredAborts() {
    deferredAborts.forEach((deferred, timer) => {
      clearTimeout(timer);
      noteNavigationAbort(deferred.error, deferred.context);
    });
    deferredAborts.clear();
  }

  window.logError = (error, context) => {
    if (isNavigationAbort(error)) {
      // still visible to a developer, but it is not a fault of this session
      noteNavigationAbort(error, context);
      return;
    }
    if (looksLikeCancelledFetch(error)) {
      const timer = setTimeout(() => {
        deferredAborts.delete(timer);
        recordError(error, context);
      }, navigationWindowMs());
      deferredAborts.set(timer, { error, context });
      return;
    }
    recordError(error, context);
  };

  function recordError(error, context) {
    markError();
    if (window.console && typeof window.console.error === 'function') {
      window.console.error(error, context || '');
    }
    const safe = safeContext(context);
    const message = messageFromError(error);
    const diagnosticConsole = window.convey?.diagnosticConsole;
    if (diagnosticConsole && typeof diagnosticConsole.push === 'function') {
      diagnosticConsole.push({
        severity: 'error',
        source: 'js',
        summary: ownerSummary(message),
        detail: {
          message,
          stack: stackFromError(error),
          filename: safe.filename,
          lineno: safe.lineno,
          colno: safe.colno,
          kind: safe.kind || 'logError',
          context: safe
        }
      });
    }
  }

  // Mark status icon as error state (red with glow)
  function markError() {
    const statusIcon = document.querySelector('#status-instrument .status-icon');
    if (statusIcon) {
      statusIcon.classList.add('error');
    }
  }

  // Global error handler
  window.addEventListener('error', (e) => {
    const error = e.error instanceof Error ? e.error : new Error(e.message || 'unknown error');
    window.logError(error, {
      kind: 'error',
      filename: e.filename,
      lineno: e.lineno,
      colno: e.colno
    });
  });

  // Unhandled promise rejection handler
  window.addEventListener('unhandledrejection', (e) => {
    // The reason is handed through as it came: logError reads a message off an
    // Error, a plain object or a string, and String()-ing it here was what put
    // "[object Object]" in front of the owner.
    const reason = e.reason;
    const error = (reason && typeof reason === 'object') || typeof reason === 'string'
      ? reason
      : new Error(coerce(reason ?? 'unknown rejection'));
    window.logError(error, { kind: 'unhandled-rejection' });
  });

  window.showError = (text) => {
    const errorModal = document.getElementById('errorModal');
    const errorMessage = document.getElementById('errorMessage');
    if (errorModal && errorMessage) {
      errorMessage.textContent = text;
      errorModal.style.display = 'block';
    }
  };

  function bindModalControls() {
    const errorModal = document.getElementById('errorModal');
    const closeButton = errorModal ? errorModal.querySelector('.close') : null;
    if (!errorModal || !closeButton) {
      return;
    }
    closeButton.onclick = () => {
      errorModal.style.display = 'none';
    };

    window.addEventListener('click', (e) => {
      if (e.target === errorModal) {
        errorModal.style.display = 'none';
      }
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', bindModalControls, { once: true });
  } else {
    bindModalControls();
  }
})();
