// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Thinking on the view contract, under plain Node: the real fragment and
// script, mounted through the real view contract, read store, router and
// focus helpers, in a stand-in page with events, focus, a history and a
// clock the cases move by hand. The journal's answers are the recorded ones
// in the thinking corpus. thinking_runs_dom.js runs these after its own
// cases. Every compare is of plain values, ids or tag names: never an
// element.

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const ORIGIN = 'http://journal.test';
const VOID_TAGS = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'source', 'track', 'wbr']);

let cases = 0;
async function test(name, fn) {
  try {
    await fn();
    cases += 1;
  } catch (error) {
    error.message = `${name}: ${error.message}`;
    throw error;
  }
}

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
  listenerCount() {
    let n = 0;
    this.listeners.forEach((list) => { n += list.length; });
    return n;
  }
  fire(event) {
    event.currentTarget = this;
    for (const entry of [...(this.listeners.get(event.type) || [])]) {
      if (entry.once) this.removeEventListener(event.type, entry.fn, entry.capture);
      entry.fn.call(this, event);
    }
    const handler = this['on' + event.type];
    if (typeof handler === 'function') handler.call(this, event);
  }
}

class TextNode {
  constructor(text) { this.nodeType = 3; this.data = String(text); this.parentNode = null; }
  get textContent() { return this.data; }
  set textContent(v) { this.data = String(v); }
  get isConnected() { return !!this.parentNode && this.parentNode.isConnected; }
}

function simpleMatch(el, part) {
  const re = /:nth-child\((\d+)\)|([a-zA-Z][a-zA-Z0-9-]*)|#([\w-]+)|\.([\w-]+)|\[([\w-]+)(?:=["']?([^\]"']*)["']?)?\]/g;
  let m;
  let any = false;
  while ((m = re.exec(part))) {
    any = true;
    if (m[1]) {
      if (!el.parentNode || el.parentNode.children.indexOf(el) + 1 !== Number(m[1])) return false;
      continue;
    }
    m.splice(1, 1);
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
    const parts = one.trim().replace(/\s*>\s*/g, ' > ').split(/\s+/);
    if (!simpleMatch(el, parts[parts.length - 1])) return false;
    let at = el.parentNode;
    for (let i = parts.length - 2; i >= 0; i -= 1) {
      // `a > b`: the parent itself must match.
      if (parts[i] === '>') {
        i -= 1;
        if (!(at && at.nodeType === 1 && simpleMatch(at, parts[i]))) return false;
        at = at.parentNode;
        continue;
      }
      while (at && !(at.nodeType === 1 && simpleMatch(at, parts[i]))) at = at.parentNode;
      if (!at) return false;
      at = at.parentNode;
    }
    return true;
  });
}

function makeStyle(el) {
  const props = new Map();
  const style = {
    setProperty(name, value) { props.set(name, String(value)); },
    removeProperty(name) { props.delete(name); },
    getPropertyValue(name) { return props.get(name) || ''; },
  };
  ['display', 'overflow', 'backgroundColor', 'color', 'width'].forEach((key) => {
    Object.defineProperty(style, key, {
      get: () => props.get(key) || '',
      set: (v) => { props.set(key, String(v)); },
    });
  });
  const inline = /display:\s*([a-z-]+)/.exec(el.getAttribute('style') || '');
  if (inline) props.set('display', inline[1]);
  return style;
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
    // A field's value follows its `value` attribute until something sets it, as a browser's does,
    // with a caret that a new value puts at the end.
    this._value = null;
    this._caret = null;
    this.checked = false;
    this._style = null;
    const self = this;
    this.classList = {
      list: () => (self.getAttribute('class') || '').split(/\s+/).filter(Boolean),
      add(...cs) { const l = this.list(); cs.forEach((c) => { if (!l.includes(c)) l.push(c); }); self.setAttribute('class', l.join(' ')); },
      remove(...cs) { self.setAttribute('class', this.list().filter((x) => !cs.includes(x)).join(' ')); },
      contains(c) { return this.list().includes(c); },
      // As in a browser: a toggle that changes nothing writes nothing.
      toggle(c, on) { const want = on === undefined ? !this.contains(c) : !!on; if (want === this.contains(c)) return want; if (want) this.add(c); else this.remove(c); return want; },
    };
    this.dataset = new Proxy({}, {
      get: (_, key) => self.getAttribute('data-' + String(key).replace(/[A-Z]/g, (c) => '-' + c.toLowerCase())) ?? undefined,
      set: (_, key, v) => { self.setAttribute('data-' + String(key).replace(/[A-Z]/g, (c) => '-' + c.toLowerCase()), v); return true; },
      deleteProperty: (_, key) => { self.removeAttribute('data-' + String(key).replace(/[A-Z]/g, (c) => '-' + c.toLowerCase())); return true; },
    });
  }
  get style() { if (!this._style) this._style = makeStyle(this); return this._style; }
  get value() { return this._value ?? this.getAttribute('value') ?? ''; }
  set value(v) { this._value = String(v ?? ''); this._caret = null; }
  get defaultValue() { return this.getAttribute('value') ?? ''; }
  get selectionStart() { return this._caret ? this._caret[0] : this.value.length; }
  get selectionEnd() { return this._caret ? this._caret[1] : this.value.length; }
  get selectionDirection() { return this._caret ? this._caret[2] : 'none'; }
  setSelectionRange(start, end, direction = 'none') { this._caret = [start, end, direction]; }
  get children() { return this.childNodes.filter((n) => n.nodeType === 1); }
  get parentElement() { return this.parentNode && this.parentNode.nodeType === 1 ? this.parentNode : null; }
  get firstElementChild() { return this.children[0] || null; }
  get options() { return this.querySelectorAll('option'); }
  add(option) { this.append(option); }
  get id() { return this.getAttribute('id') || ''; }
  set id(v) { this.setAttribute('id', v); }
  get className() { return this.getAttribute('class') || ''; }
  set className(v) { this.setAttribute('class', v); }
  get type() { return this.getAttribute('type') || ''; }
  set type(v) { this.setAttribute('type', v); }
  get title() { return this.getAttribute('title') || ''; }
  set title(v) { this.setAttribute('title', v); }
  get placeholder() { return this.getAttribute('placeholder') || ''; }
  set placeholder(v) { this.setAttribute('placeholder', v); }
  get tabIndex() { const v = this.getAttribute('tabindex'); return v === null ? -1 : Number(v); }
  set tabIndex(v) { this.setAttribute('tabindex', String(v)); }
  get href() {
    const raw = this.getAttribute('href');
    return raw === null ? '' : new URL(raw, this.ownerDocument.location.href).href;
  }
  set href(v) { this.setAttribute('href', v); }
  get target() { return this.getAttribute('target') || ''; }
  // `disabled` reflects its attribute, as in a browser, so writing it is a change.
  get disabled() { return this.hasAttribute('disabled'); }
  set disabled(v) { if (v) this.setAttribute('disabled', ''); else this.removeAttribute('disabled'); }
  get hidden() { return this.hasAttribute('hidden'); }
  set hidden(v) { if (v) this.setAttribute('hidden', ''); else this.removeAttribute('hidden'); }
  setAttribute(k, v) { this.attrs.set(k, String(v)); if (k === 'style') this._style = null; }
  getAttribute(k) { return this.attrs.has(k) ? this.attrs.get(k) : null; }
  hasAttribute(k) { return this.attrs.has(k); }
  // Removing an attribute that isn't there changes nothing, in a browser too.
  removeAttribute(k) { if (!this.attrs.has(k)) return; this.attrs.delete(k); }
  toggleAttribute(k, on) { if (on) this.setAttribute(k, ''); else this.removeAttribute(k); }
  get textContent() { return this.childNodes.map((n) => n.textContent).join(''); }
  set textContent(v) { this.replaceChildren(); if (String(v ?? '')) this.append(String(v)); }
  get innerText() { return this.textContent; }
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
  matches(selector) { return matches(this, selector); }
  closest(selector) {
    for (let at = this; at && at.nodeType === 1; at = at.parentNode) if (matches(at, selector)) return at;
    return null;
  }
  get open() { return this.hasAttribute('open'); }
  set open(v) { this.toggleAttribute('open', !!v); }
  checkVisibility() {
    if (!this.isConnected) return false;
    for (let at = this; at && at.nodeType === 1; at = at.parentNode) {
      if (at.hasAttribute('hidden') || at.style.display === 'none') return false;
      // A closed disclosure shows only its summary.
      const parent = at.parentNode;
      if (parent && parent.tagName === 'DETAILS' && !parent.hasAttribute('open') && at.tagName !== 'SUMMARY') return false;
    }
    return true;
  }
  adopt(node) {
    if (node.parentNode) node.parentNode.removeChild(node);
    node.parentNode = this;
    return node;
  }
  append(...nodes) {
    for (let node of nodes) {
      if (node === null || node === undefined) continue;
      if (typeof node === 'string' || typeof node === 'number') node = new TextNode(node);
      if (node.nodeType === 11) {
        const kids = [...node.childNodes];
        node.childNodes = [];
        kids.forEach((k) => { k.parentNode = this; this.childNodes.push(k); });
        continue;
      }
      this.childNodes.push(this.adopt(node));
    }
  }
  prepend(...nodes) {
    const kept = this.childNodes.splice(0);
    this.append(...nodes);
    kept.forEach((k) => this.childNodes.push(k));
  }
  appendChild(node) { this.append(node); return node; }
  insertBefore(node, ref) {
    if (!ref) return this.appendChild(node);
    this.adopt(node);
    this.childNodes.splice(this.childNodes.indexOf(ref), 0, node);
    return node;
  }
  after(...nodes) {
    const parent = this.parentNode;
    if (!parent) return;
    let at = parent.childNodes.indexOf(this) + 1;
    nodes.forEach((node) => {
      if (typeof node === 'string') node = new TextNode(node);
      parent.adopt(node);
      parent.childNodes.splice(at, 0, node);
      at += 1;
    });
  }
  before(...nodes) {
    const parent = this.parentNode;
    if (!parent) return;
    let at = parent.childNodes.indexOf(this);
    nodes.forEach((node) => {
      if (typeof node === 'string') node = new TextNode(node);
      parent.adopt(node);
      parent.childNodes.splice(at, 0, node);
      at += 1;
    });
  }
  removeChild(node) {
    const i = this.childNodes.indexOf(node);
    if (i >= 0) this.childNodes.splice(i, 1);
    node.parentNode = null;
    this.ownerDocument.dropFocusFrom(node);
    return node;
  }
  remove() { if (this.parentNode) this.parentNode.removeChild(this); }
  replaceWith(node) {
    const parent = this.parentNode;
    if (!parent) return;
    const i = parent.childNodes.indexOf(this);
    parent.adopt(node);
    parent.childNodes.splice(parent.childNodes.indexOf(this), 1, node);
    this.parentNode = null;
    void i;
  }
  replaceChildren(...nodes) {
    [...this.childNodes].forEach((n) => this.removeChild(n));
    this.append(...nodes);
  }
  // Walked in a loop, not a closure: while the engine compiles a closure in
  // the background it keeps the closure's matches, and through `parentNode`
  // the tree they came from, past a collection case's `gc()` rounds.
  querySelectorAll(selector) {
    const out = [];
    const pending = [this];
    while (pending.length) {
      const el = pending.pop();
      if (el !== this && matches(el, selector)) out.push(el);
      const kids = el.children;
      for (let i = kids.length - 1; i >= 0; i -= 1) pending.push(kids[i]);
    }
    return out;
  }
  // `:last-of-type` is modelled as the last match, which is how the view uses it.
  querySelector(selector) {
    if (selector.includes(':last-of-type')) { const all = this.querySelectorAll(selector.replace(/:last-of-type/g, '')); return all[all.length - 1] || null; }
    return this.querySelectorAll(selector)[0] || null;
  }
  insertAdjacentHTML(position, html) {
    const holder = new Element(this.ownerDocument, 'div');
    parseInto(this.ownerDocument, holder, html);
    const kids = [...holder.childNodes];
    if (position === 'beforeend') kids.forEach((k) => this.append(k));
    else if (position === 'afterbegin') { const kept = this.childNodes.splice(0); kids.forEach((k) => this.append(k)); kept.forEach((k) => this.childNodes.push(k)); }
    else throw new Error(`insertAdjacentHTML ${position} isn't modelled`);
  }
  getBoundingClientRect() { return { left: 0, top: 0, width: 0, height: 0 }; }
  scrollIntoView() { this.ownerDocument.scrolledTo = this.id || this.tagName; }
  // As in Chromium: content in a closed <details> still has boxes, while
  // checkVisibility() says it isn't shown.
  getClientRects() {
    if (!this.isConnected) return [];
    for (let at = this; at && at.nodeType === 1; at = at.parentNode) {
      if (at.hasAttribute('hidden') || at.style.display === 'none') return [];
    }
    return [{ width: 1, height: 1 }];
  }
  set outerHTML(html) { const holder = new Element(this.ownerDocument, "div"); parseInto(this.ownerDocument, holder, html); const parent = this.parentNode; if (!parent) return; const at = parent.childNodes.indexOf(this); parent.removeChild(this); holder.childNodes.forEach((n) => { n.parentNode = parent; }); parent.childNodes.splice(at, 0, ...holder.childNodes); }
  focus() {
    if (!this.isConnected || !this.checkVisibility()) return;
    // As in a browser, only a focusable element takes focus: a control, a link, or one with a tabindex.
    const focusable = ['BUTTON', 'INPUT', 'SELECT', 'TEXTAREA', 'SUMMARY'].includes(this.tagName) || (this.tagName === 'A' && this.hasAttribute('href')) || this.hasAttribute('tabindex');
    if (!focusable) return;
    // The shell's modal layer: outside an open dialog the page is inert,
    // its ancestors excepted, until the layer reconciles.
    const dialog = this.ownerDocument.inertDialog;
    if (dialog && !dialog.contains(this) && !this.contains(dialog)) return;
    this.ownerDocument.activeElement = this;
  }
  blur() { if (this.ownerDocument.activeElement === this) this.ownerDocument.activeElement = this.ownerDocument.body; }
  // A click with the browser's default: a link nobody prevented loads.
  click(init = {}) {
    const event = this.ownerDocument.dispatch(this, Object.assign({ type: 'click', button: 0 }, init));
    if (!event.defaultPrevented) {
      const link = this.closest('a[href]');
      if (link) this.ownerDocument.loads.push(link.href);
    }
    return event;
  }
}

