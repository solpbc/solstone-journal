// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Agents on the view contract, under plain Node: the real fragment's mount
// through the real view contract, read store and router, in a stand-in page
// with events, focus, a history and a clock the cases move by hand.
// agents_workspace_dom.js runs these after its own cases. Every compare is of
// plain values, ids or tag names: never an element.

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
    this.value = '';
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
      if (!focused.isConnected || !focused.checkVisibility() || focused.disabled) return doc.body;
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
    confirmAnswer: true,
    promptAnswer: null,
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
    confirm: () => page.confirmAnswer,
    prompt: () => page.promptAnswer,
    navigator: { clipboard: { writeText: async () => {} } },
  });
  context.self = window;
  const shell = path.join(env.manifestDir, 'assets/static');
  vm.runInContext(fs.readFileSync(path.join(shell, 'store.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'views.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'router.js'), 'utf8'), context);
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
    p.press(p.find('button', 'check again'));
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
    p.promptAnswer = 'renamed';
    p.press(p.q('[data-rename]'));
    await settle();
    assert.ok(p.writes().some((c) => c.method === 'PATCH' && c.url === '/app/agents/api/connections/bearer/k1'), 'rename writes');
    p.press(p.q('[data-connection="oauth:g1"]'));
    await settle();
    p.press(p.q('[data-revoke]'));
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

  return cases;
};

function settleThen(p, fn) {
  return settle().then(fn).then(() => true);
}
