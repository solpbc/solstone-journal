// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Import on the view contract, under plain Node: the real fragment, its
// script and both of its script files, mounted through the real view
// contract, read store, router and focus helpers, in a stand-in page with
// events, focus, a history and a clock the cases move by hand. Moves between
// views go through the real router, with a stand-in for the shell's fragment
// mount that places a view's markup and mounts it. import_workspace_dom.js
// runs these after its own cases. Every compare is of plain values, ids or
// tag names: never an element.

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
  doc.title = 'import - journal';
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

const DONE = '20261101_101500';
const RUNNING = '20261102_090000';
const SAVED = '20261103_100000';

function importRow(timestamp, status, extra = {}) {
  return {
    timestamp, status, source_type: 'chatgpt', source_display: 'ChatGPT', target_day: timestamp.slice(0, 8),
    imported_at: 1762000000, entries_written: 5, entities_seeded: 1, deletable: status === 'success', ...extra,
  };
}

function listAnswer(rows = [importRow(DONE, 'success'), importRow(RUNNING, 'running')]) {
  return { imports: rows, source_options: ['chatgpt'], total: rows.length, page: 1, pages: 1, total_entries_written: 5 };
}

function sourcesAnswer() {
  return {
    items: [
      { name: 'chatgpt', display_name: 'ChatGPT', description: 'your chats', has_guide: false, input_type: 'file', accept: '.zip' },
      { name: 'quick', display_name: 'quick import', description: 'a file or text' },
    ],
  };
}

function detailAnswer(timestamp = DONE, extra = {}) {
  return {
    timestamp, status: 'success', source_type: 'chatgpt', source_display: 'ChatGPT', deletable: true,
    import_json: {}, imported_json: {}, thinking: { status: 'not_published' }, ...extra,
  };
}

function contentAnswer(page = 1) {
  return {
    page, pages: 2, total: 2, months: { 202611: 2 }, source_display: 'ChatGPT',
    items: [{ id: 'i1', type: 'note', title: 'a note', date: '20261101' }],
  };
}

