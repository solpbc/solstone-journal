// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Network on the view contract, under plain Node: the real fragment's mount
// and the real pairing ceremony, through the real view contract, read store
// and router, in a stand-in page with events, focus, a history and a clock the
// cases move by hand. network_pairing_dom.js runs these after its own cases.
// Every compare is of plain values, ids or tag names: never an element.

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
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
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
  doc.title = 'network - journal';
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

const DELIVERED = {
  cid: 'sha256:delivered', display_label: 'suze', role: 'device', state: 'connected', capture_state: 'active',
  last_seen_at: '2026-11-13T10:00:00Z', last_accepted_ingest_at: '2026-11-13T10:00:00Z', paired_at: '2026-07-02T18:23:01Z',
  failing: false, owner_label: null,
};
const NEVER = {
  cid: 'sha256:never', display_label: 'old phone', role: 'device', state: 'disconnected', capture_state: 'no_capture',
  last_seen_at: null, last_accepted_ingest_at: null, paired_at: '2026-07-02T18:23:01Z', failing: false, owner_label: null,
};

function material() {
  return {
    nonce: 'nonce-a', pair_link: 'https://go.example/p#ALPHANUMERIC', expires_in: 125, device_label: 'sample device',
    ca_fingerprint: 'abcdef0123456789', home_address_is_public: false,
  };
}

// Answers by address and method. `undefined` leaves the request out.
function journal(copy, overrides = {}) {
  return (url, opts) => {
    const u = new URL(url, ORIGIN);
    const method = String(opts?.method || 'GET').toUpperCase();
    const key = `${method} ${u.pathname}`;
    if (key in overrides) return typeof overrides[key] === 'function' ? overrides[key](u, opts) : overrides[key];
    switch (key) {
      case 'GET /app/network/api/state': return { posture: 'direct', link_copy: copy };
      case 'GET /app/network/api/status': return { posture: 'direct', reachability: 'online', device_addresses: [], vpn: null, home_address: '192.168.1.5:7657', home_candidates: [] };
      case 'GET /app/network/api/private-link': return { state: 'not_enabled' };
      case 'GET /app/network/api/local-network': return { open: true, can_change: true, windows_asks: false, listening_on: 'local_network', agents_on_network: false };
      case 'GET /app/network/api/identity': return { availability: 'none' };
      case 'GET /app/network/api/clients': return { clients: [DELIVERED, NEVER] };
      case 'GET /app/network/api/pair/nonce-status': return { present: true, used: false };
      case 'POST /app/network/pair-start': return material();
      case 'POST /app/network/local-network/open': return { open: true, can_change: true, windows_asks: false, listening_on: 'local_network' };
      case 'POST /app/network/private-link/enable': return { operation: { phase: 'waiting', portal_url: 'https://services.example/consent' } };
      default: break;
    }
    if (method === 'DELETE' && u.pathname.startsWith('/app/network/api/clients/')) return {};
    if (method === 'POST' && /\/forget$/.test(u.pathname)) return {};
    return {};
  };
}

// ---- one page with network in it

