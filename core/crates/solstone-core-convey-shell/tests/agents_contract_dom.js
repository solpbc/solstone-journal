// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Agents on the view contract, under plain Node: the real fragment's mount
// through the real view contract, read store and router, in a stand-in page
// with events, focus, a history and a clock the cases move by hand.
// agents_workspace_dom.js runs these after its own cases. Every compare is of
// plain values, ids or tag names: never an element.

const assert = require('assert');
const { answerQuestion } = require('./question_controls.js');
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
  doc.title = 'agents - journal';
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

function apiError(status, body) {
  const error = new Error(body?.detail || body?.error || `HTTP ${status}`);
  error.status = status;
  error.payload = body;
  error.reasonCode = body?.reason_code;
  return error;
}


// ---- the journal the page reads

const SOURCE = 'a'.repeat(64);
const MEMORY = { day: '20261101', stream: `agent-memory-${SOURCE}`, segment: '120000_1', content: 'the owner prefers short answers', created_at: '2026-11-01T12:00:00Z' };
const MEMORY_HASH = `#memories/${SOURCE}/20261101/120000_1`;
function connection(kind, id, name, door, extra = {}) {
  return { kind, id, key: `${kind}:${id}`, name, door, created_at: '2026-11-01T12:00:00Z', permission: null, requests_this_week: 2, last_request_at: null, activity_complete: true, ...extra };
}
const CLAUDE = connection('oauth', 'g1', 'Claude', 'local', { memory_source: SOURCE });
const PAUSED = connection('oauth', 'g2', 'Codex', 'local', { paused: { by: 'owner', since: '2026-11-02T12:00:00Z' }, paused_attempts: { count: 2, last_at: '2026-11-03T12:00:00Z' } });
const KEY = connection('bearer', 'k1', 'Script', null);
const ENTRY = {
  timestamp: '2026-11-03T09:42:00Z', name: 'Claude', memory_source: SOURCE, tool: 'search', outcome: 'served',
  request: { arguments: { query: 'standup' } },
  result: { targets: ['entity:alex', '20261103/default/094200_1/audio.jsonl', 'week:20261102', 'facet:work', `20261101/agent-memory-${SOURCE}/120000_1/note.txt`] },
};

function agentsState(overrides = {}) {
  return {
    enabled: false, status: 'off', local_agent_helpers: true, owner_state: null, certificate: { current: false },
    local_door: { enabled: true, listening: true, address: 'http://127.0.0.1:7659/mcp' },
    connections: [CLAUDE, PAUSED, KEY], facets: [], pairing: null, ...overrides,
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
      case 'GET /app/agents/api/state': return agentsState();
      case 'GET /app/agents/api/memories': return { agents: [{ source: SOURCE, count: 1, label: 'Claude', latest_at: '2026-11-01T12:00:00Z' }], complete: true };
      case `GET /app/agents/api/memories/${SOURCE}`: return { memories: [MEMORY], next: null, complete: true };
      case 'GET /app/agents/api/activity': return { entries: [ENTRY], counts: { served: 1 }, examination_complete: true };
      case 'GET /app/agents/api/pairing': return { now: '2026-11-14T12:00:00Z', today: '20261114', pairing: { generation: 1, locked: false, door: 'local' }, connections: [] };
      case 'GET /app/agents/api/enable': return { operation: null };
      case 'POST /app/agents/api/pairing': return { code: 'LABONLY1', expires_at: '2026-11-14T12:10:00Z', generation: 1, door: 'local' };
      case 'POST /app/agents/api/enable': return { operation: { phase: 'waiting', portal_url: 'https://services.example/consent' } };
      case 'POST /app/settings/api/trash/recover': return { done: ['trash-1'], failed: [] };
      default: break;
    }
    if (method === 'DELETE' && u.pathname.startsWith('/app/transcripts/api/segment/')) return { trash: { id: 'trash-1', erase_at: '2026-11-21T12:00:00Z' } };
    return {};
  };
}

// ---- one page with agents in it

