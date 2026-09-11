// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function() {
  document.getElementById('make-report')?.addEventListener('click', () => {
    const report = window.parent?.convey?.reportError;
    if (typeof report === 'function') {
      report({ source: 'support-page' });
    }
  });
})();
