// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Stats on the view contract, under plain Node: the real fragment and both of
// its scripts, mounted through the real view contract, read store, router,
// focus helpers and day control, in a stand-in page with events, focus, a
// history and a clock the cases move by hand. stats_web_dom.js runs these
// after its own cases. Every compare is of plain values, ids or tag names:
// never an element.

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
    this.disabled = false;
    this.checked = false;
    this._style = null;
    const self = this;
    this.classList = {
      list: () => (self.getAttribute('class') || '').split(/\s+/).filter(Boolean),
      add(...cs) { const l = this.list(); cs.forEach((c) => { if (!l.includes(c)) l.push(c); }); self.setAttribute('class', l.join(' ')); },
      remove(...cs) { self.setAttribute('class', this.list().filter((x) => !cs.includes(x)).join(' ')); },
      contains(c) { return this.list().includes(c); },
      toggle(c, on) { const want = on === undefined ? !this.contains(c) : !!on; if (want) this.add(c); else this.remove(c); return want; },
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
  get hidden() { return this.hasAttribute('hidden'); }
  set hidden(v) { if (v) this.setAttribute('hidden', ''); else this.removeAttribute('hidden'); }
  setAttribute(k, v) { this.attrs.set(k, String(v)); if (k === 'style') this._style = null; }
  getAttribute(k) { return this.attrs.has(k) ? this.attrs.get(k) : null; }
  hasAttribute(k) { return this.attrs.has(k); }
  removeAttribute(k) { this.attrs.delete(k); }
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
  querySelectorAll(selector) {
    const out = [];
    const walk = (el) => el.children.forEach((c) => { if (matches(c, selector)) out.push(c); walk(c); });
    walk(this);
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
  doc.title = 'stats - journal';
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
function counted(original) {
  return function (...args) {
    if (depth === 0 && this.isConnected) mutations += 1;
    depth += 1;
    try { return original.apply(this, args); } finally { depth -= 1; }
  };
}
for (const name of ['append', 'removeChild', 'insertBefore', 'setAttribute', 'removeAttribute']) {
  Element.prototype[name] = counted(Element.prototype[name]);
}
const textContent = Object.getOwnPropertyDescriptor(Element.prototype, 'textContent');
Object.defineProperty(Element.prototype, 'textContent', { get: textContent.get, set: counted(textContent.set) });

// ---- the journal the page reads

const TODAY = '20261114';
const YESTERDAY = '20261113';

function heatmap() {
  return Array.from({ length: 7 }, (_, d) => Array.from({ length: 24 }, (_, h) => (d + h) % 5));
}

function statsAnswer(overrides = {}) {
  const days = {};
  ['20261112', '20261113', '20261114'].forEach((day, i) => {
    days[day] = { transcript_duration: 3600 * (i + 1), percept_duration: 1800, day_bytes: 1024 * (i + 1) };
  });
  const stats = {
    schema_version: 10,
    generated_at: '2026-11-14T10:00:00Z',
    days,
    totals: { total_transcript_duration: 21600, total_percept_duration: 5400, day_bytes: 6144, transcript_sessions: 4, pending_segments: 1, segments_pending_think: 0 },
    heatmap: heatmap(),
    tokens: {
      by_day: { [YESTERDAY]: { m: { total_tokens: 40 } }, [TODAY]: { m: { total_tokens: 100 } } },
      by_model: { m: { total_tokens: 140 } },
    },
    talents: { counts_by_day: { [TODAY]: { ai_conversation: 2 } } },
    facets: { counts_by_day: { [TODAY]: { work: 3 } } },
    backlog: {
      pending_days: 1, stuck_days: 1, degraded: false,
      days: [
        { day: '20261110', state: 'stuck', segments: 2, units: 1, why: [{ why: 'failed' }] },
        { day: '20261111', state: 'pending', segments: 1, units: 0, why: [{ why: 'never_attempted' }] },
      ],
    },
    journal_status: { verdict: '1 day needs a hand. 1 more day is still catching up.', freshness: 'fresh', copy: {} },
    ...overrides,
  };
  return { stats, facet_titles: { work: 'work' }, file_mtime: 1763114400, generators: {} };
}

function usageAnswer(day) {
  return {
    day,
    total: { requests: 3, tokens: 100 },
    by_provider: [{ provider: 'google', requests: 3, tokens: 100, cached_tokens: 0, percent: 100 }],
    by_model: [
      { model: 'a', provider: 'google', requests: 2, tokens: 60, cached_tokens: 0, percent: 60 },
      { model: 'b', provider: 'google', requests: 1, tokens: 40, cached_tokens: 0, percent: 40 },
    ],
    by_type: { generate: { requests: 2, tokens: 60 }, cogitate: { requests: 1, tokens: 40 } },
  };
}

// Answers by address and method. `undefined` leaves the request out.
function journal(overrides = {}) {
  return (url, opts) => {
    const u = new URL(url, ORIGIN);
    const method = String(opts?.method || 'GET').toUpperCase();
    const key = `${method} ${u.pathname}`;
    if (key in overrides) return typeof overrides[key] === 'function' ? overrides[key](u, opts) : overrides[key];
    switch (key) {
      case 'GET /app/stats/api/stats': return statsAnswer();
      case 'GET /app/stats/api/usage': return usageAnswer(u.searchParams.get('day'));
      case 'GET /app/stats/api/index': return { coverage: [[YESTERDAY, TODAY]], months: {} };
      case 'POST /app/health/api/reprocess': return { status: 'queued' };
      default: return {};
    }
  };
}

// ---- one page with stats in it

function makePage(env, address = '/app/stats/', { answers } = {}) {
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
  const clock = makeClock();
  const calls = [];
  const errors = [];
  const window = new Target();
  doc.window = window;
  const entries = [{ url: current.href, state: { solRoute: { id: 'e0', owner: 'router' } } }];
  let index = 0;
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
    back() {
      index -= 1;
      location.set(entries[index].url);
      window.fire({ type: 'popstate', state: entries[index].state });
    },
    forward() {
      index += 1;
      location.set(entries[index].url);
      window.fire({ type: 'popstate', state: entries[index].state });
    },
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
  window.JournalFormat = { day: (key) => `day ${key}`, duration: (s) => `${s} s` };
  window.JournalClock = { today: () => TODAY, now: () => new Date('2026-11-14T12:00:00Z'), dayKey: () => TODAY, placeNote() {} };
  window.ConveyIcons = { svg: (name) => `<i>${name}</i>` };
  window.SurfaceState = { empty: ({ heading }) => `<p class="empty">${heading}</p>` };
  window.relativeTime = (ms) => `${Math.round(ms / 60000)} min`;
  window.logError = (error, context) => { errors.push({ error, context }); };
  window.appEvents = { buildChanged: () => false, onConnectionState: () => () => {}, listen: () => () => {} };
  window.__SOL_ENTRY_TABLE__ = { views: { stats: { contract: true, fragment: '/app/stats/workspace', scripts: [], styles: [] } } };
  window.solShellData = { apps: [{ name: 'stats', label: 'stats', workspace_url: '/app/stats/workspace' }] };

  const serve = answers || journal();
  const ask = (url, opts, kind) => new Promise((resolve, reject) => {
    const u = new URL(url, current.href);
    const call = {
      url: u.pathname + u.search,
      method: String(opts?.method || 'GET').toUpperCase(),
      body: opts?.body,
      kind, aborted: false, settled: false,
      answer(body, status = 200, extra = {}) {
        if (call.settled) return;
        call.settled = true;
        if (kind === 'fetch') {
          resolve({ ok: status < 400, status, redirected: !!extra.redirected, json: async () => body });
          return;
        }
        if (status >= 400) {
          const error = new Error(`HTTP ${status}`);
          error.status = status;
          error.serverMessage = body?.error;
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

  parseInto(doc, doc.body, '<nav id="app-rail"><a href="/app/home/">home</a><a href="/app/stats/">stats</a></nav><main id="main-content" tabindex="-1"></main><div id="view-announcer"></div>');
  const main = doc.getElementById('main-content');

  const page = {
    env, window, doc, main, clock, calls, errors, loads,
    confirmAnswer: true,
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
      env.scripts.forEach(([name, source]) => vm.runInContext(source, context, { filename: name }));
    },
    async mount() {
      page.ready = [];
      page.view = await window.SolViews.mount('stats', { reportReady: (s) => page.ready.push(s) });
      await settle();
      return page.view;
    },
    // Another view takes the page, as the router's move and place() do.
    leave(next = '/app/home/') {
      window.history.pushState({ solRoute: { id: 'r' + entries.length, owner: 'router' } }, '', next);
      window.SolViews.dispose();
      main.replaceChildren();
      parseInto(doc, main, '<section id="home"><h1>home</h1><p id="home-text">home as it was</p></section>');
    },
    // Back onto stats in the same document, as a move to it does.
    async arrive(addressAgain = '/app/stats/') {
      window.history.pushState({ solRoute: { id: 'a' + entries.length, owner: 'router' } }, '', addressAgain);
      page.place();
      return page.mount();
    },
    press(node) {
      assert.ok(node, 'the control to press is drawn');
      return node.click();
    },
    find(selector, text) {
      return page.qa(selector).find((node) => node.textContent.trim() === text) || null;
    },
  };

  const context = vm.createContext({
    window, document: doc, location, history: window.history,
    URL, URLSearchParams, AbortController, AbortSignal, DOMException, WeakMap, WeakRef, Map, Set, Promise,
    console, Math, Number, String, Object, Array, Boolean, Date, Intl, JSON, Error, TypeError, RegExp, Symbol, Infinity, CustomEvent: class {},
    setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval, clearInterval: clock.clearInterval,
    addEventListener: window.addEventListener.bind(window),
    performance: { mark() {}, clearMarks() {} },
    fetch: window.fetch,
    confirm: () => page.confirmAnswer,
    relativeTime: window.relativeTime,
  });
  context.self = window;
  window.confirm = () => page.confirmAnswer;
  const shell = env.shellDir;
  vm.runInContext(fs.readFileSync(path.join(shell, 'store.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'views.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'router.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'focus.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'date-nav.js'), 'utf8'), context);
  // The page's router is on, as it is once a contract view has mounted: it owns the history's ids.
  window.solRouter.start('stats');
  page.context = context;
  return page;
}

function flush() {
  return new Promise((resolve) => setImmediate(resolve));
}

async function settle() {
  for (let i = 0; i < 12; i += 1) await flush();
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
  const label = active.textContent.trim();
  return label ? `${active.tagName}:${label}` : active.tagName;
}

function listenerCounts(p) {
  return [p.window.listenerCount(), p.doc.listenerCount()];
}

function answerHeld(p, test, body, status = 200, extra) {
  p.calls.filter((call) => !call.settled && test(call)).forEach((call) => call.answer(body, status, extra));
}

// The gone view's markup, as it was when the owner left: no status line,
// the pressed button still busy.
function goneViewUntouched(root, outcome) {
  const gone = root.deref();
  assert.ok(gone, 'the gone markup is still reachable here');
  assert.ok(gone.querySelector('[data-backlog-status="20261110"]').textContent === '', `${outcome}: no status line in the gone view`);
  const pressed = gone.querySelector('[data-backlog-day="20261110"][data-backlog-flavor="process-now"]');
  assert.ok(pressed.getAttribute('aria-busy') === 'true', `${outcome}: the gone view is left as it was`);
}

const isStats = (call) => call.url === '/app/stats/api/stats';
const isWrite = (call) => call.method === 'POST' && call.url === '/app/health/api/reprocess';
const GONE = ['Dashboard', 'BACKLOG_COPY', 'mountStats', 'render', 'load', 'reportStatsReady', 'requestBacklogReprocess', 'renderBacklog', 'select', 'renderRollup'];

// Run from stats_web_dom.js: answers with the number of cases.
module.exports = async function statsContractCases(crateDir) {
  const workspace = fs.readFileSync(path.join(crateDir, 'assets/workspace.html'), 'utf8');
  const dashboard = fs.readFileSync(path.join(crateDir, 'assets/static/dashboard.js'), 'utf8');
  const tokenCard = fs.readFileSync(path.join(crateDir, 'assets/static/token-card.js'), 'utf8');
  const markup = workspace.replace(/<style>[\s\S]*?<\/style>/g, '').replace(/<script[\s\S]*?<\/script>/g, '').replace(/<link[^>]*>/g, '');
  const shellDir = path.join(crateDir, '../solstone-core-convey-shell/assets/static');
  const env = { markup, shellDir, scripts: [['token-card.js', tokenCard], ['dashboard.js', dashboard]] };

  await test('the fragment is a contract view with its first read, no inline script and no handler naming a global', async () => {
    assert.ok(workspace.includes('<!-- view-contract -->'));
    assert.ok(workspace.includes('<!-- first-read: /app/stats/ -> /app/stats/api/stats -->'));
    assert.ok(!/\son[a-z]+="/.test(workspace), 'no inline handler attribute');
    assert.ok(!/<script>/.test(workspace), 'no inline script');
    const order = [...workspace.matchAll(/<script src="([^"]+)"/g)].map((m) => m[1]);
    assert.deepStrictEqual(order, ['/static/date-nav.js', '/app/stats/static/token-card.js', '/app/stats/static/dashboard.js']);
    for (const source of [dashboard, tokenCard]) {
      assert.ok(!/history\.(pushState|replaceState)/.test(source), 'the address is written only through the view contract');
      assert.ok(!/\bdisabled\s*=\s*true/.test(source), 'no control is turned off while its press is out');
      assert.ok(!/document\.(addEventListener|dispatchEvent)/.test(source), 'nothing listens on or fires at the document');
    }
  });

  await test('the scripts only register: nothing reads, draws, listens or starts a timer at load', async () => {
    const p = makePage(env);
    p.place();
    const keys = p.windowKeys();
    const listeners = listenerCounts(p);
    const timers = p.clock.live();
    p.loadScripts();
    assert.ok(p.calls.length === 0, 'no read at load');
    assert.deepStrictEqual(p.windowKeys().filter((k) => !keys.includes(k)), ['StatsTokenCard'], 'only the card factory is defined');
    assert.deepStrictEqual(listenerCounts(p), listeners, 'no listener at load');
    assert.ok(p.window.SolViews.registered('stats') === true);
    assert.ok(p.clock.live() === timers, 'no timer at load');
    assert.ok(p.byId('mainContent').style.display === 'none', 'nothing drawn');
  });

  await test('a mount reads stats, the day and the coverage, draws, and reports ready once', async () => {
    const p = await boot(env);
    assert.ok(p.reads('/app/stats/api/stats').length === 1, 'stats asked once');
    assert.ok(p.reads('/app/stats/api/stats')[0].kind === 'fetch', 'through the store\'s fetch, so the early read is adopted');
    assert.ok(p.reads(`/app/stats/api/usage?day=${TODAY}`).length === 1, 'today\'s usage');
    assert.ok(p.reads('/app/stats/api/index').length === 1, 'coverage');
    assert.ok(p.byId('mainContent').style.display === 'block', 'drawn');
    assert.ok(p.qa('.stat-card').length === 5, 'five tiles');
    assert.ok(p.qa('[data-backlog-flavor]').length === 4, 'two backlog days, two actions each');
    assert.ok(p.qa('.token-rollup__bar').length === 2, 'the rollup reached the card');
    assert.ok(p.find('button', 'refresh')?.hidden === false, 'refresh is shown');
    assert.deepStrictEqual(p.ready, ['content']);
    assert.ok(p.errors.length === 0, 'no error logged');
  });

  await test('dispose leaves no listener, timer, read, global or day control, and stats mounts again in the same page', async () => {
    const p = makePage(env);
    p.loadScripts();
    const keys = p.windowKeys();
    const listeners = listenerCounts(p);
    for (let round = 0; round < 2; round += 1) {
      p.place();
      p.holds.push((call) => call.url.startsWith('/app/stats/api/index'));
      await p.mount();
      assert.ok(p.qa('#statsDateNav [data-date-nav-prev]').length === 1, `round ${round}: the day control is drawn`);
      assert.ok(p.qa('.stat-card').length === 5, `round ${round}: drawn`);
      assert.ok(p.window.listenerCount() > listeners[0], 'the view listens on the window while it is up');
      p.leave();
      await settle();
      assert.deepStrictEqual(listenerCounts(p), listeners, `round ${round}: every window and document listener is gone`);
      assert.deepStrictEqual(p.windowKeys(), keys, `round ${round}: no global is left`);
      for (const name of GONE) assert.ok(!(name in p.window) && !(name in p.context), `${name} is not a global`);
      assert.ok(p.held().every((call) => call.aborted), `round ${round}: every read still out was cut`);
      assert.ok(p.clock.live() === 0, `round ${round}: no timer`);
      p.holds.length = 0;
      p.window.history.back();
    }
    // The day control is one per page; dispose gave it back.
    const host = p.doc.createElement('div');
    p.main.append(host);
    const probe = p.window.DateNav.mountScoped({ host, initialDay: TODAY, onSelect() {} });
    assert.ok(probe !== null, 'the page\'s day control is free again');
    probe.unmount();
  });

  await test('a read out when stats goes is cut, and its late answer draws nothing and reloads nothing', async () => {
    for (const [name, test] of [['stats', isStats], ['usage', (c) => c.url.startsWith('/app/stats/api/usage')], ['index', (c) => c.url.startsWith('/app/stats/api/index')]]) {
      const p = makePage(env);
      p.loadScripts();
      p.place();
      p.holds.push(test);
      await p.mount();
      const out = p.calls.find((c) => test(c) && !c.settled);
      assert.ok(out, `${name} is out`);
      p.leave();
      await settle();
      assert.ok(out.aborted, `${name} was cut`);
      const before = p.mutations();
      out.settled = false;
      out.answer(name === 'stats' ? statsAnswer() : {}, name === 'stats' ? 401 : 200);
      await settle();
      assert.ok(p.mutations() === before, `${name}: nothing drawn after leaving`);
      assert.ok(!p.loads.includes('reload'), `${name}: no reload from a gone view`);
      assert.ok(p.byId('home-text')?.textContent === 'home as it was');
    }
  });

  await test('a 401, or a redirected answer that is not OK, on the stats read reloads the page while stats is up', async () => {
    for (const [status, extra] of [[401, {}], [404, { redirected: true }]]) {
      const p = makePage(env);
      p.loadScripts();
      p.place();
      p.holds.push(isStats);
      await p.mount();
      answerHeld(p, isStats, {}, status, extra);
      await settle();
      assert.deepStrictEqual(p.loads, ['reload'], `${status}: one reload`);
    }
  });

  await test('a reprocess out when stats goes finishes uncut and draws and logs nothing; the gone markup is let go', async () => {
    for (const outcome of ['queued', 'fails']) {
      const p = makePage(env);
      p.loadScripts();
      p.place();
      await p.mount();
      p.holds.push(isWrite);
      // Pressed without the case keeping the button: only the page holds it.
      p.press(p.q('[data-backlog-day="20261110"][data-backlog-flavor="process-now"]'));
      await settle();
      const write = p.writes()[0];
      assert.ok(write && !write.settled, 'the write is out');
      const root = new WeakRef(p.q('.dashboard'));
      p.leave();
      await settle();
      assert.ok(!write.aborted, 'a write is never cut');
      const before = p.mutations();
      if (outcome === 'queued') write.answer({ status: 'queued' });
      else write.answer({ error: 'no' }, 503);
      await settle();
      assert.ok(p.mutations() === before, `${outcome}: nothing drawn`);
      // Not into the gone markup either. Read in a function of its own, so
      // nothing it touched stays on this case's frame.
      goneViewUntouched(root, outcome);
      assert.ok(p.errors.length === 0, `${outcome}: nothing logged from a gone view`);
      if (typeof global.gc === 'function') {
        // Collection is not immediate: give it up to ten full rounds, each
        // after the microtasks have run, before calling the markup held.
        for (let round = 0; round < 10 && root.deref() !== undefined; round += 1) {
          await settle();
          global.gc();
        }
        assert.ok(root.deref() === undefined, `${outcome}: the gone markup is collected once the write has answered`);
      }
    }
  });

  await test('a mount that throws part-way takes back what it added', async () => {
    const p = makePage(env);
    p.loadScripts();
    p.place();
    const keys = p.windowKeys();
    const listeners = listenerCounts(p);
    const real = p.window.StatsTokenCard.mount;
    p.window.StatsTokenCard.mount = (ctx, root) => { real(ctx, root); throw new Error('part-way'); };
    let threw = false;
    try { await p.window.SolViews.mount('stats', {}); } catch (_) { threw = true; }
    await settle();
    assert.ok(threw, 'the mount threw');
    assert.deepStrictEqual(listenerCounts(p), listeners, 'its listeners are gone');
    assert.deepStrictEqual(p.windowKeys(), keys, 'no global is left');
    assert.ok(p.held().every((call) => call.aborted), 'its reads were cut');
    p.window.StatsTokenCard.mount = real;
    p.place();
    await p.mount();
    assert.ok(p.qa('#statsDateNav [data-date-nav-prev]').length === 1, 'trying again mounts the day control');
  });

  await test('ready reports once, across an error, "try again" and "refresh"', async () => {
    const p = makePage(env);
    p.loadScripts();
    p.place();
    p.statuses.push([isStats, 503]);
    await p.mount();
    assert.deepStrictEqual(p.ready, ['error']);
    p.statuses.length = 0;
    p.press(p.find('#stats-read-error button', 'try again'));
    await settle();
    assert.ok(p.qa('.stat-card').length === 5, 'try again drew');
    p.press(p.find('button', 'refresh'));
    await settle();
    assert.deepStrictEqual(p.ready, ['error'], 'still once');
  });

  await test('every control in the markup works by a dispatched click after an in-page mount', async () => {
    const p = makePage(env);
    p.loadScripts();
    p.leave();
    await p.arrive();
    const stats = () => p.reads('/app/stats/api/stats').length;
    // refresh
    const s0 = stats();
    p.press(p.find('button', 'refresh'));
    await settle();
    assert.ok(stats() === s0 + 1, 'refresh reads again');
    // process now
    p.press(p.q('[data-backlog-day="20261110"][data-backlog-flavor="process-now"]'));
    await settle();
    assert.ok(p.writes().length === 1 && JSON.parse(p.writes()[0].body).flavor === 'process-now', 'process now posts');
    // redo, declined then accepted
    p.confirmAnswer = false;
    p.press(p.q('[data-backlog-day="20261111"][data-backlog-flavor="from-scratch"]'));
    await settle();
    assert.ok(p.writes().length === 1, 'a declined redo writes nothing');
    p.confirmAnswer = true;
    p.press(p.q('[data-backlog-day="20261111"][data-backlog-flavor="from-scratch"]'));
    await settle();
    assert.ok(p.writes().length === 2 && JSON.parse(p.writes()[1].body).flavor === 'from-scratch', 'redo posts');
    // a sort header
    const sort = p.q('[data-token-table="models"][data-token-key="model"]');
    p.press(sort);
    assert.ok(sort.closest('th').getAttribute('aria-sort') === 'ascending', 'sorts');
    // a rollup bar
    p.press(p.q(`[data-tokens-day="${YESTERDAY}"]`));
    await settle();
    assert.ok(p.reads(`/app/stats/api/usage?day=${YESTERDAY}`).length === 1, 'the bar reads its day');
    // the day control
    p.press(p.q('#statsDateNav [data-date-nav-prev]'));
    await settle();
    assert.ok(p.reads('/app/stats/api/usage?day=20261112').length === 1, 'the day control steps back');
    // a backlog day's link is the router's (a contract view's address)
    const link = p.q('a.backlog-row-link');
    assert.ok(link.getAttribute('href') === '/app/transcripts/20261110');
    assert.ok(p.errors.length === 0 && p.loads.length === 0, 'no error, no page load');
  });

  await test('a day pick is the card\'s own entry with the router\'s id; back and forward show each day; a back off stats draws nothing', async () => {
    const p = await boot(env);
    p.press(p.q(`[data-tokens-day="${YESTERDAY}"]`));
    await settle();
    const picked = p.entry();
    assert.ok(new URL(picked.url).pathname === '/app/stats/' && new URL(picked.url).search === `?tokens=${YESTERDAY}` && new URL(picked.url).hash === '#tokens', 'the address is stats\' own');
    assert.ok(typeof picked.state?.solRoute?.id === 'string' && picked.state.solRoute.owner === 'view', 'the entry carries the router\'s id');
    assert.ok(focusOn(p) === 'H3#tokenDayHeading', `a pick focuses the day (${focusOn(p)})`);
    const id = picked.state.solRoute.id;
    p.window.history.back();
    await settle();
    assert.ok(p.byId('tokenDayHeading').textContent === `day ${TODAY}`, 'back shows today');
    assert.ok(focusOn(p) === 'H3#tokenDayHeading', 'back focuses the day');
    p.window.history.forward();
    await settle();
    assert.ok(p.byId('tokenDayHeading').textContent === `day ${YESTERDAY}`, 'forward shows the picked day');
    assert.ok(p.window.history.state.solRoute.id === id, 'the id is intact');
    // Leaving: the router's pop. The card reads nothing for it.
    const usage = p.reads('/app/stats/api/usage').length;
    p.window.history.pushState({ solRoute: { id: 'x', owner: 'router' } }, '', '/app/transcripts/20261110');
    p.window.fire({ type: 'popstate', state: p.window.history.state });
    await settle();
    assert.ok(p.reads('/app/stats/api/usage').length === usage, 'no read for another view\'s address');
  });

  await test('an arrival onto a token entry opens that day', async () => {
    const p = makePage(env);
    p.loadScripts();
    p.leave();
    await p.arrive(`/app/stats/?tokens=${YESTERDAY}#tokens`);
    assert.ok(p.reads(`/app/stats/api/usage?day=${YESTERDAY}`).length === 1, 'reads that day');
    assert.ok(p.reads(`/app/stats/api/usage?day=${TODAY}`).length === 0, 'not today');
    assert.ok(p.byId('tokenDayHeading').textContent === `day ${YESTERDAY}`);
    assert.ok(p.q(`[data-tokens-day="${YESTERDAY}"]`).getAttribute('aria-pressed') === 'true', 'its bar is pressed');
  });

  await test('"process now" keeps focus, busy, never disabled; each answer leaves focus on it', async () => {
    for (const [answer, status, after] of [
      [{ status: 'queued' }, 200, 'queued'],
      [{ status: 'already_complete', message: 'already done' }, 200, 'again'],
      [{ error: 'no' }, 503, 'again'],
    ]) {
      const p = await boot(env);
      p.holds.push(isWrite);
      const button = p.q('[data-backlog-day="20261110"][data-backlog-flavor="process-now"]');
      button.focus();
      p.press(button);
      await settle();
      assert.ok(focusOn(p) === 'BUTTON:process now', `focus stays while out (${focusOn(p)})`);
      assert.ok(button.getAttribute('aria-busy') === 'true' && button.getAttribute('aria-disabled') === 'true' && !button.disabled, 'busy, not disabled');
      const pair = p.q('[data-backlog-day="20261110"][data-backlog-flavor="from-scratch"]');
      assert.ok(pair.getAttribute('aria-busy') === 'true', 'the pair is busy too');
      p.press(button);
      await settle();
      assert.ok(p.writes().length === 1, 'a press while busy does nothing');
      answerHeld(p, isWrite, answer, status);
      await settle();
      assert.ok(focusOn(p) === 'BUTTON:process now', `${status} ${answer.status}: focus stays (${focusOn(p)})`);
      assert.ok(button.getAttribute('aria-busy') === null, 'no longer busy');
      const status_ = p.q('[data-backlog-status="20261110"]').textContent;
      if (after === 'queued') {
        assert.ok(button.getAttribute('aria-disabled') === 'true', 'a queued day can\'t be asked for twice');
        assert.ok(status_ === 'queued, working on it now');
        p.press(button);
        await settle();
        assert.ok(p.writes().length === 1, 'pressing a queued day writes nothing');
      } else {
        assert.ok(button.getAttribute('aria-disabled') === null, 'it can be pressed again');
        assert.ok(status_ === (status === 200 ? 'already done' : 'no'), `the line says what happened (${status_})`);
      }
    }
  });

  await test('"redo from scratch" keeps focus; a declined confirm leaves it as it was', async () => {
    const p = await boot(env);
    const redo = p.q('[data-backlog-day="20261110"][data-backlog-flavor="from-scratch"]');
    redo.focus();
    p.confirmAnswer = false;
    p.press(redo);
    await settle();
    assert.ok(redo.getAttribute('aria-busy') === null && p.writes().length === 0, 'declined: nothing');
    assert.ok(focusOn(p) === 'BUTTON:redo from scratch');
    p.confirmAnswer = true;
    p.holds.push(isWrite);
    p.press(redo);
    await settle();
    assert.ok(redo.getAttribute('aria-busy') === 'true' && focusOn(p) === 'BUTTON:redo from scratch', 'busy with focus');
    answerHeld(p, isWrite, { status: 'queued' });
    await settle();
    assert.ok(focusOn(p) === 'BUTTON:redo from scratch', 'focus stays after the answer');
  });

  await test('"refresh" is the same button through its read, busy while out, and keeps focus', async () => {
    const p = await boot(env);
    const refresh = p.find('button', 'refresh');
    refresh.focus();
    p.holds.push(isStats);
    p.press(refresh);
    await settle();
    assert.ok(refresh.getAttribute('aria-busy') === 'true' && !refresh.disabled, 'busy, not disabled');
    p.press(refresh);
    assert.ok(p.reads('/app/stats/api/stats').length === 2, 'a press while busy asks nothing');
    const grid = p.q('.stat-card');
    answerHeld(p, isStats, statsAnswer({ totals: { total_transcript_duration: 36000, total_percept_duration: 5400, day_bytes: 6144, transcript_sessions: 5, pending_segments: 0, segments_pending_think: 0 } }));
    await settle();
    assert.ok(p.q('.stat-card') !== grid, 'a changed answer redraws');
    assert.ok(p.find('button', 'refresh') === refresh, 'the same button');
    assert.ok(focusOn(p) === 'BUTTON:refresh' && p.doc.activeElement === refresh, `focus stays (${focusOn(p)})`);
    assert.ok(refresh.getAttribute('aria-busy') === null, 'no longer busy');
  });

  await test('a refresh whose answer draws nothing new leaves the page alone but the freshness line', async () => {
    const p = await boot(env);
    p.clock.advance(5000);
    const refresh = p.find('button', 'refresh');
    refresh.focus();
    const grid = p.q('.stat-card');
    // A second real read: the file's time, when it was made and the freshness word move; nothing drawn does.
    p.holds.push(isStats);
    p.press(refresh);
    await settle();
    const before = p.mutations();
    answerHeld(p, isStats, Object.assign(statsAnswer({ generated_at: '2026-11-14T11:00:00Z', journal_status: { verdict: '1 day needs a hand. 1 more day is still catching up.', freshness: 'stale', copy: {} } }), { file_mtime: 1763118000 }));
    await settle();
    const moved = p.mutations() - before;
    assert.ok(p.q('.stat-card') === grid, 'nothing redrawn');
    // busy cleared (2 attributes) and the freshness text written (1)
    assert.ok(moved <= 3, `only the button's busy and the freshness text change (${moved})`);
    assert.ok(focusOn(p) === 'BUTTON:refresh' && p.doc.activeElement === refresh, 'focus stays');
    // And the same answer again, read as it was: nothing at all but busy.
    p.press(refresh);
    await settle();
    const again = p.mutations();
    answerHeld(p, isStats, Object.assign(statsAnswer({ generated_at: '2026-11-14T11:00:00Z', journal_status: { verdict: '1 day needs a hand. 1 more day is still catching up.', freshness: 'stale', copy: {} } }), { file_mtime: 1763118000 }));
    await settle();
    assert.ok(p.mutations() - again <= 2, `only busy clears (${p.mutations() - again})`);
  });

  await test('the read error\'s "try again" is busy, not disabled; success lands on "refresh", failure on the new "try again"', async () => {
    const p = makePage(env);
    p.loadScripts();
    p.place();
    p.statuses.push([isStats, 503]);
    await p.mount();
    let retry = p.find('#stats-read-error button', 'try again');
    retry.focus();
    p.holds.push(isStats);
    p.press(retry);
    await settle();
    assert.ok(retry.getAttribute('aria-busy') === 'true' && !retry.disabled, 'busy, not disabled');
    assert.ok(focusOn(p) === 'BUTTON:try again');
    answerHeld(p, isStats, {}, 503);
    await settle();
    retry = p.find('#stats-read-error button', 'try again');
    assert.ok(focusOn(p) === 'BUTTON:try again' && p.doc.activeElement === retry, `a second failure lands on the new "try again" (${focusOn(p)})`);
    p.statuses.length = 0;
    p.press(retry);
    await settle();
    answerHeld(p, isStats, statsAnswer());
    await settle();
    assert.ok(p.byId('stats-read-error') === null, 'the error is gone');
    assert.ok(focusOn(p) === 'BUTTON:refresh', `success lands on "refresh" (${focusOn(p)})`);
  });

  await test('a refresh that redraws gives focus back to the control the owner was on, by what it is', async () => {
    const p = await boot(env);
    const changed = () => statsAnswer({ totals: { total_transcript_duration: 7200, total_percept_duration: 5400, day_bytes: 6144, transcript_sessions: 4, pending_segments: 1, segments_pending_think: 0 } });
    p.holds.push(isStats);
    p.press(p.find('button', 'refresh'));
    await settle();
    const redo = p.q('[data-backlog-day="20261110"][data-backlog-flavor="from-scratch"]');
    redo.focus();
    answerHeld(p, isStats, changed());
    await settle();
    const stuck = p.q('[data-backlog-day="20261110"][data-backlog-flavor="from-scratch"]');
    assert.ok(stuck !== redo, 'the backlog was drawn again');
    assert.ok(p.doc.activeElement === stuck, `focus is on the same control (${focusOn(p)})`);
    // Inside "processing details", opened by the owner: it stays open, and focus comes back.
    p.q('.backlog-list').open = true;
    const inside = p.q('[data-backlog-day="20261111"][data-backlog-flavor="from-scratch"]');
    inside.focus();
    assert.ok(p.doc.activeElement === inside, 'focus is inside the open disclosure');
    p.press(p.find('button', 'refresh'));
    inside.focus();
    await settle();
    answerHeld(p, isStats, statsAnswer());
    await settle();
    const now = p.q('[data-backlog-day="20261111"][data-backlog-flavor="from-scratch"]');
    assert.ok(now !== inside && p.q('.backlog-list').open === true, 'drawn again, still open');
    assert.ok(p.doc.activeElement === now, `focus is on the same control inside it (${focusOn(p)})`);
    // A write out keeps its busy through the redraw.
    p.holds.length = 0;
    p.holds.push(isWrite);
    p.press(now);
    await settle();
    p.holds.push(isStats);
    p.press(p.find('button', 'refresh'));
    await settle();
    now.focus();
    answerHeld(p, isStats, changed());
    await settle();
    const drawn = p.q('[data-backlog-day="20261111"][data-backlog-flavor="from-scratch"]');
    assert.ok(drawn !== now && drawn.getAttribute('aria-busy') === 'true', 'drawn busy again');
    answerHeld(p, isWrite, { status: 'queued' });
    await settle();
    assert.ok(drawn.getAttribute('aria-disabled') === 'true' && p.q('[data-backlog-status="20261111"]').textContent === 'queued, working on it now', 'the answer reaches the buttons on the page');
    // Focus outside stats is left alone.
    p.holds.length = 0;
    const rail = p.q('#app-rail a');
    rail.focus();
    p.press(p.find('button', 'refresh'));
    rail.focus();
    await settle();
    assert.ok(focusOn(p) === 'A:home', 'the rail keeps focus');
  });

  return cases;
};
