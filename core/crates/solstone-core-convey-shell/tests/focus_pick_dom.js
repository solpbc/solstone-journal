// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// SolFocus.keepAfterPick: a select's focus after a phone's late blur.
//
// A small page with capture order (the document's capture listeners run
// before the target's), focus that moves only to shown elements, and a moved
// clock. The view's lifetime is the real view contract's (store.js, views.js).

const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const crateDir = process.argv[2] || path.join(__dirname, '..');
const staticDir = path.join(crateDir, 'assets', 'static');

let passed = 0;
const cases = [];
function test(name, fn) { cases.push({ name, fn }); }

// ---- the clock
function makeClock() {
  let now = 0;
  let serial = 0;
  const timers = new Map();
  return {
    setTimeout(fn, ms = 0, ...args) {
      const id = ++serial;
      timers.set(id, { at: now + Math.max(0, Number(ms) || 0), fn, args });
      return id;
    },
    clearTimeout(id) { timers.delete(id); },
    setInterval() { return 0; },
    clearInterval() {},
    live() { return timers.size; },
    advance(ms) {
      const until = now + ms;
      for (;;) {
        let next = null;
        for (const [id, timer] of timers) {
          if (timer.at <= until && (!next || timer.at < next[1].at || (timer.at === next[1].at && id < next[0]))) next = [id, timer];
        }
        if (!next) break;
        timers.delete(next[0]);
        now = next[1].at;
        next[1].fn(...next[1].args);
      }
      now = until;
    }
  };
}

// ---- the page
class Node {
  constructor(doc, tag) {
    this.ownerDocument = doc;
    this.tagName = tag;
    this.parentNode = null;
    this.children = [];
    this.listeners = [];
    this.hidden = false;
  }
  append(child) { child.parentNode = this; this.children.push(child); return child; }
  remove() {
    if (!this.parentNode) return;
    this.parentNode.children = this.parentNode.children.filter((c) => c !== this);
    this.parentNode = null;
  }
  get isConnected() {
    let at = this;
    while (at.parentNode) at = at.parentNode;
    return at === this.ownerDocument;
  }
  addEventListener(type, fn, options) {
    const capture = typeof options === 'boolean' ? options : !!options?.capture;
    if (this.listeners.some((l) => l.type === type && l.fn === fn && l.capture === capture)) return;
    this.listeners.push({ type, fn, capture });
  }
  removeEventListener(type, fn, options) {
    const capture = typeof options === 'boolean' ? options : !!options?.capture;
    this.listeners = this.listeners.filter((l) => !(l.type === type && l.fn === fn && l.capture === capture));
  }
  getClientRects() {
    for (let at = this; at && at !== this.ownerDocument; at = at.parentNode) if (at.hidden) return [];
    return this.isConnected ? [{ width: 10, height: 10 }] : [];
  }
  focus() {
    const doc = this.ownerDocument;
    if (!this.getClientRects().length || doc.activeElement === this) return;
    const from = doc.activeElement;
    doc.activeElement = this;
    if (from && from !== doc.body) {
      dispatch(from, 'blur', { relatedTarget: this, bubbles: false });
      dispatch(from, 'focusout', { relatedTarget: this });
    }
    dispatch(this, 'focus', { bubbles: false });
    dispatch(this, 'focusin', {});
  }
}

// Capture from the document down, then the target, then bubble up.
function dispatch(target, type, init = {}) {
  let stopped = false;
  const event = {
    type, target, relatedTarget: init.relatedTarget || null, defaultPrevented: false,
    preventDefault() { this.defaultPrevented = true; },
    stopPropagation() { stopped = true; }
  };
  const chain = [];
  for (let at = target; at; at = at.parentNode) chain.push(at);
  const run = (node, capture) => node.listeners.filter((l) => l.type === type && (capture === null || l.capture === capture)).forEach((l) => l.fn.call(node, event));
  for (let i = chain.length - 1; i > 0 && !stopped; i -= 1) run(chain[i], true);
  if (!stopped) run(target, null);
  if (init.bubbles !== false) for (let i = 1; i < chain.length && !stopped; i += 1) run(chain[i], false);
  return event;
}

