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

  function messageFromError(error) {
    if (error instanceof Error) {
      return error.message || String(error);
    }
    if (typeof error === 'string') {
      return error;
    }
    return String(error ?? 'unknown error');
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
  document.addEventListener('submit', (e) => {
    if (!e.defaultPrevented) markUnloadingForNavigation();
  });
  // B22: pagehide fires on paths that never reach a bfcache restore, and the
  // flag would otherwise latch true for the life of the document and drop
  // every later cancelled-fetch-shaped error. A document that is visible again
  // is plainly not going away.
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') {
      unloading = false;
    }
  });

  function isNavigationAbort(error) {
    if (!error) {
      return false;
    }
    if (error.name === 'AbortError') {
      return true;
    }
    const message = String(error.message || '');
    const looksLikeCancelledFetch = (error instanceof TypeError || error.name === 'TypeError')
      && /failed to fetch|networkerror|load failed|network request failed/i.test(message);
    if (!looksLikeCancelledFetch) {
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

  window.logError = (error, context) => {
    if (isNavigationAbort(error)) {
      // still visible to a developer, but it is not a fault of this session
      if (window.console && typeof window.console.debug === 'function') {
        window.console.debug('navigation aborted a read:', error, context || '');
      }
      return;
    }
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
          stack: error instanceof Error ? (error.stack || '') : '',
          filename: safe.filename,
          lineno: safe.lineno,
          colno: safe.colno,
          kind: safe.kind || 'logError',
          context: safe
        }
      });
    }
  };

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
    const error = e.reason instanceof Error ? e.reason : new Error(String(e.reason ?? 'unknown rejection'));
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