function makePage(env, address = '/app/agents/', { answers } = {}) {
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
  const opened = [];
  const visible = new Set();
  let reconciles = 0;
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
  window.JournalFormat = { day: (key) => `day ${key}` };
  window.AppServices = {
    escapeHtml: (s) => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;'),
    visibleInterval(fn, ms) {
      const id = clock.setInterval(fn, ms);
      visible.add(id);
      return () => { clock.clearInterval(id); visible.delete(id); };
    },
    notifications: { show(notice) { notices.push(notice); } },
  };
  window.logError = (error, context) => { errors.push({ error, context }); };
  window.solstoneOutside = { openDelayed: (url) => opened.push(url) };
  window.ConveyModalLayer = { reconcile() { reconciles += 1; } };
  window.appEvents = { buildChanged: () => false, onConnectionState: () => () => {}, listen: () => () => {} };
  window.__SOL_ENTRY_TABLE__ = { views: { agents: { contract: true, fragment: '/app/agents/workspace', scripts: [], styles: [] } } };
  window.solShellData = { apps: [{ name: 'agents', label: 'agents', workspace_url: '/app/agents/workspace' }] };

  const serve = answers || journal();
  const ask = (url, opts, kind) => new Promise((resolve, reject) => {
    const u = new URL(url, current.href);
    const call = {
      url: u.pathname + u.search,
      method: String(opts?.method || 'GET').toUpperCase(),
      kind, aborted: false, settled: false,
      answer(body, status = 200) {
        if (call.settled) return;
        call.settled = true;
        if (status >= 400) {
          const error = new Error(`HTTP ${status}`);
          error.status = status;
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

  parseInto(doc, doc.body, '<nav id="app-rail"><a href="/app/home/">home</a><a href="/app/agents/">agents</a></nav><main id="main-content" tabindex="-1"></main><div id="view-announcer"></div>');
  const main = doc.getElementById('main-content');

  const page = {
    env, window, doc, main, clock, calls, errors, notices, opened, loads,
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
    visibleIntervals: () => visible.size,
    reconciles: () => reconciles,
    entries,
    entry: () => entries[index],
    place() {
      main.replaceChildren();
      parseInto(doc, main, env.markup);
    },
    loadScript() {
      vm.runInContext(env.script, context, { filename: 'agents-workspace.js' });
    },
    async mount() {
      page.ready = [];
      page.view = await window.SolViews.mount('agents', { reportReady: (s) => page.ready.push(s) });
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
    // Back onto agents in the same document, as a move to it does.
    async arrive(addressAgain = '/app/agents/') {
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
    URL, URLSearchParams, AbortController, AbortSignal, DOMException, WeakMap, Map, Set, Promise, TextEncoder,
    console, Math, Number, String, Object, Array, Boolean, Date, Intl, JSON, Error, TypeError, RegExp, Uint8Array, Symbol,
    setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval, clearInterval: clock.clearInterval,
    performance: { mark() {}, clearMarks() {} },
    fetch: window.fetch,
    confirm: () => { throw new Error("browser confirm is forbidden"); },
    prompt: () => { throw new Error("browser prompt is forbidden"); },
    navigator: { clipboard: { writeText: async () => {} } },
  });
  context.self = window;
  const shell = path.join(env.manifestDir, 'assets/static');
  vm.runInContext(fs.readFileSync(path.join(shell, 'store.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'views.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'router.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'question.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'focus.js'), 'utf8'), context);
  // The page's router is on, as it is once a contract view has mounted: it owns the history's ids.
  window.solRouter.start('agents');
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
  p.loadScript();
  p.place();
  await p.mount();
  await settle();
  return p;
}

function focusOn(p) {
  const active = p.doc.activeElement;
  return active ? (active.id ? `${active.tagName}#${active.id}` : active.tagName) : 'none';
}

const GONE = ['reload', 'render', 'state', 'view', 'api', 'openMemoryFromAddress', 'recoverMemory', 'deleteMemory', 'pollWatch', 'pollEnable',
  'agentsHooks', 'recoverClick', 'trashReceipt', 'TRASH_COPY', 'reportAgentsReady', 'closeDialog', 'mountAgents'];

// Run from agents_workspace_dom.js: answers with the number of cases.
module.exports = async function agentsContractCases(manifestDir) {
  const workspace = fs.readFileSync(path.join(manifestDir, 'assets/agents/workspace.html'), 'utf8');
  const scriptStart = workspace.indexOf("<script>\n(() => {\n  'use strict';");
  assert.ok(scriptStart > 0, 'the fragment has its one inline script');
  const scriptEnd = workspace.indexOf('</script>', scriptStart);
  const script = workspace.slice(scriptStart + '<script>'.length, scriptEnd);
  const markup = workspace.slice(0, scriptStart).replace(/<style>[\s\S]*?<\/style>/g, '');
  const env = { manifestDir, script, markup };

  await test('the fragment is a contract view with its two first reads, and no handler in it names a global', async () => {
    assert.ok(workspace.includes('<!-- view-contract -->'));
    assert.ok(workspace.includes('<!-- first-read: /app/agents/ -> /app/agents/api/state /app/agents/api/memories -->'));
    assert.ok(!/\son[a-z]+="/.test(workspace), 'no inline handler attribute in the markup or what it draws');
    assert.ok((workspace.match(/<script>/g) || []).length === 1, 'one inline script');
    assert.ok(!/history\.replaceState|history\.pushState/.test(script), 'the address is written only through the view contract');
  });

  await test('the script only registers: nothing reads, draws, listens or starts a timer at load', async () => {
    const p = makePage(env);
    p.place();
    const keys = p.windowKeys();
    const listeners = [p.window.listenerCount(), p.doc.listenerCount()];
    const timers = p.clock.live();
    p.loadScript();
    assert.ok(p.calls.length === 0, 'no read at load');
    assert.deepStrictEqual(p.windowKeys(), keys, 'nothing is added to the window');
    assert.deepStrictEqual([p.window.listenerCount(), p.doc.listenerCount()], listeners, 'no listener at load');
    assert.ok(p.window.SolViews.registered('agents') === true);
    assert.ok(p.clock.live() === timers, 'no timer at load');
    assert.ok(p.q('h1') === null, 'nothing drawn');
  });

  await test('a mount asks for the state and the memory summary together, once each, draws and reports ready once', async () => {
    const p = makePage(env);
    p.loadScript();
    p.place();
    p.holds.push((call) => call.method === 'GET');
    const mounting = p.mount();
    await settle();
    assert.ok(p.reads('/app/agents/api/state').length === 1 && p.reads('/app/agents/api/memories').length === 1, 'both first reads are out before either answers');
    p.holds.length = 0;
    p.calls.slice().forEach((call) => { if (!call.settled) call.answer(journal()(call.url, { method: call.method })); });
    await mounting;
    await settle();
    assert.ok(p.q('h1')?.textContent === 'agents', 'the page is drawn');
    assert.ok(p.qa('.ag-row').length === 3, 'each agent has its row');
    assert.ok(p.reads('/app/agents/api/state').length === 1 && p.reads('/app/agents/api/memories').length === 1, 'each is asked once');
    assert.deepStrictEqual(p.ready, ['content']);
    assert.ok(p.visibleIntervals() === 1, 'the 20 s poll pauses while the page is hidden');
    assert.ok(p.errors.length === 0, 'no error logged');
  });

  await test('the 20 s poll and "check again" ask of their own each time', async () => {
    const answers = journal({ 'GET /app/agents/api/state': agentsState({ local_door: { enabled: true, listening: false, reason: 'not_running', address: 'http://127.0.0.1:7659/mcp' } }) });
    const p = await boot(env, '/app/agents/', { answers });
    const before = p.reads('/app/agents/api/state').length;
    p.clock.advance(20000);
    await settle();
    p.clock.advance(20000);
    await settle();
    assert.ok(p.reads('/app/agents/api/state').length === before + 2, `the poll re-read the state twice (${p.reads('/app/agents/api/state').length - before})`);
    // One press at a time: a press while its read is out is ignored (busy).
    p.press(p.find('button', 'check again'));
    await settle();
    p.press(p.find('button', 'check again'));
    await settle();
    assert.ok(p.reads('/app/agents/api/state').length === before + 4, 'each "check again" asks of its own');
  });

  await test('dispose leaves no listener, timer, poll, read or global, even with a dialog and both polls running, and agents mounts again', async () => {
    // A solstone.me sign-up waiting for the owner's approval: the enable poll runs.
    const answers = journal({ 'GET /app/agents/api/enable': { operation: { phase: 'waiting', portal_url: 'https://services.example/consent' } } });
    const p = makePage(env, '/app/agents/', { answers });
    p.loadScript();
    p.place();
    const keys = p.windowKeys();
    const counts = [p.window.listenerCount(), p.doc.listenerCount()];
    const timers = p.clock.live();
    await p.mount();
    const enableReads = p.reads('/app/agents/api/enable').length;
    p.clock.advance(1500);
    await settle();
    assert.ok(p.reads('/app/agents/api/enable').length > enableReads, 'the enable poll runs');
    // The connect dialog: a code, and the watch that follows it.
    p.press(p.q('[data-action="connect"]'));
    await settle();
    p.press(p.q('[data-action="new-code"]'));
    await settle();
    assert.ok(p.q('.copy-code')?.textContent === 'LABONLY1', 'the code is shown');
    p.holds.push((call) => call.method === 'GET' && call.url.startsWith('/app/agents/api/pairing'));
    p.clock.advance(2000);
    await settle();
    const out = p.calls.filter((c) => !c.settled);
    assert.ok(out.length >= 1, 'a watch read is out');
    assert.ok(p.clock.live() > timers, 'timers run while agents is here');
    p.leave();
    await settle();
    assert.ok(out.every((c) => c.aborted), 'the reads out are aborted');
    assert.deepStrictEqual(p.windowKeys(), keys, 'no global is left');
    assert.deepStrictEqual([p.window.listenerCount(), p.doc.listenerCount()], counts, 'no listener is left');
    assert.ok(p.clock.live() === timers, `no timer is left (${p.clock.live() - timers})`);
    assert.ok(p.visibleIntervals() === 0, 'the poll stopped');
    assert.ok(p.q('.modal') === null, 'no dialog is left');
    const asked = p.calls.length;
    p.clock.advance(60000);
    await settle();
    assert.ok(p.calls.length === asked, 'nothing is asked after the view went');
    assert.ok(p.byId('home-text')?.textContent === 'home as it was', 'the next view is untouched');
    p.holds.length = 0;
    await p.arrive();
    assert.ok(p.q('h1')?.textContent === 'agents', 'agents mounts again');
    assert.deepStrictEqual(p.ready, ['content']);
    assert.ok(p.q('.modal') === null, 'a second mount starts clean');
    for (const name of GONE) assert.ok(!(name in p.window), `${name} is not on the window`);
  });

  await test('leaving while the connect watch waits for its next look stops it', async () => {
    const p = await boot(env);
    const timers = p.clock.live();
    p.press(p.q('[data-action="connect"]'));
    await settle();
    p.press(p.q('[data-action="new-code"]'));
    await settle();
    p.clock.advance(2000);
    await settle();
    const looks = p.reads('/app/agents/api/pairing').length;
    assert.ok(looks >= 2, 'the watch looked again');
    assert.ok(p.calls.every((c) => c.settled), 'nothing is out: the watch waits on its clock');
    assert.ok(p.clock.live() > timers, 'its next look is scheduled');
    p.leave();
    assert.ok(p.clock.live() === timers - 1, `only the view's poll and its next look went (${p.clock.live() - timers})`);
    p.clock.advance(10000);
    await settle();
    assert.ok(p.reads('/app/agents/api/pairing').length === looks, 'it looks no more');
  });

  await test('a mount that throws part-way takes back what it added', async () => {
    const p = makePage(env);
    p.loadScript();
    p.place();
    const counts = [p.window.listenerCount(), p.doc.listenerCount()];
    const timers = p.clock.live();
    p.window.AppServices.visibleInterval = () => { throw new Error('no poll'); };
    let threw = false;
    try { await p.mount(); } catch (_) { threw = true; }
    assert.ok(threw, 'the mount threw');
    await settle();
    assert.deepStrictEqual([p.window.listenerCount(), p.doc.listenerCount()], counts, 'its listeners are gone');
    assert.ok(p.clock.live() === timers, 'no timer is left');
    assert.ok(p.calls.filter((c) => c.method === 'GET').every((c) => c.aborted || c.settled), 'its reads ended');
    assert.ok(p.window.SolViews.current() === null, 'nothing is mounted');
  });

  await test('view:ready once: a failed state read, then its "try again"', async () => {
    const p = makePage(env);
    p.loadScript();
    p.place();
    p.statuses.push([(call) => call.url === '/app/agents/api/state', 503]);
    await p.mount();
    assert.deepStrictEqual(p.ready, ['error']);
    assert.ok(p.find('button', 'try again'), 'the error page offers to try again');
    p.statuses.length = 0;
    p.press(p.find('button', 'try again'));
    await settle();
    assert.ok(p.q('h1')?.textContent === 'agents', 'the page draws');
    assert.deepStrictEqual(p.ready, ['error'], 'ready is not reported again');
  });

  await test('reads out when the view goes are aborted, and late answers draw nothing', async () => {
    const p = makePage(env);
    p.loadScript();
    p.place();
    p.holds.push((call) => call.method === 'GET');
    const mounting = p.mount();
    await settle();
    const out = p.calls.slice();
    p.leave();
    await mounting;
    await settle();
    assert.ok(out.length === 2 && out.every((c) => c.aborted), 'state and memories are aborted');
    out.forEach((c) => c.answer(journal()(c.url, { method: c.method })));
    await settle();
    assert.ok(p.byId('home-text')?.textContent === 'home as it was', 'nothing is drawn');
    assert.deepStrictEqual(p.ready, [], 'no readiness');
    assert.ok(p.errors.length === 0 && p.notices.length === 0, 'nothing is logged or noticed');
  });

  await test('writes in flight when the owner leaves finish, and their answers draw, read, open and say nothing', async () => {
    for (const [label, press, match] of [
      ['a door', (p) => p.press(p.q('[data-action="own-off"]')) && p.press(p.q('[data-action="door-off"]')), (c) => c.url === '/app/agents/api/local-door'],
      ['a permission', (p) => p.press(p.q('[data-connection="oauth:g1"]')) && settleThen(p, () => p.press(p.q('[data-save-permission]'))), (c) => c.url.endsWith('/permission')],
      ['solstone.me', (p) => p.press(p.q('[data-lane="me"]')) && settleThen(p, () => p.press(p.q('[data-action="turn-on"]'))), (c) => c.url === '/app/agents/api/enable' && c.method === 'POST'],
      ['a code', (p) => p.press(p.q('[data-action="connect"]')) && settleThen(p, () => p.press(p.q('[data-action="new-code"]'))), (c) => c.url === '/app/agents/api/pairing' && c.method === 'POST'],
    ]) {
      const p = await boot(env);
      p.holds.push((call) => call.method !== 'GET' && match(call));
      await press(p);
      await settle();
      const write = p.calls.find((c) => c.method !== 'GET' && match(c));
      assert.ok(write && !write.settled, `${label}: the write is out`);
      p.leave();
      await settle();
      assert.ok(!write.aborted, `${label}: the write is not cut`);
      const asked = p.calls.length;
      write.answer({ changed: true, operation: { phase: 'waiting', portal_url: 'https://services.example/consent' }, code: 'LABONLY2', expires_at: '2026-11-14T12:10:00Z', generation: 2, door: 'local' });
      await settle();
      p.clock.advance(30000);
      await settle();
      assert.ok(p.calls.length === asked, `${label}: nothing is asked after its answer`);
      assert.ok(p.opened.length === 0, `${label}: no consent window opens`);
      assert.ok(p.notices.length === 0 && p.errors.length === 0, `${label}: nothing is said`);
      assert.ok(p.byId('home-text')?.textContent === 'home as it was' && p.q('.modal') === null, `${label}: nothing is drawn`);
    }
  });

  await test('a write that fails after the owner left says nothing, and one that answers draws nothing into the gone markup', async () => {
    const byo = { hostname: 'journal.example.com', enabled: false, account_uri: 'https://acme.example/acct/1', caa_record: '0 issue "x"', dns_verdict: 'unchecked', socket_listening: false, certificate_active: false, socket_path: '/tmp/x.sock', next_action: 'publish_caa' };
    const answers = journal({ 'GET /app/agents/api/state': agentsState({ byo }), 'GET /app/agents/api/byo/relay/commands': { setup: 'lab', remove: 'lab' } });
    const p = await boot(env, '/app/agents/', { answers });
    p.press(p.q('[data-lane="byo"]'));
    await settle();
    p.holds.push((call) => call.method === 'POST');
    p.press(p.q('[data-action="byo-dns-check"]'));
    await settle();
    const check = p.calls.find((c) => c.method === 'POST' && c.url === '/app/agents/api/byo/dns-check');
    assert.ok(check && !check.settled, 'the check is out');
    const gone = p.byId('agents-view');
    const drawn = gone.textContent;
    p.leave();
    check.answer({ verdict: 'admitted', addresses: ['203.0.113.10'], diagnostics: [] });
    await settle();
    assert.ok(gone.textContent === drawn, 'the gone markup is not drawn again');
    // A door write refused after the owner left.
    const q = await boot(env);
    q.holds.push((call) => call.url === '/app/agents/api/local-door');
    q.press(q.q('[data-action="own-off"]'));
    q.press(q.q('[data-action="door-off"]'));
    await settle();
    const door = q.calls.find((c) => c.url === '/app/agents/api/local-door');
    q.leave();
    door.answer({ detail: 'refused' }, 409);
    await settle();
    assert.ok(q.notices.length === 0, 'no notice for a view that has gone');
  });

  await test('the receipt and its recover hold nothing of the gone view', async () => {
    if (typeof global.gc !== 'function') throw new Error('run with --expose-gc');
    const p = await boot(env);
    p.press(p.q('[data-connection="oauth:g1"]'));
    await settle();
    p.press(p.q('[data-memory-delete]'));
    p.press(p.q('[data-memory-confirm]'));
    await settle();
    assert.ok(p.notices.length === 1, 'the receipt is up');
    const notices = p.notices;
    const gone = new WeakRef(p.byId('agents-view'));
    p.leave();
    p.view = null;
    await settle();
    // deref() keeps its target for the rest of the job, so each collection runs in a job of its own.
    let collected = false;
    for (let i = 0; i < 6 && !collected; i += 1) {
      await flush();
      global.gc();
      await flush();
      collected = gone.deref() === undefined;
    }
    // The page keeps the receipt; the view's markup must not be reachable through it.
    const kept = notices[0];
    assert.ok(kept.buttons.length === 2, 'the receipt keeps its buttons');
    assert.ok(collected, 'the gone view\'s markup was collected while its receipt is still up');
  });

  await test('a delete answered after the owner left still says so, and its recover works from the next view without the mount', async () => {
    const p = await boot(env);
    p.press(p.q('[data-connection="oauth:g1"]'));
    await settle();
    p.press(p.q('[data-memory-delete]'));
    p.holds.push((call) => call.method === 'DELETE');
    p.press(p.q('[data-memory-confirm]'));
    await settle();
    const del = p.calls.find((c) => c.method === 'DELETE');
    assert.ok(del && !del.settled, 'the delete is out');
    p.leave();
    await settle();
    del.answer({ trash: { id: 'trash-1', erase_at: '2026-11-21T12:00:00Z' } });
    await settle();
    assert.ok(p.notices.length === 1 && p.notices[0].title === 'moved to the trash', 'the receipt says it moved');
    const recover = p.notices[0].buttons.find((b) => b.label === 'recover');
    assert.ok(recover, 'with its recover');
    const asked = p.calls.length;
    await recover.onClick();
    await settle();
    const after = p.calls.slice(asked);
    assert.ok(after.length === 1 && after[0].method === 'POST' && after[0].url === '/app/settings/api/trash/recover', `recover asks the journal itself, and reads nothing (${after.map((c) => c.method + ' ' + c.url).join(', ')})`);
    assert.ok(p.notices.length === 2 && p.notices[1].title === 'memory recovered', 'its own card says it came back');
    assert.ok(p.byId('home-text')?.textContent === 'home as it was', 'the view on the page is untouched');
    assert.ok(p.errors.length === 0, 'no error');
  });

  await test('recover pressed while agents is on the page reads the memories again there', async () => {
    const p = await boot(env);
    p.press(p.q('[data-connection="oauth:g1"]'));
    await settle();
    p.press(p.q('[data-memory-delete]'));
    p.press(p.q('[data-memory-confirm]'));
    await settle();
    const recover = p.notices[0].buttons.find((b) => b.label === 'recover');
    const asked = p.calls.length;
    await recover.onClick();
    await settle();
    const urls = p.calls.slice(asked).map((c) => c.method + ' ' + c.url);
    assert.ok(urls.includes('POST /app/settings/api/trash/recover') && urls.includes(`GET /app/agents/api/memories/${SOURCE}`) && urls.includes('GET /app/agents/api/memories'), `reads the memories again (${urls.join(', ')})`);
    assert.ok(p.notices[p.notices.length - 1].title === 'memory recovered');
  });

  await test('a pending rename or disconnect goes with its view and never writes from its old buttons', async () => {
    for (const [connection, selector] of [['bearer:k1', '[data-rename]'], ['oauth:g1', '[data-revoke]']]) {
      const p = await boot(env);
      p.press(p.q(`[data-connection="${connection}"]`));
      await settle();
      p.press(p.q(selector));
      const oldAccept = p.q('[data-question-accept]');
      assert.ok(oldAccept && p.writes().length === 0, 'a question is drawn, nothing has acted');
      p.leave();
      p.main.focus();
      oldAccept.click();
      await settle();
      assert.ok(p.q('.convey-question') === null && p.writes().length === 0, 'leaving cancels; old buttons cannot act');
      assert.ok(p.doc.activeElement === p.main, 'arrival focus stays on the new page');
      await p.arrive();
      assert.ok(p.q('.convey-question') === null, 'returning starts with no pending question');
    }
  });

  await test('the controls work by a dispatched click after an in-page mount', async () => {
    const p = await boot(env);
    p.leave();
    await p.arrive();
    // A way in.
    p.press(p.q('[data-lane="me"]'));
    await settle();
    assert.ok(p.q('[data-lane="me"]').getAttribute('aria-checked') === 'true', 'a way is chosen');
    // A dialog, and its close.
    p.press(p.q('[data-action="connect"]'));
    await settle();
    assert.ok(p.q('[role="dialog"]'), 'the connect dialog opens');
    p.press(p.q('.modal-card [data-close-modal]'));
    await settle();
    assert.ok(p.q('[role="dialog"]') === null, 'and closes');
    // A connection's page, a rename behind prompt(), and a disconnect behind confirm().
    p.press(p.q('[data-connection="bearer:k1"]'));
    await settle();
    p.press(p.q('[data-rename]'));
    assert.strictEqual(p.q('.convey-question input').value, 'Script', 'rename starts with the current name');
    answerQuestion(p, 'renamed');
    await settle();
    assert.ok(p.writes().some((c) => c.method === 'PATCH' && c.url === '/app/agents/api/connections/bearer/k1'), 'rename writes');
    p.press(p.q('[data-connection="oauth:g1"]'));
    await settle();
    p.press(p.q('[data-revoke]'));
    answerQuestion(p);
    await settle();
    assert.ok(p.writes().some((c) => c.method === 'DELETE' && c.url === '/app/agents/api/connections/oauth/g1'), 'disconnect writes');
    // The paused card's "see what it tried" is a link to "#": the view takes it, not the router.
    p.press(p.q('[data-connection="oauth:g2"]'));
    await settle();
    const tried = p.q('a[data-activity-connection]');
    const event = p.press(tried);
    await settle();
    assert.ok(event.defaultPrevented, 'the link is the view\'s');
    assert.ok(p.loads.length === 0, 'no load');
    assert.ok(p.q('h1')?.textContent.startsWith('activity'), 'the activity page is drawn');
    assert.ok(p.errors.length === 0, 'no error');
  });

  await test('a #memories address opens that memory on the first view and clears the hash keeping the entry\'s place', async () => {
    const p = await boot(env, `/app/agents/${MEMORY_HASH}`);
    assert.ok(p.q('.mem.here'), 'the memory is marked');
    assert.ok(p.doc.scrolledTo !== undefined, 'and scrolled to');
    assert.ok(p.window.location.hash === '' && p.window.location.pathname === '/app/agents/', 'the hash is cleared');
    assert.ok(p.window.history.state?.solRoute?.id === 'e0' && p.window.history.state.solRoute.owner === 'router', 'the entry keeps its id and owner');
  });

  await test('an in-page arrival with #memories opens it too, and a back onto the cleared entry keeps its id', async () => {
    const p = await boot(env);
    p.leave();
    await p.arrive(`/app/agents/${MEMORY_HASH}`);
    await settle();
    assert.ok(p.q('.mem.here'), 'the memory is open');
    const kept = p.window.history.state?.solRoute;
    assert.ok(kept && kept.id === p.window.solRouter.held().entryId, `the entry keeps the router's id (${kept && kept.id})`);
    assert.ok(kept.owner === 'router', 'and its owner');
    assert.ok(p.window.location.hash === '', 'the hash is cleared');
  });

  await test('the request dialog\'s #memories link opens the memory in place and closes the dialog', async () => {
    const p = await boot(env);
    p.press(p.q('[data-route="activity"]'));
    await settle();
    p.press(p.q('[data-activity-index="0"]'));
    await settle();
    const dialog = p.q('[role="dialog"]');
    assert.ok(dialog, 'one request is open');
    const links = dialog.querySelectorAll('a[href]').map((a) => a.getAttribute('href'));
    assert.ok(links.includes('/app/entities/?entity=alex') && links.includes('/app/settings/?facet=work') && links.includes(`/app/agents/${MEMORY_HASH}`), `its links (${links.join(' ')})`);
    // The browser's fragment navigation: a new entry with no state, popstate, then hashchange.
    p.window.history.pushState(null, '', MEMORY_HASH);
    p.window.fire({ type: 'popstate', state: null });
    p.window.fire({ type: 'hashchange' });
    await settle();
    assert.ok(p.q('[role="dialog"]') === null, 'the dialog is gone');
    assert.ok(p.q('.mem.here'), 'the memory is open');
    assert.ok(p.window.history.state?.solRoute?.id, 'the cleared entry has an id');
    assert.ok(p.window.solRouter.held().entryId === p.window.history.state.solRoute.id, 'and it is the router\'s');
  });

  await test('leaving with a dialog open leaves nothing of it; a back onto an own entry closes one', async () => {
    const p = await boot(env);
    p.press(p.q('[data-route="activity"]'));
    await settle();
    p.press(p.q('[data-activity-index="0"]'));
    await settle();
    const before = p.reconciles();
    p.leave();
    await settle();
    assert.ok(p.q('[role="dialog"]') === null, 'no dialog is left');
    assert.ok(p.reconciles() > before, 'the modal layer is told');
    // An own entry: a hash on agents' path, then a dialog, then back.
    const q = await boot(env);
    q.window.history.pushState(null, '', '#x');
    q.window.fire({ type: 'popstate', state: null });
    q.press(q.q('[data-action="connect"]'));
    await settle();
    const close = q.q('.modal-card [data-close-modal]');
    close.focus();
    assert.ok(q.doc.activeElement === close, 'focus is in the dialog');
    q.window.history.back();
    await settle();
    assert.ok(q.q('[role="dialog"]') === null, 'the back closed the dialog');
    assert.ok(focusOn(q) === 'MAIN#main-content', `focus is on the workspace (${focusOn(q)})`);
  });

  // ---- focus that stays where the owner is (each press by the keyboard: focus, then the press)

  const DOOR_OFF = { enabled: false, listening: false, address: 'http://127.0.0.1:7659/mcp' };
  const DOOR_ON = { enabled: true, listening: true, address: 'http://127.0.0.1:7659/mcp' };
  const LAN_OFF = { enabled: false, listening: false, port: 7660, fingerprint: 'AA:BB', addresses: [{ url: 'https://192.168.4.47:7660/mcp', listening: false }] };
  const LAN_ON = { ...LAN_OFF, enabled: true, listening: true, addresses: [{ url: 'https://192.168.4.47:7660/mcp', listening: true }] };
  const ME_ON = { enabled: true, status: 'on', owner_state: { address: 'abcd1234.solstone.me', status: 'on' } };
  const BYO = { hostname: 'journal.example.test', enabled: false, account_uri: 'https://acme.example.test/acct/1', caa_record: '0 issue "example.test"',
    dns_verdict: 'unchecked', socket_listening: false, certificate_active: false, socket_path: '/tmp/test.sock', next_action: 'publish_caa',
    relay: { address: '203.0.113.24', port: 22, state: 'connected' } };
  const SECOND = { ...MEMORY, segment: '090000_1', content: 'a second note', created_at: '2026-11-01T09:00:00Z' };

  // A journal whose state answers what its writes did.
  function living(start, writes = {}) {
    const box = { st: agentsState(start), memories: [MEMORY, SECOND] };
    const answers = journal(Object.assign({
      'GET /app/agents/api/state': () => box.st,
      [`GET /app/agents/api/memories/${SOURCE}`]: () => ({ memories: box.memories, next: null, complete: true }),
      'GET /app/agents/api/byo/relay/commands': { setup: 'echo setup', remove: 'echo remove' },
    }, Object.fromEntries(Object.entries(writes).map(([key, fn]) => [key, (u, opts) => fn(box, u, opts)]))));
    return { box, answers };
  }
  function where(p) {
    const a = p.doc.activeElement;
    if (!a || a === p.doc.body) return 'BODY';
    const named = [...a.attrs.keys()].filter((k) => k.startsWith('data-') || k === 'id' || k === 'aria-label');
    return `${a.tagName}${named.map((k) => `[${k}=${a.getAttribute(k)}]`).join('')} "${a.textContent.trim().slice(0, 40)}"`;
  }
  async function keyPress(p, node) {
    assert.ok(node, 'the control to press is drawn');
    node.focus();
    assert.ok(p.doc.activeElement === node, `the press starts on its control (${where(p)})`);
    p.press(node);
    await settle();
  }
  function escape(p) {
    const event = p.doc.dispatch(p.doc.activeElement, { type: 'keydown', key: 'Escape' });
    return settle().then(() => event);
  }
  const lands = (p, node, what) => assert.ok(node && p.doc.activeElement === node, `${what} (focus on ${where(p)})`);
  const notBusy = (node, what) => assert.ok(node && !node.hasAttribute('aria-busy') && !node.hasAttribute('aria-disabled'), `${what} isn't busy`);
  // The harness's matcher splits on spaces, so a spaced label is found by hand.
  const sw = (p, label) => p.qa('[role="switch"]').find((node) => node.getAttribute('aria-label') === label) || null;
  const row = (p, key) => p.q(`.ag-row[data-connection="${key}"]`);
  const goTo = async (p, key) => { await keyPress(p, row(p, key)); };

  await test('a switch\'s write: focus stays on the switch, drawn again with its new state', async () => {
    const { answers } = living({ local_door: DOOR_OFF, lan_door: LAN_OFF }, {
      'PUT /app/agents/api/local-door': (box) => { box.st = { ...box.st, local_door: DOOR_ON }; return {}; },
      'PUT /app/agents/api/lan-door': (box) => { box.st = { ...box.st, lan_door: LAN_ON }; return {}; },
    });
    const p = await boot(env, '/app/agents/', { answers });
    const own = sw(p, 'agents on this device');
    await keyPress(p, own);
    assert.ok(own.isConnected === false, 'the switch was drawn again');
    lands(p, sw(p, 'agents on this device'), 'this device on: the same switch');
    assert.ok(p.doc.activeElement.getAttribute('aria-checked') === 'true', 'now on');
    await keyPress(p, sw(p, 'agents on your network'));
    lands(p, sw(p, 'agents on your network'), 'your network on: the same switch');
    assert.ok(p.doc.activeElement.getAttribute('aria-checked') === 'true', 'now on');
  });

  await test('a control whose write is out is busy, never disabled, ignores presses, stays busy through a redraw, and clears on its answer', async () => {
    const { box, answers } = living({ local_door: DOOR_OFF }, { 'PUT /app/agents/api/local-door': (box) => { box.st = { ...box.st, local_door: DOOR_ON }; return {}; } });
    const p = await boot(env, '/app/agents/', { answers });
    p.holds.push((call) => call.method === 'PUT');
    const own = sw(p, 'agents on this device');
    await keyPress(p, own);
    assert.ok(own.getAttribute('aria-busy') === 'true' && own.getAttribute('aria-disabled') === 'true' && !own.hasAttribute('disabled'), 'busy, not disabled');
    lands(p, own, 'focus stays on it while the write is out');
    p.press(own);
    await settle();
    assert.ok(p.writes().length === 1, `a press while busy writes nothing (${p.writes().length})`);
    // A 20 s poll that changes nothing leaves the switch, busy, as it was, and writes nothing to it.
    let marks = 0;
    const set = own.setAttribute;
    own.setAttribute = function (...args) { marks += 1; return set.apply(this, args); };
    p.clock.advance(20000);
    await settle();
    own.setAttribute = set;
    assert.ok(sw(p, 'agents on this device') === own && own.getAttribute('aria-busy') === 'true', 'a poll that changes nothing leaves the busy switch alone');
    assert.ok(marks === 0, `and marks nothing on it again (${marks})`);
    // One that changes something draws the page again while the write is out.
    box.st = { ...box.st, lan_door: LAN_ON };
    p.clock.advance(20000);
    await settle();
    const redrawn = sw(p, 'agents on this device');
    assert.ok(redrawn !== own && redrawn.getAttribute('aria-busy') === 'true', 'drawn busy again by the redraw');
    lands(p, redrawn, 'and focus is still on it');
    p.holds.length = 0;
    p.calls.filter((c) => c.method === 'PUT' && !c.settled).forEach((c) => c.answer(answers(c.url, { method: 'PUT' })));
    await settle();
    notBusy(sw(p, 'agents on this device'), 'once answered, the switch');
    lands(p, sw(p, 'agents on this device'), 'focus on the switch');
  });

  await test('a draw moves nothing when focus wasn\'t in the view: the first draw, and a poll with focus on the page', async () => {
    const down = { enabled: true, listening: false, reason: 'not_running', address: 'http://127.0.0.1:7659/mcp' };
    const p = await boot(env, '/app/agents/', { answers: living({ local_door: down }).answers });
    assert.ok(where(p) === 'BODY', `a full load's first draw leaves focus where it was (${where(p)})`);
    p.clock.advance(20000);
    await settle();
    assert.ok(where(p) === 'BODY', `so does a poll's (${where(p)})`);
  });

  await test('a draw that opens a dialog leaves focus to the modal layer', async () => {
    const p = await boot(env);
    await keyPress(p, p.q('.sh [data-action="connect"]'));
    assert.ok(p.q('[role="dialog"]'), 'the dialog is open');
    // The layer puts focus in a dialog it hasn't seen; the view moves nothing itself.
    assert.ok(where(p) === 'BODY', `the view moved nothing (${where(p)})`);
  });

  await test('a write that fails leaves focus on the control that sent it, no longer busy', async () => {
    const p = await boot(env, '/app/agents/', { answers: living({ local_door: DOOR_OFF }).answers });
    p.statuses.push([(call) => call.method === 'PUT', 503]);
    const own = sw(p, 'agents on this device');
    await keyPress(p, own);
    lands(p, own, 'the switch');
    notBusy(own, 'the switch');
    assert.ok(p.notices.length === 1, 'and the error is said');
  });

  await test('turning a door off from its dialog lands on the switch that opened it', async () => {
    const { answers } = living({ local_door: DOOR_ON, lan_door: LAN_ON }, {
      'PUT /app/agents/api/local-door': (box) => { box.st = { ...box.st, local_door: DOOR_OFF }; return {}; },
      'PUT /app/agents/api/lan-door': (box) => { box.st = { ...box.st, lan_door: LAN_OFF }; return {}; },
    });
    const p = await boot(env, '/app/agents/', { answers });
    await keyPress(p, sw(p, 'agents on this device'));
    await keyPress(p, p.q('[role="dialog"] [data-action="door-off"]'));
    assert.ok(p.q('[role="dialog"]') === null, 'the dialog is gone');
    lands(p, sw(p, 'agents on this device'), 'this device off: its switch');
    await keyPress(p, sw(p, 'agents on your network'));
    await keyPress(p, p.q('[role="dialog"] [data-action="lan-door-off"]'));
    lands(p, sw(p, 'agents on your network'), 'your network off: its switch');
  });

  await test('a dialog\'s close, by its button or Escape, puts focus back on what opened it', async () => {
    const p = await boot(env);
    const opener = () => p.q('.sh [data-action="connect"]');
    await keyPress(p, opener());
    p.q('[role="dialog"] [data-close-modal]').focus();
    await keyPress(p, p.q('[role="dialog"] button[data-close-modal]'));
    lands(p, opener(), 'connect, close: "connect an agent"');
    await keyPress(p, opener());
    p.q('[role="dialog"] button[data-close-modal]').focus();
    await escape(p);
    assert.ok(p.q('[role="dialog"]') === null, 'Escape closed it');
    lands(p, opener(), 'connect, Escape: "connect an agent"');
    await keyPress(p, sw(p, 'agents on this device'));
    await keyPress(p, p.find('[role="dialog"] button', 'keep it on'));
    lands(p, sw(p, 'agents on this device'), 'turn off this device, keep it on: the switch');
    await goTo(p, 'oauth:g1');
    const del = () => p.q(`[data-memory-delete="20261101/agent-memory-${SOURCE}/120000_1"]`);
    await keyPress(p, del());
    lands(p, p.q('[role="dialog"] [data-memory-confirm]'), 'the delete dialog opens on "move to trash"');
    await keyPress(p, p.find('[role="dialog"] button', 'cancel'));
    lands(p, del(), 'delete a memory, cancel: its "delete"');
    await keyPress(p, del());
    await escape(p);
    lands(p, del(), 'delete a memory, Escape: its "delete"');
  });

  await test('set up solstone.me lands on its next step, and turning it off on its switch', async () => {
    const { answers } = living({ local_door: DOOR_ON }, { 'PUT /app/agents/api/capability': (box) => { box.st = { ...box.st, enabled: false, status: 'off', owner_state: { address: 'abcd1234.solstone.me', status: 'off' } }; return {}; } });
    const p = await boot(env, '/app/agents/', { answers });
    await keyPress(p, p.q('[data-lane="me"]'));
    await keyPress(p, p.q('button[data-action="turn-on"]'));
    lands(p, p.q('a[data-solstone-outside]'), 'set up: "continue to approve"');
    const q = await boot(env, '/app/agents/', { answers: living({ local_door: DOOR_ON, ...ME_ON }, { 'PUT /app/agents/api/capability': (box) => { box.st = { ...box.st, enabled: false, status: 'off' }; return {}; } }).answers });
    await keyPress(q, q.q('[data-lane="me"]'));
    await keyPress(q, sw(q, 'solstone.me'));
    await keyPress(q, q.q('[role="dialog"] [data-action="turn-off"]'));
    lands(q, sw(q, 'solstone.me'), 'turn off: the solstone.me switch');
    assert.ok(q.doc.activeElement.getAttribute('aria-checked') === 'false', 'now off');
  });

  await test('your own domain: each check keeps its button, turning on lands on "turn off", remove on what to do next, replace on the DNS step', async () => {
    const { box, answers } = living({ local_door: DOOR_ON, byo: BYO }, {
      'POST /app/agents/api/byo/relay/check': () => ({ code: 'relay_ok' }),
      'POST /app/agents/api/byo/dns-check': () => ({ verdict: 'admitted', addresses: ['203.0.113.24'], diagnostics: [] }),
      'PUT /app/agents/api/byo': (b, _u, opts) => { b.st = { ...b.st, byo: { ...b.st.byo, enabled: JSON.parse(opts.body).enabled } }; return {}; },
      'DELETE /app/agents/api/byo': (b) => { b.st = { ...b.st, byo: { hostname: null } }; return {}; },
      'POST /app/agents/api/byo/account/replace': (b) => { b.st = { ...b.st, byo: { ...b.st.byo, next_action: 'publish_caa' } }; return {}; },
    });
    const p = await boot(env, '/app/agents/', { answers });
    await keyPress(p, p.q('[data-lane="byo"]'));
    for (const action of ['byo-relay-check', 'byo-dns-check']) {
      const button = p.q(`[data-action="${action}"]`);
      await keyPress(p, button);
      // Drawn again where its answer changed the page, left as it was where it didn't.
      lands(p, p.q(`[data-action="${action}"]`), `${action}: the same button`);
    }
    await keyPress(p, p.q('[data-action="byo-on"]'));
    lands(p, p.q('[data-action="byo-off"]'), 'turn it on: "turn off"');
    await keyPress(p, p.q('[data-action="byo-off"]'));
    lands(p, p.q('[data-action="byo-on"]'), 'turn off: "turn it on"');
    // A fresh page on a journal that lost its registration.
    box.st = { ...box.st, byo: { ...BYO, next_action: 'account_key_lost' } };
    const q = await boot(env, '/app/agents/', { answers });
    await keyPress(q, q.q('[data-lane="byo"]'));
    await keyPress(q, q.q('[data-action="byo-replace"]'));
    answerQuestion(q);
    await settle();
    assert.ok(q.q('[data-action="byo-replace"]') === null, 'the key-lost notice is gone');
    lands(q, q.q('[data-byo-step="dns"]'), 'replace: the DNS records step');
    await keyPress(q, q.q('[data-action="byo-remove"]'));
    answerQuestion(q);
    await settle();
    assert.ok(q.q('[data-action="byo-remove"]') === null, 'the address is gone');
    lands(q, q.q('[data-byo-removed]'), 'remove: what to do next');
  });

  await test('pause lands on resume and resume on pause, the same agent\'s other half', async () => {
    const { answers } = living({}, { 'PUT /app/agents/api/connections/oauth/g1/pause': (box, _u, opts) => {
      const paused = JSON.parse(opts.body).paused;
      box.st = { ...box.st, connections: box.st.connections.map((c) => (c.id === 'g1' ? { ...c, paused: paused ? { by: 'owner', since: '2026-11-14T12:00:00Z' } : null } : c)) };
      return {};
    } });
    const p = await boot(env, '/app/agents/', { answers });
    await goTo(p, 'oauth:g1');
    await keyPress(p, p.q('[data-pause="oauth:g1"][data-paused-to="true"]'));
    assert.ok(p.q('[data-paused-to="true"]') === null, 'the pause section is gone');
    lands(p, p.q('.paused-card [data-pause="oauth:g1"]'), 'pause: "resume"');
    await keyPress(p, p.q('.paused-card [data-pause="oauth:g1"]'));
    assert.ok(p.q('.paused-card') === null, 'the paused card is gone');
    lands(p, p.q('[data-pause="oauth:g1"][data-paused-to="true"]'), 'resume: "pause"');
  });

  await test('saving what an agent may see, and renaming a key, land on that agent\'s row', async () => {
    const { answers } = living({}, { 'PATCH /app/agents/api/connections/bearer/k1': (box) => {
      box.st = { ...box.st, connections: box.st.connections.map((c) => (c.id === 'k1' ? { ...c, name: 'renamed' } : c)) };
      return {};
    } });
    const p = await boot(env, '/app/agents/', { answers });
    await goTo(p, 'oauth:g1');
    await keyPress(p, p.q('[data-save-permission="oauth:g1"]'));
    lands(p, row(p, 'oauth:g1'), 'save: its row');
    await goTo(p, 'bearer:k1');
    await keyPress(p, p.q('[data-rename="k1"]'));
    answerQuestion(p, 'renamed');
    await settle();
    lands(p, row(p, 'bearer:k1'), 'rename: its row');
    assert.ok(p.doc.activeElement.textContent.includes('renamed'), 'with its new name');
  });

  await test('a disconnect or revoke lands on the next agent, the previous when it was last, and the heading when none are left', async () => {
    const gone = (box, id) => { box.st = { ...box.st, connections: box.st.connections.filter((c) => c.id !== id) }; return {}; };
    const { answers } = living({}, {
      'DELETE /app/agents/api/connections/oauth/g1': (b) => gone(b, 'g1'),
      'DELETE /app/agents/api/connections/oauth/g2': (b) => gone(b, 'g2'),
      'DELETE /app/agents/api/connections/bearer/k1': (b) => gone(b, 'k1'),
    });
    const p = await boot(env, '/app/agents/', { answers });
    await goTo(p, 'oauth:g1');
    await keyPress(p, p.q('[data-revoke="oauth:g1"]'));
    answerQuestion(p);
    await settle();
    assert.ok(row(p, 'oauth:g1') === null, 'its row is gone');
    lands(p, row(p, 'oauth:g2'), 'disconnect: the next agent');
    await goTo(p, 'bearer:k1');
    await keyPress(p, p.q('[data-revoke="bearer:k1"]'));
    answerQuestion(p);
    await settle();
    lands(p, row(p, 'oauth:g2'), 'revoke the last: the previous agent');
    await goTo(p, 'oauth:g2');
    await keyPress(p, p.q('[data-revoke="oauth:g2"]'));
    answerQuestion(p);
    await settle();
    assert.ok(p.qa('.list .ag-row[data-connection]').length === 0, 'no agent is left');
    lands(p, p.q('[data-agents-heading]'), 'the last one: "your agents"');
  });

  await test('a cancelled confirm or prompt, or an empty one, writes nothing and clears busy at once; the same name writes as before and clears on its answer', async () => {
    const p = await boot(env, '/app/agents/', { answers: living({ byo: { ...BYO, next_action: 'account_key_lost' } }).answers });
    await goTo(p, 'oauth:g1');
    const revoke = p.q('[data-revoke="oauth:g1"]');
    await keyPress(p, revoke);
    answerQuestion(p, null);
    await settle();
    assert.ok(p.writes().length === 0, 'a cancelled disconnect writes nothing');
    notBusy(revoke, 'disconnect, cancelled,');
    lands(p, revoke, 'and focus stays on it');
    await keyPress(p, p.q('[data-route="home"]'));
    await keyPress(p, p.q('[data-lane="byo"]'));
    for (const action of ['byo-remove', 'byo-replace']) {
      const button = p.q(`[data-action="${action}"]`);
      await keyPress(p, button);
      answerQuestion(p, null);
      await settle();
      assert.ok(p.writes().length === 0, `a cancelled ${action} writes nothing`);
      notBusy(button, `${action}, cancelled,`);
      lands(p, button, 'and focus stays on it');
    }
    await goTo(p, 'bearer:k1');
    for (const answer of [null, '']) {
      const rename = p.q('[data-rename="k1"]');
      await keyPress(p, rename);
      answerQuestion(p, answer);
      await settle();
      assert.ok(p.writes().length === 0, `a rename answered ${JSON.stringify(answer)} writes nothing`);
      notBusy(rename, `rename, answered ${JSON.stringify(answer)},`);
      lands(p, rename, 'and focus stays on it');
    }
    p.holds.push((call) => call.method === 'PATCH');
    const rename = p.q('[data-rename="k1"]');
    await keyPress(p, rename);
    answerQuestion(p, 'Script');
    await settle();
    assert.ok(p.writes().length === 1 && rename.getAttribute('aria-busy') === 'true', 'the same name writes, as before, and is busy while it does');
    p.holds.length = 0;
    p.calls.filter((c) => c.method === 'PATCH').forEach((c) => c.answer({}));
    await settle();
    notBusy(row(p, 'bearer:k1'), 'answered, its row');
    lands(p, row(p, 'bearer:k1'), 'focus on the key\'s row');
  });

  await test('a deleted memory lands on the next one\'s "delete", the previous when it was last, then the line that none are left; a failed one on its own "delete"', async () => {
    const at = (m) => `20261101/agent-memory-${SOURCE}/${m.segment}`;
    const { box, answers } = living({}, {});
    const take = (u) => { const seg = u.pathname.split('/').pop(); box.memories = box.memories.filter((m) => m.segment !== decodeURIComponent(seg)); return { trash: { id: 'trash-1', erase_at: '2026-11-21T12:00:00Z' } }; };
    const served = journal();
    const all = (u, opts) => (String(opts?.method).toUpperCase() === 'DELETE' && u.pathname.startsWith('/app/transcripts/api/segment/') ? take(u) : answers(u.href, opts));
    const p = await boot(env, '/app/agents/', { answers: (url, opts) => all(new URL(url, ORIGIN), opts) });
    assert.ok(served);
    await goTo(p, 'oauth:g1');
    p.statuses.push([(call) => call.method === 'DELETE', 409]);
    await keyPress(p, p.q(`[data-memory-delete="${at(MEMORY)}"]`));
    await keyPress(p, p.q('[role="dialog"] [data-memory-confirm]'));
    lands(p, p.q(`[data-memory-delete="${at(MEMORY)}"]`), 'a refused delete: its own "delete"');
    p.statuses.length = 0;
    await keyPress(p, p.q(`[data-memory-delete="${at(MEMORY)}"]`));
    await keyPress(p, p.q('[role="dialog"] [data-memory-confirm]'));
    assert.ok(p.q(`[data-memory-delete="${at(MEMORY)}"]`) === null, 'the memory is gone');
    lands(p, p.q(`[data-memory-delete="${at(SECOND)}"]`), 'delete: the next memory\'s "delete"');
    await keyPress(p, p.q(`[data-memory-delete="${at(SECOND)}"]`));
    await keyPress(p, p.q('[role="dialog"] [data-memory-erase-step]'));
    await keyPress(p, p.q('[role="dialog"] [data-memory-erase]'));
    assert.ok(p.qa('[data-memory-delete]').length === 0, 'none are left');
    lands(p, p.q('[data-memories-none]'), 'erase the last: the line that says none are left');
  });

  await test('a dismissed replay notice lands on the next one, then "your agents"', async () => {
    const two = [{ id: 'n1', name: 'Claude', door: 'this device', dismissed: false }, { id: 'n2', name: 'Codex', door: 'this device', dismissed: false }];
    const { answers } = living({ replay_notices: two }, {
      'POST /app/agents/api/replay-notices/n1/dismiss': (b) => { b.st = { ...b.st, replay_notices: b.st.replay_notices.filter((n) => n.id !== 'n1') }; return {}; },
      'POST /app/agents/api/replay-notices/n2/dismiss': (b) => { b.st = { ...b.st, replay_notices: b.st.replay_notices.filter((n) => n.id !== 'n2') }; return {}; },
    });
    const p = await boot(env, '/app/agents/', { answers });
    await keyPress(p, p.q('[data-action="dismiss-replay"][data-notice="n1"]'));
    lands(p, p.q('[data-action="dismiss-replay"][data-notice="n2"]'), 'the next notice\'s "dismiss"');
    await keyPress(p, p.q('[data-action="dismiss-replay"][data-notice="n2"]'));
    lands(p, p.q('[data-agents-heading]'), 'the last: "your agents"');
  });

  await test('moving between agents\' own pages: forward to the page\'s heading, back to what opened it', async () => {
    const p = await boot(env);
    await keyPress(p, p.q('[data-lane="me"]'));
    lands(p, p.q('[data-lane="me"]'), 'a lane choice: the same choice');
    await goTo(p, 'oauth:g1');
    lands(p, p.q('h1'), 'an agent\'s page: its heading');
    assert.ok(p.doc.activeElement.textContent.startsWith('Claude'), 'named for the agent');
    await keyPress(p, p.q('[data-route="home"]'));
    lands(p, row(p, 'oauth:g1'), 'back to agents: its row');
    await keyPress(p, p.q('[data-route="activity"]'));
    lands(p, p.q('h1'), 'recent activity: its heading');
    await keyPress(p, p.q('[data-route="home"]'));
    lands(p, p.q('a[data-route="activity"]'), 'back from activity: "recent activity"');
  });

  // ---- a poll that changes nothing leaves the page alone; a field being typed in keeps its text
  const viewNodes = (p) => {
    const out = [];
    const walk = (node) => { out.push(node); (node.childNodes || []).forEach(walk); };
    walk(p.q('#agents-view'));
    return out;
  };
  const sameNodes = (p, before, why) => {
    const after = viewNodes(p);
    assert.ok(after.length === before.length && after.every((node, i) => node === before[i]), why);
  };
  await test('a poll that changes nothing leaves the page alone; one that changes something keeps focus where it was', async () => {
    const down = { enabled: true, listening: false, reason: 'not_running', address: 'http://127.0.0.1:7659/mcp' };
    const { box, answers } = living({ local_door: down });
    const p = await boot(env, '/app/agents/', { answers });
    const own = sw(p, 'agents on this device');
    own.focus();
    const nodes = viewNodes(p);
    const reads = p.reads('/app/agents/api/state').length;
    p.clock.advance(20000);
    await settle();
    assert.ok(p.reads('/app/agents/api/state').length === reads + 1, 'the poll read');
    assert.ok(own.isConnected && p.doc.activeElement === own, 'the 20 s re-read with nothing changed: the same switch, the same node');
    sameNodes(p, nodes, 'and every node in the view is the one it was');
    box.st = { ...box.st, local_door: { ...down, reason: 'port_in_use' } };
    p.clock.advance(20000);
    await settle();
    assert.ok(!own.isConnected, 'a re-read that changes something draws again');
    lands(p, sw(p, 'agents on this device'), 'the 20 s re-read: the same switch');
  });

  // The DNS check answers what `dns.verdict` says, so a test can change it between polls.
  function byoJournal(start, dns) {
    return living({ byo: BYO, ...start }, {
      'POST /app/agents/api/byo/dns-check': () => ({ verdict: dns.verdict, addresses: dns.addresses || [], diagnostics: [] }),
    });
  }

  await test('your own domain: a DNS poll that changes nothing leaves the page alone', async () => {
    const dns = { verdict: 'unchecked' };
    const { answers } = byoJournal({}, dns);
    const p = await boot(env, '/app/agents/', { answers });
    await keyPress(p, p.q('[data-lane="byo"]'));
    p.clock.advance(20000);
    await settle();
    const check = p.q('[data-action="byo-dns-check"]');
    check.focus();
    const before = viewNodes(p);
    p.clock.advance(20000);
    await settle();
    assert.ok(p.calls.filter((c) => c.url.endsWith('/byo/dns-check')).length >= 2, 'the poll asked again');
    lands(p, check, 'the DNS poll with nothing new: the same "check", the same node');
    sameNodes(p, before, 'and every node in the view is the one it was');
  });

  await test('a read with the same state after an error draws the page again, and a copied button reads "copy" again at the next draw', async () => {
    const down = { enabled: true, listening: false, reason: 'not_running', address: 'http://127.0.0.1:7659/mcp' };
    const p = await boot(env, '/app/agents/', { answers: living({ local_door: down }).answers });
    p.statuses.push([(call) => call.url === '/app/agents/api/state', 503]);
    p.clock.advance(20000);
    await settle();
    assert.ok(p.q('[data-action="reload"]'), 'the poll\'s failed read drew the error');
    p.statuses.length = 0;
    await keyPress(p, p.q('[data-action="reload"]'));
    assert.ok(!p.q('[data-action="reload"]') && sw(p, 'agents on this device'), 'the same state as before the error draws the page again');
    const copy = p.q('[data-copy]');
    p.press(copy);
    await settle();
    assert.ok(copy.textContent === 'copied', 'the button says it copied');
    p.clock.advance(20000);
    await settle();
    assert.ok(p.q('[data-copy]').textContent === 'copy', `the next draw puts "copy" back (${p.q('[data-copy]').textContent})`);
  });

  await test('your own domain: the DNS poll keeps the server address being typed, its caret and focus', async () => {
    const dns = { verdict: 'unchecked' };
    const { answers } = byoJournal({}, dns);
    const p = await boot(env, '/app/agents/', { answers });
    await keyPress(p, p.q('[data-lane="byo"]'));
    const field = p.q('#byo-relay-address');
    field.focus();
    field.value = '198.51.100.7';
    field.setSelectionRange(4, 7, 'forward');
    dns.verdict = 'admitted';
    dns.addresses = ['203.0.113.24'];
    p.clock.advance(20000);
    await settle();
    const now = p.q('#byo-relay-address');
    assert.ok(now !== field, 'the poll drew the lane again');
    assert.ok(now.value === '198.51.100.7', `the typed address is kept (${now.value})`);
    lands(p, now, 'focus stays in the field');
    assert.ok(now.selectionStart === 4 && now.selectionEnd === 7 && now.selectionDirection === 'forward', 'with its caret');
  });

  await test('your own domain: the SSH port being typed keeps its text, and its box stays open', async () => {
    const dns = { verdict: 'unchecked' };
    const { answers } = byoJournal({}, dns);
    const p = await boot(env, '/app/agents/', { answers });
    await keyPress(p, p.q('[data-lane="byo"]'));
    const port = p.q('#byo-relay-port');
    port.closest('details').open = true;
    port.focus();
    port.value = '2222';
    port.setSelectionRange(4, 4);
    dns.verdict = 'admitted';
    p.clock.advance(20000);
    await settle();
    const now = p.q('#byo-relay-port');
    assert.ok(now !== port && now.closest('details').open, 'drawn again, in a box still open');
    assert.ok(now.value === '2222', `the typed port is kept (${now.value})`);
    lands(p, now, 'focus stays in the port');
    assert.ok(now.selectionStart === 4 && now.selectionEnd === 4, 'with its caret');
  });

  await test('your own domain: the 20 s re-read keeps the address being typed, and the journal\'s own value wins once it changes', async () => {
    const { box, answers } = living({ lan_door: LAN_ON, byo: { hostname: null } });
    const p = await boot(env, '/app/agents/', { answers });
    await keyPress(p, p.q('[data-lane="byo"]'));
    await keyPress(p, p.q('[data-action="byo-setup"]'));
    const field = p.q('#byo-hostname');
    field.focus();
    field.value = 'journal.exam';
    field.setSelectionRange(12, 12);
    // solstone.me goes offline meanwhile: its choice above the lane says so.
    box.st = { ...box.st, enabled: true, status: 'offline', owner_state: { address: 'abcd1234.solstone.me', status: 'offline' } };
    p.clock.advance(20000);
    await settle();
    const now = p.q('#byo-hostname');
    assert.ok(now !== field, 'the re-read drew the page again');
    assert.ok(now.value === 'journal.exam', `the typed address is kept (${now.value})`);
    lands(p, now, 'focus stays in the field');
    assert.ok(now.selectionStart === 12 && now.selectionEnd === 12, 'with its caret');
    // A server address the owner typed, then saved elsewhere: the journal's value wins.
    const saved = living({ lan_door: LAN_ON, byo: BYO });
    const q = await boot(env, '/app/agents/', { answers: saved.answers });
    await keyPress(q, q.q('[data-lane="byo"]'));
    const typed = q.q('#byo-relay-address');
    typed.focus();
    typed.value = '198.51.100.7';
    saved.box.st = { ...saved.box.st, byo: { ...BYO, relay: { ...BYO.relay, address: '192.0.2.9' } } };
    q.clock.advance(20000);
    await settle();
    assert.ok(q.q('#byo-relay-address').value === '192.0.2.9', `the journal's new value is shown (${q.q('#byo-relay-address').value})`);
  });

  await test('a back onto an own entry with the box open, opened from its focused control, lands on the workspace', async () => {
    const q = await boot(env);
    q.window.history.pushState(null, '', '#x');
    q.window.fire({ type: 'popstate', state: null });
    await settle();
    // The modal layer's two moves that matter here, as the shell's makes them: focus moved out of
    // the open dialog is pulled back into it, and once the dialog is gone the control that last had
    // focus outside it gets it back, if it's still on the page and the close wasn't a move's.
    const focus = Element.prototype.focus;
    let open = null;
    let outside = null;
    Element.prototype.focus = function () {
      focus.call(this);
      if (q.doc.activeElement !== this) return;
      if (open && open.isConnected && !open.contains(this)) focus.call(open.querySelector('button'));
      else if (!open) outside = this;
    };
    q.window.ConveyModalLayer = {
      reconcile(options) {
        const dialog = q.q('[role="dialog"]');
        if (dialog) { open = dialog; return; }
        const was = open;
        open = null;
        if (was && !(options && options.restore === false) && outside && outside.isConnected) outside.focus();
      },
    };
    try {
      await keyPress(q, q.q('[data-action="connect"]'));
      q.window.ConveyModalLayer.reconcile();
      q.q('.modal-card [data-close-modal]').focus();
      assert.ok(q.doc.activeElement.closest('[role="dialog"]'), 'focus is in the dialog');
      const hops = [];
      const layer = Element.prototype.focus;
      Element.prototype.focus = function () { layer.call(this); hops.push(focusOn(q)); };
      q.window.history.back();
      await settle();
      Element.prototype.focus = layer;
      assert.ok(q.q('[role="dialog"]') === null, 'the back closed the dialog');
      assert.ok(focusOn(q) === 'MAIN#main-content', `focus is on the workspace, not the page (${focusOn(q)})`);
      // One move, straight there: nothing pulled back into the closing dialog for a reader to speak.
      assert.ok(hops.join(' → ') === 'MAIN#main-content', `focus moved once (${hops.join(' → ')})`);
      // Opened by a tap that left focus on agents' root, as Safari's does: the back still lands on
      // the workspace, with no stop on the root on the way.
      q.window.history.pushState(null, '', '#y');
      q.q('#agents-root').focus();
      q.press(q.q('[data-action="connect"]'));
      await settle();
      q.window.ConveyModalLayer.reconcile();
      q.q('.modal-card [data-close-modal]').focus();
      const seen = [];
      const record = Element.prototype.focus;
      Element.prototype.focus = function () { record.call(this); seen.push(focusOn(q)); };
      q.window.history.back();
      await settle();
      Element.prototype.focus = record;
      assert.ok(q.q('[role="dialog"]') === null && focusOn(q) === 'MAIN#main-content', `a tap's box: the back lands on the workspace (${focusOn(q)})`);
      assert.ok(!seen.includes('MAIN#agents-root'), `with no stop on agents' root (${seen.join(' → ')})`);
      // Focus already lost to the page when the back comes: the workspace too.
      q.window.history.pushState(null, '', '#z');
      await keyPress(q, q.q('[data-action="connect"]'));
      q.window.ConveyModalLayer.reconcile();
      q.doc.activeElement.blur();
      q.window.history.back();
      await settle();
      assert.ok(focusOn(q) === 'MAIN#main-content', `focus lost before the back: the workspace (${focusOn(q)})`);
    } finally {
      Element.prototype.focus = focus;
    }
  });

  await test('a draw that changes nothing still shows the time-zone note once the journal\'s zone differs', async () => {
    const zone = { differs: false };
    const offline = { enabled: true, status: 'starting', owner_state: { address: 'abcd1234.solstone.me', status: 'starting', certificate_leg: 'not_this_week', next_attempt_at: '2026-11-14T12:00:00Z' } };
    const { answers } = living({ ...offline, lan_door: LAN_ON });
    const p = await boot(env, '/app/agents/', { answers });
    {
      p.window.JournalClock = {
        formatInstant: () => '12:00',
        differs: () => zone.differs,
        zoneNoteText: () => 'journal time',
        placeNote(anchor, options) {
          const doc = anchor.ownerDocument;
          let note = anchor.querySelector('[data-zone-note]');
          if (!zone.differs) { note?.remove(); return; }
          if (!note) { note = doc.createElement('span'); note.setAttribute('data-zone-note', ''); anchor.append(note); }
          note.textContent = 'journal time';
        },
      };
    }
    await keyPress(p, p.q('[data-lane="me"]'));
    assert.ok(p.q('[data-next-attempt]'), 'the page shows when solstone.me tries again');
    assert.ok(!p.q('[data-zone-note]'), 'no note while the zones agree');
    zone.differs = true;
    p.clock.advance(20000);
    await settle();
    assert.ok(p.q('[data-zone-note]'), 'the next draw shows the note, though the page is otherwise the same');
  });

  await test('a redraw leaves focus outside agents alone: on the rail, or on a shell notice', async () => {
    const down = { enabled: true, listening: false, reason: 'not_running', address: 'http://127.0.0.1:7659/mcp' };
    const p = await boot(env, '/app/agents/', { answers: living({ local_door: down }).answers });
    const reads = p.reads('/app/agents/api/state').length;
    const rail = p.q('#app-rail a');
    rail.focus();
    p.clock.advance(20000);
    await settle();
    lands(p, rail, 'focus on the rail stays there');
    parseInto(p.doc, p.doc.body, '<section id="notices"><button id="notice-recover">recover</button></section>');
    const notice = p.byId('notice-recover');
    notice.focus();
    p.clock.advance(20000);
    await settle();
    lands(p, notice, 'focus on a shell notice stays there');
    assert.ok(p.reads('/app/agents/api/state').length === reads + 2, 'both polls read and drew');
  });

  await test('the enable poll\'s redraw keeps focus on its link', async () => {
    let op = { phase: 'waiting', portal_url: 'https://services.example/consent' };
    const { answers } = living({ local_door: DOOR_ON }, { 'GET /app/agents/api/enable': () => ({ operation: op }) });
    const p = await boot(env, '/app/agents/', { answers });
    await keyPress(p, p.q('[data-lane="me"]'));
    const link = p.q('a[data-solstone-outside]');
    link.focus();
    op = { phase: 'needs_subscription', guidance: 'not yet', subscribe_url: 'https://services.example/portal' };
    p.clock.advance(20000);
    await settle();
    assert.ok(!link.isConnected, 'the lane was drawn again');
    lands(p, p.q('a[data-solstone-outside]'), 'the same kind of link');
  });

  await test('a control a redraw takes away inside a lane lands on the lane\'s choice', async () => {
    let op = { phase: 'waiting', portal_url: 'https://services.example/consent' };
    const { box, answers } = living({ local_door: DOOR_ON }, { 'GET /app/agents/api/enable': () => ({ operation: op }) });
    const p = await boot(env, '/app/agents/', { answers });
    await keyPress(p, p.q('[data-lane="me"]'));
    p.q('a[data-solstone-outside]').focus();
    // The portal approved it: the next poll finds it on and turning on, with no link left.
    op = { phase: 'enabled' };
    box.st = { ...box.st, enabled: true, status: 'starting', owner_state: { address: 'abcd1234.solstone.me', status: 'starting' } };
    p.clock.advance(20000);
    await settle();
    assert.ok(p.q('a[data-solstone-outside]') === null, 'the link is gone');
    lands(p, p.q('[data-lane="me"]'), 'the lane\'s choice, solstone.me');
  });

  await test('the request dialog\'s #memories link lands on the memory it names', async () => {
    const p = await boot(env);
    await keyPress(p, p.q('[data-route="activity"]'));
    await keyPress(p, p.q('[data-activity-index="0"]'));
    const link = p.q(`[role="dialog"] a[href="/app/agents/${MEMORY_HASH}"]`);
    await keyPress(p, link);
    // The browser's fragment navigation follows the press: a new entry, popstate, then hashchange.
    p.window.history.pushState(null, '', MEMORY_HASH);
    p.window.fire({ type: 'popstate', state: null });
    p.window.fire({ type: 'hashchange' });
    await settle();
    assert.ok(p.q('[role="dialog"]') === null, 'the dialog is gone');
    lands(p, p.q('.mem.here'), 'the memory');
  });

  await test('kept: making a code stays on "make a new code", and an agent connecting lands on the dialog\'s heading', async () => {
    let seen = { now: '2026-11-14T12:00:00Z', today: '20261114', pairing: { generation: 1, locked: false, door: 'local' }, connections: [] };
    const { answers } = living({ local_door: DOOR_ON }, { 'GET /app/agents/api/pairing': () => seen });
    const p = await boot(env, '/app/agents/', { answers });
    await keyPress(p, p.q('.sh [data-action="connect"]'));
    await keyPress(p, p.find('[role="dialog"] button', 'make a pairing code'));
    lands(p, p.q('[role="dialog"] [data-action="new-code"]'), 'make a code: "make a new code"');
    seen = { ...seen, pairing: null, connections: [{ ...CLAUDE, key: 'oauth:new', id: 'new', name: 'New agent', created_at: '2026-11-14T12:01:00Z', paired_at: '2026-11-14T12:01:00Z' }] };
    p.clock.advance(2000);
    await settle();
    assert.ok(p.q('[data-connect-phase="connected"]'), 'the agent connected');
    lands(p, p.q('[role="dialog"] h2'), 'the dialog\'s heading');
    assert.ok(p.doc.activeElement.textContent.includes('is connected'), 'which says so');
  });

  return cases;
};

function settleThen(p, fn) {
  return settle().then(fn).then(() => true);
}