function makePage() {
  const clock = makeClock();
  const doc = new Node(null, '#document');
  doc.ownerDocument = doc;
  Object.defineProperty(doc, 'isConnected', { value: true });
  doc.documentElement = doc.append(new Node(doc, 'HTML'));
  doc.body = doc.documentElement.append(new Node(doc, 'BODY'));
  doc.activeElement = doc.body;
  doc.title = '';
  const make = (tag, parent = doc.body) => parent.append(new Node(doc, tag));
  const window = {
    document: doc,
    location: { pathname: '/app/pick/', search: '', hash: '' },
    setTimeout: clock.setTimeout,
    clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval,
    clearInterval: clock.clearInterval,
    AbortController,
    console
  };
  window.window = window;
  window.globalThis = window;
  const context = vm.createContext(window);
  for (const name of ['store.js', 'views.js', 'focus.js']) {
    vm.runInContext(fs.readFileSync(path.join(staticDir, name), 'utf8'), context, { filename: name });
  }
  const SolFocus = window.SolFocus;
  // The owner's moves, and the phone's.
  const blurToBody = (el) => {
    if (doc.activeElement !== el) return;
    doc.activeElement = doc.body;
    dispatch(el, 'blur', { bubbles: false });
    dispatch(el, 'focusout', {});
  };
  const press = (el, type) => dispatch(el, type, {});
  const pick = (select, life) => {
    select.focus();
    dispatch(select, 'change', {});
    return SolFocus.keepAfterPick(select, life);
  };
  // A view on the real contract: its ctx, and a way to end it.
  let serial = 0;
  async function mountView() {
    const name = `pick${++serial}`;
    let ctx = null;
    window.SolViews.register(name, { mount(c) { ctx = c; return () => {}; } });
    await window.SolViews.mount(name, { location: window.location });
    return { ctx, end: () => window.SolViews.dispose() };
  }
  const listening = (node) => node.listeners.length;
  return { clock, doc, window, make, SolFocus, blurToBody, press, pick, mountView, listening };
}

// ---- cases

test('a pick, then the late blur to the page: focus back on the select one task later, and nothing left', async () => {
  const p = makePage();
  const view = await p.mountView();
  const select = p.make('SELECT');
  assert.ok(p.pick(select, view.ctx) === true, 'the wait arms');
  p.clock.advance(865);
  p.blurToBody(select);
  assert.ok(p.doc.activeElement === p.doc.body, 'not inside the blur itself');
  p.clock.advance(0);
  assert.ok(p.doc.activeElement === select, 'back on the select');
  assert.ok(p.listening(p.doc) === 0 && p.listening(select) === 0, 'no listener left');
  assert.ok(view.ctx.held().timeouts === 0, 'no timer left');
});

test('a second blur after the landing is left alone', async () => {
  const p = makePage();
  const view = await p.mountView();
  const select = p.make('SELECT');
  p.pick(select, view.ctx);
  p.clock.advance(865);
  p.blurToBody(select);
  p.clock.advance(0);
  p.blurToBody(select);
  p.clock.advance(10);
  assert.ok(p.doc.activeElement === p.doc.body, 'the second blur stays');
});

test('a blur at the window\'s last moment lands; one after it is left', async () => {
  const p = makePage();
  const view = await p.mountView();
  const a = p.make('SELECT');
  p.pick(a, view.ctx);
  p.clock.advance(1729);
  p.blurToBody(a);
  p.clock.advance(0);
  assert.ok(p.doc.activeElement === a, 'inside the window: lands');
  const b = p.make('SELECT');
  p.pick(b, view.ctx);
  p.clock.advance(1730);
  p.blurToBody(b);
  p.clock.advance(10);
  assert.ok(p.doc.activeElement === p.doc.body, 'after the window: left');
  assert.ok(view.ctx.held().timeouts === 0, 'the window took its timer');
});

