// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  let resolved = false;
  let resolveReady;

  const readyPromise = new Promise((resolve) => {
    resolveReady = resolve;
  });

  function domReady() {
    if (document.readyState !== 'loading') {
      return Promise.resolve();
    }
    return new Promise((resolve) => {
      document.addEventListener('DOMContentLoaded', resolve, { once: true });
    });
  }

  window.solShellReady = readyPromise;

  window.resolveSolShellReady = function resolveSolShellReady(data) {
    if (resolved) return;
    resolved = true;
    window.solShellData = data || {};
    resolveReady(window.solShellData);
  };

  window.whenShellReady = function whenShellReady(callback) {
    return Promise.all([domReady(), window.solShellReady]).then((results) => {
      return callback(results[1]);
    });
  };
})();
