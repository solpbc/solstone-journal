// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Support on the view contract: this script only registers. The button opens
// the shell's report flow, which is the page's and outlives the view.
(function() {
  function mountSupport(ctx) {
    const button = document.getElementById('make-report');
    if (!button) throw new Error('support has no report button');
    ctx.on(button, 'click', () => {
      const report = window.convey?.reportError;
      if (typeof report === 'function') {
        report({ source: 'support-page' });
      }
    });
    ctx.ready('content');
  }

  window.SolViews.register('support', { mount: mountSupport });
})();
