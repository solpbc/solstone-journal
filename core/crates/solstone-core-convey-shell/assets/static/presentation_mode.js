// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Presentation mode (?present=1, Shift+P toggle, sessionStorage-persisted).
// Boosts typography + contrast for back-of-room legibility on a projector.
(function () {
  var KEY = 'solstone:presentation-mode';
  function emit(on) {
    try {
      window.dispatchEvent(
        new CustomEvent('presentation-mode:change', { detail: { on: on } })
      );
    } catch (_) {}
  }
  function set(on) {
    try {
      if (on) sessionStorage.setItem(KEY, '1');
      else sessionStorage.removeItem(KEY);
    } catch (_) {}
    document.body.classList.toggle('presentation-mode', !!on);
    emit(!!on);
  }
  function isOn() {
    return document.body.classList.contains('presentation-mode');
  }
  // Whether it starts on is decided once, by the shell page's inline script
  // before the first paint; this only reads the class that leaves.
  document.addEventListener('keydown', function (e) {
    if (e.key !== 'P' || !e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
    var t = e.target;
    if (
      t &&
      (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)
    ) {
      return;
    }
    e.preventDefault();
    set(!isOn());
  });
  window.solstonePresentation = {
    on: function () {
      set(true);
    },
    off: function () {
      set(false);
    },
    toggle: function () {
      set(!isOn());
    },
    isOn: isOn
  };
})();
