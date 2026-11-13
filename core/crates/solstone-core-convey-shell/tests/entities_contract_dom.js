// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Entities on the view contract, under plain Node: the real fragment, mounted
// through the real view contract, read store and router, in a stand-in page
// with events, focus, styles, a history and a clock the cases move by hand.
// entities_workspace_dom.js runs these after its own cases.

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
  closest(selector) {
    for (let at = this; at && at.nodeType === 1; at = at.parentNode) if (matches(at, selector)) return at;
    return null;
  }
  checkVisibility() {
    if (!this.isConnected) return false;
    for (let at = this; at && at.nodeType === 1; at = at.parentNode) {
      if (at.hasAttribute('hidden') || at.style.display === 'none') return false;
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
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  getBoundingClientRect() { return { left: 0, top: 0, width: 0, height: 0 }; }
  scrollIntoView() {}
  focus() {
    if (!this.isConnected || !this.checkVisibility()) return;
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
  doc.title = 'entities - journal';
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
  doc.activeElement = doc.body;
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

function summary({ offset = 0, total = 120, folder = '' } = {}) {
  const count = Math.max(0, Math.min(50, total - offset));
  return {
    items: Array.from({ length: count }, (_, i) => ({
      id: `${folder || 'p'}${offset + i}`,
      name: `${folder === 'hidden' ? 'Hidden ' : 'Person '}${offset + i}`,
      type: 'Person',
      type_bucket: 'people',
      facets: ['work'],
      total_observation_count: 2,
      last_active_ts: 1, last_active_day: '20261101',
      blocked: folder === 'hidden',
    })),
    total,
    offset,
    entity_total: 125,
    folder_counts: { all: 120, people: 120, hidden: 5 },
    facet_counts: { work: 3 },
  };
}

function detail(id) {
  return {
    entity: {
      id, name: `Person ${id}`, type: 'Person', aka: [], facets: [], blocked: false,
      is_principal: false, total_observation_count: 0,
    },
  };
}

// Answers by address. A function that returns `undefined` leaves the read out.
function journal(copy) {
  return (url, opts) => {
    const u = new URL(url, ORIGIN);
    const method = String(opts?.method || 'GET').toUpperCase();
    if (method !== 'GET') return undefined;
    if (u.pathname === '/app/entities/api/state') return { entities_copy: copy, attendance_kinds: [] };
    if (u.pathname === '/app/settings/api/facets') return { facets: [{ name: 'work', title: 'Work', color: '#2f6fdd' }] };
    if (u.pathname === '/app/entities/api/types') return { types: [{ name: 'Person' }, { name: 'Company' }, { name: 'Project' }, { name: 'Tool' }] };
    if (u.pathname === '/app/entities/api/journal/summary') {
      const hidden = u.searchParams.get('hidden') === 'only';
      return summary({ offset: Number(u.searchParams.get('offset') || 0), total: hidden ? 5 : (u.searchParams.get('q') ? 3 : 120), folder: hidden ? 'hidden' : '' });
    }
    if (u.pathname === '/app/entities/api/journal/tidy-up') return { id_shaped: { count: 0, members: [], items: [] }, duplicates: { count: 0, members: [], items: [] }, damage: { count: 0, paths: [] } };
    if (u.pathname === '/app/entities/api/merge-candidates') return { items: [] };
    const entity = /^\/app\/entities\/api\/journal\/entity\/([^/]+)$/.exec(u.pathname);
    if (entity) return detail(decodeURIComponent(entity[1]));
    if (/^\/app\/entities\/api\/journal\/entity\/[^/]+\/(history|suggestions|preview)/.test(u.pathname)) return { items: [] };
    const facetEntity = /^\/app\/entities\/api\/work\/entity\/([^/]+)$/.exec(u.pathname);
    if (facetEntity) return { entity: { id: facetEntity[1], name: 'Work One', type: 'Person', aka: [], attached: true, detached: false }, observations: [] };
    if (/^\/app\/entities\/api\/work\/entity\/[^/]+\/grid/.test(u.pathname)) return { coverage: null, days: {} };
    if (u.pathname === '/app/entities/api/work') return { attached: [{ id: 'w1', name: 'Work One', type: 'Person' }], detected: [], hidden: [] };
    if (u.pathname.startsWith('/app/entities/api/network') || u.pathname.startsWith('/app/entities/api/history')) return { neighbors: [], items: [] };
    return {};
  };
}

// ---- one page with entities in it

function makePage(env, address = '/app/entities/', { answers, storage } = {}) {
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
  const marks = [];
  const notices = [];
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
    go(delta) {
      const next = index + delta;
      if (next < 0 || next >= entries.length) return;
      const before = new URL(current.href);
      index = next;
      location.set(entries[index].url);
      window.fire({ type: 'popstate', state: entries[index].state });
      if (before.hash !== current.hash) window.fire({ type: 'hashchange' });
    },
    back() { this.go(-1); },
    forward() { this.go(1); },
  };
  window.scrollY = 0;
  window.scrollTo = (_x, y) => { window.scrollY = y; window.fire({ type: 'scroll' }); };
  window.location = location;
  window.document = doc;
  window.setTimeout = clock.setTimeout;
  window.clearTimeout = clock.clearTimeout;
  window.setInterval = clock.setInterval;
  window.clearInterval = clock.clearInterval;
  window.matchMedia = () => ({ matches: false });
  window.confirm = () => true;
  window.crypto = { getRandomValues: (bytes) => { bytes.fill(9); return bytes; } };
  const store = storage || new Map();
  window.sessionStorage = {
    getItem: (k) => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, String(v)),
    removeItem: (k) => store.delete(k),
  };
  window.beginViewReady = (view, detail) => {
    let done = false;
    return (state) => { if (!done) { done = true; marks.push(Object.assign({ view, state }, detail)); } };
  };
  window.CONVEY_COPY = { RELOAD_HINT: 'reload to try again.', STILL_WORKING: 'still working…' };
  window.SurfaceState = {
    loading: ({ text = 'loading…' } = {}) => `<div class="surface-state surface-state--loading" role="status"><span class="surface-state-text">${text}</span></div>`,
    error: ({ heading = '', retry } = {}) => `<div class="surface-state surface-state--error" role="alert"><h2 class="surface-state-heading">${heading}</h2>${retry ? '<button type="button" class="surface-state-retry">try again</button>' : ''}</div>`,
    replaceLoading(id, html) { const node = doc.getElementById(id); if (node) node.innerHTML = html; },
    serverMessageFrom: (err) => err?.serverMessage || '',
  };
  window.JournalFormat = { day: (key) => `day ${key}`, timestamp: (ts) => String(ts) };
  window.AppServices = { notifications: { show: (notice) => { notices.push(notice); return notices.length; }, dismiss() {} }, escapeHtml: (s) => String(s) };
  window.logError = (error, context) => { errors.push({ error, context }); };
  window.appEvents = { buildChanged: () => page.buildChanged, onConnectionState: () => () => {} };
  window.__SOL_ENTRY_TABLE__ = { views: { entities: { contract: true, fragment: '/app/entities/workspace', scripts: [], styles: [] } } };
  window.solShellData = { apps: [{ name: 'entities', label: 'entities', workspace_url: '/app/entities/workspace' }] };

  const serve = answers || journal(env.copy);
  const ask = (url, opts, kind) => new Promise((resolve, reject) => {
    const call = {
      url: new URL(url, current.href).pathname + new URL(url, current.href).search,
      method: String(opts?.method || 'GET').toUpperCase(),
      body: opts?.body ? JSON.parse(opts.body) : null,
      kind, aborted: false, settled: false,
      answer(body, status = 200) {
        if (call.settled) return;
        call.settled = true;
        if (kind === 'fetch') {
          resolve({ ok: status < 400, status, redirected: false, json: async () => body, text: async () => JSON.stringify(body) });
        } else if (status >= 400) reject(apiError(status, body));
        else resolve(body);
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
    if (value !== undefined) Promise.resolve().then(() => call.answer(value));
  });
  window.apiJson = (url, opts = {}) => ask(url, opts, 'api');
  window.fetch = (url, opts = {}) => ask(url, opts, 'fetch');

  // The shell around the view: a rail with home and entities, and <main>.
  parseInto(doc, doc.body, '<nav id="app-rail"><a href="/app/home/" data-app-name="home">home</a><a href="/app/entities/" data-app-name="entities">entities</a></nav><main id="main-content" tabindex="-1"></main><div id="view-announcer"></div>');
  const main = doc.getElementById('main-content');

  const context = vm.createContext({
    window, document: doc, location, history: window.history,
    URL, URLSearchParams, AbortController, AbortSignal, DOMException, WeakMap, Map, Set, Promise,
    console, Math, Number, String, Object, Array, Boolean, Date, Intl, JSON, Error, TypeError, RegExp, Uint8Array, Symbol,
    setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval, clearInterval: clock.clearInterval,
    performance: { mark() {}, clearMarks() {} },
  });
  context.self = window;
  // The window's own names, and the view's code sees the page's names.
  Object.keys(window).forEach(() => {});
  const shell = path.join(env.manifestDir, 'assets/static');
  vm.runInContext(fs.readFileSync(path.join(shell, 'store.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'views.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'router.js'), 'utf8'), context);
  // The window is the context's global object in a browser; here the
  // contract's own names are copied across.
  ['SolStore', 'SolViews', 'solRouter'].forEach((name) => { window[name] = context.window[name] || context[name]; });

  const page = {
    env, window, doc, main, clock, calls, marks, notices, errors, entries, location, loads, store, context,
    buildChanged: false,
    holds: [],
    hold(call) { return page.holds.some((test) => test(call)); },
    index: () => index,
    windowKeys: () => Object.keys(window).sort(),
    q: (s) => doc.querySelector(s),
    byId: (id) => doc.getElementById(id),
    reads: (prefix) => calls.filter((c) => c.method === 'GET' && c.url.startsWith(prefix)),
    writes: () => calls.filter((c) => c.method !== 'GET'),
    place() {
      main.replaceChildren();
      parseInto(doc, main, env.markup);
    },
    loadScript() {
      vm.runInContext(env.script, context, { filename: 'entities-workspace.js' });
    },
    async mount() {
      page.ready = [];
      page.view = await window.SolViews.mount('entities', { reportReady: (s) => page.ready.push(s) });
      await settle();
      return page.view;
    },
    startRouter() {
      window.solRouter.start('entities');
    },
    // Another view takes the page, as the router's move and place() do.
    leave(next = '/app/home/') {
      window.history.pushState({ solRoute: { id: 'r' + entries.length, owner: 'router' } }, '', next);
      window.SolViews.dispose();
      main.replaceChildren();
      parseInto(doc, main, '<section id="home"><h1>home</h1></section>');
    },
    type(id, value) {
      const field = doc.getElementById(id);
      field.value = value;
      doc.dispatch(field, { type: 'input' });
    },
    visible(id) {
      const node = doc.getElementById(id);
      return !!node && node.checkVisibility();
    },
  };
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
  p.startRouter();
  await settle();
  return p;
}

function listRows(p) {
  return p.byId('journal-entity-cards-content').querySelectorAll('.journal-row');
}

function focusIsSomewhere(p, label) {
  const active = p.doc.activeElement;
  assert.ok(active && active !== p.doc.body, `${label}: focus is never on the body`);
}

// Run from entities_workspace_dom.js: answers with the number of cases.
module.exports = async function entitiesContractCases(manifestDir) {
  const workspace = fs.readFileSync(path.join(manifestDir, 'assets/entities/workspace.html'), 'utf8');
  const scriptStart = workspace.indexOf('<script>\n// Entities on the view contract');
  const script = workspace.slice(scriptStart + '<script>'.length, workspace.lastIndexOf('</script>'));
  const markup = workspace.slice(0, scriptStart)
    .replace(/<style>[\s\S]*?<\/style>/g, '')
    .replace(/<script[^>]*><\/script>/g, '')
    .replace(/<link[^>]*>/g, '');
  const copy = JSON.parse(fs.readFileSync(path.join(manifestDir, '../solstone-core-entities/src/data/entities_copy.json'), 'utf8'));
  const env = { manifestDir, script, markup, copy };

  // ---- AC 1: a contract view

  await test('the script only registers: nothing reads, draws or listens at load', async () => {
    const p = makePage(env);
    p.place();
    const keys = p.windowKeys();
    const listeners = [p.window.listenerCount(), p.doc.listenerCount()];
    p.loadScript();
    assert.strictEqual(p.calls.length, 0, 'no read at load');
    assert.deepStrictEqual(p.windowKeys(), keys, 'nothing is added to the window');
    assert.deepStrictEqual([p.window.listenerCount(), p.doc.listenerCount()], listeners, 'no listener at load');
    assert.strictEqual(p.window.SolViews.registered('entities'), true);
    assert.strictEqual(p.clock.live(), 0, 'no timer at load');
  });

  await test('a mount reads its first reads once each, through the store, and draws the list', async () => {
    const p = await boot(env);
    for (const url of ['/app/entities/api/state', '/app/settings/api/facets', '/app/entities/api/journal/summary?offset=0&limit=50&hidden=exclude']) {
      assert.strictEqual(p.calls.filter((c) => c.url === url).length, 1, `${url} is read once`);
    }
    assert.ok(p.calls.every((c) => c.url.startsWith('/app/')), 'every read is an absolute address');
    assert.strictEqual(listRows(p).length, 50, 'the first page of the list is drawn');
    assert.deepStrictEqual(p.ready, ['content']);
  });

  await test('dispose leaves no listener, timer, read, global or onclick, and the view mounts again', async () => {
    const p = makePage(env);
    p.loadScript();
    p.place();
    const keys = p.windowKeys();
    const counts = [p.window.listenerCount(), p.doc.listenerCount()];
    const timers = p.clock.live();
    p.holds.push((call) => call.url.startsWith('/app/entities/api/journal/tidy-up'));
    await p.mount();
    // Open the scope menu (a capture press on the document), start a search's
    // debounce and an inline error's timer, then leave with all of it live.
    p.byId('entities-scope-trigger').click();
    p.type('journal-entity-card-search', 'per');
    assert.ok(p.clock.live() > timers, 'the debounce is a live timer');
    assert.ok(p.view.ctx.held().listeners >= 5, 'window and document listeners are the mount\'s');
    const tidy = p.calls.find((c) => c.url.startsWith('/app/entities/api/journal/tidy-up'));
    const view = p.view;
    p.leave();
    assert.strictEqual(tidy.aborted, true, 'a read in flight is aborted');
    assert.deepStrictEqual(JSON.parse(JSON.stringify(view.ctx.held())), { listeners: 0, timeouts: 0, intervals: 0, observers: 0, undo: 0, bodyClasses: 0, reads: 0 });
    assert.deepStrictEqual([p.window.listenerCount(), p.doc.listenerCount()], counts, 'every window and document listener is gone');
    assert.strictEqual(p.clock.live(), timers, 'every timer is gone');
    assert.deepStrictEqual(p.windowKeys(), keys, 'nothing stays on the window');
    assert.strictEqual(p.window.onclick, undefined, 'no onclick slot is taken');
    p.clock.advance(5000);
    await settle();
    assert.strictEqual(p.byId('home') !== null, true, 'the next view is untouched');
    // And again, in the same document.
    p.window.history.pushState({ solRoute: { id: 'again', owner: 'router' } }, '', '/app/entities/');
    p.place();
    await p.mount();
    assert.strictEqual(listRows(p).length, 50, 'a second mount in one document draws the list');
    assert.strictEqual(p.errors.length, 0, 'no error logged');
  });

  await test('a mount that throws part-way takes back what it added', async () => {
    const p = makePage(env);
    p.loadScript();
    p.place();
    p.byId('journal-detail-edit-form').remove();
    const counts = [p.window.listenerCount(), p.doc.listenerCount()];
    await assert.rejects(p.window.SolViews.mount('entities', {}));
    assert.deepStrictEqual([p.window.listenerCount(), p.doc.listenerCount()], counts, 'the listeners it added are gone');
    assert.strictEqual(p.window.SolViews.current(), null);
  });

  await test('leaving with the delete dialog open gives the page its scroll back', async () => {
    const p = await boot(env, '/app/entities/#p3');
    p.byId('journal-detail-delete-btn').click();
    await settle();
    assert.strictEqual(p.doc.body.style.overflow, 'hidden', 'the dialog holds the page');
    p.leave();
    assert.strictEqual(p.doc.body.style.overflow, '', 'the next view scrolls');
  });

  // ---- AC 2: a write is never lost or doubled by leaving

  await test('a merge in flight when the owner leaves finishes, and its late answer sets no address and draws nothing', async () => {
    const serve = journal(copy);
    const answers = (url, opts) => {
      if (opts?.method === 'POST' && String(url) === '/app/entities/api/merge') {
        const body = JSON.parse(opts.body);
        return body.commit ? undefined : { aliases_added: 1, emails_added: 0 };
      }
      return serve(url, opts);
    };
    const p = await boot(env, '/app/entities/#p3', { answers });
    p.type('journal-detail-merge-target', 'Person 1');
    p.clock.advance(400);
    await settle();
    const option = p.byId('journal-detail-merge-results').querySelectorAll('button').find((b) => /Person 1/.test(b.textContent));
    assert.ok(option, 'the merge picker offers the target');
    option.click();
    p.byId('journal-detail-merge-btn').click();
    await settle();
    p.byId('journal-detail-merge-preview').querySelector('button').click();
    await settle();
    const commit = p.calls.find((c) => c.method === 'POST' && c.body?.commit);
    assert.ok(commit, 'the merge is committed');
    p.leave();
    const entries = p.entries.map((e) => e.url);
    const homeText = p.main.textContent;
    const reads = p.calls.length;
    assert.strictEqual(commit.aborted, false, 'leaving never cuts a write');
    commit.answer({ merge_id: 'm1' });
    await settle();
    assert.deepStrictEqual(p.entries.map((e) => e.url), entries, 'no hash is written onto the next view');
    assert.strictEqual(p.location.href, `${ORIGIN}/app/home/`);
    assert.strictEqual(p.main.textContent, homeText, 'nothing is drawn on the next view');
    assert.strictEqual(p.calls.length, reads, 'no list read after the view has gone');
    assert.strictEqual(p.calls.filter((c) => c.method === 'POST' && c.body?.commit).length, 1, 'the merge went out once');
  });

  await test('a merge that answers while entities is showing opens the target, one entry', async () => {
    const serve = journal(copy);
    const answers = (url, opts) => {
      if (opts?.method === 'POST' && String(url) === '/app/entities/api/merge') {
        return JSON.parse(opts.body).commit ? { merge_id: 'm1' } : { aliases_added: 0, emails_added: 0 };
      }
      return serve(url, opts);
    };
    const p = await boot(env, '/app/entities/#p3', { answers });
    p.type('journal-detail-merge-target', 'Person 1');
    p.clock.advance(400);
    await settle();
    p.byId('journal-detail-merge-results').querySelectorAll('button').find((b) => /Person 1/.test(b.textContent)).click();
    p.byId('journal-detail-merge-btn').click();
    await settle();
    const i = p.index();
    p.byId('journal-detail-merge-preview').querySelector('button').click();
    await settle();
    assert.strictEqual(p.index(), i + 1, 'one entry, as the hash was');
    assert.strictEqual(p.location.hash, '#p1');
    assert.ok(p.calls.some((c) => c.url === '/app/entities/api/journal/entity/p1'), 'the target is read');
  });

  await test('a delete in flight when the owner leaves is not cut, and its late answer shows nothing', async () => {
    const p = await boot(env, '/app/entities/#p3');
    p.holds.push((call) => call.method === 'DELETE');
    p.byId('journal-detail-delete-btn').click();
    await settle();
    p.byId('entityDeleteModalConfirm').click();
    await settle();
    const del = p.calls.find((c) => c.method === 'DELETE');
    assert.ok(del, 'the delete went out');
    const notices = p.notices.length;
    p.leave();
    const entries = p.entries.length;
    assert.strictEqual(del.aborted, false, 'leaving never cuts a write');
    const homeText = p.main.textContent;
    del.answer({ success: true, trash: { id: 't1', erase_at: null } });
    await settle();
    assert.strictEqual(p.notices.length, notices, 'no receipt after the view has gone');
    assert.strictEqual(p.entries.length, entries, 'no history entry is written');
    assert.strictEqual(p.main.textContent, homeText, 'nothing is drawn on the next view');
    assert.strictEqual(p.calls.filter((c) => c.method === 'DELETE').length, 1, 'the write went out once');
    assert.strictEqual(p.errors.length, 0);
  });

  await test('leaving during tidy-up ends as a reload does: stopped, the same operation, on return', async () => {
    const storage = new Map();
    const ids = Array.from({ length: 120 }, (_, i) => `id${i}`);
    const serve = journal(copy);
    const answers = (url, opts) => {
      if (String(url).startsWith('/app/entities/api/journal/tidy-up')) {
        return { id_shaped: { count: ids.length, actionable: true, members: ids, items: ids.slice(0, 50).map((id) => ({ id, name: id })) }, duplicates: { count: 0, members: [], items: [] }, damage: { count: 0, paths: [] } };
      }
      if (opts?.method === 'POST' && String(url) === '/app/entities/api/journal/block') {
        const body = JSON.parse(opts.body);
        return { hidden: body.ids, already_hidden: [], missing: [], unreadable: [] };
      }
      return serve(url, opts);
    };
    const p = await boot(env, '/app/entities/?folder=ids', { answers, storage });
    // The second batch is still out when the owner leaves.
    p.holds.push((call) => call.method === 'POST' && call.body?.ids?.[0] === 'id50');
    const hide = p.byId('journal-ids-panel').querySelector('[data-action="tidy-hide"]');
    assert.ok(hide, 'the panel offers its bulk hide');
    hide.click();
    await settle();
    p.clock.advance(200);
    await settle();
    const second = p.calls.find((c) => c.method === 'POST' && c.body?.ids?.[0] === 'id50');
    assert.ok(second, 'the second batch is in flight');
    const record = JSON.parse(storage.get('entities.tidyOperation'));
    assert.strictEqual(record.phase, 'hiding');
    p.leave();
    second.answer({ hidden: second.body.ids, already_hidden: [], missing: [], unreadable: [] });
    await settle();
    p.clock.advance(5000);
    await settle();
    assert.strictEqual(p.calls.filter((c) => c.method === 'POST').length, 2, 'no batch starts after the view has gone');
    assert.deepStrictEqual(JSON.parse(storage.get('entities.tidyOperation')), record, 'the record is the one a reload finds');
    // Back to entities.
    p.window.history.pushState({ solRoute: { id: 'back', owner: 'router' } }, '', '/app/entities/?folder=ids');
    p.place();
    await p.mount();
    const toast = p.byId('journal-tidy-toast');
    assert.strictEqual(toast.getAttribute('data-tidy-state'), 'hide-stopped', 'the toast shows it stopped');
    toast.querySelectorAll('button').find((b) => b.getAttribute('data-action') === 'retry-hide').click();
    await settle();
    const again = p.calls.filter((c) => c.method === 'POST').at(-1);
    assert.strictEqual(again.body.operation, record.id, 'going on keeps its operation id');
  });

  await test('a recover from a receipt after leaving runs the same write and draws only its notice', async () => {
    const p = await boot(env, '/app/entities/#p3');
    p.byId('journal-detail-delete-btn').click();
    await settle();
    p.holds.push(() => false);
    const del = () => p.calls.find((c) => c.method === 'DELETE');
    p.byId('entityDeleteModalConfirm').click();
    await settle();
    del().answer({ success: true, trash: { id: 't9', erase_at: null } });
    await settle();
    const receipt = p.notices.at(-1);
    const recover = receipt.buttons.find((b) => b.label === receipt.buttons[0].label);
    p.leave();
    const homeText = p.main.textContent;
    const entries = p.entries.length;
    const reads = p.calls.filter((c) => c.method === 'GET').length;
    const done = recover.onClick();
    await settle();
    const post = p.calls.find((c) => c.method === 'POST' && c.url === '/app/settings/api/trash/recover');
    assert.ok(post && post.body.ids[0] === 't9', 'the recover goes out');
    post.answer({ done: ['t9'] });
    await done;
    await settle();
    assert.ok(/back|recover/i.test(p.notices.at(-1).title), 'its receipt is shown');
    assert.strictEqual(p.main.textContent, homeText, 'nothing is drawn on the next view');
    assert.strictEqual(p.entries.length, entries, 'no history entry');
    assert.strictEqual(p.calls.filter((c) => c.method === 'GET').length, reads, 'no list read');
  });

  // ---- AC 3: its own moves, in the page

  await test('a folder, a page, a search, an entity and back: one entry each, or a replace where the list replaces', async () => {
    const p = await boot(env);
    const at = () => p.index();
    let i = at();
    p.byId('journal-folders').querySelectorAll('button').find((b) => b.getAttribute('data-folder') === 'hidden').click();
    await settle();
    assert.strictEqual(at(), i + 1, 'a folder is one entry');
    assert.strictEqual(p.location.search, '?folder=hidden');
    focusIsSomewhere(p, 'folder');
    assert.strictEqual(p.doc.activeElement.getAttribute('data-folder'), 'hidden', 'focus is on the folder chosen');
    p.byId('journal-folders').querySelectorAll('button').find((b) => b.getAttribute('data-folder') === '').click();
    await settle();
    i = at();
    p.byId('journal-list-pager').querySelectorAll('button')[1].click();
    await settle();
    assert.strictEqual(at(), i + 1, 'a page is one entry');
    assert.strictEqual(p.location.search, '?page=2');
    focusIsSomewhere(p, 'page');
    i = at();
    p.type('journal-entity-card-search', 'per');
    p.clock.advance(300);
    await settle();
    assert.strictEqual(at(), i, 'a search replaces, as it did');
    assert.strictEqual(p.location.search, '?q=per');
    i = at();
    listRows(p)[1].click();
    await settle();
    assert.strictEqual(at(), i + 1, 'opening an entity is one entry');
    assert.strictEqual(p.location.hash, '#p1');
    assert.ok(p.visible('journal-entity-detail-view'), 'the entity is shown');
    assert.strictEqual(p.doc.activeElement, p.byId('journal-detail-name'), 'focus is on its name');
    assert.ok(p.marks.some((m) => m.view === 'entities' && m.update), 'an own move reports ready');
    i = at();
    p.q('#journal-entity-detail-view [data-entities-action="list"]').click();
    await settle();
    assert.strictEqual(at(), i + 1, 'back to the list is one entry');
    assert.strictEqual(p.location.hash, '');
    assert.ok(p.visible('journal-entities-view'), 'the list is shown');
    assert.strictEqual(p.doc.activeElement.getAttribute('data-entity-id'), 'p1', 'focus is on the entity just left');
    assert.ok(p.entries.every((e) => e.state?.solRoute?.id), 'every entry carries its router id');
    assert.strictEqual(p.loads.length, 0, 'no page load');
  });

  await test('back and forward restore the list, the entity and the scroll, without reading the list again', async () => {
    const p = await boot(env);
    p.byId('journal-list-pager').querySelectorAll('button')[1].click();
    await settle();
    p.window.scrollTo(0, 640);
    listRows(p)[3].click();
    await settle();
    p.window.scrollTo(0, 0);
    const summaries = p.reads('/app/entities/api/journal/summary').length;
    p.window.history.back();
    await settle();
    assert.strictEqual(p.location.search, '?page=2');
    assert.ok(p.visible('journal-entities-view'), 'back shows the list');
    assert.strictEqual(listRows(p)[0].getAttribute('data-entity-id'), 'p50', 'on its page');
    assert.strictEqual(p.reads('/app/entities/api/journal/summary').length, summaries, 'the list it holds, no read');
    assert.strictEqual(p.window.scrollY, 640, 'its scroll');
    focusIsSomewhere(p, 'back');
    p.window.history.forward();
    await settle();
    assert.strictEqual(p.location.hash, '#p53');
    assert.ok(p.visible('journal-entity-detail-view'), 'forward shows the entity');
    p.window.history.back();
    p.window.history.back();
    await settle();
    assert.strictEqual(p.location.search, '');
    assert.strictEqual(listRows(p)[0].getAttribute('data-entity-id'), 'p0', 'the first page again');
    assert.strictEqual(p.loads.length, 0);
  });

  await test('a facet is one entry, back restores the whole journal, and the inline links work after an in-page mount', async () => {
    const p = await boot(env, '/app/entities/');
    const i = p.index();
    p.byId('entities-scope-trigger').click();
    p.byId('entities-scope-menu').querySelectorAll('[role="option"]')[1].click();
    await settle();
    assert.strictEqual(p.index(), i + 1, 'a facet is one entry');
    assert.strictEqual(p.location.search, '?facet=work');
    assert.strictEqual(p.doc.activeElement, p.byId('entities-scope-trigger'), 'focus is on the scope control');
    // The facet's card opens its entity; its back link and "in all facets" work.
    p.window.history.pushState(p.window.history.state, '', '/app/entities/?facet=work');
    p.context.window.solRouter.viewEntry('push', '/app/entities/?facet=work#w1');
    p.window.fire({ type: 'popstate' });
    await settle();
    assert.ok(p.visible('entity-detail-view'), 'the facet\'s entity is shown');
    p.q('#entity-detail-view [data-entities-action="list"]').click();
    await settle();
    assert.ok(p.visible('entities-list-view'), 'its back link shows the facet\'s list');
    p.context.window.solRouter.viewEntry('push', '/app/entities/?facet=work#w1');
    p.window.fire({ type: 'popstate' });
    await settle();
    p.q('[data-entities-action="all-facets"]').click();
    await settle();
    assert.strictEqual(p.location.search, '', '"in all facets" leaves the facet');
    assert.strictEqual(p.location.hash, '#w1');
    p.window.history.back();
    await settle();
    assert.strictEqual(p.location.search, '?facet=work', 'back restores the facet');
    assert.strictEqual(p.loads.length, 0);
  });

  await test('the detach dialog\'s buttons work after an in-page mount, and closing it keeps focus', async () => {
    const p = await boot(env, '/app/entities/?facet=work#w1');
    const opener = p.doc.createElement('button');
    p.byId('entity-detail-view').append(opener);
    opener.focus();
    p.context.window.__show = null;
    const modal = p.byId('detachConfirmModal');
    modal.style.display = 'flex';
    p.q('#detachConfirmModal [data-entities-action="detach-close"]').click();
    assert.strictEqual(modal.style.display, 'none', 'cancel closes it');
    await settle();
    focusIsSomewhere(p, 'detach close');
    modal.style.display = 'flex';
    p.byId('confirmDetachBtn').click();
    assert.strictEqual(modal.style.display, 'none', 'detach closes it');
    await settle();
    focusIsSomewhere(p, 'detach confirm');
  });

  await test('the no-slash address opens what a full load opens, at the address a full load lands on', async () => {
    const p = await boot(env, '/app/entities#p7');
    assert.strictEqual(p.location.pathname, '/app/entities/');
    assert.strictEqual(p.location.hash, '#p7');
    assert.ok(p.visible('journal-entity-detail-view'), 'the entity\'s page');
    assert.strictEqual(p.calls.filter((c) => c.url === '/app/entities/api/journal/entity/p7').length >= 1, true);
    const q = await boot(env, '/app/entities');
    assert.strictEqual(q.location.pathname, '/app/entities/');
    assert.ok(q.visible('journal-entities-view'), 'the list');
    const r = await boot(env, '/app/entities/#p2');
    assert.ok(r.visible('journal-entity-detail-view'), 'search\'s hash link opens the entity');
  });

  await test('the rail\'s entities on a folder or an entity is entities\' own entry, back to the list', async () => {
    const p = await boot(env, '/app/entities/?folder=hidden#h2');
    const i = p.index();
    const reads = p.calls.length;
    const view = p.window.SolViews.current();
    const rail = p.q('#app-rail a[data-app-name="entities"]');
    rail.focus();
    const event = rail.click();
    await settle();
    assert.strictEqual(event.defaultPrevented, true, 'no page load');
    assert.strictEqual(p.index(), i + 1, 'one entry');
    assert.strictEqual(p.location.href, `${ORIGIN}/app/entities/`);
    assert.strictEqual(p.window.SolViews.current(), view, 'not a fresh mount');
    assert.ok(p.visible('journal-entities-view'), 'the whole journal\'s list');
    assert.strictEqual(p.doc.activeElement, rail, 'focus stays on the rail');
    assert.ok(p.calls.length > reads, 'the default list is read');
    assert.strictEqual(p.loads.length, 0);
  });

  await test('an own move on a page that learned of a new build loads the address instead', async () => {
    const p = await boot(env);
    p.buildChanged = true;
    const i = p.index();
    p.byId('journal-folders').querySelectorAll('button').find((b) => b.getAttribute('data-folder') === 'hidden').click();
    await settle();
    assert.strictEqual(p.index(), i, 'no entry is written');
    assert.deepStrictEqual(p.loads, [`${ORIGIN}/app/entities/?folder=hidden`]);
    p.window.history.back();
    await settle();
  });

  await test('an entity read still out when the owner leaves draws nothing, and later pops are not entities\'', async () => {
    const p = await boot(env);
    p.holds.push((call) => call.url === '/app/entities/api/journal/entity/p4');
    listRows(p)[4].click();
    await settle();
    const read = p.calls.find((c) => c.url === '/app/entities/api/journal/entity/p4');
    assert.ok(read, 'the entity is being read');
    p.leave('/app/search/');
    assert.strictEqual(read.aborted, true, 'its read ends with the view');
    const entries = p.entries.map((e) => e.url);
    const homeText = p.main.textContent;
    read.answer(detail('p4'));
    p.window.fire({ type: 'popstate' });
    p.window.fire({ type: 'hashchange' });
    await settle();
    assert.deepStrictEqual(p.entries.map((e) => e.url), entries, 'nothing is written');
    assert.strictEqual(p.main.textContent, homeText, 'nothing is drawn');
  });

  await test('a delete from the entity\'s page goes back to the list with focus in the view', async () => {
    const serve = journal(copy);
    const answers = (url, opts) => (opts?.method === 'DELETE' ? { success: true, trash: { id: 't1', erase_at: null } } : serve(url, opts));
    const p = await boot(env, '/app/entities/#p3', { answers });
    const button = p.byId('journal-detail-delete-btn');
    button.focus();
    button.click();
    await settle();
    p.byId('entityDeleteModalConfirm').click();
    await settle();
    assert.ok(p.visible('journal-entities-view'), 'the list');
    focusIsSomewhere(p, 'after the delete');
    assert.ok(p.notices.length >= 1, 'its receipt');
  });

  await test('a dialog closed after its opener has gone leaves focus in the view', async () => {
    const p = await boot(env, '/app/entities/#p3');
    const button = p.byId('journal-detail-delete-btn');
    button.focus();
    button.click();
    await settle();
    button.remove();
    p.byId('entityDeleteModalCancel').click();
    await settle();
    focusIsSomewhere(p, 'cancel');
  });

  await test('unhide and try again from a hidden name\'s notice, after leaving, run their write and draw only a notice', async () => {
    const serve = journal(copy);
    const answers = (url, opts) => {
      const u = String(url);
      if (u === '/app/entities/api/work' && !opts?.method) return { attached: [], detected: [{ name: 'Ada', type: 'Person', count: 2, last_seen: '20261101' }], hidden: [] };
      if (opts?.method === 'POST' && u === '/app/entities/api/work/detected/hide') return { success: true, changed: true, search_queued: false };
      if (opts?.method === 'POST' && u === '/app/entities/api/work/detected/unhide') return { success: true, changed: true, still_hidden_by: { id: 'p9', name: 'Person 9' } };
      return serve(url, opts);
    };
    const p = await boot(env, '/app/entities/?facet=work', { answers });
    const hide = p.byId('all-entities-tbody').querySelector('.ent-hide-btn');
    assert.ok(hide, 'the name has its hide');
    hide.click();
    await settle();
    const receipt = p.notices.at(-1);
    assert.strictEqual(receipt.buttons.length, 2, 'try again and unhide');
    p.leave();
    const homeText = p.main.textContent;
    const entries = p.entries.length;
    const gets = p.calls.filter((c) => c.method === 'GET').length;
    await receipt.buttons[0].onClick();
    await settle();
    assert.strictEqual(p.calls.filter((c) => c.method === 'POST' && c.url.endsWith('/detected/hide')).length, 2, 'try again hides again');
    assert.strictEqual(p.notices.at(-1).buttons.length, 2, 'and shows its own notice');
    await receipt.buttons[1].onClick();
    await settle();
    assert.ok(p.calls.some((c) => c.method === 'POST' && c.url.endsWith('/detected/unhide') && c.body.name === 'Ada'), 'unhide posts the name');
    const still = p.notices.at(-1);
    assert.ok(still.buttons[0].label && still.title, 'the still-hidden notice');
    assert.strictEqual(p.main.textContent, homeText, 'nothing is drawn on the next view');
    assert.strictEqual(p.entries.length, entries, 'no entry');
    assert.strictEqual(p.calls.filter((c) => c.method === 'GET').length, gets, 'no read');
    // Its button opens entities' address through the router, or as a page.
    still.buttons[0].onClick();
    assert.deepStrictEqual(p.loads, [`${ORIGIN}/app/entities/#p9`]);
  });

  return cases;
};
