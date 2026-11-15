// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Focus that stays where the owner is, for a view that draws its markup
// again after a press, a poll or a dialog's close.
//
//     SolFocus.setBusy(el, busy)   a control whose write is out keeps focus
//     SolFocus.isBusy(el)
//     SolFocus.isShown(el)
//     SolFocus.focusLost(doc)
//     SolFocus.refocus(doc, held, targets)
//
// The shell carries it in its boot part, so every view has it before it mounts
// and no view asks for it on its own.
(function (global) {
  // A control whose write is out keeps focus. It is marked busy and ignores
  // presses until the write answers; `disabled` would drop focus to the page.
  function setBusy(el, busy) {
    if (!el || typeof el.setAttribute !== 'function') return;
    if (busy) {
      el.setAttribute('aria-disabled', 'true');
      el.setAttribute('aria-busy', 'true');
    } else {
      el.removeAttribute('aria-disabled');
      el.removeAttribute('aria-busy');
    }
  }

  function isBusy(el) {
    return Boolean(el && typeof el.getAttribute === 'function' && el.getAttribute('aria-disabled') === 'true');
  }

  // Content in a closed <details> still has layout boxes in some engines, so
  // ask checkVisibility() too where there is one.
  function isShown(el) {
    if (!el || !el.isConnected || typeof el.getClientRects !== 'function' || !el.getClientRects().length) return false;
    return typeof el.checkVisibility === 'function' ? el.checkVisibility() : true;
  }

  // Focus is lost when it is on the page itself, or on something the page has
  // hidden or taken out (a browser may not have moved it off yet).
  function focusLost(doc) {
    const active = doc?.activeElement;
    return !active || active === doc.body || active === doc.documentElement || !isShown(active);
  }

  // After a redraw: when focus was held and is now lost, the first target
  // still shown takes it. Focus held elsewhere, or nowhere, is left alone.
  function refocus(doc, held, targets) {
    if (!held || !focusLost(doc)) return false;
    // A target the browser won't focus (inert, say) passes to the next.
    for (const target of targets || []) {
      if (!isShown(target)) continue;
      target.focus();
      if (doc.activeElement === target) return true;
    }
    return false;
  }

  const SolFocus = { setBusy, isBusy, isShown, focusLost, refocus };
  global.SolFocus = SolFocus;
  if (typeof module !== 'undefined' && module.exports) module.exports = SolFocus;
})(typeof window !== 'undefined' ? window : globalThis);