test('no late blur: focus untouched, and the window takes the wait with it', async () => {
  const p = makePage();
  const view = await p.mountView();
  const select = p.make('SELECT');
  const before = view.ctx.held();
  p.pick(select, view.ctx);
  assert.ok(view.ctx.held().listeners === before.listeners + 5, 'four input listeners and the blur');
  p.clock.advance(2000);
  assert.ok(p.doc.activeElement === select, 'still on the select');
  assert.ok(view.ctx.held().listeners === before.listeners && view.ctx.held().timeouts === 0, 'nothing left');
  assert.ok(p.listening(p.doc) === 0 && p.listening(select) === 0, 'no listener on the page');
});

for (const type of ['pointerdown', 'touchstart', 'mousedown', 'keydown']) {
  test(`a ${type} before the blur ends the wait`, async () => {
    const p = makePage();
    const view = await p.mountView();
    const select = p.make('SELECT');
    const text = p.make('P');
    p.pick(select, view.ctx);
    p.clock.advance(300);
    p.press(text, type);
    p.blurToBody(select);
    p.clock.advance(10);
    assert.ok(p.doc.activeElement === p.doc.body, `${type}: left where the move put it`);
    assert.ok(view.ctx.held().timeouts === 0, `${type}: no timer left`);
  });
}

test('a control that stops its own press still ends the wait: it is heard in capture', async () => {
  const p = makePage();
  const view = await p.mountView();
  const select = p.make('SELECT');
  const button = p.make('BUTTON');
  button.addEventListener('pointerdown', (event) => event.stopPropagation());
  p.pick(select, view.ctx);
  p.clock.advance(300);
  p.press(button, 'pointerdown');
  p.blurToBody(select);
  p.clock.advance(10);
  assert.ok(p.doc.activeElement === p.doc.body, 'the press ended the wait');
});

test('a touch between the blur and its landing ends it', async () => {
  const p = makePage();
  const view = await p.mountView();
  const select = p.make('SELECT');
  const blank = p.make('DIV');
  p.pick(select, view.ctx);
  p.clock.advance(865);
  p.blurToBody(select);
  p.press(blank, 'touchstart');
  p.clock.advance(10);
  assert.ok(p.doc.activeElement === p.doc.body, 'the touch wins');
  assert.ok(view.ctx.held().timeouts === 0, 'the landing was cleared');
});

test('a tap on another control moves focus there, and the blur that follows is left', async () => {
  const p = makePage();
  const view = await p.mountView();
  const select = p.make('SELECT');
  const button = p.make('BUTTON');
  p.pick(select, view.ctx);
  p.clock.advance(400);
  p.press(button, 'pointerdown');
  button.focus();
  p.clock.advance(10);
  assert.ok(p.doc.activeElement === button, 'on the control');
});

test('a blur to something shown, with no input first, is left there', async () => {
  const p = makePage();
  const view = await p.mountView();
  const select = p.make('SELECT');
  const heading = p.make('H2');
  p.pick(select, view.ctx);
  p.clock.advance(865);
  heading.focus();
  p.clock.advance(10);
  assert.ok(p.doc.activeElement === heading, 'focus held elsewhere stays');
});

test('a second pick ends the first, on another select too', async () => {
  const p = makePage();
  const view = await p.mountView();
  const a = p.make('SELECT');
  const b = p.make('SELECT');
  p.pick(a, view.ctx);
  p.clock.advance(200);
  p.pick(b, view.ctx);
  assert.ok(p.listening(p.doc) === 5, 'one wait on the page: four inputs and the blur');
  assert.ok(view.ctx.held().timeouts === 1, 'one window left: the second\'s');
  p.clock.advance(865);
  p.blurToBody(b);
  p.clock.advance(0);
  assert.ok(p.doc.activeElement === b, 'the second lands');
});

