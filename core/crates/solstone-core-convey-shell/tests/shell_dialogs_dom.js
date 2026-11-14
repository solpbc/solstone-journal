// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// The shell's two dialogs, the report review and the error box, under plain
// Node: the real modal layer, error handler, report flow and diagnostic
// console, with the error box and the console taken from the real shell page,
// in a stand-in page with capture and bubble, focus events, inert, the
// browser's focus fixup and a frame queue the cases flush by hand. With the
// shell's notices loaded too (the real app.js), a fake clock and the pointer,
// the cases also check that a notice's countdown waits under a dialog.
// Every compare is of plain values, ids or tag names: never an element.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const crateDir = path.resolve(__dirname, '..');
const staticDir = path.join(crateDir, 'assets', 'static');
const VOID_TAGS = new Set(['br', 'hr', 'img', 'input', 'link', 'meta']);

// ---- a small document

class Target {
  constructor() { this.listeners = new Map(); }
  addEventListener(type, fn, options) {
    const capture = typeof options === 'boolean' ? options : !!options?.capture;
    const list = this.listeners.get(type) || [];
    if (!list.some((l) => l.fn === fn && l.capture === capture)) list.push({ fn, capture, once: !!options?.once });
    this.listeners.set(type, list);
  }
  removeEventListener(type, fn, options) {
    const capture = typeof options === 'boolean' ? options : !!options?.capture;
    const list = this.listeners.get(type) || [];
    this.listeners.set(type, list.filter((l) => !(l.fn === fn && l.capture === capture)));
  }
  // One phase's listeners on this node; false once propagation is stopped.
  invoke(event, capture) {
    event.currentTarget = this;
    for (const entry of [...(this.listeners.get(event.type) || [])]) {
      if (entry.capture !== capture) continue;
      if (entry.once) this.removeEventListener(event.type, entry.fn, entry.capture);
      entry.fn.call(this, event);
      if (event.immediateStopped) break;
    }
    if (!capture) {
      const handler = this['on' + event.type];
      if (typeof handler === 'function') handler.call(this, event);
    }
    return !event.stopped;
  }
}

class TextNode {
  constructor(text) { this.nodeType = 3; this.data = String(text); this.parentNode = null; }
  get textContent() { return this.data; }
}

function simpleMatch(el, part) {
  const re = /([a-zA-Z][a-zA-Z0-9-]*)|#([\w-]+)|\.([\w-]+)|\[([\w-]+)(?:=["']?([^\]"']*)["']?)?\]/g;
  let m;
  let any = false;
  while ((m = re.exec(part))) {
    any = true;
    if (m[1] && el.tagName !== m[1].toUpperCase()) return false;
    if (m[2] && el.getAttribute('id') !== m[2]) return false;
    if (m[3] && !el.classList.contains(m[3])) return false;
    if (m[4]) {
      if (!el.hasAttribute(m[4])) return false;
      if (m[5] !== undefined && el.getAttribute(m[4]) !== m[5]) return false;
    }
  }
  return any;
}

function matches(el, selector) {
  return selector.split(',').some((one) => {
    const parts = one.trim().split(/\s+/);
    if (!simpleMatch(el, parts[parts.length - 1])) return false;
    let at = el.parentNode;
    for (let i = parts.length - 2; i >= 0; i -= 1) {
      while (at && !(at.nodeType === 1 && simpleMatch(at, parts[i]))) at = at.parentNode;
      if (!at) return false;
      at = at.parentNode;
    }
    return true;
  });
}

