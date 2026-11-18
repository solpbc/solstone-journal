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
//     SolFocus.keepAfterPick(select, life, done)
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

  // After a pick from a select that stays shown. A phone's own menu for the
  // select can take focus off it as the menu closes, after the change and
  // with nothing pressed in the page. That focus goes back to the select, one
  // task later, unless the owner has moved first: a tap, a press or a key ends
  // the wait, and so do its window, its landing, the next pick and the view
  // going. A browser that keeps focus on the select never blurs it, and the
  // wait ends with nothing done.
  //
  // Called from the select's change, after the view's own work for the pick:
  // it arms only while the select still has focus and is still shown, so a
  // change that hid it, replaced it or moved focus off it keeps nothing. A
  // null select ends the earlier wait and keeps nothing, for a pick that
  // leaves the page.
  //
  // `life` is the view's mount: its listeners and timers go with the view.
  // A page that isn't a view leaves it out and lasts as long as the page.
  // `done`, if given, is called once when the wait ends, after its landing:
  // for a view that has more to draw once the pick has settled. It is called
  // at once when nothing arms, and never when the view goes first.
  // The window is twice the slowest delay seen on a phone from the change to
  // that blur (865 ms).
  const PICK_BLUR_MS = 1730;
  const pageLife = {
    on(target, type, fn, options) {
      target.addEventListener(type, fn, options);
      return () => target.removeEventListener(type, fn, options);
    },
    setTimeout: (fn, ms) => global.setTimeout(fn, ms),
    clearTimeout: (id) => global.clearTimeout(id)
  };
  const pickWaits = new WeakMap();

  function keepAfterPick(select, life, done) {
    life = life || pageLife;
    const settled = () => { if (typeof done === 'function') done(); };
    const earlier = pickWaits.get(life);
    if (earlier) earlier();
    const doc = select?.ownerDocument;
    if (!doc || doc.activeElement !== select || !isShown(select)) {
      settled();
      return false;
    }
    let landing = 0;
    let expiry = 0;
    let ended = false;
    const offs = [];
    const end = (targets) => {
      if (ended) return;
      ended = true;
      offs.splice(0).forEach((off) => off());
      life.clearTimeout(expiry);
      life.clearTimeout(landing);
      if (pickWaits.get(life) === stop) pickWaits.delete(life);
      if (targets) refocus(doc, true, targets);
      settled();
    };
    const stop = () => end();
    ['pointerdown', 'touchstart', 'mousedown', 'keydown'].forEach((type) => {
      offs.push(life.on(doc, type, stop, { capture: true }));
    });
    // Landed a task later: the menu is still closing as the blur arrives.
    // The view may have drawn the select again since the pick (a refused
    // write, a poll) and put focus on the new copy, which is then the one
    // blurred; it is heard on the document, and focus goes back to it.
    // A select the page took out isn't a menu closing: some browsers blur it
    // as it goes, while it is still in the document, so it is read a task
    // later, and the wait goes on for the copy drawn in its place.
    offs.push(life.on(doc, 'focusout', (event) => {
      const blurred = event.target;
      if (blurred !== select && blurred?.tagName !== 'SELECT') return;
      life.clearTimeout(landing);
      landing = life.setTimeout(() => {
        landing = 0;
        if (!blurred.isConnected) return;
        end([blurred, select]);
      }, 0);
    }, { capture: true }));
    expiry = life.setTimeout(stop, PICK_BLUR_MS);
    pickWaits.set(life, stop);
    return true;
  }

  const SolFocus = { setBusy, isBusy, isShown, focusLost, refocus, keepAfterPick };
  global.SolFocus = SolFocus;
  if (typeof module !== 'undefined' && module.exports) module.exports = SolFocus;
})(typeof window !== 'undefined' ? window : globalThis);