const ENTITIES = { '&times;': '×', '&hellip;': '…', '&rarr;': '→', '&larr;': '←', '&nbsp;': ' ', '&middot;': '·' };
function decode(text) {
  return text.replace(/&[a-z]+;/g, (e) => ENTITIES[e] || e)
    .replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&amp;/g, '&');
}

// A tiny parser for the fragment's own markup and the strings it draws.
function parseInto(doc, parent, html) {
  const stack = [parent];
  const re = /<!--[\s\S]*?-->|<\/([a-zA-Z0-9]+)\s*>|<([a-zA-Z][a-zA-Z0-9-]*)((?:\s+[^\s=>]+(?:=(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*\/?>|([^<]+)/g;
  let m;
  while ((m = re.exec(html))) {
    const top = stack[stack.length - 1];
    if (m[0].startsWith('<!--')) continue;
    if (m[1]) {
      const tag = m[1].toUpperCase();
      while (stack.length > 1 && stack.pop().tagName !== tag) {}
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

function makeDocument(location) {
  const doc = new Target();
  doc.location = location;
  doc.title = 'thinking - journal';
  doc.readyState = 'complete';
  doc.createElement = (tag) => new Element(doc, tag);
  doc.createTextNode = (t) => new TextNode(t);
  doc.createDocumentFragment = () => {
    const f = new Element(doc, 'fragment');
    f.nodeType = 11;
    return f;
  };
  doc.documentElement = new Element(doc, 'html');
  doc.body = new Element(doc, 'body');
  doc.documentElement.append(doc.body);
  // The browser's focus fixup: focus on something hidden, taken out or turned
  // off reads as the page itself.
  let focused = doc.body;
  Object.defineProperty(doc, 'activeElement', {
    get() {
      if (!focused || focused === doc.body) return doc.body;
      // A browser holds no node it has taken out of the page.
      if (!focused.isConnected) { focused = doc.body; return doc.body; }
      if (!focused.checkVisibility() || focused.disabled) return doc.body;
      return focused;
    },
    set(node) { focused = node; },
  });
  doc.getElementById = (id) => doc.documentElement.querySelector('#' + id);
  doc.querySelector = (s) => doc.documentElement.querySelector(s);
  doc.querySelectorAll = (s) => doc.documentElement.querySelectorAll(s);
  doc.contains = (node) => doc.documentElement.contains(node);
  // Focus on a node the page takes out goes to the page, as a browser does.
  doc.dropFocusFrom = (node) => {
    for (let at = doc.activeElement; at; at = at.parentNode) {
      if (at === node) { doc.activeElement = doc.body; return; }
    }
  };
  doc.dispatch = (target, init) => {
    const event = Object.assign({
      target,
      bubbles: true,
      defaultPrevented: false,
      metaKey: false, ctrlKey: false, shiftKey: false, altKey: false,
      preventDefault() { this.defaultPrevented = true; },
      stopPropagation() {},
    }, init);
    const chain = [];
    for (let at = target; at; at = at.parentNode) chain.push(at);
    chain.forEach((node) => { if (node.fire) node.fire(event); });
    doc.fire(event);
    if (doc.window) doc.window.fire(event);
    return event;
  };
  return doc;
}

// ---- a clock the cases move

function makeClock() {
  let now = 0;
  let serial = 0;
  const timers = new Map();
  return {
    setTimeout(fn, ms = 0, ...args) { serial += 1; timers.set(serial, { at: now + ms, fn, args }); return serial; },
    clearTimeout(id) { timers.delete(id); },
    setInterval(fn, ms, ...args) { serial += 1; timers.set(serial, { at: now + ms, fn, args, every: ms }); return serial; },
    clearInterval(id) { timers.delete(id); },
    advance(ms) {
      const end = now + ms;
      for (;;) {
        const due = [...timers.entries()].filter(([, t]) => t.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
        if (!due) break;
        const [id, t] = due;
        now = t.at;
        if (t.every) t.at += t.every; else timers.delete(id);
        t.fn(...t.args);
      }
      now = end;
    },
    live: () => timers.size,
    now: () => now,
  };
}

function abortError() {
  const error = new Error('The operation was aborted');
  error.name = 'AbortError';
  return error;
}

// Every change the page makes to a connected node is counted once (what a
// change does inside this stand-in isn't), so a case can say a redraw left
// the page alone.
let mutations = 0;
let depth = 0;
function counted(original, changes = () => true) {
  return function (...args) {
    if (depth === 0 && this.isConnected && changes.apply(this, args)) mutations += 1;
    depth += 1;
    try { return original.apply(this, args); } finally { depth -= 1; }
  };
}
for (const name of ['append', 'removeChild', 'insertBefore', 'setAttribute']) {
  Element.prototype[name] = counted(Element.prototype[name]);
}
Element.prototype.removeAttribute = counted(Element.prototype.removeAttribute, function (k) { return this.attrs.has(k); });
const textContent = Object.getOwnPropertyDescriptor(Element.prototype, 'textContent');
Object.defineProperty(Element.prototype, 'textContent', { get: textContent.get, set: counted(textContent.set) });

// A native <dialog>, as far as the view uses one.
Element.prototype.showModal = function () {
  if (!this.isConnected) throw new Error('InvalidStateError: the dialog is not in the page');
  this.setAttribute('open', '');
};
Element.prototype.close = function () { this.removeAttribute('open'); };

// ---- the journal the page reads: the recorded corpus

const TODAY = '20261114';
const RUN_DAY = '20261113';

function recorded(crateDir, phase) {
  const corpus = JSON.parse(fs.readFileSync(path.join(crateDir, '../../fixtures/convey_thinking_corpus.json'), 'utf8'));
  const answers = new Map();
  for (const entry of corpus.phases[phase]) {
    if (entry.method === 'GET' && entry.status === 200 && entry.json !== undefined && !answers.has(entry.path)) {
      answers.set(entry.path, entry.json);
    }
  }
  return answers;
}

const clone = (value) => JSON.parse(JSON.stringify(value));

function runsDay() {
  return {
    day: RUN_DAY,
    facets: [],
    uses: [{ id: 'use-1', name: 'pulse', day: RUN_DAY, status: 'done', start: 1763020800, runtime_seconds: 30, provider: 'google', model: 'gemini' }],
  };
}

function runRecord() {
  return { id: 'use-1', name: 'pulse', day: RUN_DAY, status: 'done', prompt: 'the saved request', events: [], output_file: '' };
}

// Answers by method and address; a recorded GET otherwise, and {} for any
// other read. `undefined` leaves the request out.
function journal(answers, overrides = {}) {
  return (url, opts) => {
    const u = new URL(url, ORIGIN);
    const method = String(opts?.method || 'GET').toUpperCase();
    const full = `${method} ${u.pathname}${u.search}`;
    const bare = `${method} ${u.pathname}`;
    for (const key of [full, bare]) {
      if (key in overrides) return typeof overrides[key] === 'function' ? overrides[key](u, opts) : clone(overrides[key]);
    }
    if (method === 'GET') {
      if (u.pathname.startsWith('/app/thinking/api/talents/')) return runsDay();
      if (u.pathname.startsWith('/app/thinking/api/run/')) return runRecord();
      if (u.pathname === '/app/thinking/api/updated-days') return [];
      if (u.pathname === '/app/thinking/api/preview/pulse') return { full_prompt: 'the template' };
      const kept = answers.get(u.pathname + u.search) ?? answers.get(u.pathname);
      if (kept !== undefined) return clone(kept);
      return {};
    }
    return {};
  };
}

// ---- one page with thinking in it

function makePage(env, address = '/app/thinking/', { phase = 'bundled_local', overrides = {}, storage = {} } = {}) {
  let current = new URL(address, ORIGIN);
  const loads = [];
  const location = {
    get origin() { return ORIGIN; },
    get href() { return current.href; },
    set href(url) { loads.push(new URL(url, current.href).href); },
    get pathname() { return current.pathname; },
    get search() { return current.search; },
    get hash() { return current.hash; },
    set(url) { current = new URL(url, current.href); },
    assign(url) { loads.push(new URL(url, current.href).href); },
    replace(url) { loads.push(new URL(url, current.href).href); },
    reload() { loads.push('reload'); },
  };
  const doc = makeDocument(location);
  doc.loads = loads;
  doc.visibilityState = 'visible';
  const clock = makeClock();
  const calls = [];
  const errors = [];
  const opened = [];
  const reserved = [];
  const reconciles = [];
  const window = new Target();
  doc.window = window;
  const entries = [{ url: current.href, state: { solRoute: { id: 'e0', owner: 'router' } } }];
  let index = 0;
  // A move between entries: popstate, then hashchange when only the hash
  // differs, as a browser sends them.
  const traverse = (to) => {
    const before = new URL(entries[index].url);
    index = to;
    location.set(entries[index].url);
    window.fire({ type: 'popstate', state: entries[index].state });
    const after = new URL(entries[index].url);
    if (before.pathname === after.pathname && before.hash !== after.hash) window.fire({ type: 'hashchange' });
  };
  window.history = {
    get state() { return entries[index].state; },
    get length() { return entries.length; },
    scrollRestoration: 'auto',
    pushState(state, _t, url) {
      entries.splice(index + 1);
      entries.push({ url: new URL(url, current.href).href, state });
      index += 1;
      location.set(entries[index].url);
    },
    replaceState(state, _t, url) {
      entries[index] = { url: url === undefined || url === '' ? entries[index].url : new URL(url, current.href).href, state };
      location.set(entries[index].url);
    },
    back() { traverse(index - 1); },
    forward() { traverse(index + 1); },
  };
  window.scrollY = 0;
  window.scrollTo = (_x, y) => { window.scrollY = y; };
  window.location = location;
  window.document = doc;
  window.setTimeout = clock.setTimeout;
  window.clearTimeout = clock.clearTimeout;
  window.setInterval = clock.setInterval;
  window.clearInterval = clock.clearInterval;
  window.requestAnimationFrame = (fn) => clock.setTimeout(fn, 16);
  window.matchMedia = () => ({ matches: false });
  window.navigator = { userAgent: 'Mozilla/5.0 (X11; Linux x86_64)', platform: 'Linux' };
  const storageMap = new Map(Object.entries(storage));
  window.sessionStorage = {
    getItem: (k) => (storageMap.has(k) ? storageMap.get(k) : null),
    setItem: (k, v) => { storageMap.set(k, String(v)); },
    removeItem: (k) => { storageMap.delete(k); },
  };
  window.JournalFormat = {
    day: (key) => `day ${key}`, duration: (s) => `${s} s`, timestamp: () => 'at a time', processingLane: (lane) => String(lane || ''),
    stream: (s) => String(s), compactTokens: (n) => String(n), segmentTime: () => 'a time',
  };
  window.JournalClock = { today: () => TODAY, now: () => new Date('2026-11-14T12:00:00Z'), formatInstant: () => 'nov 14', placeNote() {} };
  window.SurfaceState = {
    loading: ({ text }) => `<div class="surface-state" role="status"><span>${text}</span></div>`,
    error: ({ heading, retry }) => `<div class="surface-state"><h2>${heading}</h2>${retry ? '<button type="button" class="surface-state-retry">try again</button>' : ''}</div>`,
    serverMessageFrom: () => '',
    empty: ({ heading }) => `<p class="empty">${heading}</p>`,
  };
  window.CONVEY_COPY = { RELOAD_HINT: 'try again', CONSOLE_SUMMARY_REQUEST_FAILED: "couldn't finish that request." };
  window.AppServices = { renderMarkdown: (text) => String(text || '') };
  window.solstoneOutside = {
    openDelayed: (url) => { opened.push(url); },
    allowedUrl: (url) => typeof url === 'string' && url.startsWith('https://'),
    reserveBlank: () => {
      const reservation = { closed: false, href: 'about:blank', close() { this.closed = true; } };
      reserved.push(reservation);
      return reservation;
    },
    navigateReserved: (reservation, url) => { reservation.href = url; return true; },
  };
  window.ConveyModalLayer = { reconcile: (options) => { reconciles.push(options || null); } };
  window.logError = (error, context) => { errors.push({ error, context }); };
  window.appEvents = { buildChanged: () => false, onConnectionState: () => () => {}, listen: () => () => {} };
  window.__SOL_ENTRY_TABLE__ = {
    views: {
      thinking: { contract: true, fragment: '/app/thinking/workspace', scripts: [], styles: [] },
      network: { contract: true, fragment: '/app/network/workspace', scripts: [], styles: [] },
    },
  };
  window.solShellData = { apps: [
    { name: 'thinking', label: 'thinking', workspace_url: '/app/thinking/workspace' },
    { name: 'network', label: 'network', workspace_url: '/app/network/workspace' },
  ] };

  const serve = journal(env.answers(phase), overrides);
  const ask = (url, opts, kind) => new Promise((resolve, reject) => {
    const u = new URL(url, current.href);
    const call = {
      url: u.pathname + u.search,
      asked: String(url),
      method: String(opts?.method || 'GET').toUpperCase(),
      body: opts?.body,
      kind, aborted: false, settled: false,
      answer(body, status = 200) {
        if (call.settled) return;
        call.settled = true;
        if (status >= 400) {
          const error = new Error(`HTTP ${status}`);
          error.status = status;
          error.payload = body;
          reject(error);
        } else resolve(body);
      },
      fail(error) { if (!call.settled) { call.settled = true; reject(error); } },
    };
    if (opts?.signal) {
      const onAbort = () => { call.aborted = true; call.fail(abortError()); };
      if (opts.signal.aborted) onAbort();
      else opts.signal.addEventListener('abort', onAbort, { once: true });
    }
    calls.push(call);
    const value = page.hold(call) ? undefined : serve(url, opts);
    if (value !== undefined) Promise.resolve().then(() => call.answer(value, page.status(call)));
  });
  window.apiJson = (url, opts = {}) => ask(url, opts, 'api');
  window.fetch = (url, opts = {}) => ask(url, opts, 'fetch');
  // The router's move, as far as these cases need it: the leaving view goes,
  // the next one's markup goes in, and it mounts. The fragment and its code
  // are the page's already.
  window.mountWorkspaceFragment = async (_url, options) => {
    const place = () => {
      window.SolViews.dispose();
      if (options.appName === 'thinking') page.place();
      else { main.replaceChildren(); parseInto(doc, main, OTHER_VIEW); }
      window.solRouter.placed(main);
      page.ready = [];
      return window.SolViews.mount(options.appName, { reportReady: (s) => page.ready.push(s) }).then((view) => { page.view = view; });
    };
    if (typeof options.swap === 'function') await options.swap(place);
    else await place();
  };

  parseInto(doc, doc.body, '<nav id="app-rail"><a href="/app/home/">home</a><a href="/app/thinking/">thinking</a></nav><main id="main-content" tabindex="-1"></main><div id="view-announcer"></div>');
  const main = doc.getElementById('main-content');

  const page = {
    env, window, doc, main, clock, calls, errors, loads, opened, reserved, reconciles, storageMap,
    holds: [],
    statuses: [],
    hold(call) { return page.holds.some((test) => test(call)); },
    status(call) { const hit = page.statuses.find(([test]) => test(call)); return hit ? hit[1] : 200; },
    windowKeys: () => Object.keys(window).sort(),
    byId: (id) => doc.getElementById(id),
    q: (s) => doc.querySelector(s),
    qa: (s) => doc.querySelectorAll(s),
    reads: (prefix) => calls.filter((c) => c.method === 'GET' && c.url.startsWith(prefix)),
    writes: () => calls.filter((c) => c.method !== 'GET'),
    held: () => calls.filter((c) => !c.settled),
    entries,
    entry: () => entries[index],
    mutations: () => mutations,
    place() {
      main.replaceChildren();
      parseInto(doc, main, env.markup);
    },
    loadScripts() {
      vm.runInContext(env.script, context, { filename: 'thinking.js' });
    },
    async mount() {
      page.ready = [];
      page.view = await window.SolViews.mount('thinking', { reportReady: (s) => page.ready.push(s) });
      await settle();
      return page.view;
    },
    // Another contract view takes the page, through the router's move.
    leave(next = '/app/network/') {
      assert.ok(window.solRouter.navigate(next) === true, 'the move stays in the page');
    },
    // Thinking again in the same document, through the router's move.
    async arrive(again = '/app/thinking/') {
      assert.ok(window.solRouter.navigate(again) === true, 'the move stays in the page');
      await settle();
      return page.view;
    },
    press(node) {
      assert.ok(node, 'the control to press is drawn');
      return node.click();
    },
  };

  const context = vm.createContext({
    window, document: doc, location, history: window.history, navigator: window.navigator, sessionStorage: window.sessionStorage,
    URL, URLSearchParams, AbortController, AbortSignal, DOMException, WeakMap, WeakRef, Map, Set, Promise,
    console, Math, Number, String, Object, Array, Boolean, Date, Intl, JSON, Error, TypeError, RegExp, Symbol, Infinity, CustomEvent: class {},
    setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval, clearInterval: clock.clearInterval,
    requestAnimationFrame: window.requestAnimationFrame,
    addEventListener: window.addEventListener.bind(window),
    performance: { mark() {}, clearMarks() {} },
    fetch: window.fetch,
    encodeURIComponent, decodeURIComponent,
  });
  context.self = window;
  const shell = env.shellDir;
  vm.runInContext(fs.readFileSync(path.join(shell, 'store.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'views.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'router.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'focus.js'), 'utf8'), context);
  // The page's router is on, as it is once a contract view has mounted.
  window.SolViews.register('network', { mount(ctx) { ctx.ready('content'); return () => {}; } });
  window.solRouter.start('thinking');
  page.context = context;
  return page;
}

// The router's own timers after a move (its announcement, its ten-second
// loading line and scroll hold) run out; the view's are gone already.
async function drain(p) {
  await run(p, 11000, 1000);
}

const OTHER_VIEW = '<section id="other"><h1>network</h1><p id="home-text">home as it was</p></section>';

function flush() {
  return new Promise((resolve) => setImmediate(resolve));
}

async function settle() {
  for (let i = 0; i < 16; i += 1) await flush();
}

// The clock moves in steps, so each poll's read answers before its next sleep.
async function run(p, ms, step = 500) {
  for (let at = 0; at < ms; at += step) {
    p.clock.advance(step);
    await settle();
  }
}

async function boot(env, address, options) {
  const p = makePage(env, address, options);
  p.loadScripts();
  p.place();
  await p.mount();
  await settle();
  return p;
}

function focusOn(p) {
  const active = p.doc.activeElement;
  if (!active) return 'none';
  if (active.id) return `${active.tagName}#${active.id}`;
  return active.tagName;
}

function listenerCounts(p) {
  return [p.window.listenerCount(), p.doc.listenerCount()];
}

function answerHeld(p, test, body, status = 200) {
  p.calls.filter((call) => !call.settled && test(call)).forEach((call) => call.answer(body, status));
}

function shownSections(p) {
  return p.qa('#providers [data-view]').filter((s) => !s.hidden).map((s) => s.dataset.view)
    .concat(p.qa('[data-thinking-section]').filter((s) => !s.hidden).map((s) => s.dataset.thinkingSection));
}

async function collected(ref) {
  for (let i = 0; i < 10 && ref.deref(); i += 1) {
    await flush();
    global.gc();
  }
  return !ref.deref();
}

const isState = (call) => call.url === '/app/thinking/api/state';
const isProviders = (call) => call.method === 'GET' && call.url.startsWith('/app/thinking/api/providers');

// What a full load of thinking's states needs, on top of the recorded answers.
function installing(answers) {
  const status = clone(answers.get('/app/thinking/api/local/bootstrap/status'));
  return Object.assign(status || {}, { install_state: 'downloading' });
}

function confidentialTurningOn(answers) {
  const providers = clone(answers.get('/app/thinking/api/providers'));
  providers.active_lane = Object.assign({}, providers.active_lane, { confidential_operation: { phase: 'waiting', portal_url: 'https://services.example/consent' } });
  return providers;
}

// Local chosen and installed, with processing yet to confirm it.
function awaitingConfirmation(answers) {
  const providers = clone(answers.get('/app/thinking/api/providers'));
  providers.active_lane = Object.assign({}, providers.active_lane, { lane: 'local' });
  providers.provider_status = Object.assign({}, providers.provider_status, { local: { generate_ready: false, issues: [], selected: true } });
  providers.local_override = { enabled: false };
  return providers;
}

const ATTEMPT = JSON.stringify({ id: 'attempt-1', authorizeUrl: 'https://auth.example/authorize' });

// Run from thinking_runs_dom.js: answers with the number of cases.
module.exports = async function thinkingContractCases(crateDir) {
  assert.ok(typeof global.gc === 'function', 'node runs with --expose-gc');
  const workspace = fs.readFileSync(path.join(crateDir, 'assets/thinking/workspace.html'), 'utf8');
  const script = fs.readFileSync(path.join(crateDir, 'assets/thinking/thinking.js'), 'utf8');
  const markup = workspace.replace(/<style>[\s\S]*?<\/style>/g, '').replace(/<script[\s\S]*?<\/script>/g, '');
  const shellDir = path.join(crateDir, 'assets/static');
  const phases = new Map();
  const env = {
    markup, script, shellDir,
    answers(phase) {
      if (!phases.has(phase)) phases.set(phase, recorded(crateDir, phase));
      return phases.get(phase);
    },
  };
  const local = env.answers('bundled_local');

  await test('the fragment is a contract view with its first read, and the script writes nothing around the contract', async () => {
    assert.ok(workspace.includes('<!-- view-contract -->'));
    assert.ok(workspace.includes('<!-- first-read: /app/thinking/ -> /app/thinking/api/state -->'));
    assert.ok(!/\son[a-z]+="/.test(workspace), 'no inline handler attribute');
    assert.ok(!/<script>/.test(workspace), 'no inline script');
    const order = [...workspace.matchAll(/<script src="([^"]+)"/g)].map((m) => m[1]);
    assert.deepStrictEqual(order, ['/static/vendor/marked/marked.min.js', '/static/vendor/dompurify/dompurify.min.js', '/app/thinking/static/thinking.js']);
    assert.ok(!/history\.(pushState|replaceState)/.test(script), 'the address is written only through the view contract');
    assert.ok(!/window\.apiJson/.test(script), 'every read goes through the view\'s store');
    assert.ok(!/(^|[^.\w])setTimeout\(/m.test(script), 'every timer goes through the view');
    assert.ok(!/(document|window)\.addEventListener/.test(script), 'nothing listens on the page but through the view');
  });

  await test('the script only registers: nothing reads, draws, listens or starts a timer at load', async () => {
    const p = makePage(env);
    p.place();
    const keys = p.windowKeys();
    const listeners = listenerCounts(p);
    p.loadScripts();
    assert.ok(p.calls.length === 0, 'no read at load');
    assert.deepStrictEqual(p.windowKeys().filter((k) => !keys.includes(k)), [], 'nothing is defined on the window');
    assert.deepStrictEqual(listenerCounts(p), listeners, 'no listener at load');
    assert.ok(p.window.SolViews.registered('thinking') === true);
    assert.ok(p.clock.live() === 0, 'no timer at load');
    assert.ok(p.byId('thinkingApp').hidden === true, 'nothing drawn');
  });

  await test('a mount reads its state through the store, draws, reports ready once, and asks only absolute addresses', async () => {
    const p = await boot(env);
    assert.ok(p.reads('/app/thinking/api/state').length === 1, 'state asked once');
    assert.ok(p.reads('/app/thinking/api/state')[0].kind === 'api', 'through the store\'s read, so the early read is adopted');
    assert.ok(p.byId('thinkingApp').hidden === false, 'drawn');
    assert.ok(p.byId('thinkingHeading').textContent.length > 0, 'the heading is written');
    assert.deepStrictEqual(p.ready, ['content']);
    for (const call of p.calls) assert.ok(call.asked.startsWith('/'), `absolute: ${call.asked}`);
    assert.ok(p.reads('/app/thinking/api/keys').length === 1, 'keys, after the first paint');
    assert.ok(p.reads('/app/thinking/api/local/models').length === 1, 'the model list');
    assert.ok(p.reads('/app/network/api/clients').length === 1, 'the devices invite');
  });

  await test('dispose leaves no listener, timer, read or global, and thinking mounts again in the same page', async () => {
    const p = makePage(env);
    p.loadScripts();
    const keys = p.windowKeys();
    const listeners = listenerCounts(p);
    p.place();
    for (let round = 0; round < 2; round += 1) {
      p.holds.push((call) => call.url.startsWith('/app/thinking/api/local/runtime'));
      if (round === 0) await p.mount();
      else { p.window.history.back(); await settle(); }
      assert.ok(p.byId('thinkingApp').hidden === false, `round ${round}: drawn`);
      assert.ok(listenerCounts(p)[0] > listeners[0] && listenerCounts(p)[1] > listeners[1], `round ${round}: the view listens on the window and the document`);
      p.leave();
      await settle();
      const asked = p.calls.length;
      await drain(p);
      assert.ok(p.calls.length === asked, `round ${round}: nothing more is asked`);
      assert.deepStrictEqual(listenerCounts(p), listeners, `round ${round}: every window and document listener is gone`);
      assert.deepStrictEqual(p.windowKeys(), keys, `round ${round}: no global is left`);
      assert.ok(p.held().every((call) => call.aborted), `round ${round}: every read still out was cut`);
      assert.ok(p.clock.live() === 0, `round ${round}: no timer`);
      p.holds.length = 0;
    }
  });

  // Each poll, running when the owner leaves: it stops at once, its read
  // still out is cut, and nothing it would have done afterwards happens.
  const polls = [
    ['install status', '/app/thinking/', { overrides: { 'GET /app/thinking/api/local/bootstrap/status': installing(local) } }, '/app/thinking/api/local/bootstrap/status'],
    ['local runtime', '/app/thinking/#local-setup', { overrides: { 'GET /app/thinking/api/local/runtime': Object.assign(clone(local.get('/app/thinking/api/local/runtime') || {}), { poll: true }) } }, '/app/thinking/api/local/runtime'],
    ['confidential turn-on', '/app/thinking/', { overrides: { 'GET /app/thinking/api/providers': confidentialTurningOn(local) } }, '/app/thinking/api/providers'],
    ['local confirmation', '/app/thinking/#main', { overrides: { 'GET /app/thinking/api/providers': awaitingConfirmation(local), 'GET /app/thinking/api/local/availability': { available: true, reason_code: 'ready' } } }, '/app/thinking/api/providers'],
    ['the access re-read', '/app/thinking/', { phase: 'confidential' }, '/app/thinking/api/state'],
    ['ChatGPT sign-in', '/app/thinking/#byo-setup', { storage: { 'solstone.thinking.chatgptAttempt': ATTEMPT }, overrides: { 'GET /app/thinking/api/chatgpt/sign-in/attempt-1': { state: 'pending' } } }, '/app/thinking/api/chatgpt/sign-in/attempt-1'],
  ];
  for (const [name, address, options, prefix] of polls) {
    await test(`the ${name} poll stops when thinking goes, sleeping or reading`, async () => {
      for (const when of ['sleeping', 'reading']) {
        const p = await boot(env, address, options);
        const before = p.reads(prefix).length;
        const oneShot = name === 'the access re-read';
        if (when === 'reading') p.holds.push((call) => call.url.startsWith(prefix));
        if (oneShot && when === 'sleeping') {
          // Its one timer is set; leave before it fires.
          assert.ok(p.clock.live() > 0, `${name}: its timer is set`);
        } else {
          await run(p, 4000);
          assert.ok(p.reads(prefix).length > before, `${name}: it polls while thinking is up (${before} → ${p.reads(prefix).length})`);
        }
        if (when === 'reading') assert.ok(p.held().some((call) => call.url.startsWith(prefix)), `${name}: a read is out`);
        const out = p.held().filter((call) => call.method === 'GET');
        p.leave();
        await settle();
        assert.ok(out.every((call) => call.aborted), `${name} (${when}): the reads out were cut`);
        const asked = p.calls.length;
        p.holds.length = 0;
        await drain(p);
        assert.ok(p.clock.live() === 0, `${name} (${when}): no timer`);
        const drawn = p.mutations();
        await run(p, 120000, 1500);
        assert.ok(p.calls.length === asked, `${name} (${when}): nothing more is asked`);
        assert.ok(p.mutations() === drawn, `${name} (${when}): nothing is drawn`);
        assert.ok(p.byId('home-text').textContent === 'home as it was', `${name} (${when}): the next view is left alone`);
      }
    });
  }

  await test('the brain follow stops when thinking goes', async () => {
    const providers = clone(local.get('/app/thinking/api/providers'));
    providers.brain = Object.assign({}, providers.brain, { state: 'unknown', headline: 'thinking status unknown', action: { label: 'check again', refresh: true } });
    const p = await boot(env, '/app/thinking/', { overrides: { 'GET /app/thinking/api/providers': providers, 'POST /app/thinking/api/brain/check': { ok: true } } });
    const check = p.byId('brainCheckAction');
    assert.ok(check && !check.hidden, 'the brain card offers a check');
    p.press(check);
    await settle();
    assert.ok(p.writes().some((c) => c.url === '/app/thinking/api/brain/check'), 'the check is asked');
    const before = p.reads('/app/thinking/api/providers').length;
    await run(p, 7000);
    assert.ok(p.reads('/app/thinking/api/providers').length > before, 'the follow re-reads');
    p.leave();
    await settle();
    const asked = p.calls.length;
    await drain(p);
    assert.ok(p.clock.live() === 0, 'no timer');
    await run(p, 150000, 3000);
    assert.ok(p.calls.length === asked, 'nothing more is asked');
  });

  await test('a read out when thinking goes is cut, and its late answer draws nothing', async () => {
    for (const [name, prefix] of [['state', '/app/thinking/api/state'], ['keys', '/app/thinking/api/keys'], ['runs day', '/app/thinking/api/talents/']]) {
      const p = makePage(env, name === 'runs day' ? `/app/thinking/#runs/${RUN_DAY}` : '/app/thinking/');
      p.loadScripts();
      p.place();
      p.holds.push((call) => call.url.startsWith(prefix));
      await p.mount();
      const out = p.calls.find((c) => c.url.startsWith(prefix) && !c.settled);
      assert.ok(out, `${name} is out`);
      p.leave();
      await settle();
      assert.ok(out.aborted, `${name} was cut`);
      await drain(p);
      const before = p.mutations();
      out.settled = false;
      out.answer(name === 'state' ? clone(local.get('/app/thinking/api/state')) : {});
      await run(p, 10000);
      assert.ok(p.mutations() === before, `${name}: its late answer draws nothing`);
      assert.ok(p.errors.length === 0, `${name}: nothing logged`);
    }
  });

  await test('a write out when thinking goes finishes uncut, draws into neither the next view nor a new thinking, and lets the gone markup go', async () => {
    for (const next of ['home', 'thinking again']) {
      const p = await boot(env, '/app/thinking/#byo-setup');
      const isWrite = (call) => call.method === 'POST' && call.url === '/app/thinking/api/local/endpoint';
      p.holds.push(isWrite);
      p.byId('localEndpointUrl').value = 'http://127.0.0.1:8080/v1';
      p.byId('localEndpointModel').value = 'model';
      p.press(p.byId('localEndpointSave'));
      await settle();
      const write = p.calls.find((c) => isWrite(c) && !c.settled);
      assert.ok(write, 'the save is out');
      let gone = new WeakRef(p.byId('providers'));
      p.leave();
      await settle();
      if (next === 'thinking again') {
        p.holds.length = 0;
        p.holds.push(isWrite);
        await p.arrive();
      }
      await settle();
      assert.ok(!write.aborted, 'the write was not cut');
      await drain(p);
      const before = p.mutations();
      const markupNow = p.byId('providers');
      write.answer({ ok: true, endpoint: { endpoint_url: 'http://127.0.0.1:8080/v1', served_model_id: 'model' } });
      await run(p, 10000);
      assert.ok(p.mutations() === before, `${next}: the late answer draws nothing in the page`);
      if (next === 'thinking again') assert.ok(p.byId('providers') === markupNow, 'the new thinking keeps its markup');
      assert.ok(await collected(gone), `${next}: the gone markup is let go after the last late answer`);
      gone = null;
    }
  });

  await test('a lane switch that answers after the owner left and came back draws nothing, writes no entry, and skips its redraw', async () => {
    const p = await boot(env, '/app/thinking/#byo-setup');
    const isSwitch = (call) => call.method === 'PUT' && call.url === '/app/thinking/api/providers';
    p.holds.push(isSwitch);
    const button = p.qa('[data-switch-lane]')[0];
    assert.ok(button, 'a lane switch is drawn');
    // The lane the switch panel names, as renderLaneSwitch writes it.
    button.setAttribute('data-switch-lane', 'local');
    p.press(button);
    await settle();
    const put = p.calls.find((c) => isSwitch(c) && !c.settled);
    assert.ok(put, 'the switch is out');
    const goneRoot = p.byId('providers');
    p.leave();
    await settle();
    // The old markup's lookups after it has gone: the answer's own lines only.
    let lookups = 0;
    const look = goneRoot.querySelector.bind(goneRoot);
    goneRoot.querySelector = (selector) => { lookups += 1; return look(selector); };
    const lookAll = goneRoot.querySelectorAll.bind(goneRoot);
    goneRoot.querySelectorAll = (selector) => { lookups += 1; return lookAll(selector); };
    p.holds.length = 0;
    p.holds.push(isSwitch);
    await p.arrive('/app/thinking/#byo-setup');
    await drain(p);
    const entries = p.entries.length;
    const address = p.window.location.href;
    const drawn = p.mutations();
    put.answer(clone(local.get('/app/thinking/api/providers')));
    await run(p, 10000);
    assert.ok(p.entries.length === entries && p.window.location.href === address, `no entry is written (${p.window.location.hash})`);
    assert.ok(p.mutations() === drawn, 'the new thinking is left as it was');
    assert.ok(lookups < 20, `the gone view skips its redraw (${lookups} lookups)`);
  });

  await test('a "clear key" that answers after the owner left and came back leaves the new thinking\'s typed key alone', async () => {
    const p = await boot(env, '/app/thinking/#byo-setup', { phase: 'byo_cloud' });
    const isClear = (call) => call.method === 'PUT' && call.url === '/app/thinking/api/keys';
    p.holds.push(isClear);
    p.press(p.byId('byoClearKey'));
    await settle();
    const put = p.calls.find((c) => isClear(c) && !c.settled);
    assert.ok(put, 'the clear is out');
    p.leave();
    await settle();
    p.holds.length = 0;
    await p.arrive('/app/thinking/#byo-setup');
    await drain(p);
    p.byId('byoKeyInput').value = 'typed in the new thinking';
    put.answer(clone(env.answers('none').get('/app/thinking/api/keys')));
    await run(p, 5000);
    assert.ok(p.byId('byoKeyInput').value === 'typed in the new thinking', 'the late answer clears nothing in the new thinking');
  });

  await test('a key check that answers after the owner left sends no save', async () => {
    const p = await boot(env, '/app/thinking/#byo-setup', { phase: 'none', overrides: { 'POST /app/thinking/api/keys/check': { valid: true } } });
    const isCheck = (call) => call.method === 'POST' && call.url === '/app/thinking/api/keys/check';
    p.holds.push(isCheck);
    p.press(p.q('[data-byo-provider="openai"]'));
    await settle();
    p.byId('byoKeyInput').value = 'sk-lab';
    p.press(p.byId('byoSaveKey'));
    await settle();
    const check = p.calls.find((c) => isCheck(c) && !c.settled);
    assert.ok(check, 'the check is out');
    p.leave();
    await settle();
    check.answer({ valid: true });
    await run(p, 5000);
    assert.ok(!check.aborted, 'the check itself was not cut');
    assert.ok(!p.writes().some((c) => c.method === 'PUT' && c.url === '/app/thinking/api/keys'), 'the save it would send next is never sent');
  });

  await test('a confidential turn-on that answers after the owner left opens no consent window, and the return follows it', async () => {
    const overrides = {};
    const p = await boot(env, '/app/thinking/#confidential-setup', { overrides });
    const isEnable = (call) => call.method === 'POST' && call.url === '/app/thinking/api/confidential/enable';
    p.holds.push(isEnable);
    p.press(p.byId('confidentialEnable'));
    await settle();
    const enable = p.calls.find((c) => isEnable(c) && !c.settled);
    assert.ok(enable, 'the turn-on is out');
    p.leave();
    await settle();
    enable.answer({ operation: { phase: 'waiting', portal_url: 'https://services.example/consent' } });
    await run(p, 5000);
    assert.ok(p.opened.length === 0, 'no window opens over the next view');
    assert.ok(p.byId('home-text').textContent === 'home as it was', 'the next view is left alone');
    // Back to thinking: the journal says the turn-on is open, and it is followed.
    overrides['GET /app/thinking/api/providers'] = confidentialTurningOn(local);
    p.holds.length = 0;
    p.window.history.back();
    await settle();
    assert.ok(p.window.SolViews.current()?.name === 'thinking', 'thinking is back, in the page');
    const reads = p.reads('/app/thinking/api/providers').length;
    await run(p, 4000);
    assert.ok(p.reads('/app/thinking/api/providers').length >= reads + 2, 'the return follows the turn-on');
    assert.ok(p.opened.length === 0, 'and opens no window by itself');
  });

  await test('the next mount picks up what is still running in the journal, as a full load does', async () => {
    for (const [name, address, options, prefix] of [polls[0], polls[2], polls[5]]) {
      const p = await boot(env, address, options);
      p.leave();
      await settle();
      const before = p.reads(prefix).length;
      p.window.history.back();
      await settle();
      await run(p, 4000);
      assert.ok(p.reads(prefix).length >= before + 2, `${name}: the return polls again (${before} → ${p.reads(prefix).length})`);
      if (name === 'ChatGPT sign-in') assert.ok(p.storageMap.get('solstone.thinking.chatgptAttempt') === ATTEMPT, 'the stored attempt is kept');
    }
  });

  await test('"Continue with ChatGPT" answering after the owner left still sends its window to the sign-in and keeps the attempt', async () => {
    const p = await boot(env, '/app/thinking/#byo-setup');
    const isStart = (call) => call.method === 'POST' && call.url === '/app/thinking/api/chatgpt/sign-in';
    p.holds.push(isStart);
    p.press(p.byId('chatgptCardContinue'));
    await settle();
    const start = p.calls.find((c) => isStart(c) && !c.settled);
    assert.ok(start, 'the sign-in start is out');
    assert.ok(p.reserved.length === 1 && p.reserved[0].href === 'about:blank', 'its window is reserved on the press');
    p.leave();
    await settle();
    await drain(p);
    const drawn = p.mutations();
    start.answer({ attempt_id: 'attempt-2', id: 'attempt-2', authorize_url: 'https://auth.example/authorize', authorizeUrl: 'https://auth.example/authorize' });
    await run(p, 5000);
    assert.ok(p.mutations() === drawn, 'nothing is drawn');
    assert.ok(p.reserved[0].href.startsWith('https://auth.example/'), `the window goes to the sign-in (${p.reserved[0].href})`);
    assert.ok(!!p.storageMap.get('solstone.thinking.chatgptAttempt'), 'the attempt is kept for the return');
    assert.ok(!p.writes().some((c) => c.url.includes('/finish')), 'nothing finishes without the owner');
  });

  await test('ready reports once, across the error and its "try again"', async () => {
    const p = makePage(env);
    p.loadScripts();
    p.place();
    p.statuses.push([isState, 503]);
    await p.mount();
    assert.deepStrictEqual(p.ready, ['error']);
    p.statuses.length = 0;
    p.press(p.q('.surface-state-retry'));
    await settle();
    assert.ok(p.byId('thinkingApp').hidden === false, 'try again draws');
    assert.deepStrictEqual(p.ready, ['error'], 'and reports nothing more');
  });

  await test('a mount that throws part-way takes back what it added', async () => {
    const p = makePage(env);
    p.loadScripts();
    p.place();
    const keys = p.windowKeys();
    const listeners = listenerCounts(p);
    const original = p.doc.getElementById;
    p.doc.getElementById = () => { throw new Error('part-way'); };
    let threw = false;
    try { await p.window.SolViews.mount('thinking', { reportReady() {} }); } catch (_) { threw = true; }
    p.doc.getElementById = original;
    assert.ok(threw, 'the mount threw');
    assert.ok(p.window.SolViews.current() === null, 'nothing is mounted');
    assert.deepStrictEqual(listenerCounts(p), listeners);
    assert.deepStrictEqual(p.windowKeys(), keys);
    assert.ok(p.clock.live() === 0);
    // Its first draw failing after the listeners are on: dispose still takes them back.
    await p.mount();
    p.leave();
    await settle();
    await drain(p);
    assert.deepStrictEqual(listenerCounts(p), listeners, 'gone with the view');
  });

  await test('every markup control works by a click after an in-page mount', async () => {
    const p = await boot(env);
    p.leave();
    await settle();
    await p.arrive();
    p.press(p.byId('thinkingRunsTab'));
    await settle();
    assert.ok(p.window.location.hash === `#runs/${TODAY}`, `the runs tab moves to today's runs (${p.window.location.hash})`);
    p.press(p.byId('thinkingRunsPrevious'));
    await settle();
    assert.ok(p.window.location.hash === '#runs/20261113', `the previous day (${p.window.location.hash})`);
    p.press(p.byId('thinkingSetupTab'));
    await settle();
    assert.ok(p.window.location.hash === '#main', `setup (${p.window.location.hash})`);
    for (const opener of p.qa('[data-open-view]')) {
      if (opener.hidden || opener.closest('[hidden]')) continue;
      const view = opener.dataset.openView;
      p.press(opener);
      await settle();
      assert.ok(shownSections(p).includes(view) || p.window.location.hash === '#main', `"${view}" opens (${shownSections(p).join(',')})`);
      p.window.history.back();
      await settle();
    }
    const asked = p.writes().length;
    p.press(p.byId('localRefresh'));
    await settle();
    assert.ok(p.reads('/app/thinking/api/local/runtime').length >= 2, 'refresh reads again');
    assert.ok(p.writes().length === asked, 'and writes nothing');
    assert.ok(p.errors.every((e) => !/is not defined/.test(String(e.error?.message))), 'no handler reaches for a global');
  });

  await test('each address is an arrival that lands where a full load does, keeping its entry id', async () => {
    const cases = [
      ['', ['main']], ['#main', ['main']], ['#byo-setup', ['byo-setup']], ['#confidential-setup', ['confidential-setup']],
      ['#local-setup', ['local-setup']], ['#lane-switch', ['main']], [`#runs/${RUN_DAY}`, ['runs']], [`#runs/${RUN_DAY}/pulse`, ['runs']],
      [`#runs/${RUN_DAY}/pulse/use-1`, ['runs']], ['#runs', ['runs']], ['#runs/run/use-1', ['runs']],
    ];
    const full = new Map();
    for (const [hash, sections] of cases) {
      const f = await boot(env, `/app/thinking/${hash}`);
      full.set(hash, { sections: shownSections(f), address: f.window.location.hash });
      assert.deepStrictEqual(shownSections(f), sections, `a full load of ${hash || 'thinking'}`);
      const p = await boot(env, '/app/thinking/#main');
      p.leave();
      await settle();
      await p.arrive(`/app/thinking/${hash}`);
      const id = p.entry().state?.solRoute?.id;
      assert.ok(p.entry().state.solRoute.owner === 'router', `${hash}: the move's entry`);
      assert.ok(focusOn(p) !== 'BODY', `${hash}: focus is not on the page (${focusOn(p)})`);
      assert.deepStrictEqual(shownSections(p), full.get(hash).sections, `in the page, ${hash || 'thinking'} shows what a full load shows`);
      assert.ok(p.window.location.hash === full.get(hash).address, `and its address (${p.window.location.hash} / ${full.get(hash).address})`);
      assert.ok(typeof id === 'string' && id === p.window.history.state.solRoute.id, `${hash}: the entry keeps its id`);
      assert.ok(p.window.solRouter.held().entryId === id, `${hash}: the router knows the entry`);
    }
  });

  await test('a slashless address takes thinking\'s own, keeping its entry', async () => {
    const p = await boot(env, '/app/thinking/#main');
    p.leave();
    await settle();
    const reads = p.reads('/app/thinking/api/state').length;
    await p.arrive(`/app/thinking#runs/${RUN_DAY}`);
    assert.ok(p.window.location.pathname === '/app/thinking/', `the address is thinking's (${p.window.location.pathname})`);
    assert.ok(p.window.location.hash === `#runs/${RUN_DAY}`, 'with its hash');
    assert.ok(p.entries.length === 3, 'replaced, not added');
    assert.ok(p.window.solRouter.held().mountedPath === '/app/thinking/', 'the router follows the address');
    assert.ok(p.window.solRouter.held().entryId === p.window.history.state.solRoute.id, 'and keeps the entry');
    assert.ok(p.reads('/app/thinking/api/state').length === reads + 1, 'state read once');
  });

  await test('a back and a forward over thinking\'s entries show each and keep the router\'s ids', async () => {
    const p = await boot(env);
    p.press(p.byId('thinkingRunsTab'));
    await settle();
    p.press(p.byId('thinkingRunsPrevious'));
    await settle();
    const ids = p.entries.map((e) => e.state?.solRoute?.id);
    assert.ok(ids.every((id) => typeof id === 'string') && new Set(ids).size === ids.length, `each entry has its own id (${ids.join(',')})`);
    assert.ok(p.entries.slice(1).every((e) => e.state.solRoute.owner === 'view'), 'thinking\'s own entries');
    p.window.history.back();
    await settle();
    assert.ok(p.window.location.hash === `#runs/${TODAY}` && shownSections(p).includes('runs'), 'back: today\'s runs');
    assert.ok(p.window.solRouter.held().entryId === ids[1], 'the router follows the entry');
    // The section's heading, or the tab the owner is on, as at the base: never the page.
    assert.ok(['H2#thinkingRunsHeading', 'BUTTON#thinkingRunsTab', 'BUTTON#thinkingRunsPrevious'].includes(focusOn(p)), `focus stays with thinking (${focusOn(p)})`);
    p.window.history.back();
    await settle();
    assert.ok(shownSections(p).includes('main'), 'back: setup');
    p.window.history.forward();
    await settle();
    assert.ok(p.window.location.hash === `#runs/${TODAY}`, 'forward: runs');
    p.window.history.forward();
    await settle();
    assert.ok(p.window.location.hash === '#runs/20261113', 'forward: the day before');
    assert.ok(p.window.solRouter.held().entryId === ids[2]);
    // Leaving, then back onto a runs entry from the other view: a new mount at that address.
    p.leave();
    await settle();
    p.window.history.back();
    await settle();
    assert.ok(p.window.SolViews.current()?.name === 'thinking', 'thinking, in the page');
    assert.ok(p.window.location.hash === '#runs/20261113' && shownSections(p).includes('runs'), 'the runs day, as a full load of it');
    assert.ok(focusOn(p) !== 'BODY', `focus is not on the page (${focusOn(p)})`);
    assert.ok(p.window.history.state.solRoute.id === ids[2], 'the entry kept its id');
  });

  await test('the run dialog closes with dispose and with a back', async () => {
    const p = await boot(env, `/app/thinking/#runs/${RUN_DAY}/pulse/use-1`);
    p.press(p.byId('thinkingRunsPrompt'));
    await settle();
    assert.ok(p.byId('thinkingRunsPromptModal').open === true, 'the dialog is open');
    const modal = p.byId('thinkingRunsPromptModal');
    p.reconciles.length = 0;
    p.leave();
    await settle();
    assert.ok(modal.open === false, 'dispose closed it');
    assert.ok(p.reconciles.some((o) => o && o.restore === false), 'and the modal layer was told without a restore');
    // A back that leaves the run's entry for another of thinking's.
    const q = await boot(env, `/app/thinking/#runs/${RUN_DAY}`);
    q.press(q.qa('#thinkingRunsList button, .thinking-runs-cards button').find((b) => !b.closest('[hidden]')) || null);
    await settle();
    assert.ok(q.window.location.hash.endsWith('/use-1'), `a run is open (${q.window.location.hash})`);
    q.press(q.byId('thinkingRunsPrompt'));
    await settle();
    assert.ok(q.byId('thinkingRunsPromptModal').open === true, 'the dialog is open');
    q.reconciles.length = 0;
    q.window.history.back();
    await settle();
    assert.ok(q.byId('thinkingRunsPromptModal').open === false, 'the back closed it');
    // The layer hears the close at once, so it doesn't pull the focus that
    // follows back into a closed dialog.
    assert.ok(q.reconciles.some((o) => o && o.restore === false), 'the modal layer was told, without a restore');
    assert.ok(focusOn(q) !== 'BODY', `focus is not on the page (${focusOn(q)})`);
  });

  // ---- a poll whose answer changes nothing leaves the page alone

  const brainChecking = clone(local.get('/app/thinking/api/providers'));
  brainChecking.brain = Object.assign({}, brainChecking.brain, { state: 'checking', headline: 'checking thinking', action: { label: 'check again', refresh: true } });
  const runtimePolling = Object.assign(clone(local.get('/app/thinking/api/local/runtime') || {}), { poll: true, status: 'starting', phase: 'starting' });
  const quietPolls = [
    ['install status', '/app/thinking/#local-setup', { overrides: { 'GET /app/thinking/api/local/bootstrap/status': installing(local) } }, '/app/thinking/api/local/bootstrap/status', 'localCancel'],
    ['local runtime', '/app/thinking/#local-setup', { overrides: { 'GET /app/thinking/api/local/runtime': runtimePolling } }, '/app/thinking/api/local/runtime', 'localRefresh'],
    ['confidential turn-on', '/app/thinking/#confidential-setup', { overrides: { 'GET /app/thinking/api/providers': confidentialTurningOn(local) } }, '/app/thinking/api/providers', 'confidentialLaneOperationLink'],
    ['ChatGPT sign-in', '/app/thinking/#byo-setup', { storage: { 'solstone.thinking.chatgptAttempt': ATTEMPT }, overrides: { 'GET /app/thinking/api/chatgpt/sign-in/attempt-1': { state: 'pending' } } }, '/app/thinking/api/chatgpt/sign-in/attempt-1', 'chatgptFinish'],
    ['brain follow', '/app/thinking/', { overrides: { 'GET /app/thinking/api/providers': brainChecking, 'POST /app/thinking/api/brain/check': { brain: brainChecking.brain } } }, '/app/thinking/api/providers', 'brainCheckAction'],
    ['access re-read', '/app/thinking/', { phase: 'confidential' }, '/app/thinking/api/state', 'thinkingRunsTab'],
    ['access re-read, local blocked', '/app/thinking/', { phase: 'confidential', overrides: { 'GET /app/thinking/api/state': Object.assign(clone(env.answers('confidential').get('/app/thinking/api/state')), { local_capability: { overall: 'blocked', reason_code: 'gpu', detail: 'this computer has no GPU local thinking can use.' } }) } }, '/app/thinking/api/state', 'thinkingRunsTab'],
  ];
  for (const [name, address, options, prefix, focusId] of quietPolls) {
    await test(`the ${name} poll, answering what the page already shows, writes nothing and keeps the focused node`, async () => {
      const p = await boot(env, address, options);
      if (name === 'brain follow') {
        p.press(p.byId('brainCheckAction'));
        await settle();
      }
      // Each poll's first answer can still differ from the first read; the
      // ones after it say the same thing.
      if (!name.startsWith('access re-read')) await run(p, 3000);
      const control = p.byId(focusId);
      control.focus();
      assert.ok(focusOn(p) === `${control.tagName}#${focusId}`, `${name}: focus is on ${focusId} (${focusOn(p)})`);
      const asked = p.reads(prefix).length;
      const before = p.mutations();
      await run(p, 4500);
      assert.ok(p.reads(prefix).length > asked, `${name}: the poll read again`);
      assert.ok(p.mutations() === before, `${name}: a poll that changes nothing writes nothing (${p.mutations() - before} writes)`);
      assert.ok(p.doc.activeElement === control, `${name}: the same node keeps focus (${focusOn(p)})`);
    });
  }

  await test('a poll that changes something draws it, and focus stays on the same control', async () => {
    const one = installing(local);
    const p = await boot(env, '/app/thinking/#local-setup', { overrides: { 'GET /app/thinking/api/local/bootstrap/status': () => one } });
    await run(p, 3000);
    const cancel = p.byId('localCancel');
    cancel.focus();
    const before = p.mutations();
    Object.assign(one, { progress_bytes_received: 2e9, progress_bytes_total: 5e9 });
    await run(p, 3000);
    assert.ok(p.mutations() > before, 'the new progress is drawn');
    assert.ok(p.byId('localSetupMessage').textContent.includes('2.0 GB'), 'the line says so');
    assert.ok(p.doc.activeElement === cancel, `focus stays on cancel (${focusOn(p)})`);
  });

  await test('a redraw keeps an endpoint the owner is typing, until the journal\'s saved one changes', async () => {
    const turning = confidentialTurningOn(env.answers('byo_endpoint'));
    const p = await boot(env, '/app/thinking/#byo-setup', { phase: 'byo_endpoint', overrides: { 'GET /app/thinking/api/providers': () => turning } });
    p.press(p.byId('byoModeEndpoint'));
    await settle();
    const url = p.byId('localEndpointUrl');
    const model = p.byId('localEndpointModel');
    url.value = 'http://127.0.0.1:9';
    url.focus();
    url.setSelectionRange(3, 5);
    model.value = 'half-typ';
    await run(p, 4500);
    assert.ok(p.reads('/app/thinking/api/providers').length > 1, 'the turn-on poll redrew');
    assert.ok(url.value === 'http://127.0.0.1:9' && model.value === 'half-typ', `typed text stays (${url.value} · ${model.value})`);
    assert.ok(p.doc.activeElement === url && url.selectionStart === 3 && url.selectionEnd === 5, 'focus and caret stay');
    turning.local_override = Object.assign({}, turning.local_override, { endpoint_url: 'http://saved.example/v1', served_model_id: 'saved-model' });
    await run(p, 3000);
    assert.ok(url.value === 'http://saved.example/v1' && model.value === 'saved-model', `a new saved endpoint shows (${url.value} · ${model.value})`);
  });

  // ---- presses keep focus where the owner is

  // A keyboard press: focus on the control, then its click.
  async function pressFrom(p, id, ms = 2000) {
    const el = typeof id === 'string' ? p.byId(id) : id;
    assert.ok(el && !el.hidden && !el.closest('[hidden]'), `${typeof id === 'string' ? id : el.id} is drawn`);
    el.focus();
    assert.ok(p.doc.activeElement === el, `focus is on ${el.id || el.tagName} before its press (${focusOn(p)})`);
    p.press(el);
    await run(p, ms);
    return el;
  }

  const byoKeys = clone(env.answers('byo_cloud').get('/app/thinking/api/keys'));
  const noKeys = clone(env.answers('none').get('/app/thinking/api/keys'));
  const validOpenai = Object.assign(clone(byoKeys), { api_keys: Object.assign({}, byoKeys.api_keys, { openai: true }), key_validation: { openai: { valid: true, timestamp: '2026-11-14T12:00:00Z' } } });
  const byoProviders = clone(env.answers('byo_cloud').get('/app/thinking/api/providers'));
  const localProviders = clone(local.get('/app/thinking/api/providers'));
  const confProviders = clone(env.answers('confidential').get('/app/thinking/api/providers'));
  const confOff = clone(env.answers('confidential_inactive').get('/app/thinking/api/providers'));
  confOff.active_lane = Object.assign({}, confOff.active_lane, { confidential_provenance_configured: false, confidential_operation: null, confidential_attestation: { state: 'off' } });
  const confStarting = clone(confOff);
  confStarting.active_lane = Object.assign({}, confStarting.active_lane, { confidential_operation: { phase: 'waiting', portal_url: 'https://services.example/consent' } });
  const signedIn = { signed_in: true, email: 'owner@example.com' };
  const models = { models: [{ slug: 'gpt-lab', display_name: 'lab model' }] };

  // A journal whose bundled model isn't installed yet.
  const missingProviders = clone(env.answers('none').get('/app/thinking/api/providers'));
  missingProviders.provider_status = Object.assign({}, missingProviders.provider_status, { local: { generate_ready: false, issues: ['model_missing'], selected: false } });
  const notInstalled = {
    'GET /app/thinking/api/providers': missingProviders,
    'GET /app/thinking/api/local/availability': { available: false, reason_code: 'model_missing', reason: 'the model is not installed' },
  };
  // [row, address, options, steps before the press, the control, expected landing after success, and after failure]
  const rows = [
    ['check this key →', '/app/thinking/#byo-setup', { phase: 'none', overrides: { 'POST /app/thinking/api/keys/check': { valid: true }, 'PUT /app/thinking/api/keys': validOpenai } },
      async (p) => { await pressFrom(p, p.q('[data-byo-provider="openai"]'), 500); p.byId('byoKeyInput').value = 'sk-lab'; }, 'byoSaveKey', 'H3#byoModelHeading',
      { 'POST /app/thinking/api/keys/check': { valid: false, reason_code: 'invalid' } }, 'BUTTON#byoSaveKey'],
    ['clear key', '/app/thinking/#byo-setup', { phase: 'byo_cloud', overrides: { 'PUT /app/thinking/api/keys': noKeys } },
      async (p) => { await pressFrom(p, p.q('[data-byo-provider="openai"]'), 500); await pressFrom(p, 'byoDifferentKey', 500); }, 'byoClearKey', 'INPUT#byoKeyInput',
      { 'PUT /app/thinking/api/keys': () => { throw Object.assign(new Error('no'), { status: 500 }); } }, 'BUTTON#byoClearKey'],
    ['use this model', '/app/thinking/#byo-setup', { phase: 'byo_cloud', overrides: { 'POST /app/thinking/api/validate-model': { valid: true }, 'PUT /app/thinking/api/providers': byoProviders } },
      async (p) => { await pressFrom(p, p.q('[data-byo-provider="openai"]'), 500); p.byId('byoModelInput').value = 'gpt-lab'; p.doc.dispatch(p.byId('byoModelInput'), { type: 'input' }); }, 'byoModelSave', 'BUTTON#byoLaneStatus',
      { 'POST /app/thinking/api/validate-model': { valid: false, reason_code: 'model_not_found' } }, 'BUTTON#byoModelSave'],
    ['turn on confidential processing', '/app/thinking/#confidential-setup', { phase: 'confidential_inactive', overrides: { 'GET /app/thinking/api/providers': () => (turnedOn ? confStarting : confOff), 'POST /app/thinking/api/confidential/enable': () => { turnedOn = true; return { operation: { phase: 'waiting', portal_url: 'https://services.example/consent' } }; } } },
      () => { turnedOn = false; }, 'confidentialEnable', 'A#confidentialLaneOperationLink',
      { 'POST /app/thinking/api/confidential/enable': () => ({ __status: 500 }) }, 'BUTTON#confidentialEnable'],
    ['turn off confidential processing', '/app/thinking/#confidential-setup', { phase: 'confidential', overrides: { 'POST /app/thinking/api/confidential/disable': { ok: true } } },
      null, 'confidentialDisable', 'BUTTON#confidentialLaneStatus',
      { 'POST /app/thinking/api/confidential/disable': () => ({ __status: 500 }) }, 'BUTTON#confidentialDisable'],
    ['clear endpoint', '/app/thinking/#byo-setup', { phase: 'byo_endpoint', overrides: { 'DELETE /app/thinking/api/local/endpoint': { local_endpoint: {} } } },
      async (p) => { await pressFrom(p, 'byoModeEndpoint', 500); }, 'localEndpointClear', 'BUTTON#byoModeEndpoint',
      { 'DELETE /app/thinking/api/local/endpoint': () => ({ __status: 500 }) }, 'BUTTON#localEndpointClear'],
    ['Continue with ChatGPT', '/app/thinking/#byo-setup', { phase: 'none', overrides: { 'POST /app/thinking/api/chatgpt/sign-in': { attempt_id: 'attempt-1', authorize_url: 'https://auth.example/authorize' }, 'GET /app/thinking/api/chatgpt/sign-in/attempt-1': { state: 'pending' } } },
      null, 'chatgptCardContinue', 'H3#chatgptPendingHeading',
      { 'POST /app/thinking/api/chatgpt/sign-in': () => ({ __status: 500 }) }, 'BUTTON#chatgptCardContinue'],
    ['sign out (ChatGPT)', '/app/thinking/#byo-setup', { phase: 'none', overrides: { 'GET /app/thinking/api/chatgpt/status': () => signedInNow(), 'GET /app/thinking/api/chatgpt/models': models, 'POST /app/thinking/api/chatgpt/sign-out': () => { signedOut = true; return { revoked: true }; } } },
      async (p) => { await pressFrom(p, 'chatgptCardManage', 1000); }, 'chatgptSignOut', 'BUTTON#chatgptCardContinue',
      null, null],
    ['finish signing in', '/app/thinking/#byo-setup', { phase: 'none', storage: { 'solstone.thinking.chatgptAttempt': ATTEMPT }, overrides: { 'GET /app/thinking/api/chatgpt/sign-in/attempt-1': { state: 'pending' }, 'POST /app/thinking/api/chatgpt/sign-in/attempt-1/finish': () => { finished = true; return { reason: 'signed_in' }; }, 'GET /app/thinking/api/chatgpt/status': () => (finished ? signedIn : { signed_in: false }), 'GET /app/thinking/api/chatgpt/models': models } },
      (p) => { p.byId('chatgptRedirectUrl').value = 'http://localhost:1455/auth/callback?code=lab'; }, 'chatgptFinish', 'LEGEND#chatgptModelHeading',
      { 'POST /app/thinking/api/chatgpt/sign-in/attempt-1/finish': { reason: 'refused' } }, 'BUTTON#chatgptFinish'],
    ['install', '/app/thinking/#local-setup', { phase: 'none', overrides: { ...notInstalled, 'POST /app/thinking/api/local/bootstrap': () => { installStatus = installing(local); return installStatus; }, 'GET /app/thinking/api/local/bootstrap/status': () => installStatus } },
      () => { installStatus = Object.assign(clone(local.get('/app/thinking/api/local/bootstrap/status') || {}), { install_state: 'idle' }); }, 'localBootstrap', 'BUTTON#localCancel',
      { 'POST /app/thinking/api/local/bootstrap': () => ({ __status: 500 }) }, 'BUTTON#localBootstrap'],
    ['cancel the install', '/app/thinking/#local-setup', { phase: 'none', overrides: { ...notInstalled, 'POST /app/thinking/api/local/bootstrap/cancel': () => { installStatus = Object.assign(clone(installStatus), { install_state: 'idle' }); return installStatus; }, 'GET /app/thinking/api/local/bootstrap/status': () => installStatus } },
      () => { installStatus = Object.assign(installing(local), { attempt_id: 'attempt-9' }); }, 'localCancel', 'BUTTON#localBootstrap',
      null, null],
  ];
  let signedOut = false;
  let turnedOn = false;
  let finished = false;
  let disabled = false;
  let installStatus = null;
  function signedInNow() { return signedOut ? { signed_in: false } : signedIn; }

  for (const [row, address, options, before, control, landing, failing, failLanding] of rows) {
    for (const outcome of failing ? ['success', 'failure'] : ['success']) {
      await test(`${row}: after ${outcome === 'success' ? 'it answers' : 'it fails'}, focus is on ${outcome === 'success' ? landing : failLanding}, never the page`, async () => {
        const overrides = Object.assign({}, options.overrides, outcome === 'failure' ? failing : {});
        for (const [key, value] of Object.entries(overrides)) {
          if (typeof value === 'function') {
            const fn = value;
            // The page keeps what it is given, so each answer is its own copy.
            overrides[key] = (...args) => {
              const answer = fn(...args);
              return answer === undefined ? undefined : clone(answer);
            };
          }
        }
        signedOut = false;
        turnedOn = false;
        finished = false;
        // A step that takes no page sets the journal's state before the load.
        if (before && before.length === 0) await before();
        const p = await boot(env, address, Object.assign({}, options, { overrides }));
        // A failure answer is marked by `__status`.
        p.statuses.push([(call) => call.method !== 'GET' && Object.keys(failing || {}).includes(`${call.method} ${call.url.split('?')[0]}`) && outcome === 'failure', 500]);
        if (before && before.length > 0) await before(p);
        await pressFrom(p, control, 4000);
        const expected = outcome === 'success' ? landing : failLanding;
        assert.ok(focusOn(p) === expected, `${row} (${outcome}): focus on ${expected}, not ${focusOn(p)}`);
        assert.ok(p.byId(control).getAttribute('aria-busy') === null, `${row} (${outcome}): not busy once answered`);
      });
    }
  }

  await test('switch: focus goes to the new lane\'s control', async () => {
    const ready = clone(localProviders);
    ready.provider_status = Object.assign({}, ready.provider_status, { local: { generate_ready: true, issues: [], selected: false } });
    const byo = Object.assign(clone(byoProviders), { provider_status: Object.assign({}, byoProviders.provider_status, { local: { generate_ready: true, issues: [], selected: false } }), local_override: { enabled: false } });
    let current = byo;
    const p = await boot(env, '/app/thinking/#local-setup', { phase: 'byo_cloud', overrides: {
      'GET /app/thinking/api/providers': () => current,
      'GET /app/thinking/api/local/availability': { available: true, reason_code: 'ready' },
      'GET /app/thinking/api/local/bootstrap/status': Object.assign(clone(local.get('/app/thinking/api/local/bootstrap/status') || {}), { install_state: 'installed' }),
      'PUT /app/thinking/api/providers': () => { current = ready; return ready; },
    } });
    await pressFrom(p, 'localActivate', 500);
    assert.ok(shownSections(p).includes('lane-switch'), `the switch is asked (${shownSections(p)})`);
    await pressFrom(p, 'switchConfirmPrimary', 3000);
    assert.ok(focusOn(p) === 'BUTTON#localLaneStatus', `focus on the local lane's control (${focusOn(p)})`);
  });

  await test('turn on local: focus goes to the nearest control left, not the status line', async () => {
    const ready = clone(localProviders);
    ready.active_lane = Object.assign({}, ready.active_lane, { lane: 'none' });
    ready.provider_status = Object.assign({}, ready.provider_status, { local: { generate_ready: true, issues: [], selected: false } });
    const on = clone(localProviders);
    on.active_lane = Object.assign({}, on.active_lane, { lane: 'local' });
    on.provider_status = Object.assign({}, on.provider_status, { local: { generate_ready: true, issues: [], selected: true } });
    const runtimeReady = Object.assign(clone(local.get('/app/thinking/api/local/runtime') || {}), { status: 'ok', phase: 'ready', poll: false });
    on.local_runtime = runtimeReady;
    let current = ready;
    const p = await boot(env, '/app/thinking/#local-setup', { overrides: {
      'GET /app/thinking/api/local/runtime': () => (current === on ? runtimeReady : Object.assign(clone(runtimeReady), { phase: 'stopped', status: 'stopped' })),
      'GET /app/thinking/api/providers': () => current,
      'GET /app/thinking/api/local/availability': { available: true, reason_code: 'ready' },
      'GET /app/thinking/api/local/bootstrap/status': Object.assign(clone(local.get('/app/thinking/api/local/bootstrap/status') || {}), { install_state: 'installed' }),
      'PUT /app/thinking/api/providers': () => { current = on; return on; },
    } });
    await pressFrom(p, 'localActivate', 3000);
    assert.ok(p.byId('localActivate').hidden, 'turn on local is gone');
    assert.ok(focusOn(p) === 'BUTTON#localRefresh', `focus on "check again" (${focusOn(p)})`);
  });

  await test('a press held open keeps focus on its control: busy, drawn, not turned off, and a second press sends nothing', async () => {
    const held = [
      ['localBootstrap', '/app/thinking/#local-setup', { phase: 'none', overrides: notInstalled }, '/app/thinking/api/local/bootstrap'],
      ['confidentialDisable', '/app/thinking/#confidential-setup', { phase: 'confidential' }, '/app/thinking/api/confidential/disable'],
      ['confidentialEnable', '/app/thinking/#confidential-setup', { phase: 'confidential_inactive', overrides: { 'GET /app/thinking/api/providers': confOff } }, '/app/thinking/api/confidential/enable'],
    ];
    for (const [id, address, options, write] of held) {
      const p = await boot(env, address, options);
      p.holds.push((call) => call.url.startsWith(write));
      const el = await pressFrom(p, id, 1000);
      assert.ok(p.doc.activeElement === el, `${id}: focus stays while its write is out (${focusOn(p)})`);
      assert.ok(el.getAttribute('aria-busy') === 'true' && el.getAttribute('aria-disabled') === 'true' && el.disabled === false, `${id}: busy, not turned off`);
      p.press(el);
      await settle();
      assert.ok(p.writes().filter((c) => c.url.startsWith(write)).length === 1, `${id}: a second press sends nothing`);
    }
  });

  await test('save endpoint: busy through its check, the clears unusable meanwhile, and a read lands on the lane', async () => {
    const p = await boot(env, '/app/thinking/#byo-setup', { phase: 'byo_endpoint', overrides: {
      'POST /app/thinking/api/local/endpoint': { local_endpoint: { endpoint_url: 'http://127.0.0.1:8080/v1', served_model_id: 'lab' } },
    } });
    await pressFrom(p, 'byoModeEndpoint', 500);
    p.holds.push((call) => call.url.startsWith('/app/thinking/api/local/endpoint/check'));
    const save = await pressFrom(p, 'localEndpointSave', 1500);
    assert.ok(p.doc.activeElement === save && save.getAttribute('aria-busy') === 'true' && !save.disabled, `save keeps focus, busy (${focusOn(p)})`);
    const clear = p.byId('localEndpointClear');
    assert.ok(clear.getAttribute('aria-disabled') === 'true' && clear.getAttribute('aria-busy') === null && !clear.disabled, 'clear is unusable, not busy, not off');
    p.press(clear);
    await settle();
    assert.ok(!p.writes().some((c) => c.method === 'DELETE'), 'a press on clear does nothing meanwhile');
    answerHeld(p, (call) => call.url.startsWith('/app/thinking/api/local/endpoint/check'), { image_check: { status: 'read' } });
    p.holds.length = 0;
    await run(p, 2000);
    assert.ok(shownSections(p).includes('main'), 'the form closes back to the lanes');
    assert.ok(focusOn(p) === 'BUTTON#byoLaneStatus', `focus on the lane's control (${focusOn(p)})`);
    assert.ok(clear.getAttribute('aria-disabled') === null, 'clear is usable again');
  });

  await test('the audio toggle stays drawn and busy with the owner\'s choice while its write is out, and keeps focus', async () => {
    const p = await boot(env, '/app/thinking/#confidential-setup', { phase: 'confidential' });
    const toggle = p.byId('confidentialAudioToggle');
    assert.ok(!p.byId('confidentialAudioRow').hidden, 'the row is drawn');
    const was = toggle.checked;
    p.holds.push((call) => call.url.startsWith('/app/settings/api/config'));
    toggle.focus();
    toggle.checked = !was;
    p.doc.dispatch(toggle, { type: 'change' });
    await settle();
    assert.ok(!p.byId('confidentialAudioRow').hidden && p.doc.activeElement === toggle, `drawn, with focus (${focusOn(p)})`);
    assert.ok(toggle.checked === !was && toggle.getAttribute('aria-busy') === 'true' && !toggle.disabled, 'the owner\'s choice, busy, not off');
    assert.ok(p.byId('confidentialAudioDescription').textContent === '' && p.byId('confidentialTrustEgress').textContent === '', 'nothing says where audio goes meanwhile');
    answerHeld(p, (call) => call.url.startsWith('/app/settings/api/config'), {});
    p.holds.length = 0;
    await run(p, 1500);
    assert.ok(p.doc.activeElement === toggle && toggle.getAttribute('aria-busy') === null, `the same toggle, answered (${focusOn(p)})`);
  });

  await test('a press whose work throws still applies its kept hide, and one that ends after thinking has gone touches nothing', async () => {
    const p = await boot(env, '/app/thinking/#confidential-setup', { phase: 'confidential', overrides: { 'POST /app/thinking/api/confidential/disable': { ok: true } } });
    // The re-read after the write fails: the press throws, the turn-off still hid.
    let reads = 0;
    p.statuses.push([(call) => isProviders(call) && (reads += 1) > 0 && p.writes().length > 0, 500]);
    p.holds.push((call) => call.url.startsWith('/app/thinking/api/confidential/disable'));
    const off = await pressFrom(p, 'confidentialDisable', 500);
    const confOffNow = clone(confOff);
    p.env.lateProviders = confOffNow;
    answerHeld(p, (call) => call.url.startsWith('/app/thinking/api/confidential/disable'), { ok: true });
    p.holds.length = 0;
    await run(p, 2000);
    assert.ok(off.getAttribute('aria-busy') === null, 'not busy after the throw');
    assert.ok(focusOn(p) !== 'BODY', `focus is not on the page (${focusOn(p)})`);
    // Gone before its answer: nothing drawn, no focus moved.
    const q = await boot(env, '/app/thinking/#confidential-setup', { phase: 'confidential', overrides: { 'POST /app/thinking/api/confidential/disable': { ok: true } } });
    q.holds.push((call) => call.url.startsWith('/app/thinking/api/confidential/disable'));
    const gone = await pressFrom(q, 'confidentialDisable', 500);
    q.leave();
    await drain(q);
    const drawn = q.mutations();
    answerHeld(q, (call) => call.url.startsWith('/app/thinking/api/confidential/disable'), { ok: true });
    await run(q, 3000);
    assert.ok(q.mutations() === drawn, 'nothing is drawn after thinking has gone');
    assert.ok(gone.getAttribute('aria-busy') === 'true', 'the gone markup is left as it was');
  });

  await test('focus outside thinking is left alone, through a poll and through a press\'s draw', async () => {
    const one = Object.assign(installing(local), { attempt_id: 'attempt-9' });
    const p = await boot(env, '/app/thinking/#local-setup', { overrides: { 'GET /app/thinking/api/local/bootstrap/status': () => one, 'POST /app/thinking/api/local/bootstrap/cancel': () => Object.assign(one, { install_state: 'idle' }) } });
    await run(p, 3000);
    const rail = p.q('#app-rail a');
    p.press(p.byId('localCancel'));
    rail.focus();
    await run(p, 3000);
    assert.ok(p.byId('localCancel').hidden, 'cancel went');
    assert.ok(p.doc.activeElement === rail, `the rail keeps focus (${focusOn(p)})`);
  });

  await test('a built part someone else wrote into is built again, even with the same values', async () => {
    const p = await boot(env, '/app/thinking/#byo-setup', { phase: 'none', overrides: { 'GET /app/thinking/api/chatgpt/status': signedIn, 'GET /app/thinking/api/chatgpt/models': models } });
    await pressFrom(p, 'chatgptCardManage', 1000);
    const choices = p.byId('chatgptModelChoices');
    assert.ok(choices.querySelectorAll('input').length === 1, 'the model is drawn');
    // Something else writes into it, leaving as many nodes as it had.
    choices.replaceChildren(p.doc.createElement('span'));
    // Coming back to the tab re-reads and draws the page again.
    p.doc.visibilityState = 'visible';
    p.doc.dispatch(p.doc.body, { type: 'visibilitychange' });
    await run(p, 1500);
    assert.ok(choices.querySelectorAll('input').length === 1, 'drawn again after something else emptied it');
  });

  await test('a press keeps its own control drawn while its reads are still out, then moves focus once', async () => {
    const p = await boot(env, '/app/thinking/#confidential-setup', { phase: 'confidential', overrides: {
      'POST /app/thinking/api/confidential/disable': { ok: true },
      'GET /app/thinking/api/providers': () => (disabled ? confOff : confProviders),
      // The access re-read reads the whole state; it says the same.
      'GET /app/thinking/api/state': () => Object.assign(clone(env.answers('confidential').get('/app/thinking/api/state')), { providers: disabled ? confOff : confProviders }),
    } });
    disabled = false;
    p.holds.push((call) => call.url.startsWith('/app/thinking/api/keys'));
    const off = await pressFrom(p, 'confidentialDisable', 0);
    disabled = true;
    await run(p, 1500);
    assert.ok(p.reads('/app/thinking/api/providers').length > 0 && p.held().some((c) => c.url.startsWith('/app/thinking/api/keys')), 'the providers read answered; the keys read is out');
    assert.ok(!off.hidden && p.doc.activeElement === off && off.getAttribute('aria-busy') === 'true', `still drawn, busy, with focus (${focusOn(p)})`);
    answerHeld(p, (c) => c.url.startsWith('/app/thinking/api/keys'), clone(byoKeys));
    p.holds.length = 0;
    await run(p, 1500);
    assert.ok(focusOn(p) === 'BUTTON#confidentialLaneStatus', `then the lane's control (${focusOn(p)})`);
  });

  await test('a turn-on that answers after thinking has gone leaves its gone control as it was', async () => {
    const q = await boot(env, '/app/thinking/#confidential-setup', { phase: 'confidential_inactive', overrides: { 'GET /app/thinking/api/providers': confOff } });
    q.holds.push((call) => call.url.startsWith('/app/thinking/api/confidential/enable'));
    const on = await pressFrom(q, 'confidentialEnable', 500);
    q.leave();
    await drain(q);
    answerHeld(q, (call) => call.url.startsWith('/app/thinking/api/confidential/enable'), { operation: { phase: 'waiting', portal_url: 'https://services.example/consent' } });
    await run(q, 3000);
    assert.ok(on.getAttribute('aria-busy') === 'true' && !on.hidden, 'the gone markup is not touched');
    assert.ok(q.opened.length === 0, 'and no window opens');
  });

  await test('a control that goes takes focus to the nearest control in its own row before anything else in the part', async () => {
    let cancelled = false;
    let status = Object.assign(installing(local), { attempt_id: 'attempt-9' });
    const p = await boot(env, '/app/thinking/#local-setup', { phase: 'none', overrides: {
      'GET /app/thinking/api/providers': missingProviders,
      // A computer the install can't finish on: once stopped, nothing is offered but "check again" and the links.
      'GET /app/thinking/api/local/availability': { available: false, reason_code: 'gpu_unavailable', reason: 'no GPU' },
      'GET /app/thinking/api/local/bootstrap/status': () => status,
      'POST /app/thinking/api/local/bootstrap/cancel': () => { cancelled = true; status = Object.assign(clone(status), { install_state: 'idle' }); return status; },
    } });
    await run(p, 2000);
    await pressFrom(p, 'localCancel', 4000);
    assert.ok(p.byId('localBootstrap').hidden && p.byId('localCancel').hidden, 'neither install nor cancel is offered now');
    assert.ok(p.qa('#localSetupLinks a, #localSetupLinks button').length > 0, 'the part has links after the row');
    assert.ok(focusOn(p) === 'BUTTON#localRefresh', `focus on "check again", in cancel's own row (${focusOn(p)})`);
  });

  // ---- moves inside thinking: a lane's control, and back to the lanes

  for (const [name, phase, control, section, heading] of [
    ['local', 'bundled_local', 'localLaneStatus', 'local-setup', 'H2#localSetupHeading'],
    ['confidential', 'confidential', 'confidentialLaneStatus', 'confidential-setup', 'H2#confidentialSetupHeading'],
    ['confidential ("how it works")', 'confidential', 'confidentialLaneMore', 'confidential-setup', 'H2#confidentialSetupHeading'],
    ['your own model', 'byo_cloud', 'byoLaneStatus', 'byo-setup', 'H2#byoSetupTitle'],
  ]) {
    await test(`opening the ${name} lane lands on its heading, and its back link on the lane's control`, async () => {
      const p = await boot(env, '/app/thinking/', { phase });
      await pressFrom(p, control, 1500);
      assert.ok(shownSections(p).includes(section), `${section} is shown (${shownSections(p)})`);
      assert.ok(focusOn(p) === heading, `${name}: on its heading (${focusOn(p)})`);
      const back = section === 'byo-setup' ? p.byId('byoBackLink') : p.q(`section[data-view="${section}"] .backlink`);
      await pressFrom(p, back, 1500);
      // From a provider's step, "‹" goes to the providers first.
      if (section === 'byo-setup' && !shownSections(p).includes('main')) await pressFrom(p, back, 1500);
      assert.ok(shownSections(p).includes('main'), 'back on the lanes');
      const laneControl = control === 'confidentialLaneMore' ? 'confidentialLaneStatus' : control;
      assert.ok(focusOn(p) === `BUTTON#${laneControl}`, `${name}: back on the lane's control (${focusOn(p)})`);
    });
  }

  await test('the lane switch\'s back link and "keep" land on the lane that asked', async () => {
    const byo = Object.assign(clone(byoProviders), { provider_status: Object.assign({}, byoProviders.provider_status, { local: { generate_ready: true, issues: [], selected: false } }), local_override: { enabled: false } });
    for (const leave of ['back link', 'keep']) {
      const p = await boot(env, '/app/thinking/#local-setup', { phase: 'byo_cloud', overrides: {
        'GET /app/thinking/api/providers': byo,
        'GET /app/thinking/api/local/availability': { available: true, reason_code: 'ready' },
        'GET /app/thinking/api/local/bootstrap/status': Object.assign(clone(local.get('/app/thinking/api/local/bootstrap/status') || {}), { install_state: 'installed' }),
      } });
      await pressFrom(p, 'localActivate', 500);
      assert.ok(focusOn(p) === 'H2#switchHeading', `the switch's heading (${focusOn(p)})`);
      await pressFrom(p, leave === 'keep' ? p.byId('switchCancel') : p.q('section[data-view="lane-switch"] .backlink'), 1500);
      assert.ok(focusOn(p) === 'BUTTON#localLaneStatus', `${leave}: the local lane's control (${focusOn(p)})`);
    }
  });

  // ---- a pick from the local model select, and a phone's late blur
  //
  // A phone fires change with the select still focused, then some 865 ms
  // later takes focus to the page (blur and focusout) with no input between.

  const PHONE_BLUR_MS = 865;
  const twoModels = [
    { name: 'local/qwen3.5-4b', label: 'small', min_ram_gb: 8, size_bytes: 1 },
    { name: 'local/other-8b', label: 'larger', min_ram_gb: 16, size_bytes: 2 },
  ];
  // The reads a pick starts again: availability, providers, the install and the runtime.
  const PICK_READS = ['/app/thinking/api/local/availability', '/app/thinking/api/providers', '/app/thinking/api/local/bootstrap/status', '/app/thinking/api/local/runtime'];
  const isPickRead = (call) => call.method === 'GET' && PICK_READS.some((prefix) => call.url === prefix || call.url.startsWith(`${prefix}?`));
  async function modelPage() {
    const p = await boot(env, '/app/thinking/#local-setup', { overrides: {
      'GET /app/thinking/api/local/models': twoModels,
      'GET /app/thinking/api/local/availability': { available: true, reason_code: 'ready' },
      'GET /app/thinking/api/local/bootstrap/status': Object.assign(clone(local.get('/app/thinking/api/local/bootstrap/status') || {}), { install_state: 'installed' }),
    } });
    await run(p, 1000);
    const select = p.byId('localModelSelect');
    assert.ok(select && select.checkVisibility() && select.options.length === 2, 'the model select is shown with both models');
    return p;
  }
  function pickModel(p, select) {
    select.focus();
    select.value = twoModels[1].name;
    p.doc.dispatch(select, { type: 'change' });
  }
  function phoneBlur(p, select) {
    select.blur();
    p.doc.dispatch(select, { type: 'blur' });
    p.doc.dispatch(select, { type: 'focusout' });
  }

  await test("the local model: a phone's late blur after a pick lands back on the same select a task later", async () => {
    const p = await modelPage();
    const select = p.byId('localModelSelect');
    pickModel(p, select);
    await settle();
    assert.ok(p.calls.filter(isPickRead).every((call) => call.settled), 'the reads are in before the blur');
    p.clock.advance(PHONE_BLUR_MS);
    await settle();
    assert.ok(p.doc.activeElement === select, `still on the select before the blur (${focusOn(p)})`);
    phoneBlur(p, select);
    assert.ok(p.doc.activeElement === p.doc.body, 'the blur takes focus to the page');
    p.clock.advance(0);
    await settle();
    assert.ok(p.doc.activeElement === select, `focus is back on the same select (${focusOn(p)})`);
    assert.ok(p.byId('localModelSelect') === select, 'the select is the one picked from');
  });

  await test('the local model: reads that answer after the landing redraw, and focus stays on the same select', async () => {
    const p = await modelPage();
    const select = p.byId('localModelSelect');
    const from = p.calls.length;
    p.holds.push(isPickRead);
    pickModel(p, select);
    await settle();
    p.clock.advance(PHONE_BLUR_MS);
    phoneBlur(p, select);
    p.clock.advance(0);
    await settle();
    assert.ok(p.doc.activeElement === select, `landed before the reads are in (${focusOn(p)})`);
    const out = p.calls.slice(from).filter((call) => isPickRead(call) && !call.settled);
    assert.ok(out.length === PICK_READS.length, `the pick's reads are out (${out.map((c) => c.url).join(' ')})`);
    p.holds.length = 0;
    const answers = journal(env.answers('bundled_local'), {
      'GET /app/thinking/api/local/availability': { available: true, reason_code: 'ready' },
      'GET /app/thinking/api/local/bootstrap/status': Object.assign(clone(local.get('/app/thinking/api/local/bootstrap/status') || {}), { install_state: 'installed' }),
    });
    out.forEach((call) => call.answer(answers(call.url, { method: 'GET' })));
    await settle();
    assert.ok(p.byId('localModelSelect') === select, 'the redraw keeps the select');
    assert.ok(p.doc.activeElement === select, `focus stays on the same select (${focusOn(p)})`);
  });

  await test('the local model: a pick with no late blur leaves focus alone, and the wait leaves nothing behind', async () => {
    const p = await modelPage();
    const select = p.byId('localModelSelect');
    select.focus();
    const before = [p.doc.listenerCount(), select.listenerCount(), p.clock.live()];
    pickModel(p, select);
    await settle();
    p.clock.advance(2000);
    await settle();
    assert.ok(p.doc.activeElement === select, `focus stays on the select (${focusOn(p)})`);
    assert.deepStrictEqual([p.doc.listenerCount(), select.listenerCount(), p.clock.live()], before, 'no listener or timer of the wait is left');
    phoneBlur(p, select);
    p.clock.advance(0);
    await settle();
    assert.ok(p.doc.activeElement === p.doc.body, `a later blur is left alone (${focusOn(p)})`);
  });

  await test("the local model: a pick reads its four once, and the late blur and its landing read nothing", async () => {
    const p = await modelPage();
    const select = p.byId('localModelSelect');
    const from = p.calls.length;
    pickModel(p, select);
    await settle();
    const picked = p.calls.slice(from).filter((call) => call.method === 'GET');
    assert.ok(picked.length === PICK_READS.length && PICK_READS.every((prefix) => picked.filter(isPickRead).some((c) => c.url.split('?')[0] === prefix)),
      `one of each read for the pick (${picked.map((c) => c.url).join(' ')})`);
    assert.ok(picked.some((c) => c.url === `/app/thinking/api/local/availability?model=${encodeURIComponent(twoModels[1].name)}`), 'availability asks after the picked model');
    const afterPick = p.calls.length;
    p.clock.advance(PHONE_BLUR_MS);
    phoneBlur(p, select);
    p.clock.advance(0);
    await settle();
    assert.ok(p.calls.length === afterPick, `nothing more is read (${p.calls.slice(afterPick).map((c) => c.url).join(' ')})`);
  });

  return cases;
};