class Element extends Target {
  constructor(doc, tagName) {
    super();
    this.ownerDocument = doc;
    this.nodeType = 1;
    this.tagName = tagName.toUpperCase();
    this.attrs = new Map();
    this.childNodes = [];
    this.parentNode = null;
    this.value = '';
    this.disabled = false;
    this.style = {};
    const self = this;
    this.classList = {
      list: () => (self.getAttribute('class') || '').split(/\s+/).filter(Boolean),
      add(...cs) { const l = this.list(); cs.forEach((c) => { if (!l.includes(c)) l.push(c); }); self.setAttribute('class', l.join(' ')); },
      remove(...cs) { self.setAttribute('class', this.list().filter((x) => !cs.includes(x)).join(' ')); },
      contains(c) { return this.list().includes(c); },
      toggle(c, on) { const want = on === undefined ? !this.contains(c) : !!on; if (want) this.add(c); else this.remove(c); return want; },
    };
    const doc2 = doc;
    this.style = new Proxy({}, {
      set(target, key, value) { target[key] = String(value); doc2.mutated(self, 'attributes', 'style'); return true; },
    });
  }
  get dataset() {
    const self = this;
    const attr = (k) => 'data-' + String(k).replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());
    return new Proxy({}, {
      get(_t, k) { return typeof k === 'string' ? (self.getAttribute(attr(k)) ?? undefined) : undefined; },
      set(_t, k, v) { self.setAttribute(attr(k), v); return true; },
    });
  }
  get children() { return this.childNodes.filter((n) => n.nodeType === 1); }
  get parentElement() { return this.parentNode && this.parentNode.nodeType === 1 ? this.parentNode : null; }
  get nextElementSibling() {
    const siblings = this.parentNode ? this.parentNode.children : [];
    return siblings[siblings.indexOf(this) + 1] || null;
  }
  get id() { return this.getAttribute('id') || ''; }
  set id(v) { this.setAttribute('id', v); }
  get className() { return this.getAttribute('class') || ''; }
  set className(v) { this.setAttribute('class', v); }
  get type() { return this.getAttribute('type') || ''; }
  set type(v) { this.setAttribute('type', v); }
  get href() { return this.getAttribute('href') || ''; }
  set href(v) { this.setAttribute('href', v); }
  get target() { return this.getAttribute('target') || ''; }
  set target(v) { this.setAttribute('target', v); }
  get rel() { return this.getAttribute('rel') || ''; }
  set rel(v) { this.setAttribute('rel', v); }
  get maxLength() { return Number(this.getAttribute('maxlength') || -1); }
  set maxLength(v) { this.setAttribute('maxlength', String(v)); }
  get hidden() { return this.hasAttribute('hidden'); }
  set hidden(v) { if (v) this.setAttribute('hidden', ''); else this.removeAttribute('hidden'); }
  get inert() { return this.hasAttribute('inert'); }
  set inert(v) { if (v) this.setAttribute('inert', ''); else this.removeAttribute('inert'); }
  setAttribute(k, v) { this.attrs.set(k, String(v)); this.ownerDocument.mutated(this, 'attributes', k); }
  getAttribute(k) { return this.attrs.has(k) ? this.attrs.get(k) : null; }
  hasAttribute(k) { return this.attrs.has(k); }
  removeAttribute(k) { this.attrs.delete(k); this.ownerDocument.mutated(this, 'attributes', k); }
  get textContent() { return this.childNodes.map((n) => n.textContent).join(''); }
  set textContent(v) { this.replaceChildren(); if (String(v ?? '')) this.append(String(v)); }
  set innerHTML(html) { this.replaceChildren(); parseInto(this.ownerDocument, this, String(html ?? '')); }
  get innerHTML() { return this.textContent; }
  get isConnected() {
    let at = this;
    while (at.parentNode) at = at.parentNode;
    return at === this.ownerDocument.documentElement;
  }
  contains(node) {
    for (let at = node; at; at = at.parentNode) if (at === this) return true;
    return false;
  }
  matches(selector) {
    if (selector === ':hover') return Boolean(this.ownerDocument.hovered && this.contains(this.ownerDocument.hovered));
    if (selector === ':focus-within') return this.contains(this.ownerDocument.activeElement);
    return matches(this, selector);
  }
  closest(selector) {
    for (let at = this; at && at.nodeType === 1; at = at.parentNode) if (matches(at, selector)) return at;
    return null;
  }
  // Shown: connected, and no ancestor hidden or not displayed.
  shown() {
    if (!this.isConnected) return false;
    for (let at = this; at && at.nodeType === 1; at = at.parentNode) {
      if (at.hasAttribute('hidden') || this.ownerDocument.displayOf(at) === 'none') return false;
    }
    return true;
  }
  inInert() {
    for (let at = this; at && at.nodeType === 1; at = at.parentNode) if (at.hasAttribute('inert')) return true;
    return false;
  }
  adopt(node) {
    if (node.parentNode) node.parentNode.removeChild(node);
    node.parentNode = this;
    return node;
  }
  append(...nodes) {
    for (let node of nodes) {
      if (node === null || node === undefined) continue;
      if (typeof node === 'string') node = new TextNode(node);
      this.childNodes.push(this.adopt(node));
    }
    this.ownerDocument.mutated(this, 'childList');
  }
  appendChild(node) { this.append(node); return node; }
  removeChild(node) {
    const i = this.childNodes.indexOf(node);
    if (i >= 0) this.childNodes.splice(i, 1);
    node.parentNode = null;
    this.ownerDocument.mutated(this, 'childList');
    return node;
  }
  remove() { if (this.parentNode) this.parentNode.removeChild(this); }
  replaceChildren(...nodes) {
    [...this.childNodes].forEach((n) => this.removeChild(n));
    this.append(...nodes);
  }
  querySelectorAll(selector) {
    const out = [];
    const walk = (el) => el.children.forEach((c) => { if (matches(c, selector)) out.push(c); walk(c); });
    walk(this);
    return out;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  // As a browser: an element that isn't shown, is turned off or is inert
  // refuses focus, and a move of focus fires focusin.
  focus() {
    if (!this.shown() || this.disabled || this.inInert()) return;
    const doc = this.ownerDocument;
    if (doc.focused === this) return;
    const was = doc.focused;
    doc.focused = this;
    if (was && was !== doc.body && was.isConnected) doc.dispatch(was, { type: 'focusout', bubbles: true, relatedTarget: this });
    doc.dispatch(this, { type: 'focusin', bubbles: true });
  }
  // A pointer press, as in Chromium: it focuses the nearest focusable
  // ancestor, or else the page, then clicks.
  click() {
    const doc = this.ownerDocument;
    const focusable = this.closest('button, a, textarea, input, select, [tabindex]');
    if (focusable) focusable.focus();
    else doc.focused = doc.body;
    return doc.dispatch(this, { type: 'click', bubbles: true, button: 0 });
  }
}

const ENTITIES = { '&times;': '×', '&nbsp;': ' ' };
function decode(text) {
  return text.replace(/&[a-z]+;/g, (e) => ENTITIES[e] || e)
    .replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&amp;/g, '&');
}

