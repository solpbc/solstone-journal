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
  var on = false;
  try {
    var p = new URL(window.location.href).searchParams.get('present');
    if (p === '1' || p === 'on' || p === 'true') {
      on = true;
      sessionStorage.setItem(KEY, '1');
    } else if (p === '0' || p === 'off' || p === 'false') {
      on = false;
      sessionStorage.removeItem(KEY);
    } else {
      on = sessionStorage.getItem(KEY) === '1';
    }
  } catch (_) {}
  if (on) document.body.classList.add('presentation-mode');
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
