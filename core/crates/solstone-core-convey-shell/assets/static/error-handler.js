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

  // X-01: a rejection reason is not always an Error. Coercing a plain object
  // with String() writes the literal "[object Object]" into the entry the
  // "show details" disclosure renders, so the owner gets a calm summary with
  // nothing behind it. Read the message, then serialise, then coerce.
  function messageFromError(error) {
    if (error instanceof Error) {
      return error.message || String(error);
    }
    if (typeof error === 'string') {
      return error;
    }
    if (error && typeof error === 'object') {
      if (typeof error.message === 'string' && error.message) {
        return error.message;
      }
      try {
        const json = JSON.stringify(error);
        if (json && json !== '{}' && json !== 'null') {
          return json;
        }
      } catch (_) {
        // a circular or unserialisable reason falls through to String()
      }
    }
    return String(error ?? 'unknown error');
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
  const deferredAborts = new Set();
  function dropDeferredAborts() {
    deferredAborts.forEach(timer => clearTimeout(timer));
    deferredAborts.clear();
  }

  window.logError = (error, context) => {
    if (isNavigationAbort(error)) {
      // still visible to a developer, but it is not a fault of this session
      if (window.console && typeof window.console.debug === 'function') {
        window.console.debug('navigation aborted a read:', error, context || '');
      }
      return;
    }
    if (looksLikeCancelledFetch(error)) {
      const timer = setTimeout(() => {
        deferredAborts.delete(timer);
        recordError(error, context);
      }, navigationWindowMs());
      deferredAborts.add(timer);
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
      : new Error(String(reason ?? 'unknown rejection'));
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