test('a null select ends the earlier wait and keeps nothing', async () => {
  const p = makePage();
  const view = await p.mountView();
  const a = p.make('SELECT');
  p.pick(a, view.ctx);
  p.clock.advance(100);
  assert.ok(p.SolFocus.keepAfterPick(null, view.ctx) === false, 'nothing armed');
  assert.ok(p.listening(a) === 0 && p.listening(p.doc) === 0 && view.ctx.held().timeouts === 0, 'the earlier wait is gone');
  p.blurToBody(a);
  p.clock.advance(10);
  assert.ok(p.doc.activeElement === p.doc.body, 'nothing lands');
});

test('the earlier wait\'s queued landing is cleared by the next call', async () => {
  // A pick whose own work moves focus off the select (a hidden select, its
  // section's heading taking focus): the old wait hears that blur, and the
  // call after the work clears what it queued.
  const p = makePage();
  const view = await p.mountView();
  const select = p.make('SELECT');
  const heading = p.make('H2');
  p.pick(select, view.ctx);
  p.clock.advance(300);
  select.focus();
  dispatch(select, 'change', {});
  heading.focus();
  assert.ok(p.SolFocus.keepAfterPick(select, view.ctx) === false, 'a select without focus arms nothing');
  heading.hidden = true; // focus is lost before the old landing's task would run
  p.clock.advance(10);
  assert.ok(p.doc.activeElement === heading, 'the old wait landed nothing on the select');
  assert.ok(view.ctx.held().timeouts === 0, 'and left nothing');
});

test('the select drawn again before the blur: the new copy, blurred, gets focus back', async () => {
  // A refused write or a poll redraws the list; the view puts focus on the
  // new copy, and the phone's blur then takes it from that copy.
  const p = makePage();
  const view = await p.mountView();
  const old = p.make('SELECT');
  p.pick(old, view.ctx);
  p.clock.advance(200);
  const copy = p.make('SELECT');
  old.remove();
  copy.focus();
  p.clock.advance(665);
  p.blurToBody(copy);
  p.clock.advance(0);
  assert.ok(p.doc.activeElement === copy, 'back on the new copy');
  assert.ok(p.listening(p.doc) === 0 && view.ctx.held().timeouts === 0, 'nothing left');
});

test('something that isn\'t a select, blurred while the wait is armed, is left', async () => {
  const p = makePage();
  const view = await p.mountView();
  const select = p.make('SELECT');
  const input = p.make('INPUT');
  p.pick(select, view.ctx);
  // Focus on the field with no event the wait hears, then the field's blur.
  p.doc.activeElement = input;
  p.clock.advance(100);
  p.blurToBody(input);
  p.clock.advance(10);
  assert.ok(p.doc.activeElement === p.doc.body, 'not pulled back to the select');
});

test('a view that stops its own focusout still gets the landing: the blur is heard in capture', async () => {
  const p = makePage();
  const view = await p.mountView();
  const form = p.make('FORM');
  const select = p.make('SELECT', form);
  form.addEventListener('focusout', (event) => event.stopPropagation());
  p.pick(select, view.ctx);
  p.clock.advance(865);
  p.blurToBody(select);
  p.clock.advance(0);
  assert.ok(p.doc.activeElement === select, 'back on the select');
});

test('two blurs before the landing: it lands on the last one blurred', async () => {
  const p = makePage();
  const view = await p.mountView();
  const a = p.make('SELECT');
  const b = p.make('SELECT');
  p.pick(a, view.ctx);
  p.clock.advance(865);
  p.blurToBody(a);
  p.doc.activeElement = b; // the view put focus on another copy meanwhile
  p.blurToBody(b);
  p.clock.advance(0);
  assert.ok(p.doc.activeElement === b, 'on the one that last had focus');
  assert.ok(view.ctx.held().timeouts === 0, 'one landing, nothing left');
});

