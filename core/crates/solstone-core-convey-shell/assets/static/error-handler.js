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

  window.logError = (error, context) => {
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
        summary: safe.context ? `${safe.context}: ${message}` : message,
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
    const statusIcon = document.querySelector('.facet-bar .status-icon');
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