function parseInto(doc, parent, html) {
  const stack = [parent];
  const re = /<!--[\s\S]*?-->|<\/([a-zA-Z0-9]+)\s*>|<([a-zA-Z][a-zA-Z0-9-]*)((?:\s+[^\s=>]+(?:=(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*\/?>|([^<]+)/g;
  let m;
  while ((m = re.exec(html))) {
    const top = stack[stack.length - 1];
    if (m[0].startsWith('<!--')) continue;
    if (m[1]) {
      const tag = m[1].toUpperCase();
      while (stack.length > 1 && stack.pop().tagName !== tag) { /* close */ }
      continue;
    }
    if (m[2]) {
      const el = doc.createElement(m[2]);
      const attrRe = /([^\s=>]+)(?:=("[^"]*"|'[^']*'|[^\s>]+))?/g;
      let a;
      while ((a = attrRe.exec(m[3] || ''))) {
        let v = a[2] === undefined ? '' : a[2];
        if (/^["']/.test(v)) v = v.slice(1, -1);
        el.setAttribute(a[1], decode(v));
      }
      top.append(el);
      if (!VOID_TAGS.has(m[2].toLowerCase())) stack.push(el);
      continue;
    }
    if (m[4] && m[4].trim()) top.append(decode(m[4]));
  }
}

// The shell page's own markup for a part, by its opening tag.
function shellPart(open, close) {
  const html = fs.readFileSync(path.join(staticDir, 'shell.html'), 'utf8');
  const start = html.indexOf(open);
  assert.ok(start >= 0, `shell.html has ${open}`);
  const end = html.indexOf(close, start);
  return html.slice(start, end + close.length);
}

function makePage(options = {}) {
  const win = new Target();
  const doc = new Target();
  const frames = [];
  const observers = [];
  doc.readyState = 'complete';
  // As in a browser, observers hear a change at the next microtask: after
  // the task or frame callback that made it, not inside it.
  // Each observer hears only what it asked for: its target, or below it with
  // subtree; child lists, or attributes through its filter.
  doc.mutated = (node, kind, name) => observers.forEach((o) => {
    if (!o.on || !node) return;
    const { target, options: opts } = o;
    if (node !== target && !(opts.subtree && target.contains(node))) return;
    if (kind === 'childList' ? !opts.childList
      : !(opts.attributes && (!opts.attributeFilter || opts.attributeFilter.includes(name)))) return;
    o.pending = true;
  });
  const deliver = () => {
    for (let i = 0; i < 10; i += 1) {
      const due = observers.filter((o) => o.on && o.pending);
      if (!due.length) return;
      due.forEach((o) => { o.pending = false; o.callback([]); });
    }
  };
  doc.createElement = (tag) => new Element(doc, tag);
  doc.documentElement = new Element(doc, 'html');
  doc.body = new Element(doc, 'body');
  doc.documentElement.append(doc.body);
  // The page's own stylesheet, as far as these cases need it: a .modal is
  // not displayed until its inline style shows it.
  doc.displayOf = (el) => {
    if (el.style.display) return el.style.display;
    if (el.classList.contains('modal')) return 'none';
    return 'block';
  };
  // The browser's focus fixup: focus on something taken out, hidden or
  // made inert reads as the page itself.
  doc.focused = doc.body;
  Object.defineProperty(doc, 'activeElement', {
    get() {
      const f = doc.focused;
      if (!f || f === doc.body || !f.shown() || f.inInert() || f.disabled) return doc.body;
      return f;
    },
  });
  doc.getElementById = (id) => doc.documentElement.querySelector('#' + id);
  doc.querySelector = (s) => doc.documentElement.querySelector(s);
  doc.querySelectorAll = (s) => doc.documentElement.querySelectorAll(s);
  // Capture from the window down, the target, then bubble back up.
  doc.dispatch = (target, init) => {
    const event = Object.assign({
      target,
      bubbles: false,
      defaultPrevented: false,
      stopped: false,
      immediateStopped: false,
      preventDefault() { this.defaultPrevented = true; },
      stopPropagation() { this.stopped = true; },
      stopImmediatePropagation() { this.stopped = true; this.immediateStopped = true; },
    }, init);
    const chain = [];
    for (let at = target; at; at = at.parentNode) chain.push(at);
    if (target !== doc && target !== win) chain.push(doc);
    chain.push(win);
    const down = chain.slice().reverse();
    for (const node of down.slice(0, -1)) if (!node.invoke(event, true)) return event;
    if (!target.invoke(event, true)) return event;
    if (!target.invoke(event, false)) return event;
    if (!event.bubbles) return event;
    for (const node of chain.slice(1)) if (!node.invoke(event, false)) return event;
    return event;
  };
  doc.dispatchEvent = (event) => { doc.dispatch(doc, event); return !event.defaultPrevented; };
  win.dispatchEvent = (event) => { doc.dispatch(win, event); return !event.defaultPrevented; };
  win.document = doc;
  win.window = win;
  win.requestAnimationFrame = (fn) => { frames.push(fn); return frames.length; };
  win.getComputedStyle = (el) => ({
    display: doc.displayOf(el),
    position: el.classList.contains('modal') ? 'fixed' : 'static',
  });
  win.whenShellReady = (fn) => fn({});
  win.sessionStorage = { getItem() { return null; }, setItem() {}, removeItem() {} };
  win.location = { pathname: '/app/support/', href: 'http://journal.test/app/support/', origin: 'http://journal.test' };
  win.CustomEvent = class CustomEvent { constructor(type, init = {}) { this.type = type; this.detail = init.detail; } };
  win.fetch = async (url) => {
    assert.ok(String(url) === '/app/support/api/context', `unexpected fetch ${url}`);
    return { ok: true, json: async () => ({ version: '2.0.0', os: 'linux', os_version: '6', about: '' }) };
  };
  win.console = { error() {}, debug() {}, warn() {}, log() {} };

  // The page: the rail, a workspace with its controls, a notice, the console
  // and the error box from the shell page, in the shell page's order.
  doc.body.innerHTML = [
    '<nav id="app-rail"><a id="rail-home" href="/app/home/">home</a></nav>',
    '<main id="main-content" class="workspace" tabindex="-1">',
    '<h1 id="support-title" tabindex="-1">support</h1>',
    '<button id="make-report" type="button">make a report</button>',
    '<button id="unpair" type="button">unpair</button>',
    '<button id="view-open" type="button">open</button>',
    '<div id="view-dialog" class="modal" role="dialog" aria-modal="true">',
    '<button id="view-first" type="button">first</button>',
    '<button id="view-save" type="button">save</button>',
    '</div>',
    options.notices ? [
      '<div id="confirm-dialog" class="modal" role="dialog" aria-modal="true">',
      '<button id="confirm-yes" type="button">delete</button>',
      '</div>',
    ].join('') : '',
    '</main>',
    shellPart('<aside id="diagnostic-console"', '</aside>'),
    '<div class="notification-center" id="notification-center">',
    '<button id="recover" type="button">recover</button>',
    '</div>',
    shellPart('<div id="errorModal"', '</div>\n</div>'),
  ].join('');

  // A clock the cases move by hand, for the notices' countdowns.
  const clock = { now: 0, next: 1, timers: new Map() };
  const later = (fn, ms, every) => {
    const id = clock.next++;
    clock.timers.set(id, { fn, at: clock.now + Math.max(0, Number(ms) || 0), every: every ? Math.max(1, Number(ms) || 0) : 0 });
    return id;
  };
  const cancel = (id) => { clock.timers.delete(id); };
  const timing = options.notices
    ? { setTimeout: (fn, ms) => later(fn, ms, false), clearTimeout: cancel, setInterval: (fn, ms) => later(fn, ms, true), clearInterval: cancel }
    : { setTimeout, clearTimeout, setInterval, clearInterval };
  win.localStorage = { getItem() { return null; }, setItem() {}, removeItem() {} };
  win.navigator = { userAgent: 'node' };

  const context = vm.createContext({
    window: win,
    document: doc,
    Element,
    MutationObserver: class {
      constructor(callback) { this.callback = () => callback([]); this.on = false; observers.push(this); }
      observe(target, options) { this.on = true; this.target = target; this.options = options || {}; }
      disconnect() { this.on = false; }
    },
    requestAnimationFrame: win.requestAnimationFrame,
    getComputedStyle: win.getComputedStyle,
    CustomEvent: win.CustomEvent,
    URLSearchParams,
    URL,
    ...timing,
    navigator: win.navigator,
    fetch: win.fetch,
    console: win.console,
  });
  context.globalThis = context;
  // The notices are in the boot file, the layer in the rest that loads after.
  const files = options.notices ? ['convey_icons.js', 'app.js'] : [];
  files.push('modal_layer.js', 'error-handler.js', 'report-error.js');
  if (options.console) files.push('diagnostic-console.js');
  for (const file of files) {
    vm.runInContext(fs.readFileSync(path.join(staticDir, file), 'utf8'), context, { filename: file });
  }
  // The real page reconciles on the frame after a change; so does this one,
  // when a case calls frame().
  const frame = () => {
    deliver();
    for (let i = 0; i < 5 && frames.length; i += 1) frames.splice(0).forEach((fn) => { fn(); deliver(); });
  };
  frame();
  const page = {
    win,
    doc,
    frame,
    el: (id) => doc.getElementById(id),
    focusId() {
      const a = doc.activeElement;
      return a === doc.body ? 'BODY' : (a.id || a.tagName);
    },
    key(key) {
      const target = doc.activeElement;
      return doc.dispatch(target, { type: 'keydown', key, bubbles: true });
    },
    back() { doc.dispatch(win, { type: 'popstate' }); },
    async openReport() {
      win.convey.reportError({ source: 'support-page' });
      await settle();
      return doc.querySelector('.report-error-modal');
    },
    errorOpen() { return page.el('errorModal').style.display === 'block'; },
    // Move the clock, running what falls due, in order.
    tick(ms) {
      deliver();
      const end = clock.now + ms;
      for (;;) {
        let due = null;
        clock.timers.forEach((t, id) => { if (t.at <= end && (!due || t.at < due.t.at)) due = { id, t }; });
        if (!due) break;
        clock.now = due.t.at;
        if (due.t.every) due.t.at += due.t.every;
        else clock.timers.delete(due.id);
        due.t.fn();
        deliver();
      }
      clock.now = end;
    },
    observers: () => observers.length,
    notices: () => win.AppServices.notifications,
    card: (id) => doc.querySelector(`.notification-card[data-id="${id}"]`),
    // The pointer: over an element, or off it. mouseenter and mouseleave
    // don't bubble.
    hover(el) {
      const was = doc.hovered;
      doc.hovered = el;
      if (was && !(el && was.contains(el))) doc.dispatch(was, { type: 'mouseleave', bubbles: false });
      if (el) doc.dispatch(el, { type: 'mouseenter', bubbles: false });
    },
    showDialog(id) { page.el(id).style.display = 'block'; },
    hideDialog(id) { page.el(id).style.display = 'none'; },
  };
  return page;
}

const settle = () => new Promise((resolve) => setImmediate(resolve));

let cases = 0;
const failed = [];
async function test(name, fn) {
  try {
    await fn();
    cases += 1;
  } catch (error) {
    failed.push(`${name}: ${error.message}`);
  }
}

function focusIs(page, id, what) {
  const at = page.focusId();
  assert.ok(at === id, `${what}: focus on ${at}, expected ${id}`);
}

async function main() {
  // ---- the report review

  for (const [how, close] of [
    ['cancel', (p, modal) => modal.querySelector('.report-error-actions button').click()],
    ['Escape', (p) => p.key('Escape')],
    ['a click on its backdrop', (p, modal) => modal.click()],
  ]) {
    await test(`the review closed by ${how} gives focus back to what opened it`, async () => {
      const p = makePage();
      p.el('make-report').focus();
      const modal = await p.openReport();
      p.frame();
      focusIs(p, 'report-error-recent', 'open');
      assert.ok(p.el('main-content').hasAttribute('inert'), 'the page is inert while it is open');
      assert.ok(modal.hasAttribute('data-convey-active-dialog'), 'the review is the active dialog');
      close(p, modal);
      assert.ok(!p.doc.querySelector('.report-error-modal'), 'the review is gone');
      focusIs(p, 'make-report', 'closed');
      assert.ok(!p.el('main-content').hasAttribute('inert'), 'the page is not inert after');
      p.frame();
      focusIs(p, 'make-report', 'after the next frame');
    });
  }

  await test('the review closed when what opened it is gone puts focus on <main>', async () => {
    const p = makePage();
    p.el('make-report').focus();
    const modal = await p.openReport();
    p.frame();
    p.el('make-report').remove();
    modal.querySelector('.report-error-actions button').click();
    focusIs(p, 'main-content', 'closed');
  });

  await test('the review opened from a notice gives focus back to the notice, which still works', async () => {
    const p = makePage();
    let recovered = 0;
    p.el('recover').addEventListener('click', () => { recovered += 1; });
    p.el('recover').focus();
    await p.openReport();
    p.frame();
    assert.ok(p.el('notification-center').hasAttribute('inert'), 'the notice is inert under the review');
    p.key('Escape');
    focusIs(p, 'recover', 'closed');
    p.el('recover').click();
    assert.ok(recovered === 1, 'recover still takes a press');
  });

  // ---- the error box

  await test('the error box is a named modal dialog that takes focus and keeps the page inert', async () => {
    const p = makePage();
    const box = p.el('errorModal');
    assert.ok(box.getAttribute('role') === 'dialog' && box.getAttribute('aria-modal') === 'true', 'modal dialog markup');
    assert.ok(box.getAttribute('aria-labelledby') === 'errorMessage', 'named by its message');
    p.el('unpair').focus();
    p.win.showError("couldn't unpair that device.");
    assert.ok(p.doc.activeElement.classList.contains('close'), 'focus on its close button');
    p.frame();
    assert.ok(p.el('main-content').hasAttribute('inert'), 'the page is inert while it is open');
    p.el('unpair').focus();
    p.frame();
    assert.ok(p.doc.activeElement.classList.contains('close'), 'focus stays in the box');
  });

  for (const [how, close] of [
    ['its close button', (p) => p.el('errorModal').querySelector('.close').click()],
    ['Escape', (p) => p.key('Escape')],
    ['a click on its backdrop', (p) => p.el('errorModal').click()],
  ]) {
    await test(`the error box closed by ${how} gives focus back to where it was`, async () => {
      const p = makePage();
      p.el('unpair').focus();
      p.win.showError("couldn't unpair that device.");
      p.frame();
      close(p);
      assert.ok(!p.errorOpen(), 'the box is closed');
      focusIs(p, 'unpair', 'closed');
      p.frame();
      focusIs(p, 'unpair', 'after the next frame');
    });
  }

  await test('the error box closed when what had focus is gone puts focus on <main>', async () => {
    const p = makePage();
    p.el('unpair').focus();
    p.win.showError('x');
    p.frame();
    p.el('unpair').remove();
    p.el('errorModal').querySelector('.close').click();
    focusIs(p, 'main-content', 'closed');
  });

  await test('a second showError replaces the text and keeps focus in the box', async () => {
    const p = makePage();
    p.el('unpair').focus();
    p.win.showError('first');
    p.frame();
    p.win.showError('second');
    p.frame();
    assert.ok(p.el('errorMessage').textContent === 'second', 'the text is replaced');
    assert.ok(p.doc.activeElement.classList.contains('close'), 'focus stays on its close button');
    p.key('Escape');
    focusIs(p, 'unpair', 'one close returns');
  });

  // ---- one over the other

  await test('the error over the review closes back into the review, where focus was', async () => {
    const p = makePage();
    p.el('make-report').focus();
    const modal = await p.openReport();
    p.frame();
    modal.querySelector('.report-error-actions button').focus();
    p.win.showError('x');
    assert.ok(p.doc.activeElement.classList.contains('close'), 'the error takes focus at once');
    p.frame();
    assert.ok(p.el('errorModal').hasAttribute('data-convey-active-dialog'), 'the error is on top');
    assert.ok(modal.hasAttribute('inert'), 'the review is inert under it');
    p.key('Escape');
    assert.ok(p.doc.querySelector('.report-error-modal'), 'one Escape leaves the review open');
    assert.ok(p.doc.activeElement.textContent === 'cancel', 'focus is back on cancel in the review');
    assert.ok(!modal.hasAttribute('inert'), 'the review is live again');
    p.frame();
    assert.ok(p.doc.activeElement.textContent === 'cancel', 'and stays there after the frame');
    p.key('Escape');
    focusIs(p, 'make-report', 'the second Escape closes the review');
  });

  await test('the review over the error closes back into the error', async () => {
    const p = makePage();
    p.el('unpair').focus();
    p.win.showError('x');
    p.frame();
    const modal = await p.openReport();
    focusIs(p, 'report-error-recent', 'the review takes focus at once');
    p.frame();
    focusIs(p, 'report-error-recent', 'and keeps it');
    assert.ok(modal.hasAttribute('data-convey-active-dialog'), 'the review is on top');
    p.key('Escape');
    assert.ok(!p.doc.querySelector('.report-error-modal'), 'Escape closes the review on top');
    assert.ok(p.errorOpen(), 'the error box under it is still open');
    assert.ok(p.doc.activeElement.classList.contains('close'), 'focus back on its close button');
    p.el('errorModal').querySelector('.close').click();
    focusIs(p, 'unpair', 'then back to where it was');
  });

  // ---- a back with either open

  await test('a back closes the review, and focus goes to <main> for the move, not to the opener', async () => {
    const p = makePage();
    p.el('make-report').focus();
    await p.openReport();
    p.frame();
    let openerFocused = 0;
    p.el('make-report').addEventListener('focusin', () => { openerFocused += 1; });
    p.back();
    assert.ok(!p.doc.querySelector('.report-error-modal'), 'the review is gone');
    focusIs(p, 'main-content', 'after the back');
    p.frame();
    focusIs(p, 'main-content', 'after the frame');
    assert.ok(openerFocused === 0, 'the opener never took focus');
  });

  await test('a back closes the error box, and focus goes to <main> for the move', async () => {
    const p = makePage();
    p.el('unpair').focus();
    p.win.showError('x');
    p.frame();
    p.back();
    assert.ok(!p.errorOpen(), 'the box is closed');
    focusIs(p, 'main-content', 'after the back');
  });

  // ---- a view's own dialog is as it was

  await test("a view's single dialog: its own focus move and its opener's return are unchanged", async () => {
    const p = makePage();
    const dialog = p.el('view-dialog');
    p.el('view-open').focus();
    dialog.style.display = 'block';
    p.el('view-save').focus();
    focusIs(p, 'view-save', 'the view moved focus into its dialog before the frame');
    p.frame();
    focusIs(p, 'view-save', 'the layer keeps it there');
    p.el('view-first').focus();
    p.el('view-save').focus();
    dialog.style.display = 'none';
    p.frame();
    focusIs(p, 'view-open', 'closing returns to its opener');
    dialog.style.display = 'block';
    p.frame();
    focusIs(p, 'view-first', 'opened afresh, it starts at its first control');
  });

  await test("the error over a view's dialog closes back to the control that had focus in it", async () => {
    const p = makePage();
    const dialog = p.el('view-dialog');
    p.el('view-open').focus();
    dialog.style.display = 'block';
    p.frame();
    p.el('view-save').focus();
    p.win.showError('x');
    p.frame();
    assert.ok(p.el('errorModal').hasAttribute('data-convey-active-dialog'), 'the error is on top');
    p.key('Escape');
    assert.ok(dialog.style.display === 'block', "one Escape leaves the view's dialog open");
    focusIs(p, 'view-save', "back on save in the view's dialog");
  });

  // ---- the diagnostic console, the review's opener

  for (const [how, close] of [
    ['cancel', (p, modal) => modal.querySelector('.report-error-actions button').click()],
    ['Escape', (p) => p.key('Escape')],
    ['a click on its backdrop', (p, modal) => modal.click()],
  ]) {
    await test(`a review sent from the console, closed by ${how}, comes back to the console's send`, async () => {
      const p = makePage({ console: true });
      const api = p.win.convey.diagnosticConsole;
      api.push({ severity: 'error', source: 'js', summary: 'something on this page didn\'t work.', detail: { message: 'x' } });
      api.open();
      const root = p.el('diagnostic-console');
      assert.ok(root.classList.contains('visible'), 'the console is open');
      const send = root.querySelector('[data-diagnostic-send]');
      assert.ok(send, 'the entry has its send button');
      send.click();
      await settle();
      const modal = p.doc.querySelector('.report-error-modal');
      assert.ok(modal, 'the review opened');
      p.frame();
      assert.ok(root.hasAttribute('inert'), 'the console is inert under the review');
      modal.querySelector('#report-error-recent').click();
      assert.ok(root.classList.contains('visible'), 'a click inside the review leaves the console open');
      close(p, modal);
      assert.ok(!p.doc.querySelector('.report-error-modal'), 'the review is gone');
      assert.ok(root.classList.contains('visible'), 'the console is still open');
      const at = p.doc.activeElement;
      assert.ok(at.hasAttribute && at.hasAttribute('data-diagnostic-send'), `focus back on send, not ${at.id || at.tagName}`);
    });
  }

  // ---- a notice's countdown waits while a dialog is open

  const LIFE = 10000;
  const receipt = (p, extra = {}) => p.notices().show(Object.assign({
    app: 'entities', icon: 'trash-2', title: 'deleted', message: 'one entity',
    autoDismiss: LIFE, buttons: [{ label: 'recover', onClick() {} }],
  }, extra));
  const up = (p, id) => p.notices()._stack.some((n) => n.id === id);
  const barPaused = (p, id) => {
    const bar = p.card(id) && p.card(id).querySelector('.notification-countdown');
    return Boolean(bar && bar.style.animationPlayState === 'paused');
  };
  // A view's dialog as a view opens one: shown, then its first control focused.
  const openView = (p) => { p.showDialog('view-dialog'); p.el('view-first').focus(); };

  await test("a receipt up when a view's dialog opens waits under it, drawn paused, and starts when it closes", async () => {
    const p = makePage({ notices: true });
    const id = receipt(p);
    p.tick(2000);
    openView(p);
    p.frame();
    p.tick(LIFE * 2);
    assert.ok(up(p, id), 'still up after twice its life under the dialog');
    assert.ok(barPaused(p, id), 'its bar is drawn paused');
    p.hideDialog('view-dialog');
    p.frame();
    p.tick(LIFE - 100);
    assert.ok(up(p, id), 'it starts again with its whole life');
    p.tick(200);
    assert.ok(!up(p, id), 'then it goes');
  });

  await test('under two stacked dialogs it starts only when the last one closes', async () => {
    const p = makePage({ notices: true });
    const id = receipt(p);
    openView(p);
    p.frame();
    await p.openReport();
    p.frame();
    p.tick(LIFE * 2);
    p.key('Escape');
    assert.ok(!p.doc.querySelector('.report-error-modal'), 'the review is closed');
    p.frame();
    p.tick(LIFE * 2);
    assert.ok(up(p, id), "still waiting: the view's dialog is open");
    p.hideDialog('view-dialog');
    p.frame();
    p.tick(LIFE + 100);
    assert.ok(!up(p, id), 'started when the last one closed, then gone');
  });

  for (const [when, open] of [
    ["before the layer's frame (a dialog that focuses itself)", (p) => { openView(p); }],
    ["at the layer's frame (the layer moves focus in)", (p) => { p.showDialog('view-dialog'); p.frame(); }],
  ]) {
    await test(`a focused receipt that a dialog's opening sends focusout ${when} waits`, async () => {
      const p = makePage({ notices: true });
      const id = receipt(p);
      p.card(id).focus();
      p.tick(LIFE * 2);
      assert.ok(up(p, id), 'focus pauses it, as today');
      open(p);
      assert.ok(!p.card(id).matches(':focus-within'), 'the opening took focus out of the card');
      p.frame();
      p.tick(LIFE * 2);
      assert.ok(up(p, id), 'still up under the dialog');
      assert.ok(barPaused(p, id), 'drawn paused');
      p.hideDialog('view-dialog');
      p.frame();
      assert.ok(p.card(id).matches(':focus-within'), 'the close gives focus back to the card');
      p.tick(LIFE * 2);
      assert.ok(up(p, id), 'paused while focus is in it, as today');
      p.el('make-report').focus();
      p.tick(LIFE + 100);
      assert.ok(!up(p, id), 'its focusout starts it');
    });
  }

  await test("a hovered receipt that a dialog's opening sends mouseleave waits, and starts on close", async () => {
    const p = makePage({ notices: true });
    const id = receipt(p);
    p.hover(p.card(id));
    p.tick(LIFE * 2);
    assert.ok(up(p, id), 'the pointer pauses it, as today');
    openView(p);
    p.frame();
    p.hover(null); // the scrim lands under the pointer
    p.tick(LIFE * 2);
    assert.ok(up(p, id), 'still up under the dialog');
    p.hideDialog('view-dialog');
    p.frame();
    p.tick(LIFE + 100);
    assert.ok(!up(p, id), 'it started when the dialog closed');
  });

  await test('a receipt with the pointer back on it when the dialog closes stays paused until the pointer leaves', async () => {
    const p = makePage({ notices: true });
    const id = receipt(p);
    p.hover(p.card(id));
    openView(p);
    p.frame();
    p.hover(null);
    p.tick(LIFE * 2);
    p.doc.hovered = p.card(id); // the pointer is over it as the scrim goes
    p.hideDialog('view-dialog');
    p.frame();
    p.tick(LIFE * 2);
    assert.ok(up(p, id), 'paused while the pointer is on it');
    p.hover(null);
    p.tick(LIFE + 100);
    assert.ok(!up(p, id), 'its mouseleave starts it');
  });

  await test('a receipt with focus given back into it when the review closes stays paused until focus leaves', async () => {
    const p = makePage({ notices: true });
    const id = receipt(p);
    const recover = p.card(id).querySelector('.notification-action');
    recover.focus();
    await p.openReport();
    p.frame();
    p.tick(LIFE * 2);
    p.key('Escape');
    p.frame();
    assert.ok(p.doc.activeElement === recover, 'focus is back on recover');
    p.tick(LIFE * 2);
    assert.ok(up(p, id), 'paused while focus is in it');
    p.el('make-report').focus();
    p.tick(LIFE + 100);
    assert.ok(!up(p, id), 'its focusout starts it');
  });

  await test('a receipt that arrives while a dialog is open waits, and starts when it closes', async () => {
    const p = makePage({ notices: true });
    openView(p);
    p.frame();
    const id = receipt(p);
    p.tick(LIFE * 2);
    assert.ok(up(p, id), 'still up');
    assert.ok(barPaused(p, id), 'drawn paused');
    p.hideDialog('view-dialog');
    p.frame();
    p.tick(LIFE - 100);
    assert.ok(up(p, id), 'with its whole life');
    p.tick(200);
    assert.ok(!up(p, id), 'then gone');
  });

  await test("a receipt shown after its confirm hides, before the layer's frame, starts at the frame with its whole life", async () => {
    const p = makePage({ notices: true });
    p.showDialog('confirm-dialog');
    p.el('confirm-yes').focus();
    p.frame();
    p.hideDialog('confirm-dialog');
    const id = receipt(p);
    p.frame();
    p.tick(LIFE - 100);
    assert.ok(up(p, id), 'not cut short');
    p.tick(200);
    assert.ok(!up(p, id), 'and not left waiting');
  });

  await test("a receipt shown after its confirm hides, over a view's dialog still open, waits for that one", async () => {
    const p = makePage({ notices: true });
    openView(p);
    p.frame();
    p.showDialog('confirm-dialog');
    p.el('confirm-yes').focus();
    p.frame();
    p.hideDialog('confirm-dialog');
    const id = receipt(p);
    p.frame();
    p.tick(LIFE * 2);
    assert.ok(up(p, id), "waiting: the view's dialog is still open");
    p.hideDialog('view-dialog');
    p.frame();
    p.tick(LIFE + 100);
    assert.ok(!up(p, id), 'started when it closed');
  });

  await test('a receipt updated by its key while it waits keeps waiting, and starts on close', async () => {
    const p = makePage({ notices: true });
    openView(p);
    p.frame();
    const id = receipt(p, { key: 'delete-1' });
    p.tick(3000);
    assert.ok(receipt(p, { key: 'delete-1', message: 'two entities' }) === id, 'the same notice, updated');
    p.tick(LIFE * 2);
    assert.ok(up(p, id), 'still waiting');
    p.hideDialog('view-dialog');
    p.frame();
    p.tick(LIFE + 100);
    assert.ok(!up(p, id), 'started on close');
  });

  await test("other changes to <body>'s classes neither hold nor restart a countdown", async () => {
    const p = makePage({ notices: true });
    const id = receipt(p);
    p.tick(5000);
    p.doc.body.classList.add('presenting');
    p.doc.body.classList.remove('presenting');
    p.tick(5100);
    assert.ok(!up(p, id), 'gone on time: not held, not restarted');
    openView(p);
    p.frame();
    const held = receipt(p);
    p.doc.body.classList.add('presenting');
    p.doc.body.classList.remove('presenting');
    p.tick(LIFE * 2);
    assert.ok(up(p, held), 'still waiting under the dialog');
  });

  await test('the notices watch <body> with one observer for the life of the page', async () => {
    const p = makePage({ notices: true });
    const before = p.observers();
    for (let i = 0; i < 5; i += 1) {
      const id = receipt(p);
      openView(p);
      p.frame();
      p.hideDialog('view-dialog');
      p.frame();
      p.notices().dismiss(id);
    }
    assert.ok(p.observers() === before + 1, `observers ${before} → ${p.observers()}, expected one more`);
  });

  // Nothing else moves.

  await test('with no dialog, a receipt goes at the end of its life', async () => {
    const p = makePage({ notices: true });
    const id = receipt(p);
    p.tick(LIFE - 100);
    assert.ok(up(p, id), 'up before');
    p.tick(200);
    assert.ok(!up(p, id), 'gone after');
  });

  await test('with no dialog, hover and focus pause it and their end starts its whole life again', async () => {
    const p = makePage({ notices: true });
    const id = receipt(p);
    p.tick(5000);
    p.hover(p.card(id));
    p.tick(LIFE * 2);
    p.hover(null);
    p.tick(LIFE - 100);
    assert.ok(up(p, id), 'a whole life after the pointer left');
    p.card(id).focus();
    p.tick(LIFE * 2);
    p.el('make-report').focus();
    p.tick(LIFE - 100);
    assert.ok(up(p, id), 'a whole life after focus left');
    p.tick(200);
    assert.ok(!up(p, id), 'then gone');
  });

  await test('a notice dismissed while a dialog is open is gone, with no timer left behind', async () => {
    const p = makePage({ notices: true });
    const id = receipt(p);
    openView(p);
    p.frame();
    p.notices().dismiss(id);
    assert.ok(!up(p, id), 'gone');
    p.hideDialog('view-dialog');
    p.frame();
    assert.ok(Object.keys(p.notices()._dismissTimers).length === 0, 'no timer');
    assert.ok(p.notices().count() === 0, 'nothing came back');
  });

  await test('a key update or update() leaves a running countdown as it was', async () => {
    const p = makePage({ notices: true });
    const id = receipt(p, { key: 'k' });
    p.tick(5000);
    receipt(p, { key: 'k', message: 'again' });
    p.notices().update(id, { title: 'deleted, again' });
    p.tick(5100);
    assert.ok(!up(p, id), 'gone at its first deadline');
  });

  await test('a key update that newly sets autoDismiss starts no countdown, with or without a dialog', async () => {
    const p = makePage({ notices: true });
    const id = p.notices().show({ app: 'entities', title: 'deleting', key: 'k2' });
    p.notices().show({ app: 'entities', title: 'deleted', key: 'k2', autoDismiss: 3000 });
    p.tick(LIFE);
    assert.ok(up(p, id), 'no countdown');
    openView(p);
    p.frame();
    p.hideDialog('view-dialog');
    p.frame();
    p.tick(LIFE);
    assert.ok(up(p, id), 'still none after a dialog');
  });

  await test('a waiting receipt whose autoDismiss a key update cleared does not start on close', async () => {
    const p = makePage({ notices: true });
    openView(p);
    p.frame();
    const id = receipt(p, { key: 'k3' });
    p.notices().show({ app: 'entities', title: 'deleting', key: 'k3' });
    p.hideDialog('view-dialog');
    p.frame();
    p.tick(LIFE * 2);
    assert.ok(up(p, id), 'it stays, as a notice with no autoDismiss does');
  });

  if (failed.length) {
    failed.forEach((line) => console.error(`FAIL ${line}`));
    console.error(`DOM CASES: ${cases} passed, ${failed.length} failed`);
    process.exit(1);
  }
  console.log(`DOM CASES: ${cases} passed`);
}

main().catch((error) => {
  console.error(error && error.stack ? error.stack.split('\n').slice(0, 6).join('\n') : String(error));
  process.exit(1);
});