function makePage(env, address = '/app/network/', { answers, arrival = null } = {}) {
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
  const shown = [];
  const opened = [];
  const linkListeners = new Set();
  const observers = [];
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
  window.CONVEY_COPY = { RELOAD_HINT: 'reload to try again.', STILL_WORKING: 'still working…', CONSOLE_SUMMARY_REQUEST_FAILED: "couldn't finish that request." };
  window.SurfaceState = {
    loading: ({ text = 'loading…' } = {}) => `<div class="surface-state surface-state--loading" role="status"><span class="surface-state-text">${text}</span></div>`,
    error: ({ heading = '', retry, detail } = {}) => { if (detail && process.env.NET_DBG) console.log('DBG2', String(detail.stack || detail).slice(0, 600)); return `<div class="surface-state surface-state--error" role="alert"><h2 class="surface-state-heading">${heading}</h2>${retry ? '<button type="button" class="surface-state-retry">try again</button>' : ''}</div>`; },
    empty: ({ heading = '', desc = '', action = '' } = {}) => `<div class="surface-state surface-state--empty"><h3>${heading}</h3><p>${desc}</p>${action}</div>`,
    replaceLoading(id, html) { const node = doc.getElementById(id); if (node) node.innerHTML = html; },
    serverMessageFrom: (err) => err?.serverMessage || '',
  };
  window.JournalFormat = { day: (key) => `day ${key}`, timestamp: (ts) => String(ts) };
  window.relativeTime = () => '2 minutes';
  window.AppServices = {
    escapeHtml: (s) => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;'),
    visibleInterval(fn, ms) {
      const id = clock.setInterval(fn, ms);
      visible.add(id);
      return () => { clock.clearInterval(id); visible.delete(id); };
    },
  };
  window.ApiError = class ApiError extends Error { constructor(fields) { super('api error'); Object.assign(this, fields); } };
  window.logError = (error, context) => { errors.push({ error, context }); };
  window.showError = (text) => { shown.push(text); };
  window.solstoneOutside = { openDelayed: (url) => opened.push(url) };
  window.ConveyModalLayer = { reconcile() { reconciles += 1; } };
  window.appEvents = {
    buildChanged: () => false,
    onConnectionState: () => () => {},
    listen(tract, fn) { const entry = { tract, fn }; linkListeners.add(entry); return () => linkListeners.delete(entry); },
  };
  window.qrcode = () => ({ addData() {}, make() {}, createSvgTag() { return '<svg></svg>'; }, getModuleCount() { return 23; } });
  window.__SOL_ENTRY_TABLE__ = { views: { network: { contract: true, fragment: '/app/network/workspace', scripts: [], styles: [] } } };
  window.solShellData = { apps: [{ name: 'network', label: 'network', workspace_url: '/app/network/workspace' }] };

  const serve = answers || journal(env.copy);
  const ask = (url, opts, kind) => new Promise((resolve, reject) => {
    const u = new URL(url, current.href);
    const call = {
      url: u.pathname + u.search,
      method: String(opts?.method || 'GET').toUpperCase(),
      kind, aborted: false, settled: false,
      answer(body, status = 200) {
        if (call.settled) return;
        call.settled = true;
        if (kind === 'fetch') {
          resolve({ ok: status < 400, status, redirected: false, json: async () => body, text: async () => JSON.stringify(body) });
        } else if (status >= 400) {
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

  class ResizeObserverShim {
    constructor(fn) { this.fn = fn; this.on = false; observers.push(this); }
    observe() { this.on = true; }
    disconnect() { this.on = false; }
  }

  parseInto(doc, doc.body, '<nav id="app-rail"><a href="/app/home/">home</a><a href="/app/network/">network</a></nav><main id="main-content" tabindex="-1"></main><div id="view-announcer"></div>');
  const main = doc.getElementById('main-content');

  const page = {
    env, window, doc, main, clock, calls, errors, shown, opened, loads, observers,
    confirmAnswer: true,
    holds: [],
    statuses: [],
    hold(call) { return page.holds.some((test) => test(call)); },
    status(call) { const hit = page.statuses.find(([test]) => test(call)); return hit ? hit[1] : 200; },
    windowKeys: () => Object.keys(window).sort(),
    byId: (id) => doc.getElementById(id),
    q: (s) => doc.querySelector(s),
    reads: (prefix) => calls.filter((c) => c.method === 'GET' && c.url.startsWith(prefix)),
    writes: () => calls.filter((c) => c.method !== 'GET'),
    links: () => linkListeners.size,
    visibleIntervals: () => visible.size,
    liveObservers: () => observers.filter((o) => o.on).length,
    reconciles: () => reconciles,
    place() {
      main.replaceChildren();
      parseInto(doc, main, env.markup);
    },
    loadScript() {
      vm.runInContext(env.script, context, { filename: 'network-workspace.js' });
    },
    async mount() {
      page.ready = [];
      page.view = await window.SolViews.mount('network', { reportReady: (s) => page.ready.push(s) });
      await settle();
      return page.view;
    },
    // Another view takes the page, as the router's move and place() do.
    leave(next = '/app/home/') {
      window.history.pushState({ solRoute: { id: 'r' + entries.length, owner: 'router' } }, '', next);
      window.SolViews.dispose();
      main.replaceChildren();
      parseInto(doc, main, '<section id="home"><h1>home</h1></section>');
    },
    // Back onto network in the same document, as a move to it does.
    async arrive(addressAgain = '/app/network/', mode = 'push') {
      window.history.pushState({ solRoute: { id: 'a' + entries.length, owner: 'router' } }, '', addressAgain);
      arrivalMode = mode;
      page.place();
      return page.mount();
    },
    press(node) { return node.click(); },
  };
  let arrivalMode = arrival;

  const context = vm.createContext({
    window, document: doc, location, history: window.history,
    URL, URLSearchParams, AbortController, AbortSignal, DOMException, WeakMap, Map, Set, Promise, TextEncoder,
    console, Math, Number, String, Object, Array, Boolean, Date, Intl, JSON, Error, TypeError, RegExp, Uint8Array, Symbol,
    setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval, clearInterval: clock.clearInterval,
    performance: { mark() {}, clearMarks() {} },
    fetch: window.fetch,
    confirm: () => page.confirmAnswer,
    navigator: { clipboard: { writeText: async () => {} } },
    CSS: { escape: (s) => String(s).replace(/["\\]/g, '\\$&') },
    ResizeObserver: ResizeObserverShim,
  });
  context.self = window;
  const shell = path.join(env.manifestDir, 'assets/static');
  vm.runInContext(fs.readFileSync(path.join(shell, 'store.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'views.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'router.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(env.manifestDir, 'assets/network/network.js'), 'utf8'), context);
  ['SolStore', 'SolViews', 'solRouter', 'NetworkRender'].forEach((name) => { window[name] = context.window[name] || context[name]; });
  // How the view arrived: the real router's answer, set by the case.
  window.solRouter = Object.assign({}, window.solRouter, { arrival: () => arrivalMode });
  page.setArrival = (mode) => { arrivalMode = mode; };
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

function cards(p) {
  return p.byId('clientsList').querySelectorAll('.client-card');
}

function focusOn(p) {
  const active = p.doc.activeElement;
  return active ? (active.id ? `${active.tagName}#${active.id}` : active.tagName) : 'none';
}

const GONE = ['LinkCopy', 'linkPairingCeremony', 'loadClients', 'unpairClient', 'clientsList', 'escapeHtml', 'renderClients',
  'clientLabels', 'clientsEpoch', 'landOnDevices', 'forgetDevice', 'initClients', 'initLinkWorkspace', 'showToast'];

// Run from network_pairing_dom.js: answers with the number of cases.
module.exports = async function networkContractCases(manifestDir) {
  const workspace = fs.readFileSync(path.join(manifestDir, 'assets/network/workspace.html'), 'utf8');
  const scriptStart = workspace.indexOf('<script>\n(function () {\n  function mountNetwork(ctx) {');
  assert.ok(scriptStart > 0, 'the fragment has its one inline mount');
  const scriptEnd = workspace.indexOf('</script>', scriptStart);
  const script = workspace.slice(scriptStart + '<script>'.length, scriptEnd);
  const markup = workspace.slice(0, scriptStart)
    .replace(/<style>[\s\S]*?<\/style>/g, '')
    .replace(/<script[^>]*><\/script>/g, '')
    .replace(/<link[^>]*>/g, '');
  const copy = JSON.parse(fs.readFileSync(path.join(manifestDir, 'assets/network_copy.json'), 'utf8'));
  const env = { manifestDir, script, markup, copy };

  await test('the fragment is a contract view with its two first reads, and no handler in it names a global', async () => {
    assert.ok(workspace.includes('<!-- view-contract -->'));
    assert.ok(workspace.includes('<!-- first-read: /app/network/ -> /app/network/api/state /app/network/api/clients -->'));
    assert.ok(!/\son[a-z]+="/.test(workspace), 'no inline handler attribute in the markup or the drawn cards');
    assert.ok((workspace.match(/<script>/g) || []).length === 1, 'one inline script');
  });

  await test('the script only registers: nothing reads, draws or listens at load', async () => {
    const p = makePage(env);
    p.place();
    const keys = p.windowKeys();
    const listeners = [p.window.listenerCount(), p.doc.listenerCount()];
    p.loadScript();
    assert.ok(p.calls.length === 0, 'no read at load');
    assert.deepStrictEqual(p.windowKeys(), keys, 'nothing is added to the window');
    assert.deepStrictEqual([p.window.listenerCount(), p.doc.listenerCount()], listeners, 'no listener at load');
    assert.ok(p.window.SolViews.registered('network') === true);
    assert.ok(p.clock.live() === 0, 'no timer at load');
  });

  await test('a mount reads each address once, through the store, draws the list and reports ready once', async () => {
    const p = await boot(env);
    for (const url of ['/app/network/api/state', '/app/network/api/clients', '/app/network/api/status', '/app/network/api/private-link', '/app/network/api/local-network', '/app/network/api/identity']) {
      assert.ok(p.reads(url).length === 1, `${url} is read once (${p.reads(url).length})`);
    }
    assert.ok(cards(p).length === 2, 'both devices are drawn');
    assert.ok(p.byId('link-dashboard-content').hidden === false, 'the page is shown');
    assert.deepStrictEqual(p.ready, ['content']);
    assert.ok(p.visibleIntervals() === 3, 'the two 15 s polls and the clients poll pause while the page is hidden');
    assert.ok(p.errors.length === 0, 'no error logged');
  });

  await test('dispose leaves no listener, timer, poll, read, observer, subscription or global, and network mounts again', async () => {
    const p = makePage(env, '/app/network/#devices');
    p.loadScript();
    p.place();
    const keys = p.windowKeys();
    const counts = [p.window.listenerCount(), p.doc.listenerCount()];
    p.holds.push((call) => call.url.startsWith('/app/network/api/pair/nonce-status'));
    await p.mount();
    p.clock.advance(0);
    assert.ok(p.liveObservers() === 1, 'the #devices landing watches the page');
    assert.ok(p.window.listenerCount() > counts[0], 'the landing listens for the owner on the window');
    p.press(p.q('[data-pairing-action="open"]'));
    await settle();
    assert.ok(p.byId('link-pairing-material').hidden === false, 'the code is shown');
    assert.ok(p.links() === 1, 'the ceremony hears the stream');
    const view = p.view;
    const held = view.ctx.held();
    assert.ok(held.listeners >= 5 && held.intervals >= 1 && held.timeouts >= 1, 'listeners, the tick and the expiry are the mount\'s');
    p.leave();
    assert.deepStrictEqual(JSON.parse(JSON.stringify(view.ctx.held())), { listeners: 0, timeouts: 0, intervals: 0, observers: 0, undo: 0, bodyClasses: 0, reads: 0 });
    assert.deepStrictEqual([p.window.listenerCount(), p.doc.listenerCount()], counts, 'every window and document listener is gone');
    assert.ok(p.clock.live() === 0, `every timer and poll is gone (${p.clock.live()} live)`);
    assert.ok(p.visibleIntervals() === 0, 'every poll is stopped');
    assert.ok(p.liveObservers() === 0, 'the landing\'s observer is disconnected');
    assert.ok(p.links() === 0, 'no stream subscription');
    assert.ok(p.reconciles() >= 1, 'the modal layer is reconciled');
    assert.deepStrictEqual(p.windowKeys(), keys, 'nothing stays on the window');
    for (const name of GONE) assert.ok(!(name in p.window) && !(name in p.context), `${name} is not a global`);
    p.clock.advance(60000);
    await settle();
    assert.ok(p.byId('home') !== null && p.byId('link-pairing-qr') === null, 'no QR or link is left in the page');
    const before = p.calls.length;
    // And again, in the same document.
    await p.arrive('/app/network/');
    assert.ok(cards(p).length === 2, 'a second mount in one document draws the list');
    assert.ok(p.calls.length - before === 6, `the second mount reads each address once (${p.calls.length - before})`);
    assert.deepStrictEqual(p.ready, ['content']);
    assert.ok(p.errors.length === 0, 'no error logged');
  });

  await test('a mount that throws part-way takes back what it added', async () => {
    const p = makePage(env);
    p.loadScript();
    p.place();
    p.byId('clientsList').remove();
    const counts = [p.window.listenerCount(), p.doc.listenerCount()];
    await assert.rejects(p.window.SolViews.mount('network', {}));
    assert.deepStrictEqual([p.window.listenerCount(), p.doc.listenerCount()], counts, 'the listeners it added are gone');
    assert.ok(p.window.SolViews.current() === null);
    assert.ok(p.clock.live() === 0, 'no timer left');
  });

  await test('view:ready once: a failed state read, then its "try again"', async () => {
    const p = makePage(env);
    p.statuses.push([(call) => call.url === '/app/network/api/state' && p.reads('/app/network/api/state').length === 1, 503]);
    p.loadScript();
    p.place();
    await p.mount();
    assert.deepStrictEqual(p.ready, ['error']);
    p.press(p.byId('link-state-gate').querySelector('.surface-state-retry'));
    await settle();
    assert.ok(p.byId('link-dashboard-content').hidden === false, 'the page is shown after "try again"');
    assert.deepStrictEqual(p.ready, ['error'], 'ready is reported once per mount');
  });

  await test('reads out when the view goes are aborted, and their late answers draw nothing', async () => {
    const p = makePage(env);
    p.holds.push((call) => call.method === 'GET');
    p.loadScript();
    p.place();
    await p.mount();
    const out = p.calls.filter((c) => c.method === 'GET');
    assert.ok(out.length === 2, 'state and clients are out');
    p.leave();
    assert.ok(out.every((c) => c.aborted), 'both are aborted');
    out.forEach((c) => c.answer(journal(copy)(c.url, { method: 'GET' })));
    await settle();
    assert.ok(p.calls.length === 2, 'nothing more is asked');
    assert.deepStrictEqual(p.ready, [], 'no readiness from a view that has gone');
    assert.ok(p.byId('home') !== null && p.errors.length === 0, 'the next view is untouched');
  });

  await test('a nonce read out when the owner leaves with the code shown is aborted, and the code\'s clock stops', async () => {
    const p = await boot(env);
    p.holds.push((call) => call.url.startsWith('/app/network/api/pair/nonce-status'));
    p.press(p.q('[data-pairing-action="open"]'));
    await settle();
    p.press(p.byId('link-pairing-check'));
    await settle();
    const nonce = p.reads('/app/network/api/pair/nonce-status')[0];
    assert.ok(nonce && !nonce.settled, 'the nonce read is out');
    p.leave();
    assert.ok(nonce.aborted, 'the nonce read is aborted');
    assert.ok(p.clock.live() === 0 && p.links() === 0, 'the tick, the expiry and the subscription are gone');
    assert.ok(p.byId('link-pairing-link-value') === null, 'no pairing link is left in the page');
  });

  await test('a paired view reads the closed network without offering local-only changes', async () => {
    const answers = journal(copy, {
      'GET /app/network/api/local-network': {
        open: false, can_change: false, windows_asks: true, listening_on: 'this_pc',
        host_capabilities: { same_computer_pairing: true },
      },
      'POST /app/network/pair-start': () => { throw apiError(409, { reason_code: 'local_network_closed' }); },
    });
    const p = await boot(env, '/app/network/', { answers });
    assert.ok(p.byId('link-local-network').hidden, 'local-only controls stay hidden');
    assert.ok(p.errors.length === 0, 'the status read is not a diagnostic error');
    p.press(p.byId('link-local-network-toggle'));
    await settle();
    assert.ok(p.writes().length === 0, 'a hidden control cannot change the network');
    p.press(p.q('[data-pairing-action="open"]'));
    await settle();
    assert.ok(p.q('[data-pairing-action="local-network-open"]').hidden, 'pairing offers no local-network write');
    assert.ok(p.q('[data-pairing-action="this-computer"]').hidden, 'pairing offers no same-computer link remotely');
    p.leave();
  });

  await test('writes in flight when the owner leaves finish, and their answers draw, open and ask nothing', async () => {
    const answers = journal(copy, { 'GET /app/network/api/local-network': { open: false, can_change: true, windows_asks: false, listening_on: 'this_pc' } });
    const p = await boot(env, '/app/network/', { answers });
    p.holds.push((call) => call.method !== 'GET');
    // unpair, forget, the local network and the private link, each pressed
    p.press(p.q('[data-network-action="unpair"]'));
    const forgetRow = p.q('.client-forget');
    p.press(forgetRow.querySelector('[data-forget-action="ask"]'));
    p.press(forgetRow.querySelector('[data-forget-action="confirm"]'));
    p.press(p.byId('link-local-network-toggle'));
    p.press(p.byId('link-private-link-setup'));
    await settle();
    const writes = p.writes();
    assert.deepStrictEqual(writes.map((c) => `${c.method} ${c.url}`).sort(), [
      'DELETE /app/network/api/clients/sha256%3Adelivered',
      'POST /app/network/api/devices/sha256%3Anever/forget',
      'POST /app/network/local-network/open',
      'POST /app/network/private-link/enable',
    ]);
    const readsBefore = p.calls.filter((c) => c.method === 'GET').length;
    p.leave();
    assert.ok(writes.every((c) => !c.aborted), 'no write is cut short');
    writes.find((c) => c.method === 'DELETE').answer({ error: 'no' }, 500);
    writes.filter((c) => c.method !== 'DELETE').forEach((c) => c.answer(journal(copy)(c.url, { method: c.method })));
    await settle();
    p.clock.advance(5000);
    await settle();
    assert.ok(p.calls.filter((c) => c.method === 'GET').length === readsBefore, 'nothing is read after a write answers');
    assert.ok(p.shown.length === 0, 'a failed unpair shows nothing over the next view');
    assert.ok(p.opened.length === 0, 'no consent window opens once the owner has left');
    assert.ok(p.byId('home') !== null && p.errors.length === 0, 'the next view is untouched');
  });

  await test('answers that come after the owner left draw nothing into the view they belonged to', async () => {
    const p = await boot(env);
    p.holds.push((call) => call.method !== 'GET');
    p.press(p.q('[data-pairing-action="open"]'));
    const forgetRow = p.q('.client-forget');
    p.press(forgetRow.querySelector('[data-forget-action="ask"]'));
    p.press(forgetRow.querySelector('[data-forget-action="confirm"]'));
    await settle();
    // The view's markup, held only by this case, to read after it has gone.
    const dialog = p.byId('link-pairing-dialog');
    const status = forgetRow.querySelector('.client-forget-status');
    const statusBefore = status.textContent;
    p.leave();
    p.writes().find((c) => c.url === '/app/network/pair-start').answer(material());
    p.writes().find((c) => c.url.endsWith('/forget')).answer({ detail: 'refused' }, 409);
    await settle();
    assert.ok(dialog.querySelector('#link-pairing-link-value').textContent === '', 'no pairing link is drawn');
    assert.ok(dialog.querySelector('#link-pairing-material').hidden === true, 'no code is shown');
    assert.ok(status.textContent === statusBefore, 'the forget refusal is not drawn');
    assert.ok(p.clock.live() === 0 && p.links() === 0, 'nothing is armed');
  });

  await test('a private-link enable that answers while network is showing still opens its consent window', async () => {
    const p = await boot(env);
    p.press(p.byId('link-private-link-setup'));
    await settle();
    assert.deepStrictEqual(p.opened, ['https://services.example/consent']);
  });

  await test('a card\'s unpair works by a dispatched click after an in-page mount', async () => {
    const p = await boot(env);
    p.leave();
    await p.arrive('/app/network/');
    const before = p.reads('/app/network/api/clients').length;
    p.press(p.q('.client-card[data-cid="sha256:delivered"] [data-network-action="unpair"]'));
    await settle();
    assert.ok(p.writes().filter((c) => c.method === 'DELETE').length === 1, 'one unpair');
    assert.ok(p.reads('/app/network/api/clients').length === before + 1, 'the list is read again, of its own');
    assert.ok(p.errors.length === 0, 'no error');
  });

  await test('the clients poll asks of its own each time, and every poll stops with the view', async () => {
    const p = await boot(env);
    const reads = () => p.reads('/app/network/api/clients').length;
    const status = () => p.reads('/app/network/api/status').length;
    const start = [reads(), status()];
    p.clock.advance(30000);
    await settle();
    assert.ok(reads() === start[0] + 1, 'the 30 s poll reads the list');
    assert.ok(status() === start[1] + 2, 'the 15 s poll reads the status twice');
    p.leave();
    p.clock.advance(60000);
    await settle();
    assert.ok(reads() === start[0] + 1 && status() === start[1] + 2, 'no poll after the view goes');
  });

  await test('?pair= opens the ceremony on the page\'s first view only', async () => {
    const first = await boot(env, '/app/network/?pair=phone');
    assert.ok(first.writes().filter((c) => c.url === '/app/network/pair-start').length === 1, 'a full load asks for one link');
    for (const mode of ['pop', 'push', 'replace']) {
      const p = await boot(env, '/app/network/?pair=phone', { arrival: mode });
      assert.ok(p.writes().length === 0, `an in-page ${mode} asks for no link`);
      assert.ok(p.byId('link-pairing-dialog').hidden === true, `an in-page ${mode} opens no dialog`);
    }
  });

  await test('#devices lands on the devices heading once the page is shown and the list drawn, after the router, and not on a back', async () => {
    for (const mode of [null, 'push']) {
      const p = makePage(env, '/app/network/#devices', { arrival: mode });
      p.holds.push((call) => call.url === '/app/network/api/state');
      p.loadScript();
      p.place();
      await p.mount();
      p.clock.advance(100);
      assert.ok(focusOn(p) !== 'H2#devices', 'nothing lands while the page is still hidden');
      p.calls.find((c) => c.url === '/app/network/api/state').answer({ posture: 'direct', link_copy: copy });
      await settle();
      p.doc.activeElement = p.main;
      p.clock.advance(0);
      assert.ok(focusOn(p) === 'H2#devices', `${mode || 'full load'}: focus on the devices heading (it is on ${focusOn(p)})`);
      assert.ok(p.doc.scrolledTo === 'devices', 'the heading is scrolled into view');
    }
    const back = await boot(env, '/app/network/#devices', { arrival: 'pop' });
    back.doc.activeElement = back.main;
    back.clock.advance(100);
    assert.ok(focusOn(back) === 'MAIN#main-content', `a back keeps the router's focus and scroll (it is on ${focusOn(back)})`);
    assert.ok(back.liveObservers() === 0, 'a back watches nothing');
  });

  // ---- focus after network's own writes, polls and the pairing dialog

  // A press from the keyboard: focus, then the press. Answers whether the
  // control had focus first, so a case counts only a real press.
  function keyPress(p, node) {
    node.focus();
    assert.ok(p.doc.activeElement === node, `the control takes focus first (focus is on ${focusOn(p)})`);
    p.press(node);
  }
  function lastCall(p, method, url) {
    return p.calls.filter((c) => c.method === method && c.url.startsWith(url)).pop();
  }
  function busy(node) {
    return node.getAttribute('aria-disabled') === 'true' && node.getAttribute('aria-busy') === 'true' && node.disabled === false;
  }
  const TWO = (selected) => ({
    posture: 'direct', reachability: 'online', device_addresses: [], vpn: null, home_address: selected,
    home_candidates: ['192.168.1.5:7657', '10.0.0.5:7657'].map((address) => ({ address, selected: address === selected })),
  });
  function radios(p) { return p.byId('link-home-candidates-list').querySelectorAll('input'); }

  await test('host address "apply" and "clear" keep focus while the write is out, after it, and when it fails', async () => {
    for (const which of ['apply', 'clear']) {
      const p = await boot(env);
      p.byId('link-host-address-override').open = true;
      p.holds.push((call) => call.url === '/app/network/host-address');
      const button = p.byId(`link-host-address-${which}`);
      keyPress(p, button);
      await settle();
      assert.ok(busy(button), `${which} is busy, not disabled, while the write is out`);
      assert.ok(focusOn(p) === `BUTTON#link-host-address-${which}`, `${which}: focus stays while the write is out (on ${focusOn(p)})`);
      p.press(button);
      assert.ok(p.writes().length === 1, `${which}: a press while busy sends nothing`);
      lastCall(p, 'POST', '/app/network/host-address').answer({});
      await settle();
      assert.ok(focusOn(p) === `BUTTON#link-host-address-${which}` && !busy(button), `${which}: focus stays after the redraw (on ${focusOn(p)})`);
      p.holds.length = 0;
      p.statuses.push([(call) => call.url === '/app/network/host-address', 500]);
      keyPress(p, button);
      await settle();
      assert.ok(focusOn(p) === `BUTTON#link-host-address-${which}`, `${which}: a failed write leaves focus on the control (on ${focusOn(p)})`);
      assert.ok(p.byId('link-host-address-error').hidden === false, `${which}: and its error is shown`);
    }
  });

  await test('a home address choice by arrow keeps focus while saving, ignores another, and lands on the same address after the rebuild', async () => {
    for (const fails of [false, true]) {
      let chosen = '192.168.1.5:7657';
      const answers = journal(copy, {
        'GET /app/network/api/status': () => TWO(chosen),
        'POST /app/network/host-address': (_u, opts) => { if (!fails) chosen = JSON.parse(opts.body).home_address; return {}; },
      });
      const p = await boot(env, '/app/network/', { answers });
      if (fails) p.statuses.push([(call) => call.url === '/app/network/host-address', 500]);
      p.holds.push((call) => call.url === '/app/network/host-address');
      const [first, second] = radios(p);
      first.focus();
      assert.ok(p.doc.activeElement === first, 'the first choice has focus');
      // ArrowDown: the browser moves focus to the next choice and checks it.
      first.checked = false; second.checked = true; second.focus();
      p.doc.dispatch(second, { type: 'change' });
      await settle();
      assert.ok(busy(second) && busy(first), 'the choices are busy, not disabled');
      assert.ok(p.doc.activeElement === second, `focus stays on the pressed choice while saving (on ${focusOn(p)})`);
      second.checked = false; first.checked = true; first.focus();
      p.doc.dispatch(first, { type: 'change' });
      assert.ok(second.checked === true && p.writes().length === 1, 'another choice while saving puts the pending one back and sends nothing');
      second.focus();
      const write = lastCall(p, 'POST', '/app/network/host-address');
      if (fails) write.answer({ detail: 'refused' }, 500); else { answers(write.url, { method: 'POST', body: JSON.stringify({ home_address: '10.0.0.5:7657' }) }); write.answer({}); }
      await settle();
      const active = p.doc.activeElement;
      assert.ok(active.tagName === 'INPUT' && active.value === '10.0.0.5:7657', `${fails ? 'failed' : 'saved'}: focus on the rebuilt choice for the same address (on ${focusOn(p)})`);
      assert.ok(active !== second, 'the choices were rebuilt');
      assert.ok(!busy(active), 'and nothing is busy after');
    }
  });

  await test('the 15 s status poll keeps a focused home address choice, and leaves focus outside network alone', async () => {
    const answers = journal(copy, { 'GET /app/network/api/status': () => TWO('192.168.1.5:7657') });
    const p = await boot(env, '/app/network/', { answers });
    const [, second] = radios(p);
    second.focus();
    p.clock.advance(15000);
    await settle();
    const active = p.doc.activeElement;
    assert.ok(active.tagName === 'INPUT' && active.value === '10.0.0.5:7657' && active !== second, `the rebuilt choice for the same address (on ${focusOn(p)})`);
    const rail = p.q('#app-rail a');
    rail.focus();
    p.clock.advance(30000);
    await settle();
    assert.ok(p.doc.activeElement === rail, `a poll leaves focus on the rail (on ${focusOn(p)})`);
  });

  await test('private link "set up": busy keeps focus, then the status line when the button goes, the button when it fails', async () => {
    for (const fails of [false, true]) {
      let on = false;
      const answers = journal(copy, {
        'GET /app/network/api/status': () => (on ? { posture: 'spl', reachability: 'online', device_addresses: [], vpn: null } : { posture: 'direct', reachability: 'online', device_addresses: [], vpn: null, home_candidates: [] }),
        'GET /app/network/api/private-link': () => (on ? { state: 'enabled', operation: { phase: 'enabled' } } : { state: 'not_enabled' }),
      });
      const p = await boot(env, '/app/network/', { answers });
      p.press(p.byId('link-seg-hosted'));
      p.holds.push((call) => call.url === '/app/network/private-link/enable');
      const setup = p.byId('link-private-link-setup');
      keyPress(p, setup);
      await settle();
      assert.ok(busy(setup) && focusOn(p) === 'BUTTON#link-private-link-setup', `busy and focused while out (on ${focusOn(p)})`);
      p.press(setup);
      assert.ok(p.writes().length === 1, 'a press while busy sends nothing');
      const write = lastCall(p, 'POST', '/app/network/private-link/enable');
      if (fails) write.answer({ detail: 'refused' }, 500);
      else { on = true; write.answer({ operation: { phase: 'enabled' } }); }
      await settle();
      p.clock.advance(1000);
      await settle();
      if (fails) {
        assert.ok(focusOn(p) === 'BUTTON#link-private-link-setup', `a failure leaves focus on "set up" (on ${focusOn(p)})`);
        assert.ok(p.byId('link-private-link-operation').hidden === false, 'with the status line saying why');
      } else {
        assert.ok(p.byId('link-mode-hosted-setup').hidden === true, 'the setup body is gone once the private link is on');
        assert.ok(focusOn(p) === 'DIV#link-private-link-operation', `focus on the status line (on ${focusOn(p)})`);
      }
    }
  });

  await test('private link "turn off": busy keeps focus, then the status line when the body goes, the button when it fails', async () => {
    for (const fails of [false, true]) {
      let on = true;
      const answers = journal(copy, {
        'GET /app/network/api/status': () => (on ? { posture: 'spl', reachability: 'online', device_addresses: [], vpn: null } : { posture: 'direct', reachability: 'online', device_addresses: [], vpn: null, home_candidates: [] }),
        'GET /app/network/api/private-link': () => (on ? { state: 'enabled' } : { state: 'not_enabled' }),
      });
      const p = await boot(env, '/app/network/', { answers });
      p.holds.push((call) => call.url === '/app/network/private-link/disable');
      const off = p.byId('link-private-link-disable');
      keyPress(p, off);
      await settle();
      assert.ok(busy(off) && focusOn(p) === 'BUTTON#link-private-link-disable', `busy and focused while out (on ${focusOn(p)})`);
      const write = lastCall(p, 'POST', '/app/network/private-link/disable');
      if (fails) write.answer({ detail: 'refused' }, 500);
      else { on = false; write.answer({ status: { state: 'not_enabled' } }); }
      await settle();
      if (fails) assert.ok(focusOn(p) === 'BUTTON#link-private-link-disable', `a failure leaves focus on "turn off" (on ${focusOn(p)})`);
      else {
        assert.ok(p.byId('link-mode-hosted-active').hidden === true, 'the private link body is gone');
        assert.ok(focusOn(p) === 'DIV#link-private-link-operation', `focus on the status line (on ${focusOn(p)})`);
      }
    }
  });

  await test('"open to devices on your network": the toggle keeps focus, "your own" takes it when the block goes, and a failure stays', async () => {
    const closed = { open: false, can_change: true, windows_asks: false, listening_on: 'this_pc', agents_on_network: false };
    for (const outcome of ['hidden', 'windows', 'fails']) {
      const local = outcome === 'windows' ? Object.assign({}, closed, { windows_asks: true }) : closed;
      const answers = journal(copy, { 'GET /app/network/api/local-network': local });
      const p = await boot(env, '/app/network/', { answers });
      p.holds.push((call) => call.url === '/app/network/local-network/open');
      const toggle = p.byId('link-local-network-toggle');
      keyPress(p, toggle);
      await settle();
      assert.ok(busy(toggle) && focusOn(p) === 'BUTTON#link-local-network-toggle', `${outcome}: busy and focused while out (on ${focusOn(p)})`);
      p.press(toggle);
      assert.ok(p.writes().length === 1, 'a press while busy sends nothing');
      const write = lastCall(p, 'POST', '/app/network/local-network/open');
      if (outcome === 'fails') write.answer({ detail: 'refused' }, 500);
      else write.answer({ open: true, can_change: true, windows_asks: outcome === 'windows', listening_on: 'local_network', agents_on_network: false });
      await settle();
      const want = outcome === 'hidden' ? 'BUTTON#link-seg-byo' : 'BUTTON#link-local-network-toggle';
      assert.ok(focusOn(p) === want, `${outcome}: focus on ${want} (on ${focusOn(p)})`);
      if (outcome === 'hidden') assert.ok(p.byId('link-local-network').hidden === true, 'the block is gone');
      if (outcome === 'fails') assert.ok(p.byId('link-local-network-error').hidden === false, 'the error is shown');
    }
  });

  const SECOND = Object.assign({}, DELIVERED, { cid: 'sha256:second', display_label: 'pixel' });
  async function listPage(list) {
    const state = { clients: list };
    const answers = journal(copy, {
      'GET /app/network/api/clients': () => ({ clients: state.clients }),
      'DELETE': undefined,
    });
    const p = await boot(env, '/app/network/', { answers });
    p.state = state;
    return p;
  }
  function openDetails(p, key) { p.q(`details[data-client-details="${key}"]`).open = true; }

  await test('a card\'s "unpair" lands on the next card, else a collapsed group, else the devices heading; a failure keeps the button', async () => {
    const runs = [
      { list: [DELIVERED, SECOND, NEVER], gone: DELIVERED.cid, want: 'next card' },
      { list: [DELIVERED, NEVER], gone: DELIVERED.cid, want: 'SUMMARY' },
      { list: [DELIVERED], gone: DELIVERED.cid, want: 'H2#devices' },
      { list: [SECOND, DELIVERED], gone: DELIVERED.cid, want: 'previous card' },
      // The next card sits in the closed "nothing added yet" group: not shown, so the one before.
      { list: [SECOND, DELIVERED, NEVER], gone: DELIVERED.cid, want: 'previous card' },
    ];
    for (const run of runs) {
      const p = await listPage(run.list);
      openDetails(p, run.gone);
      const button = p.q(`.client-card[data-cid="${run.gone}"] [data-network-action="unpair"]`);
      p.state.clients = run.list.filter((c) => c.cid !== run.gone);
      keyPress(p, button);
      await settle();
      const active = p.doc.activeElement;
      if (run.want.endsWith('card')) {
        assert.ok(active.classList?.contains('client-card') && active.dataset.cid === SECOND.cid, `${run.want}: focus on the card for pixel (on ${focusOn(p)} ${active.dataset?.cid || ''})`);
      } else {
        assert.ok(focusOn(p) === run.want, `focus on ${run.want} (on ${focusOn(p)})`);
      }
    }
    const p = await listPage([DELIVERED, SECOND]);
    p.statuses.push([(call) => call.method === 'DELETE', 500]);
    openDetails(p, DELIVERED.cid);
    const button = p.q(`.client-card[data-cid="${DELIVERED.cid}"] [data-network-action="unpair"]`);
    keyPress(p, button);
    await settle();
    assert.ok(p.shown.length === 1, 'a failed unpair shows the shell\'s error');
    assert.ok(p.doc.activeElement === button, `and focus stays on unpair (on ${focusOn(p)})`);
  });

  await test('"forget" keeps focus while out, lands on the nearest card after, and keeps the confirm when it fails', async () => {
    for (const fails of [false, true]) {
      const p = await listPage([DELIVERED, NEVER]);
      openDetails(p, 'group-never');
      p.press(p.q(`.client-forget[data-forget-cid="${NEVER.cid}"] [data-forget-action="ask"]`));
      p.holds.push((call) => /\/forget$/.test(call.url));
      const confirmBtn = p.q(`.client-forget[data-forget-cid="${NEVER.cid}"] [data-forget-action="confirm"]`);
      if (!fails) p.state.clients = [DELIVERED];
      keyPress(p, confirmBtn);
      await settle();
      assert.ok(busy(confirmBtn) && p.doc.activeElement === confirmBtn, `busy and focused while out (on ${focusOn(p)})`);
      p.press(confirmBtn);
      assert.ok(p.writes().length === 1, 'a press while busy sends nothing');
      const write = lastCall(p, 'POST', '/app/network/api/devices/');
      if (fails) write.answer({ detail: 'that device has added material' }, 409); else write.answer({});
      await settle();
      const active = p.doc.activeElement;
      if (fails) {
        assert.ok(active === confirmBtn && !busy(confirmBtn), `a refusal keeps focus on the confirm (on ${focusOn(p)})`);
        assert.ok(p.q(`.client-forget[data-forget-cid="${NEVER.cid}"] .client-forget-status`).hidden === false, 'with its status');
      } else {
        assert.ok(active.classList?.contains('client-card') && active.dataset.cid === DELIVERED.cid, `focus on the nearest card (on ${focusOn(p)})`);
      }
    }
  });

  await test('the clients poll keeps a focused card, and (kept) a card\'s details and forget', async () => {
    const p = await listPage([DELIVERED, SECOND, NEVER]);
    const reFocus = (selector) => { const node = p.q(selector); node.focus(); return node; };
    for (const selector of [`.client-card[data-cid="${SECOND.cid}"]`, `details[data-client-details="${SECOND.cid}"] > summary`]) {
      const before = reFocus(selector);
      p.clock.advance(30000);
      await settle();
      const after = p.q(selector);
      assert.ok(after !== before && p.doc.activeElement === after, `the poll keeps ${selector} (on ${focusOn(p)})`);
    }
    openDetails(p, 'group-never');
    const ask = reFocus(`.client-forget[data-forget-cid="${NEVER.cid}"] [data-forget-action="ask"]`);
    p.clock.advance(30000);
    await settle();
    assert.ok(p.doc.activeElement !== ask && p.doc.activeElement.dataset.forgetAction === 'ask', `the poll keeps forget (on ${focusOn(p)})`);
  });

  async function dialogPage(nonce) {
    const answers = journal(copy, { 'GET /app/network/api/pair/nonce-status': nonce || { present: true, used: false } });
    const p = await boot(env, '/app/network/', { answers });
    p.press(p.q('[data-pairing-action="open"]'));
    await settle();
    return p;
  }

  await test('"check pairing" keeps focus while out, then the outcome\'s own landing', async () => {
    const runs = [
      [{ present: true, used: false }, 200, 'BUTTON#link-pairing-check'],
      [{ present: true, used: true, device: { cid: 'sha256:new', display_label: 'pixel' } }, 200, 'H3#link-pairing-success-heading'],
      [{ present: false, used: false }, 200, 'window closed'],
      [{ detail: 'nope' }, 500, 'unavailable'],
    ];
    for (const [body, status, want] of runs) {
      const p = await dialogPage();
      p.holds.push((call) => call.url.startsWith('/app/network/api/pair/nonce-status'));
      const check = p.byId('link-pairing-check');
      keyPress(p, check);
      await settle();
      assert.ok(busy(check) && p.doc.activeElement === check, `busy and focused while out (on ${focusOn(p)})`);
      p.press(check);
      assert.ok(p.reads('/app/network/api/pair/nonce-status').length === 1, 'a press while busy asks nothing');
      lastCall(p, 'GET', '/app/network/api/pair/nonce-status').answer(body, status);
      await settle();
      const active = p.doc.activeElement;
      if (want === 'window closed') assert.ok(p.byId('link-pairing-window-closed').contains(active) && active.tagName === 'BUTTON', `the window-closed block's button (on ${focusOn(p)})`);
      else if (want === 'unavailable') assert.ok(active.tagName === 'P' && p.byId('link-pairing-unavailable').contains(active), `the unavailable block's message (on ${focusOn(p)})`);
      else assert.ok(focusOn(p) === want, `${want} (on ${focusOn(p)})`);
    }
  });

  await test('the code expiring and a new code keep focus in the dialog: its button, then the title', async () => {
    const p = await dialogPage();
    const copyBtn = p.q('[data-pairing-action="copy"]');
    copyBtn.focus();
    p.clock.advance(125000);
    await settle();
    const expired = p.byId('link-pairing-expired').querySelector('button');
    assert.ok(p.doc.activeElement === expired, `expiry: focus on "show a new one" (on ${focusOn(p)})`);
    p.press(expired);
    await settle();
    assert.ok(focusOn(p) === 'H3#link-pairing-dialog-title', `a new code: focus on the title (on ${focusOn(p)})`);
    assert.ok(p.byId('link-pairing-material').hidden === false, 'and the new code is shown');
  });

  await test('kept: closing the dialog by ×, "done" and Escape still goes back to "pair a device"', async () => {
    for (const how of ['x', 'done', 'escape']) {
      const p = await dialogPage();
      const opener = p.q('[data-pairing-action="open"]');
      const inside = how === 'x' ? p.q('#link-pairing-dialog .modal-close') : p.q('#link-pairing-material [data-pairing-action="close"]');
      inside.focus();
      if (how === 'escape') p.doc.dispatch(p.doc.body, { type: 'keydown', key: 'Escape' });
      else p.press(inside);
      p.clock.advance(16);
      await settle();
      assert.ok(p.byId('link-pairing-dialog').hidden === true, `${how}: the dialog is closed`);
      assert.ok(p.doc.activeElement === opener, `${how}: focus back on "pair a device" (on ${focusOn(p)})`);
    }
  });

  return cases;
};