// Answers by address and method. `undefined` leaves the request out.
function journal(overrides = {}) {
  return (url, opts) => {
    const u = new URL(url, ORIGIN);
    const method = String(opts?.method || 'GET').toUpperCase();
    const key = `${method} ${u.pathname}`;
    if (key in overrides) return typeof overrides[key] === 'function' ? overrides[key](u, opts) : overrides[key];
    const detail = /^\/app\/import\/api\/(\d{8}_\d{6})$/.exec(u.pathname);
    if (method === 'GET' && detail) return detailAnswer(detail[1]);
    if (method === 'GET' && /\/content$/.test(u.pathname)) return contentAnswer(Number(u.searchParams.get('page')) || 1);
    if (method === 'GET' && /\/content\/[^/]+$/.test(u.pathname)) return { content: [{ type: 'markdown', content: 'hello' }] };
    if (method === 'GET' && /\/api\/imports\/[^/]+\/delete$/.test(u.pathname)) return { segments: 1, days: ['20261101', '20261101'], kept: 0, bytes: 2048 };
    if (method === 'DELETE' && /\/api\/imports\//.test(u.pathname)) return { success: true, trash: { id: 'trash-1', erase_at: '2026-12-01T00:00:00Z' } };
    switch (key) {
      case 'GET /app/import/api/sources': return sourcesAnswer();
      case 'GET /app/import/api/list': return listAnswer();
      case 'POST /app/import/api/save': return { path: '/journal/imports/x', timestamp: SAVED, source_timestamp: SAVED };
      case 'POST /app/import/api/meta': return {};
      case 'POST /app/import/api/preview': return { preview_token: 'preview-1', size: { recordings: 1, days: 1 } };
      case 'POST /app/import/api/start': return { import_id: SAVED };
      case 'POST /app/settings/api/trash/recover': return { done: ['trash-1'] };
      case 'GET /api/journal/sources/audio': return { source: 'audio', connection: null };
      case 'GET /api/journal/sources/plaud': return { source: 'plaud', available: true };
      default:
        if (u.pathname.startsWith('/api/journal/recurrences/source/')) return { source: u.pathname.split('/').pop(), enabled: false, entries: [], blocked_names: [] };
        if (/\/sync\/[^/]+$/.test(u.pathname)) return { source: u.pathname.split('/')[4], request_id: decodeURIComponent(u.pathname.split('/').pop()), reference: 'ref-1', state: 'running', terminal: false };
        return {};
    }
  };
}

// ---- one page with import in it

const HOME_MARKUP = '<section id="home"><h1>home</h1><p id="home-text">home as it was</p><a id="to-import" href="/app/import/">import</a></section>';

function makePage(env, address = '/app/import/', { answers, folders = false } = {}) {
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
  const notices = [];
  const streams = [];
  const storage = new Map();
  const window = new Target();
  doc.window = window;
  const entries = [{ url: current.href, state: { solRoute: { id: 'e0', owner: 'router' } } }];
  let index = 0;
  // A traversal between two entries of one path that differ in the hash
  // fires popstate, then hashchange, as a browser does.
  const traverse = (to) => {
    const before = new URL(current.href);
    index = to;
    location.set(entries[index].url);
    window.fire({ type: 'popstate', state: entries[index].state });
    const after = new URL(current.href);
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
  window.sessionStorage = {
    getItem: (key) => (storage.has(key) ? storage.get(key) : null),
    setItem: (key, value) => { storage.set(key, String(value)); },
    removeItem: (key) => { storage.delete(key); },
  };
  const escapeHtml = (value) => String(value ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
  window.AppServices = {
    escapeHtml,
    renderMarkdown: (text) => `<p>${escapeHtml(text)}</p>`,
    notifications: { show: (notice) => { notices.push(notice); return notices.length; } },
  };
  window.JournalFormat = {
    day: (key) => `day ${key}`, dayFull: (key) => `day ${key}`, timestamp: (ms) => `at ${ms}`,
    segmentTime: (key) => `time ${key}`,
  };
  window.JournalClock = { today: () => '20261116', dayKey: () => '20261201', placeNote() {} };
  window.ConveyIcons = { svg: (name) => `<i>${name}</i>` };
  window.CONVEY_COPY = { RELOAD_HINT: 'reload to try again.' };
  window.SurfaceState = {
    empty: ({ heading, action }) => `<p class="empty">${heading}</p>${action || ''}`,
    error: ({ heading }) => `<p class="surface-error">${heading}</p>`,
    replaceLoading(id, html) { const node = doc.getElementById(id); if (node) node.innerHTML = html; },
    serverMessageFrom: (error) => error?.serverMessage || '',
  };
  window.ApiError = class ApiError extends Error {};
  window.Drawer = { render: (options) => `<div class="drawer"><p class="drawer-line">${options.line}</p>${options.bodyHtml}</div>` };
  window.logError = (error, context) => { errors.push({ error, context }); };
  window.showError = (message) => { errors.push({ error: new Error(String(message)), context: 'showError' }); };
  window.ConveyModalLayer = { reconciles: 0, reconcile() { this.reconciles += 1; } };
  window.appEvents = {
    buildChanged: () => false,
    onConnectionState: (fn) => { const s = { kind: 'connection', fn, stopped: false }; streams.push(s); return () => { s.stopped = true; }; },
    listen: (tract, options, fn) => {
      const s = { kind: 'listen', tract, options, fn, stopped: false };
      streams.push(s);
      const stop = () => { s.stopped = true; };
      stop.pending = { tracked: new Set(), track(id) { this.tracked.add(id); }, clear(id) { this.tracked.delete(id); } };
      s.pending = stop.pending;
      return stop;
    },
  };
  if (folders) {
    window.JournalOwnerFolders = { cancels: 0, cancel() { this.cancels += 1; }, choose: async () => ({ state: 'cancelled' }) };
    window.JournalOwnerSources = { cancels: 0, cancel() { this.cancels += 1; }, change: async () => ({ state: 'cancelled' }) };
  }
  window.__SOL_ENTRY_TABLE__ = { views: {
    import: { contract: true, fragment: '/app/import/workspace', scripts: [], styles: [] },
    home: { contract: true, fragment: '/app/home/workspace', scripts: [], styles: [] },
  } };
  window.solShellData = { apps: [
    { name: 'import', label: 'import', workspace_url: '/app/import/workspace' },
    { name: 'home', label: 'home', workspace_url: '/app/home/workspace' },
  ] };

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
          resolve({ ok: status < 400, status, redirected: !!extra.redirected, json: async () => body, text: async () => String(body) });
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

  parseInto(doc, doc.body, '<nav id="app-rail"><a id="rail-home" href="/app/home/">home</a><a id="rail-import" href="/app/import/">import</a></nav><main id="main-content" tabindex="-1"></main><div id="view-announcer"></div>');
  const main = doc.getElementById('main-content');

  const page = {
    env, window, doc, main, clock, calls, errors, loads, notices, streams, storage,
    holds: [],
    statuses: [],
    ready: [],
    hold(call) { return page.holds.some((t) => t(call)); },
    status(call) { const hit = page.statuses.find(([t]) => t(call)); return hit ? hit[1] : 200; },
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
    liveStreams: () => streams.filter((s) => !s.stopped).length,
    place(name = 'import') {
      main.replaceChildren();
      parseInto(doc, main, name === 'import' ? env.markup : HOME_MARKUP);
    },
    loadScripts() {
      env.scripts.forEach(([name, source]) => vm.runInContext(source, context, { filename: name }));
    },
    async mount() {
      page.ready = [];
      page.view = await window.SolViews.mount('import', { reportReady: (s) => page.ready.push(s) });
      await settle();
      return page.view;
    },
    // Another view takes the page, as the router's move and place() do.
    leave(next = '/app/home/') {
      window.history.pushState({ solRoute: { id: 'r' + entries.length, owner: 'router' } }, '', next);
      window.SolViews.dispose();
      main.replaceChildren();
      parseInto(doc, main, HOME_MARKUP);
    },
    // Back onto import in the same document, as a move to it does.
    async arrive(addressAgain = '/app/import/') {
      window.history.pushState({ solRoute: { id: 'a' + entries.length, owner: 'router' } }, '', addressAgain);
      page.place();
      return page.mount();
    },
    press(node) {
      assert.ok(node, 'the control to press is drawn');
      return node.click();
    },
    key(node, key) {
      assert.ok(node, 'the control to key is drawn');
      return doc.dispatch(node, { type: 'keydown', key });
    },
    find(selector, text) {
      return page.qa(selector).find((node) => node.textContent.trim() === text) || null;
    },
  };

  // The shell's fragment mount, for the router's moves: it places the view's
  // markup and mounts the view, as mount-workspace.js does for a contract
  // view whose code has run.
  window.mountWorkspaceFragment = async (_url, options = {}) => {
    if (options.signal?.aborted) return;
    const place = async () => {
      window.SolViews.dispose();
      page.place(options.appName);
      window.solRouter.placed(main);
      page.ready = [];
      page.view = await window.SolViews.mount(options.appName, { reportReady: (s) => page.ready.push(s) });
    };
    if (typeof options.swap === 'function') await options.swap(place);
    else await place();
  };

  class FormData {
    constructor() { this.entries = []; }
    append(key, value) { this.entries.push([key, value]); }
  }
  const context = vm.createContext({
    window, document: doc, location, history: window.history, sessionStorage: window.sessionStorage,
    URL, URLSearchParams, AbortController, AbortSignal, DOMException, WeakMap, WeakRef, Map, Set, Promise, FormData,
    console, Math, Number, String, Object, Array, Boolean, Date, Intl, JSON, Error, TypeError, RegExp, Symbol, Infinity, CustomEvent: class {},
    setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval, clearInterval: clock.clearInterval,
    addEventListener: window.addEventListener.bind(window),
    performance: { mark() {}, clearMarks() {} },
    fetch: window.fetch,
    crypto: { randomUUID: (() => { let n = 0; return () => `uuid-${n += 1}`; })() },
    showError: window.showError,
    confirm: () => { throw new Error('browser confirm is forbidden'); },
  });
  context.self = window;
  const shell = env.shellDir;
  for (const name of ['store.js', 'views.js', 'router.js', 'focus.js']) {
    vm.runInContext(fs.readFileSync(path.join(shell, name), 'utf8'), context);
  }
  // A view the page moves to and from; it draws its markup and nothing more.
  window.SolViews.register('home', { mount: (ctx) => { ctx.ready('content'); return () => {}; } });
  // The page's router is on, as it is once a contract view has mounted: it owns the history's ids.
  window.solRouter.start('import');
  page.context = context;
  return page;
}

function flush() {
  return new Promise((resolve) => setImmediate(resolve));
}

async function settle() {
  for (let i = 0; i < 16; i += 1) await flush();
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
  return [p.window.listenerCount(), p.doc.listenerCount(), p.main.listenerCount()];
}

function answerHeld(p, t, body, status = 200, extra) {
  p.calls.filter((call) => !call.settled && t(call)).forEach((call) => call.answer(body, status, extra));
}

const isList = (call) => call.url.startsWith('/app/import/api/list?page=1');
const isSources = (call) => call.url === '/app/import/api/sources';
const isDetail = (id = DONE) => (call) => call.method === 'GET' && call.url === `/app/import/api/${id}`;
const GONE = ['mountImport', 'navigateTo', 'showGrid', 'loadImports', 'closeDetectModal', 'loadMoreImports', 'scrollToGrid',
  'deleteImportFromPage', 'importSourceName', 'loadContent', 'toggleContentItem', 'handleHashChange', 'importDraft',
  '_quickClientItemId', '_quickSaved', '_guidedClientItemId', 'reportImportReady', 'importEvents', 'IMPORT_DELETE_COPY'];

// Opens the chatgpt guide, chooses a file, and presses its start.
async function startGuided(p) {
  p.press(p.q('[data-import-source="chatgpt"]'));
  await settle();
  const input = p.byId('guidedFileInput');
  assert.ok(input, 'the guide drew its file input');
  input.files = [{ name: 'chats.zip', size: 10 }];
  p.doc.dispatch(input, { type: 'change' });
  p.press(p.byId('guidedStartBtn'));
  await settle();
}

// Run from import_workspace_dom.js: answers with the number of cases.
module.exports = async function importContractCases(crateDir) {
  const workspace = fs.readFileSync(path.join(crateDir, 'assets/workspace.html'), 'utf8');
  const inline = [...workspace.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
  const detailJs = fs.readFileSync(path.join(crateDir, 'assets/import_detail.js'), 'utf8');
  const sourcesJs = fs.readFileSync(path.join(crateDir, 'assets/source_connections.js'), 'utf8');
  const markup = workspace.replace(/<style>[\s\S]*?<\/style>/g, '').replace(/<script[\s\S]*?<\/script>/g, '').replace(/<link[^>]*>/g, '');
  const shellDir = path.join(crateDir, '../solstone-core-convey-shell/assets/static');
  // In the fragment's order: the view's script, then its two files.
  const env = { markup, shellDir, scripts: [['workspace.html', inline[0]], ['source_connections.js', sourcesJs], ['import_detail.js', detailJs]] };
  const mountSource = inline[0].slice(inline[0].indexOf('function mountImport(ctx) {'));

  await test('the fragment is a contract view with its first reads, one script, and no handler naming a global', async () => {
    assert.ok(workspace.includes('<!-- view-contract -->'));
    assert.ok(workspace.includes('<!-- first-read: /app/import/ -> /app/import/api/sources /app/import/api/list?page=1&per_page=100&source= -->'));
    assert.ok(inline.length === 1, 'one inline script');
    const handlers = [...workspace.matchAll(/\son[a-z]+="([^"]*)"/g)].map((m) => m[1]);
    assert.deepStrictEqual(handlers, ['return false;'], "the guided form's submit guard is the only inline handler, and it calls nothing");
    for (const source of [inline[0], detailJs, sourcesJs]) {
      assert.ok(!/history\.(pushState|replaceState)/.test(source), 'the address is written only through the view contract');
      assert.ok(!/location\.hash\s*=/.test(source), 'no hash is assigned');
      assert.ok(!/window\.(onclick|loadContent|toggleContentItem|_quick|_guided)/.test(source), 'nothing is put on window');
      assert.ok(!/document\.readyState|DOMContentLoaded/.test(source), 'nothing starts itself');
    }
    assert.ok(!/document\.(addEventListener|removeEventListener)/.test(mountSource), "the mount never listens on the page's document by itself");
  });

  await test("every document member the mount reads is on its stand-in, which has no listener of its own and is never handed on", async () => {
    const standIn = /const document = Object\.freeze\(\{([\s\S]*?)\n\}\);/.exec(mountSource);
    assert.ok(standIn, 'the mount declares its document');
    const offered = new Set([...standIn[1].matchAll(/^\s*(?:get\s+)?([A-Za-z]+)\s*[:(]/gm)].map((m) => m[1]));
    const used = new Set([...mountSource.matchAll(/(?<![\w.])document\.([A-Za-z]+)/g)].map((m) => m[1]));
    for (const member of used) assert.ok(offered.has(member), `document.${member} is on the stand-in`);
    assert.ok(!offered.has('addEventListener') && !offered.has('removeEventListener'), 'no listener members');
    assert.ok(!/[(,]\s*document\s*[,)]/.test(mountSource), 'the stand-in is never passed to other code');
  });

  await test('the scripts only register: nothing reads, draws, listens or starts a timer at load', async () => {
    const p = makePage(env);
    p.place();
    const keys = p.windowKeys();
    const listeners = listenerCounts(p);
    const streams = p.liveStreams();
    p.loadScripts();
    assert.ok(p.calls.length === 0, 'no read at load');
    assert.deepStrictEqual(p.windowKeys().filter((k) => !keys.includes(k)), ['ImportDetail', 'ImportSourceConnections'], 'only the two files\' code is defined');
    assert.deepStrictEqual(listenerCounts(p), listeners, 'no listener at load');
    assert.ok(p.window.SolViews.registered('import') === true);
    assert.ok(p.clock.live() === 0, 'no timer at load');
    assert.ok(p.liveStreams() === streams, 'no stream listener at load (the router\'s own is on already)');
    assert.ok(p.byId('importGrid').children.length === 0, 'nothing drawn');
  });

  await test('the list mounts: its two first reads through the store, its rows, the stream, and ready once', async () => {
    const p = await boot(env);
    assert.ok(p.reads('/app/import/api/sources').length === 1 && p.reads('/app/import/api/sources')[0].kind === 'api', 'sources once, through the store');
    assert.ok(p.calls.filter(isList).length === 1 && p.calls.filter(isList)[0].url === '/app/import/api/list?page=1&per_page=100&source=', 'the first list read is the declared one');
    assert.ok(p.qa('tr[data-import-id]').length === 2, 'two rows');
    assert.ok(p.qa('.import-source-card').length === 2, 'the source cards');
    assert.ok(p.streams.filter((s) => s.kind === 'listen' && !s.stopped).length === 1, 'the importer stream');
    assert.ok(p.streams.find((s) => s.kind === 'listen').pending.tracked.has(RUNNING), 'the running import is tracked');
    assert.deepStrictEqual(p.ready, ['content']);
    assert.ok(p.errors.length === 0, `no error: ${p.errors.map((e) => e.error.message).join('; ')}`);
  });

  await test("an import's page mounts at its address: its read, its heading, its stream, and ready once", async () => {
    const p = await boot(env, `/app/import/${DONE}`);
    assert.ok(p.calls.filter(isDetail()).length === 1 && p.calls.filter(isDetail())[0].kind === 'api', 'the import read once, through the store');
    assert.ok(p.byId('importListView').style.display === 'none', 'the list is hidden');
    assert.ok(p.byId('importDetailView').style.display === '', 'its page shows');
    assert.ok(p.byId('importDetailHeading').textContent.includes('day 20261101'), 'its heading');
    assert.ok(p.qa('[data-import-delete]').length === 1, 'its delete button is drawn');
    assert.ok(p.streams.filter((s) => s.kind === 'listen' && !s.stopped).length === 1, 'its stream');
    assert.ok(p.calls.filter(isList).length === 0, 'the list is not read on an import\'s page');
    assert.deepStrictEqual(p.ready, ['content']);
  });

  await test("ready says error once when an import's read fails, and when the list's does", async () => {
    for (const [address, t] of [[`/app/import/${DONE}`, isDetail()], ['/app/import/', isList]]) {
      const p = makePage(env, address);
      p.statuses.push([t, 500]);
      p.loadScripts();
      p.place();
      await p.mount();
      assert.deepStrictEqual(p.ready, ['error'], address);
    }
  });

  for (const address of ['/app/import/', `/app/import/${DONE}`]) {
    await test(`mounted twice at ${address}, and dispose leaves no listener, timer, stream, read or global`, async () => {
      const p = makePage(env, address, { folders: true });
      p.loadScripts();
      const keys = p.windowKeys();
      const listeners = listenerCounts(p);
      for (let round = 0; round < 2; round += 1) {
        p.place();
        p.holds.push((call) => call.url.includes('/recurrences/'));
        await p.mount();
        assert.ok(p.window.listenerCount() > listeners[0], 'the view listens on the window while it is up');
        assert.ok(p.main.listenerCount() === 2, 'one press and one key delegation on #main-content');
        assert.ok(p.qa('[data-sync-source]').length === 2, 'the source rows draw');
        assert.ok(p.liveStreams() >= 2, 'a stream of its own is on');
        p.leave();
        await settle();
        assert.deepStrictEqual(listenerCounts(p), listeners, `round ${round}: every window, document and main listener is gone`);
        assert.deepStrictEqual(p.windowKeys(), keys, `round ${round}: no global is left`);
        for (const name of GONE) assert.ok(!(name in p.window) && !(name in p.context), `${name} is not a global`);
        assert.ok(p.held().every((call) => call.aborted), `round ${round}: every read still out was cut`);
        assert.ok(p.clock.live() === 0, `round ${round}: no timer`);
        assert.ok(p.liveStreams() === 1, `round ${round}: every stream stopped but the router's`);
        assert.ok(p.window.JournalOwnerFolders.cancels >= round + 1, 'a folder chooser the app has open is cancelled');
        assert.ok(p.window.JournalOwnerSources.cancels >= round + 1, 'a source chooser too');
        p.holds.length = 0;
        p.window.history.back();
        await settle();
      }
    });
  }

  await test('a read out when import goes is cut, and its late answer draws nothing', async () => {
    for (const [name, t, address] of [['the list', isList, '/app/import/'], ['the sources', isSources, '/app/import/'], ['an import', isDetail(), `/app/import/${DONE}`]]) {
      const p = makePage(env, address);
      p.loadScripts();
      p.place();
      p.holds.push(t);
      await p.mount();
      const out = p.calls.find((c) => t(c) && !c.settled);
      assert.ok(out, `${name} is out`);
      p.leave();
      await settle();
      assert.ok(out.aborted, `${name} was cut`);
      const before = p.mutations();
      out.settled = false;
      out.answer(name === 'the list' ? listAnswer() : name === 'the sources' ? sourcesAnswer() : detailAnswer());
      await settle();
      assert.ok(p.mutations() === before, `${name}: nothing drawn after leaving`);
      assert.ok(p.byId('home-text')?.textContent === 'home as it was');
      assert.ok(p.ready.length === 0, `${name}: no ready from a gone view`);
    }
  });

  await test('an upload out when import goes is aborted, as a page load aborts it, and no later write starts', async () => {
    const p = await boot(env);
    p.holds.push((call) => call.method === 'POST' && call.url === '/app/import/api/save');
    await startGuided(p);
    const save = p.calls.find((c) => c.url === '/app/import/api/save');
    assert.ok(save && !save.settled, 'the upload is out');
    p.leave();
    await settle();
    assert.ok(save.aborted, 'the upload was aborted');
    assert.deepStrictEqual(p.writes().map((c) => c.url), ['/app/import/api/save'], 'no meta, preview or start went out');
    assert.ok(p.byId('home-text')?.textContent === 'home as it was');
  });

  await test("a write out when import goes finishes uncut, its answer runs nothing, and the chain's next write never starts", async () => {
    const p = await boot(env);
    p.holds.push((call) => call.method === 'POST' && call.url === '/app/import/api/meta');
    await startGuided(p);
    const meta = p.calls.find((c) => c.url === '/app/import/api/meta');
    assert.ok(meta && !meta.settled, 'the meta write is out');
    p.leave();
    await settle();
    assert.ok(!meta.aborted, 'the write is not cut');
    const before = p.mutations();
    meta.answer({});
    await settle();
    assert.ok(p.mutations() === before, 'its answer drew nothing');
    assert.deepStrictEqual(p.writes().map((c) => c.url), ['/app/import/api/save', '/app/import/api/meta'], 'no preview or start after leaving');
    assert.ok(p.notices.length === 0 && p.errors.length === 0, 'no notice or error');
  });

  await test("a delete out when an import's page goes finishes, and its answer neither stashes a receipt nor moves the page", async () => {
    const p = await boot(env, `/app/import/${DONE}`);
    p.press(p.q('[data-import-delete]'));
    await settle();
    assert.ok(p.byId('importDeleteModal').style.display === 'flex', 'the delete dialog opens');
    assert.ok(p.doc.body.style.overflow === 'hidden', 'the page behind it stops scrolling');
    p.holds.push((call) => call.method === 'DELETE');
    p.press(p.byId('importDeleteModalConfirm'));
    await settle();
    const del = p.calls.find((c) => c.method === 'DELETE');
    assert.ok(del && !del.settled, 'the delete is out');
    p.leave();
    await settle();
    assert.ok(!del.aborted, 'the delete is not cut');
    del.answer({ success: true, trash: { id: 'trash-1', erase_at: '2026-12-01T00:00:00Z' } });
    await settle();
    assert.ok(p.storage.size === 0, 'no receipt was stashed');
    assert.ok(p.window.location.pathname === '/app/home/', 'the page did not move');
    assert.ok(p.loads.length === 0, 'nothing loaded');
    assert.ok(p.notices.length === 0, 'no notice');
  });

  await test('the gone markup is let go once the last late answer is in', async () => {
    if (typeof global.gc !== 'function') return;
    const p = await boot(env);
    p.holds.push((call) => call.method === 'POST' && call.url === '/app/import/api/meta');
    await startGuided(p);
    let gone = new WeakRef(p.byId('importListView'));
    p.leave();
    await settle();
    p.calls.filter((c) => !c.settled).forEach((c) => c.answer({}));
    await settle();
    // The stand-ins' own records of the stream and the calls go too, and
    // focus is read so the page drops a node it no longer holds.
    p.view = null;
    p.calls.length = 0;
    p.streams.length = 0;
    void p.doc.activeElement;
    for (let i = 0; i < 6 && gone.deref(); i += 1) {
      await settle();
      global.gc();
    }
    assert.ok(gone.deref() === undefined, 'the gone markup was collected');
  });

  await test("a receipt that outlives import keeps no mount, and its recover works from another view", async () => {
    const p = await boot(env);
    p.press(p.byId('importSelectBtn'));
    await settle();
    const box = p.q(`tr[data-import-id="${DONE}"] input[data-import-pick]`);
    box.checked = true;
    p.press(box);
    await settle();
    p.press(p.byId('importPickDelete'));
    await settle();
    p.press(p.byId('importDeleteModalConfirm'));
    await settle();
    const receipt = p.notices[p.notices.length - 1];
    assert.ok(receipt && receipt.buttons.length === 2, 'the moved receipt, with recover and open trash');
    assert.ok(p.calls.filter(isList).length === 2, 'the delete reloaded the list');
    assert.deepStrictEqual(p.ready, ['content'], 'ready is still reported once');
    const lists = p.calls.filter(isList).length;
    p.leave();
    await settle();
    receipt.buttons[0].onClick();
    await settle();
    const recover = p.calls.find((c) => c.url === '/app/settings/api/trash/recover');
    assert.ok(recover && JSON.parse(recover.body).ids[0] === 'trash-1', 'the recover went out from another view');
    assert.ok(p.notices[p.notices.length - 1].title.includes('recovered'), 'and its receipt shows');
    assert.ok(p.calls.filter(isList).length === lists, 'a gone list is not reloaded');
  });

  await test('each poll stops when import goes: the thinking progress, the source sync check and the 1 s list reload', async () => {
    // The thinking progress, on a progress view.
    const p = await boot(env, `/app/import/#progress/${RUNNING}`, { folders: true });
    assert.ok(p.byId('guideSteps').querySelector('.import-progress-panel'), 'the progress panel draws');
    assert.ok(p.clock.live() >= 1, 'the progress poll is set');
    // A source sync left running: its check is set from sessionStorage.
    p.storage.set('journal-source-operation:audio', JSON.stringify({ sync: { request_id: 'req-1', reference: 'ref-1' } }));
    p.window.fire({ type: 'pageshow' });
    await settle();
    // A completed row reloads the list a second later.
    p.streams.find((s) => s.kind === 'listen' && !s.stopped).fn({ event: 'completed', import_id: RUNNING, generation: 2 });
    await settle();
    const timers = p.clock.live();
    assert.ok(timers >= 2, `the polls are set (${timers})`);
    p.leave();
    await settle();
    assert.ok(p.clock.live() === 0, 'every poll stopped');
    const before = p.calls.length;
    p.clock.advance(60000);
    await settle();
    assert.ok(p.calls.length === before, 'nothing is read after leaving');
  });

  await test("a source sync left running is checked every 5 s on the list, and the check stops when import goes", async () => {
    const p = makePage(env, '/app/import/', { folders: true });
    p.storage.set('journal-source-operation:audio', JSON.stringify({ sync: { request_id: 'req-1', reference: 'ref-1' } }));
    p.loadScripts();
    p.place();
    await p.mount();
    await settle();
    const checks = () => p.reads('/api/journal/sources/audio/sync/req-1').length;
    assert.ok(checks() === 1, 'the sync is checked on mount');
    p.clock.advance(5000);
    await settle();
    assert.ok(checks() === 2, 'and again 5 s later');
    p.leave();
    await settle();
    assert.ok(p.clock.live() === 0, 'its check stopped');
    p.clock.advance(60000);
    await settle();
    assert.ok(checks() === 2, 'nothing is checked after leaving');
  });

  await test("a press on import's gone markup (a tap the page still had queued) starts no write", async () => {
    const p = await boot(env);
    p.press(p.q('[data-import-source="chatgpt"]'));
    await settle();
    const input = p.byId('guidedFileInput');
    input.files = [{ name: 'chats.zip', size: 10 }];
    p.doc.dispatch(input, { type: 'change' });
    const start = p.byId('guidedStartBtn');
    p.leave();
    await settle();
    start.click();
    await settle();
    assert.ok(p.writes().length === 0, 'no write starts from a gone import');
    assert.ok(p.window.location.pathname === '/app/home/' && p.loads.length === 0, 'the page stays where the owner went');
  });

  await test('a mount that throws part-way takes back what it had added', async () => {
    const p = makePage(env);
    p.loadScripts();
    const keys = p.windowKeys();
    const listeners = listenerCounts(p);
    p.window.appEvents.listen = () => { throw new Error('the stream is broken'); };
    p.place();
    await assert.rejects(p.window.SolViews.mount('import', { reportReady() {} }), /the stream is broken/);
    await settle();
    assert.deepStrictEqual(listenerCounts(p), listeners, 'no listener is left');
    assert.deepStrictEqual(p.windowKeys(), keys, 'no global is left');
    assert.ok(p.held().every((c) => c.aborted), 'its reads were cut');
    assert.ok(p.clock.live() === 0, 'no timer');
  });

  await test("each markup handler acts by its own event after an in-page mount, and the router's press stays out of it", async () => {
    const p = await boot(env, '/app/import/#guide/chatgpt');
    assert.ok(p.byId('importGuide').style.display === '', 'the guide shows from the address');
    const back = p.q('.import-back-link');
    const sourceReads = p.reads('/app/import/api/sources').length;
    const event = p.press(back);
    await settle();
    assert.ok(p.reads('/app/import/api/sources').length === sourceReads, 'the back link mounts nothing again');
    assert.ok(event.defaultPrevented, 'the back link is the view\'s');
    assert.ok(p.window.location.hash === '', 'it shows the grid');
    assert.ok(p.byId('importGrid').style.display === '', 'the grid is back');
    assert.ok(p.loads.length === 0, 'no page load');
    // The detect dialog's close and backdrop.
    const detect = p.byId('detectModal');
    for (const how of ['close', 'backdrop']) {
      detect.style.display = 'block';
      p.press(how === 'close' ? p.q('[data-import-detect-close]') : detect);
      assert.ok(detect.style.display === 'none', `the detect dialog closes by its ${how}`);
    }
    // The empty list's "choose a source" link.
    const empty = await boot(env, '/app/import/', { answers: journal({ 'GET /app/import/api/list': listAnswer([]) }) });
    const choose = empty.q('[data-import-scroll-grid]');
    const pressed = empty.press(choose);
    assert.ok(pressed.defaultPrevented && empty.doc.scrolledTo === 'importGrid', 'choose a source scrolls to the cards');
    // Load more.
    const more = await boot(env, '/app/import/', { answers: journal({ 'GET /app/import/api/list': (u) => ({ ...listAnswer(), page: Number(u.searchParams.get('page')), pages: 2 }) }) });
    more.press(more.q('[data-import-load-more]'));
    await settle();
    assert.ok(more.reads('/app/import/api/list?page=2').length === 1, 'load more reads the next page');
    // An import's page: the content tab, an item and a page.
    const d = await boot(env, `/app/import/${DONE}#content`);
    assert.ok(d.reads(`/app/import/api/${DONE}/content?`).length === 1, '#content opens the content tab');
    const item = d.q('[data-content-toggle]');
    d.press(item.querySelector('.import-content-item-header'));
    await settle();
    assert.ok(item.classList.contains('expanded') && d.reads(`/app/import/api/${DONE}/content/i1`).length === 1, 'an item opens by a press');
    d.key(item, 'Enter');
    assert.ok(!item.classList.contains('expanded'), 'and closes by Enter');
    d.press(d.q('[data-content-page="2"]'));
    await settle();
    assert.ok(d.reads(`/app/import/api/${DONE}/content?page=2`).length === 1, 'a page button reads that page');
    // A press inside #main-content but outside import's markup is left alone.
    const stray = p.doc.createElement('button');
    stray.setAttribute('data-import-detect-close', '');
    p.main.append(stray);
    detect.style.display = 'block';
    p.press(stray);
    assert.ok(detect.style.display === 'block', "a press outside import's markup is not import's");
    detect.style.display = 'none';
  });

  await test('a history row moves in the page, by a press and by Enter, and the back link comes back in the page', async () => {
    const p = await boot(env);
    p.press(p.q(`tr[data-import-id="${DONE}"] .import-file-cell`));
    await settle();
    assert.ok(p.window.location.pathname === `/app/import/${DONE}`, 'the row opened its import');
    assert.ok(p.loads.length === 0, 'in the page');
    assert.ok(p.byId('importDetailView').style.display === '', 'its page shows');
    assert.ok(focusOn(p) !== 'BODY', `focus is placed (${focusOn(p)})`);
    p.press(p.q('.back-link'));
    await settle();
    assert.ok(p.window.location.pathname === '/app/import/' && p.loads.length === 0, 'back to the list in the page');
    p.key(p.q(`tr[data-import-id="${DONE}"]`), 'Enter');
    await settle();
    assert.ok(p.window.location.pathname === `/app/import/${DONE}` && p.loads.length === 0, 'Enter on a row opens it in the page');
  });

  await test('each hash route and an import\'s page as an arrival shows what a full load of that address shows, with its entry id', async () => {
    for (const [address, check] of [
      ['/app/import/#guide/chatgpt', (p) => p.byId('guidedStartBtn') && p.byId('importGrid').style.display === 'none'],
      ['/app/import/#guide/quick', (p) => p.byId('validateBtn')],
      [`/app/import/#progress/${RUNNING}`, (p) => p.q('.import-progress-panel')],
      [`/app/import/${DONE}`, (p) => p.byId('importDetailView').style.display === '' && p.q('.tab.active').getAttribute('data-target') === 'overview'],
      [`/app/import/${DONE}#content`, (p) => p.q('.tab.active').getAttribute('data-target') === 'content'],
    ]) {
      const p = makePage(env, '/app/home/');
      p.loadScripts();
      p.place('home');
      p.window.SolViews.mount('home', {});
      const link = p.doc.createElement('a');
      link.setAttribute('href', address);
      p.main.append(link);
      p.press(link);
      await settle();
      assert.ok(p.loads.length === 0, `${address}: in the page`);
      assert.ok(p.window.location.href === new URL(address, ORIGIN).href, `${address}: at that address`);
      assert.ok(check(p), `${address}: shows what a full load shows`);
      assert.ok(p.window.history.state?.solRoute?.id, `${address}: the entry keeps its id`);
      assert.ok(focusOn(p) !== 'BODY', `${address}: focus is placed (${focusOn(p)})`);
    }
  });

  await test("a back and a forward over import's own entries show each once, with their ids, and focus never on the page", async () => {
    const p = await boot(env);
    p.press(p.q('[data-import-source="chatgpt"]'));
    await settle();
    const guideEntry = p.entry();
    assert.ok(p.window.location.hash === '#guide/chatgpt' && guideEntry.state?.solRoute?.owner === 'view', 'the guide is the view\'s own entry');
    assert.ok(p.entries.length === 2, 'one entry written');
    p.byId('guidedStartBtn').focus();
    const hashchanges = [];
    p.window.addEventListener('hashchange', () => hashchanges.push(p.window.location.hash));
    p.window.history.back();
    await settle();
    assert.ok(p.window.location.hash === '' && p.byId('importGrid').style.display === '', 'back shows the grid');
    assert.deepStrictEqual(hashchanges, [''], 'one hashchange for one back');
    assert.ok(p.entry().state.solRoute.id === 'e0', 'the first entry keeps its id');
    assert.ok(focusOn(p).startsWith('BUTTON'), `focus is on the grid's first card (${focusOn(p)})`);
    p.window.history.forward();
    await settle();
    assert.ok(p.window.location.hash === '#guide/chatgpt' && p.byId('guidedStartBtn'), 'forward shows the guide');
    assert.ok(p.entry().state.solRoute.id === guideEntry.state.solRoute.id, 'the guide entry keeps its id');
    assert.ok(p.entries.length === 2, 'a traversal writes no entry');
    assert.ok(focusOn(p) === 'H2:ChatGPT', `focus is on the guide's heading (${focusOn(p)})`);
  });

  await test('the delete dialog is closed by dispose, with the page scrolling again, and the detect dialog by a back', async () => {
    const p = await boot(env, `/app/import/${DONE}`);
    p.press(p.q('[data-import-delete]'));
    await settle();
    const modal = p.byId('importDeleteModal');
    assert.ok(modal.style.display === 'flex');
    const reconciles = p.window.ConveyModalLayer.reconciles;
    p.leave();
    await settle();
    assert.ok(modal.style.display === 'none', 'dispose closed the delete dialog');
    assert.ok(p.doc.body.style.overflow === '', 'the page scrolls again');
    assert.ok(p.window.ConveyModalLayer.reconciles > reconciles, 'the modal layer is told');
    // The detect dialog, open over the quick import's entry, and a back.
    const q = await boot(env);
    q.press(q.q('[data-import-quick]'));
    await settle();
    q.byId('pasteText').value = 'a note';
    q.doc.dispatch(q.byId('importForm'), { type: 'submit' });
    await settle();
    const detect = q.byId('detectModal');
    assert.ok(detect.style.display === 'block', 'the detect dialog opens over the quick import');
    q.window.history.back();
    await settle();
    assert.ok(detect.style.display === 'none', 'a back closes it');
    assert.ok(q.window.location.hash === '', 'and shows the grid');
    assert.ok(focusOn(q) !== 'BODY', `focus is placed (${focusOn(q)})`);
    // And dispose closes it.
    const r = await boot(env);
    r.press(r.q('[data-import-quick]'));
    await settle();
    r.byId('pasteText').value = 'a note';
    r.doc.dispatch(r.byId('importForm'), { type: 'submit' });
    await settle();
    const detect2 = r.byId('detectModal');
    assert.ok(detect2.style.display === 'block');
    r.leave();
    await settle();
    assert.ok(detect2.style.display === 'none', 'dispose closed the detect dialog');
  });

  await test('the next mount picks up what a full load would: a running import, a stashed receipt, and a source sync', async () => {
    const p = makePage(env, '/app/import/', { folders: true });
    p.storage.set('journal-import-delete-receipt', JSON.stringify({ at: Date.now(), erase: false, outcome: { kind: 'moved', trashId: 'trash-1', eraseAt: '2026-12-01T00:00:00Z' } }));
    p.storage.set('journal-source-operation:plaud', JSON.stringify({ sync: { request_id: 'req-9', reference: 'ref-1' } }));
    p.loadScripts();
    p.place();
    await p.mount();
    await settle();
    assert.ok(p.notices.some((n) => n.title === 'moved to the trash'), 'the stashed receipt shows');
    assert.ok(!p.storage.has('journal-import-delete-receipt'), 'and is taken');
    assert.ok(p.streams.find((s) => s.kind === 'listen' && !s.stopped).pending.tracked.has(RUNNING), 'the running import is tracked again');
    assert.ok(p.reads('/api/journal/sources/plaud/sync/req-9').length === 1, 'the source sync is checked again');
  });
  return cases;
};