test('not armed: focus elsewhere, the select hidden, or gone', async () => {
  const p = makePage();
  const view = await p.mountView();
  const a = p.make('SELECT');
  const b = p.make('BUTTON');
  b.focus();
  assert.ok(p.SolFocus.keepAfterPick(a, view.ctx) === false, 'focus elsewhere');
  a.focus();
  a.hidden = true;
  assert.ok(p.SolFocus.keepAfterPick(a, view.ctx) === false, 'hidden');
  a.hidden = false;
  a.focus();
  a.remove();
  assert.ok(p.SolFocus.keepAfterPick(a, view.ctx) === false, 'gone');
  assert.ok(view.ctx.held().timeouts === 0 && p.listening(p.doc) === 0, 'nothing added');
});

test('the select gone before its landing: nothing lands', async () => {
  const p = makePage();
  const view = await p.mountView();
  const select = p.make('SELECT');
  p.pick(select, view.ctx);
  p.clock.advance(865);
  p.blurToBody(select);
  select.remove();
  p.clock.advance(10);
  assert.ok(p.doc.activeElement === p.doc.body, 'a gone select takes no focus');
});

test('the view goes before the blur: nothing lands, nothing is left', async () => {
  const p = makePage();
  const view = await p.mountView();
  const select = p.make('SELECT');
  p.pick(select, view.ctx);
  p.clock.advance(300);
  view.end();
  p.blurToBody(select);
  p.clock.advance(10);
  assert.ok(p.doc.activeElement === p.doc.body, 'nothing lands');
  assert.ok(p.listening(p.doc) === 0 && p.listening(select) === 0 && p.clock.live() === 0, 'nothing left');
});

test('the view goes between the blur and its landing: nothing lands, nothing is left', async () => {
  const p = makePage();
  const view = await p.mountView();
  const select = p.make('SELECT');
  p.pick(select, view.ctx);
  p.clock.advance(865);
  p.blurToBody(select);
  view.end();
  p.clock.advance(10);
  assert.ok(p.doc.activeElement === p.doc.body, 'nothing lands');
  assert.ok(p.listening(p.doc) === 0 && p.listening(select) === 0 && p.clock.live() === 0, 'nothing left');
});

test('a new view\'s first pick isn\'t held up by the last view\'s wait', async () => {
  const p = makePage();
  const first = await p.mountView();
  const a = p.make('SELECT');
  p.pick(a, first.ctx);
  const second = await p.mountView(); // mounting the next ends the first
  const b = p.make('SELECT');
  assert.ok(p.pick(b, second.ctx) === true, 'arms');
  p.clock.advance(865);
  p.blurToBody(b);
  p.clock.advance(0);
  assert.ok(p.doc.activeElement === b, 'lands on the new view\'s select');
});

test('no life: a page that isn\'t a view gets the same landing, on the page\'s timers', () => {
  const p = makePage();
  const select = p.make('SELECT');
  assert.ok(p.pick(select) === true, 'arms');
  p.clock.advance(865);
  p.blurToBody(select);
  p.clock.advance(0);
  assert.ok(p.doc.activeElement === select, 'back on the select');
  assert.ok(p.listening(p.doc) === 0 && p.listening(select) === 0 && p.clock.live() === 0, 'nothing left');
  const other = p.make('SELECT');
  p.pick(select);
  p.pick(other);
  assert.ok(p.listening(select) === 0 && p.clock.live() === 1, 'a second pick on the page ends the first');
  p.clock.advance(2000);
  assert.ok(p.clock.live() === 0 && p.listening(p.doc) === 0, 'the window ends it');
});

(async () => {
  for (const c of cases) {
    try {
      await c.fn();
      passed += 1;
    } catch (error) {
      console.log(`FAILED: ${c.name}`);
      console.log(error && error.stack ? error.stack : String(error));
      process.exit(1);
    }
  }
  console.log(`PICK CASES: ${passed} passed`);
})();
