// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Speakers on the view contract, under plain Node: the real fragment, mounted
// through the real view contract, read store and router, in a stand-in page
// with events, focus, styles, a history and a clock the cases move by hand.
// speakers_workspace_dom.js runs these after its own cases.

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
  get innerHTML() { return this.childNodes.map(serialize).join(''); }
  get outerHTML() { return serialize(this); }
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
  getClientRects() { return this.checkVisibility() ? [{}] : []; }
  get paused() { return this._paused !== false; }
  play() { this._paused = false; this.fire({ type: 'play' }); return Promise.resolve(); }
  pause() { if (this._paused === false) { this._paused = true; this.fire({ type: 'pause' }); } }
  load() {}
  insertAdjacentHTML(position, html) {
    const holder = new Element(this.ownerDocument, 'div');
    parseInto(this.ownerDocument, holder, String(html));
    const nodes = [...holder.childNodes];
    if (position === 'beforeend') nodes.forEach((n) => this.append(n));
    else if (position === 'afterbegin') this.prepend(...nodes);
    else if (position === 'beforebegin') this.before(...nodes);
    else this.after(...nodes);
  }
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

function escapeText(text) {
  return String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}
function serialize(node) {
  if (node.nodeType === 3) return escapeText(node.data);
  const tag = node.tagName.toLowerCase();
  const attrs = [...node.attrs].map(([k, v]) => (v === '' ? ` ${k}` : ` ${k}="${String(v).replace(/&/g, '&amp;').replace(/"/g, '&quot;')}"`)).join('');
  if (VOID_TAGS.has(tag)) return `<${tag}${attrs}>`;
  return `<${tag}${attrs}>${node.childNodes.map(serialize).join('')}</${tag}>`;
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
    chain.forEach((node) => {
      if (node.fire) node.fire(event);
      const inline = node.getAttribute && node.getAttribute('on' + event.type);
      if (inline && doc.compileHandler) doc.compileHandler(inline).call(node, event);
    });
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


// ---- the journal the cases read

const TODAY = '20261112';
const DAY = '20261110';

function segment(i, day = DAY) {
  const h = String(8 + i).padStart(2, '0');
  return {
    key: `${h}0000_300`, stream: 'mic', stream_layout: 'named', day,
    start: `${h}:00`, end: `${h}:05`, duration: 300, speaker_count: 2,
    sources: ['audio'], attribution_needs_review: 1,
  };
}

function review(day, key) {
  return {
    day, segment_key: key, source: 'audio', audio_file: `/app/speakers/api/audio/${day}/${key}.flac`,
    has_labels: true, summary: { total: 2, confirmed: 0, needs_review: 1 },
    entities: [{ id: 'ada', name: 'Ada' }, { id: 'owner', name: 'Me' }],
    sentences: [
      { id: 1, text: 'first line', start: 0, end: 2, speaker: 'ada', speaker_entity_id: 'ada', speaker_name: 'Ada', method: 'acoustic', confidence: 'medium', needs_review: true },
      { id: 2, text: 'second line', start: 2, end: 4, speaker: 'owner', speaker_name: 'Me', method: 'owner_centroid', confidence: 0.9 },
    ],
  };
}

function journal(copy) {
  return (url, opts) => {
    const u = new URL(url, ORIGIN);
    const p = u.pathname;
    const method = String(opts?.method || 'GET').toUpperCase();
    if (method !== 'GET') return { ok: true, status: 'done' };
    if (p === '/app/speakers/api/state') {
      return {
        speaker_copy: copy, today: TODAY, owner_min_statements: 20,
        owner_status_routing_tokens: { candidate: 'candidate', confirmed: 'confirmed', identity_needed: 'identity_needed' },
        not_in_new_voices_copy: '', speaker_filter_name: u.searchParams.get('speaker') ? 'Ada' : '',
      };
    }
    if (p === '/app/speakers/api/owner/status') return { status: 'confirmed', relabel: null };
    if (p === '/app/speakers/api/discovery/cache') return { status: 'complete', clusters: [], scanned_at: '2026-11-12T10:00:00Z' };
    if (p.startsWith('/app/speakers/api/segments/')) {
      const day = p.split('/').pop();
      return { segments: [segment(0, day), segment(1, day), segment(2, day)], total: 3 };
    }
    if (p.startsWith('/app/speakers/api/speakers/known')) return { speakers: [] };
    if (p.startsWith('/app/speakers/api/speakers/')) return { speakers: [{ id: 'ada', name: 'Ada', count: 1 }] };
    if (p.startsWith('/app/speakers/api/review/')) {
      const parts = p.split('/');
      return review(parts[5], parts[7]);
    }
    if (p === '/app/speakers/api/quality') return { owner: null, speakers: [] };
    if (p === '/app/speakers/api/grid') return null;
    return {};
  };
}

function makePage(env, address = `/app/speakers/${DAY}`, { answers } = {}) {
  let current = new URL(address, ORIGIN);
  const loads = [];
  const location = {
    get origin() { return ORIGIN; },
    get href() { return current.href; },
    set href(url) { loads.push(new URL(url, current.href).href); },
    get pathname() { return current.pathname; },
    get search() { return current.search; },
    get hash() { return current.hash; },
    set hash(h) { loads.push('hash:' + h); },
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
  window.scrollTo = (_x, y) => { window.scrollY = typeof _x === 'object' ? (_x.top || 0) : y; window.fire({ type: 'scroll' }); };
  window.location = location;
  window.document = doc;
  window.setTimeout = clock.setTimeout;
  window.clearTimeout = clock.clearTimeout;
  window.setInterval = clock.setInterval;
  window.clearInterval = clock.clearInterval;
  window.matchMedia = () => ({ matches: false, addEventListener() {}, removeEventListener() {} });
  window.crypto = { getRandomValues: (bytes) => { bytes.fill(9); return bytes; }, randomUUID: () => 'req-1' };
  window.beginViewReady = (view, detail) => {
    let done = false;
    return (state) => { if (!done) { done = true; marks.push(Object.assign({ view, state }, detail)); } };
  };
  window.CONVEY_COPY = { RELOAD_HINT: 'reload to try again.', STILL_WORKING: 'still working…' };
  window.SurfaceState = {
    loading: ({ text = 'loading…' } = {}) => `<div class="surface-state surface-state--loading" role="status"><span class="surface-state-text">${text}</span></div>`,
    error: ({ heading = '', retry } = {}) => `<div class="surface-state surface-state--error" role="alert"><h2 class="surface-state-heading">${heading}</h2>${retry ? '<button type="button" class="surface-state-retry">try again</button>' : ''}</div>`,
    empty: ({ heading = '' } = {}) => `<div class="surface-state surface-state--empty"><h2 class="surface-state-heading">${heading}</h2></div>`,
    serverMessageFrom: (err) => err?.serverMessage || '',
  };
  window.JournalFormat = { day: (key) => `day ${key}`, dayFull: (key) => `day ${key}`, stream: (s) => s, timestamp: (ts) => String(ts) };
  window.JournalClock = { today: () => TODAY, dayKey: () => TODAY, placeNote() {} };
  window.ConveyIcons = { svg: () => '<svg></svg>' };
  window.formatDateShort = (v) => String(v);
  window.RelativeTime = { format: () => 'just now' };
  window.GateDrawer = { render: () => '' };
  window.AppServices = { notifications: { show: () => 0, dismiss() {} }, escapeHtml: (s) => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;') };
  window.logError = (error, context) => { errors.push({ error, context }); };
  window.DateNav = { days: [], setDay(day) { this.days.push(day); } };
  window.DayGrid = { mounts: [], mount(host, config) { this.mounts.push(config.data); }, gate: () => ({ ok: false }), legend() {} };
  doc.inertDialog = null;
  window.ConveyModalLayer = {
    reconciled: 0,
    reconcile() {
      this.reconciled += 1;
      const open = doc.querySelectorAll('.spk-who-backdrop').find((node) => !node.hidden);
      doc.inertDialog = open ? open.querySelector('.spk-who-dialog') : null;
    },
  };
  window.appEvents = { buildChanged: () => page.buildChanged, onConnectionState: () => () => {} };
  window.__SOL_ENTRY_TABLE__ = { views: { speakers: { contract: true, fragment: '/app/speakers/workspace', scripts: [], styles: [] } } };
  window.solShellData = { apps: [{ name: 'speakers', label: 'speakers', workspace_url: '/app/speakers/workspace' }] };

  const serve = answers || journal(env.copy);
  const ask = (url, opts, kind) => new Promise((resolve, reject) => {
    const at = new URL(url, current.href);
    const call = {
      url: at.pathname + at.search,
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

  parseInto(doc, doc.body, '<nav id="app-rail"><a href="/app/home/" data-app-name="home">home</a><a href="/app/speakers/" data-app-name="speakers">speakers</a></nav><main id="main-content" tabindex="-1"></main><div id="view-announcer"></div>');
  const main = doc.getElementById('main-content');

  const context = vm.createContext({
    window, document: doc, location, history: window.history,
    URL, URLSearchParams, AbortController, AbortSignal, DOMException, WeakMap, WeakRef, Map, Set, Promise,
    console, Math, Number, String, Object, Array, Boolean, Date, Intl, JSON, Error, TypeError, RegExp, Uint8Array, Symbol,
    setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval, clearInterval: clock.clearInterval,
    performance: { mark() {}, clearMarks() {} },
    CSS: { escape: (s) => String(s) },
  });
  context.self = window;
  // An inline handler attribute runs as the browser runs it, with `this`.
  doc.compileHandler = (code) => vm.runInContext(`(function (event) {\n${code}\n})`, context);
  const shell = path.join(env.manifestDir, 'assets/static');
  vm.runInContext(fs.readFileSync(path.join(shell, 'store.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'views.js'), 'utf8'), context);
  vm.runInContext(fs.readFileSync(path.join(shell, 'router.js'), 'utf8'), context);
  ['SolStore', 'SolViews', 'solRouter'].forEach((name) => { window[name] = context.window[name] || context[name]; });

  const page = {
    env, window, doc, main, clock, calls, marks, errors, entries, location, loads, context,
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
    loadScripts() {
      vm.runInContext(env.whoIsThis, context, { filename: 'who_is_this.js' });
      vm.runInContext(env.script, context, { filename: 'speakers-workspace.js' });
    },
    async mount() {
      page.ready = [];
      page.view = await window.SolViews.mount('speakers', { reportReady: (s) => page.ready.push(s) });
      await settle();
      return page.view;
    },
    startRouter() { window.solRouter.start('speakers'); },
    // Another view takes the page, as the router's move and place() do.
    leave(next = '/app/home/') {
      window.history.pushState({ solRoute: { id: 'r' + entries.length, owner: 'router' } }, '', next);
      window.SolViews.dispose();
      main.replaceChildren();
      parseInto(doc, main, '<section id="home"><h1>home</h1></section>');
    },
    audio: () => doc.querySelectorAll('audio'),
    playing: () => doc.querySelectorAll('audio').filter((a) => !a.paused),
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
  p.loadScripts();
  p.place();
  await p.mount();
  p.startRouter();
  await settle();
  return p;
}

function focusIsSomewhere(p, label) {
  const active = p.doc.activeElement;
  assert.ok(active && active !== p.doc.body, `${label}: focus is never on the body`);
}

function segmentItems(p) {
  return p.byId('spkSegmentList').querySelectorAll('.spk-segment');
}

// Run from speakers_workspace_dom.js: answers with the number of cases.
module.exports = async function speakersContractCases(manifestDir) {
  const workspace = fs.readFileSync(path.join(manifestDir, 'assets/speakers/workspace.html'), 'utf8');
  const scriptStart = workspace.lastIndexOf('<script>');
  const script = workspace.slice(scriptStart + '<script>'.length, workspace.lastIndexOf('</script>'));
  const markup = workspace.slice(0, workspace.indexOf('<script src="/app/speakers/static/who_is_this.js">'))
    .replace(/<style>[\s\S]*?<\/style>/g, '')
    .replace(/<script[^>]*><\/script>/g, '')
    .replace(/<link[^>]*>/g, '');
  const whoIsThis = fs.readFileSync(path.join(manifestDir, 'assets/speakers/who_is_this.js'), 'utf8');
  const copy = JSON.parse(fs.readFileSync(path.join(manifestDir, 'assets/speakers/copy.json'), 'utf8'));
  const env = { manifestDir, script, markup, whoIsThis, copy };

  // ---- AC 1: a contract view, in both modes

  await test('the script only registers: nothing reads, draws, listens or times at load', async () => {
    const p = makePage(env);
    p.place();
    const keys = p.windowKeys();
    const listeners = [p.window.listenerCount(), p.doc.listenerCount()];
    p.loadScripts();
    assert.strictEqual(p.calls.length, 0, 'no read at load');
    assert.deepStrictEqual([...p.windowKeys().filter((k) => !keys.includes(k))], ['SpeakersWhoIsThis'], 'only the stateless namespace is added');
    assert.deepStrictEqual([p.window.listenerCount(), p.doc.listenerCount()], listeners, 'no listener at load');
    assert.strictEqual(p.clock.live(), 0, 'no timer at load');
    assert.strictEqual(p.window.SolViews.registered('speakers'), true);
    assert.strictEqual(p.byId('speakersDayView').hidden, true, 'nothing is drawn');
    assert.strictEqual(p.byId('speakersOverviewView').hidden, true, 'nothing is drawn');
  });

  for (const [mode, address] of [['a day', `/app/speakers/${DAY}`], ['the overview', '/app/speakers/']]) {
    await test(`${mode}: a mount reads the state once through the store, and draws`, async () => {
      const p = await boot(env, address);
      assert.strictEqual(p.reads('/app/speakers/api/state').length, 1, 'the state is read once');
      assert.ok(p.calls.every((c) => c.url.startsWith('/app/speakers/api/')), 'every read is an absolute address');
      assert.strictEqual(p.errors.length, 0, JSON.stringify(p.errors.map((e) => String(e.error && e.error.stack))));
      if (address.endsWith('/')) {
        assert.strictEqual(p.byId('speakersOverviewView').hidden, false, 'the overview is drawn');
        assert.strictEqual(p.byId('speakersDayView').hidden, true, 'the day is not');
      } else {
        assert.strictEqual(p.byId('speakersDayView').hidden, false, 'the day is drawn');
        assert.strictEqual(segmentItems(p).length, 3, 'the day\'s segments are drawn');
        assert.ok(p.reads(`/app/speakers/api/segments/${DAY}`).length === 1, 'the day\'s segments are read once');
      }
      assert.ok(p.ready.length >= 1, 'it reports ready');
    });

    await test(`${mode}: dispose leaves no listener, timer, read or global, and a second mount in the same document works`, async () => {
      const p = makePage(env, address);
      p.loadScripts();
      p.place();
      const keys = p.windowKeys();
      const counts = [p.window.listenerCount(), p.doc.listenerCount()];
      await p.mount();
      p.startRouter();
      const routerCounts = [p.window.listenerCount(), p.doc.listenerCount()];
      const view = p.view;
      p.window.SolViews.dispose();
      await settle();
      const held = view.ctx.held();
      assert.deepStrictEqual(JSON.parse(JSON.stringify(held)), { listeners: 0, timeouts: 0, intervals: 0, observers: 0, undo: 0, bodyClasses: 0, reads: 0 });
      assert.strictEqual(p.clock.live(), 0, 'no timer stays');
      assert.deepStrictEqual(p.windowKeys(), keys, 'no global stays');
      assert.ok(p.window.listenerCount() <= routerCounts[0] && p.doc.listenerCount() <= routerCounts[1], 'no listener stays');
      assert.strictEqual(p.doc.querySelectorAll('.spk-who-backdrop').length, 0, 'no sheet stays on the page');
      void counts;
      // Again, in the same document.
      const before = p.calls.length;
      p.place();
      await p.mount();
      assert.strictEqual(p.reads('/app/speakers/api/state').length, 2, 'the second mount reads its own state');
      assert.ok(p.calls.length > before, 'and draws again');
      assert.strictEqual(p.errors.length, 0, JSON.stringify(p.errors.map((e) => String(e.error && e.error.stack))));
    });
  }

  function openSegment(p, i = 0) {
    segmentItems(p)[i].click();
  }

  await test('a day: audio that plays stops when the view goes, and no element of it is left reachable from the page', async () => {
    const p = await boot(env);
    openSegment(p, 0);
    await settle();
    const player = p.byId('spkAudio');
    assert.ok(player, 'the sentence player is drawn');
    player.play();
    assert.strictEqual(p.playing().length, 1, 'it plays');
    p.leave();
    await settle();
    assert.strictEqual(player.paused, true, 'leaving pauses it');
    assert.strictEqual(p.audio().length, 0, 'no audio element is left on the page');
  });

  await test('a day: opening a segment is one own entry; back and forward restore it, and an entry never scrolled comes back at its top', async () => {
    const p = await boot(env);
    const length = p.entries.length;
    openSegment(p, 1);
    await settle();
    assert.strictEqual(p.entries.length, length + 1, 'one entry');
    assert.ok(p.location.hash.length > 1, 'the address names the segment');
    assert.strictEqual(p.loads.length, 0, 'no page load');
    const first = p.location.href;
    p.window.scrollTo(0, 640);
    openSegment(p, 2);
    await settle();
    const second = p.location.href;
    assert.notStrictEqual(first, second);
    p.window.scrollTo(0, 120);
    p.window.history.back();
    await settle();
    assert.strictEqual(p.location.href, first);
    assert.strictEqual(segmentItems(p)[1].getAttribute('aria-selected'), 'true', 'back selects the segment it named');
    assert.strictEqual(p.window.scrollY, 640, 'at the scroll it was left at');
    p.window.history.back();
    await settle();
    assert.strictEqual(p.window.scrollY, 0, 'the day\'s first entry was never scrolled: back is at its top');
    p.window.history.forward();
    p.window.history.forward();
    await settle();
    assert.strictEqual(p.location.href, second);
    assert.strictEqual(segmentItems(p)[2].getAttribute('aria-selected'), 'true', 'forward selects its segment');
    assert.strictEqual(p.window.scrollY, 120);
    focusIsSomewhere(p, 'after back and forward');
    assert.strictEqual(p.loads.length, 0, 'still no page load');
  });

  await test('a day step is the view\'s update: every read after it is the new day\'s or day-free, nothing is re-read that the day doesn\'t need, and a late answer for the old day draws nothing', async () => {
    const p = await boot(env);
    openSegment(p, 0);
    await settle();
    const NEXT = '20261111';
    // The old day's review is still out when the owner steps.
    p.holds.push((call) => call.url.startsWith(`/app/speakers/api/review/${DAY}`));
    openSegment(p, 1);
    await settle();
    segmentItems(p)[1].focus();
    const late = p.calls.find((c) => c.url.startsWith(`/app/speakers/api/review/${DAY}`) && !c.settled);
    assert.ok(late, 'a review read is out');
    const view = p.view;
    const mark = p.calls.length;
    const moved = p.window.solRouter.navigate(`/app/speakers/${NEXT}`);
    assert.strictEqual(moved, true, 'the router takes the step');
    await settle();
    assert.strictEqual(p.window.SolViews.current(), view, 'the same mount, not a new one');
    assert.deepStrictEqual([...p.window.DateNav.days], [NEXT], 'the date control follows');
    const after = p.calls.slice(mark);
    assert.ok(after.length > 0);
    after.forEach((c) => assert.ok(!c.url.includes(DAY), `${c.url} doesn't name the old day`));
    assert.ok(after.some((c) => c.url.startsWith(`/app/speakers/api/segments/${NEXT}`)), 'the new day\'s segments are read');
    assert.strictEqual(after.filter((c) => c.url.startsWith('/app/speakers/api/state')).length, 0, 'the state is not read again');
    assert.strictEqual(late.aborted, true, 'the old day\'s read is aborted');
    late.answer(review(DAY, '090000_300'));
    await settle();
    assert.strictEqual(p.q('.spk-detail-title'), null, 'the old day draws nothing into the new one');
    assert.strictEqual(segmentItems(p).length, 3);
    assert.ok(segmentItems(p).every((li) => li.getAttribute('aria-selected') === 'false'), 'no segment is open on the new day');
    assert.ok(p.marks.some((m) => m.update && m.state), 'the step reports ready');
    focusIsSomewhere(p, 'a step from a segment that had focus');
    assert.strictEqual(p.doc.activeElement, p.q('.spk-segments-header'), 'focus is on the new day\'s heading');
    assert.strictEqual(p.loads.length, 0, 'no page load');
    assert.strictEqual(p.errors.length, 0, JSON.stringify(p.errors.map((e) => String(e.error && e.error.stack))));
  });

  await test('a day step ends the day before: its audio stops, and no listener or timer of it is left', async () => {
    const p = await boot(env);
    const view = p.view;
    const baseline = view.ctx.held();
    openSegment(p, 0);
    await settle();
    const player = p.byId('spkAudio');
    player.play();
    for (const step of ['20261111', '20261109', '20261108']) {
      p.window.solRouter.navigate(`/app/speakers/${step}`);
      await settle();
    }
    assert.strictEqual(player.paused, true, 'the old day\'s audio stops');
    const held = view.ctx.held();
    assert.strictEqual(held.listeners, baseline.listeners, 'listeners stay level across three steps');
    assert.strictEqual(held.timeouts, baseline.timeouts, 'timers stay level');
    assert.strictEqual(p.window.listenerCount(), p.window.listenerCount(), '');
  });
  // A journal with one unnamed voice waiting, and one person to name it.
  function withVoice(extra = {}) {
    const base = journal(env.copy);
    return (url, opts) => {
      const p = new URL(url, ORIGIN).pathname;
      const method = String(opts?.method || 'GET').toUpperCase();
      if (extra[p] && extra[p][method]) return extra[p][method](url, opts);
      if (p === '/app/speakers/api/discovery/cache') {
        return { status: 'complete', clusters: [{ cluster_id: 'c1', size: 3, segment_count: 2, samples: [{ text: 'hello there', audio_url: '/app/speakers/api/discovery/sample/c1/0.flac' }] }] };
      }
      if (p === '/app/speakers/api/discovery/cluster/c1/presence') {
        return { evidence_complete: true, cluster_id: 'c1', samples: [{ day: DAY, stream: 'mic', audio_url: '/app/speakers/api/discovery/sample/c1/0.flac' }], candidates: [] };
      }
      if (p === '/app/speakers/api/people/search') return { people: [{ entity_id: 'grace', name: 'Grace Ito' }] };
      return base(url, opts);
    };
  }

  function sheetOpen(p) {
    const backdrop = p.q('.spk-who-backdrop');
    return !!backdrop && !backdrop.hidden;
  }

  async function openSheet(p) {
    const trigger = p.q('.spk-who-trigger');
    assert.ok(trigger, 'the voice has a who-is-this button');
    trigger.click();
    await settle();
    assert.ok(sheetOpen(p), 'the sheet opens');
    p.window.ConveyModalLayer.reconcile();
  }

  await test('the who-is-this sheet: its search timer, its listeners and the sheet itself go with the view', async () => {
    const p = await boot(env, `/app/speakers/${DAY}`, { answers: withVoice() });
    await openSheet(p);
    const input = p.q('.spk-who-search-input');
    assert.ok(input, 'the sheet has its search');
    input.value = 'gra';
    p.doc.dispatch(input, { type: 'input' });
    const timers = p.clock.live();
    assert.ok(timers > 0, 'the search waits on a timer');
    const sheet = p.q('.spk-who-backdrop');
    p.leave();
    await settle();
    assert.strictEqual(p.q('.spk-who-backdrop'), null, 'the sheet is gone from the page');
    assert.strictEqual(sheet.isConnected, false);
    assert.strictEqual(p.clock.live(), 0, 'its timer is gone');
    assert.strictEqual(p.doc.inertDialog, null, 'the page is not left inert');
    p.clock.advance(1000);
    await settle();
    assert.strictEqual(p.reads('/app/speakers/api/people/search').length, 0, 'the search never runs');
  });

  await test('naming a voice in flight across leaving: the write is never cut, and its answer draws, reads and focuses nothing', async () => {
    const p = await boot(env, `/app/speakers/${DAY}`, { answers: withVoice() });
    await openSheet(p);
    const input = p.q('.spk-who-search-input');
    input.value = 'grace';
    p.doc.dispatch(input, { type: 'input' });
    p.clock.advance(500);
    await settle();
    const person = p.q('.spk-who-person-action');
    assert.ok(person, 'the search offers the person');
    person.click();
    await settle();
    p.holds.push((call) => call.url === '/app/speakers/api/discovery/identify');
    const confirm = p.q('.spk-who-confirm');
    if (confirm) { confirm.click(); await settle(); }
    const write = p.calls.find((c) => c.url === '/app/speakers/api/discovery/identify' && !c.settled);
    assert.ok(write, 'naming sends its write');
    p.leave();
    await settle();
    const mark = p.calls.length;
    assert.strictEqual(write.aborted, false, 'leaving never cuts a write');
    write.answer({ status: 'identified', cluster_id: 'c1', entity_id: 'grace', name: 'Grace Ito', request_id: 'req-1' });
    await settle();
    assert.strictEqual(p.calls.length, mark, 'its answer reads nothing (no segments, no review)');
    assert.ok(p.q('#home'), 'the next view is untouched');
    assert.strictEqual(p.q('.spk-who-backdrop'), null, 'no sheet comes back');
    assert.notStrictEqual(p.doc.activeElement && p.doc.activeElement.closest && p.doc.activeElement.closest('.spk-who-dialog'), true);
    assert.strictEqual(p.errors.length, 0, JSON.stringify(p.errors.map((e) => String(e.error && e.error.stack))));
  });

  await test('a back with the who-is-this sheet open closes it, lets the page stop being inert, and places focus', async () => {
    const p = await boot(env, `/app/speakers/${DAY}`, { answers: withVoice() });
    segmentItems(p)[0].click();
    await settle();
    await openSheet(p);
    const inSheet = p.q('.spk-who-search-input') || p.q('.spk-who-dialog');
    inSheet.focus();
    assert.ok(p.q('.spk-who-dialog').contains(p.doc.activeElement), 'focus is in the sheet');
    const reconciled = p.window.ConveyModalLayer.reconciled;
    p.window.history.back();
    await settle();
    assert.strictEqual(sheetOpen(p), false, 'the sheet is closed');
    assert.ok(p.window.ConveyModalLayer.reconciled > reconciled, 'the modal layer reconciles at once');
    assert.strictEqual(p.doc.inertDialog, null, 'nothing is inert');
    focusIsSomewhere(p, 'after the back');
    assert.ok(!p.q('.spk-who-dialog') || !p.q('.spk-who-dialog').contains(p.doc.activeElement), 'focus is out of the sheet');
  });

  // ---- the overview's 5 s poll

  function relabelling(state = 'running') {
    const base = journal(env.copy);
    return (url, opts) => {
      if (new URL(url, ORIGIN).pathname === '/app/speakers/api/owner/status') {
        return { status: 'confirmed', relabel: { state, done: 1, total: 4 }, entity_id: 'owner', name: 'Me' };
      }
      return base(url, opts);
    };
  }

  await test('the overview: the 5 s poll stops when the view goes, and a poll answer that arrives late draws nothing', async () => {
    const p = await boot(env, '/app/speakers/', { answers: relabelling() });
    const first = p.reads('/app/speakers/api/owner/status').length;
    assert.strictEqual(first, 1);
    p.clock.advance(5000);
    await settle();
    assert.strictEqual(p.reads('/app/speakers/api/owner/status').length, 2, 'the poll reads again at 5 s');
    // The next poll is out when the owner leaves.
    p.holds.push((call) => call.url === '/app/speakers/api/owner/status');
    p.clock.advance(5000);
    await settle();
    const late = p.calls.filter((c) => c.url === '/app/speakers/api/owner/status').pop();
    assert.strictEqual(late.settled, false, 'a poll is in flight');
    const panel = p.byId('spkOverviewOwner');
    const drawn = panel.innerHTML;
    p.leave();
    await settle();
    assert.strictEqual(late.aborted, true, 'the poll is aborted');
    late.answer({ status: 'confirmed', relabel: { state: 'running', done: 2, total: 4 } });
    await settle();
    assert.strictEqual(panel.innerHTML, drawn, 'nothing is drawn');
    assert.strictEqual(p.clock.live(), 0, 'no poll is armed');
    p.clock.advance(20000);
    await settle();
    assert.strictEqual(p.reads('/app/speakers/api/owner/status').length, 3, 'and none runs');
  });

  await test('the overview: an owner write in flight across leaving finishes, and its answer starts no read and no poll', async () => {
    const p = await boot(env, '/app/speakers/', { answers: relabelling('done') });
    const undo = p.byId('spkOwnerUndo');
    assert.ok(undo, 'a saved voice offers undo');
    undo.click();
    await settle();
    const yes = p.byId('spkOwnerUndoConfirm');
    assert.ok(yes, 'undo asks first');
    p.holds.push((call) => call.url === '/app/speakers/api/owner/undo');
    yes.click();
    await settle();
    const write = p.calls.find((c) => c.url === '/app/speakers/api/owner/undo');
    assert.ok(write && !write.settled, 'the write is out');
    p.leave();
    await settle();
    const mark = p.calls.length;
    write.answer({ ok: true });
    await settle();
    assert.strictEqual(write.aborted, false, 'never cut');
    assert.strictEqual(p.calls.length, mark, 'its answer reads nothing');
    assert.strictEqual(p.clock.live(), 0, 'and arms no poll');
  });

  await test('a mount that throws part-way takes back what it added', async () => {
    const p = makePage(env);
    p.loadScripts();
    p.place();
    const counts = [p.window.listenerCount(), p.doc.listenerCount()];
    p.window.ConveyIcons = { svg() { throw new Error('no icons'); } };
    await assert.rejects(() => p.window.SolViews.mount('speakers', { reportReady() {} }), /no icons/);
    await settle();
    assert.deepStrictEqual([p.window.listenerCount(), p.doc.listenerCount()], counts, 'no listener stays');
    assert.strictEqual(p.clock.live(), 0, 'no timer stays');
    const state = p.calls.find((c) => c.url === '/app/speakers/api/state');
    assert.ok(state && (state.aborted || state.settled), 'the state read is aborted or done');
    assert.strictEqual(p.window.SolViews.current(), null);
    p.window.ConveyIcons = { svg: () => '<svg></svg>' };
    p.place();
    await p.mount();
    assert.strictEqual(segmentItems(p).length, 3, 'a second try mounts clean');
  });
  // ---- arrivals with a hash or a query, in the page

  async function arrive(address, options) {
    const p = makePage(env, address, options);
    p.loadScripts();
    p.startRouter();
    p.place();
    await p.mount();
    p.clock.advance(0);
    await settle();
    return p;
  }

  await test('an in-page arrival at #owner-teach starts the handoff and focuses the owner banner after the router has placed its focus', async () => {
    const p = makePage(env, `/app/speakers/${TODAY}#owner-teach`);
    p.loadScripts();
    p.startRouter();
    p.place();
    p.holds.push((call) => call.url.startsWith('/app/speakers/api/segments/'));
    await p.mount();
    const banner = p.byId('spkOwnerBanner');
    assert.notStrictEqual(p.doc.activeElement, banner, 'not before the view reports ready');
    p.calls.filter((c) => c.url.startsWith('/app/speakers/api/segments/')).forEach((c) => c.answer({ segments: [segment(0, TODAY)], total: 1 }));
    await settle();
    p.clock.advance(0);
    await settle();
    assert.strictEqual(p.doc.activeElement, banner, 'the banner has focus, as on a full load');
    assert.ok(banner.textContent.length > 0, 'the banner says what to do');
    assert.strictEqual(p.loads.length, 0);
  });

  await test('an in-page arrival at ?speaker=<id> filters the day by that voice, as a full load does', async () => {
    const p = await arrive(`/app/speakers/${TODAY}?speaker=ada`);
    assert.strictEqual(p.reads('/app/speakers/api/state?speaker=ada').length, 1, 'the state names the voice');
    const segs = p.reads('/app/speakers/api/segments/');
    assert.ok(segs.length >= 1 && segs.every((c) => new URL(c.url, ORIGIN).searchParams.get('speaker') === 'ada'), 'the segments are that voice\'s');
    assert.ok(p.byId('spkFilterIndicator').textContent.includes('Ada'), 'the filter is shown');
  });

  await test('an in-page arrival at the overview\'s #your-voice focuses its your-voice section', async () => {
    const p = await arrive('/app/speakers/#your-voice');
    assert.strictEqual(p.doc.activeElement, p.byId('spkOverviewYourVoiceSection'));
  });

  await test('the overview clears a voice handoff from the address with a replace that keeps the entry its own', async () => {
    const p = await arrive('/app/speakers/?voice_cluster_id=c1', { answers: withVoice() });
    assert.ok(sheetOpen(p), 'the handed-off voice opens');
    const id = p.window.history.state.solRoute.id;
    const length = p.entries.length;
    p.doc.dispatch(p.q('.spk-who-dialog'), { type: 'keydown', key: 'Escape' });
    await settle();
    assert.strictEqual(p.location.search, '', 'the query is cleared');
    assert.strictEqual(p.entries.length, length, 'with a replace, not a new entry');
    assert.strictEqual(p.window.history.state.solRoute.id, id, 'the entry keeps its id');
  });

  await test('owner teach from another day is a day step to today in the page, not a page load', async () => {
    const p = await boot(env);
    const view = p.view;
    p.window.history.pushState(null, '', `/app/speakers/${DAY}#owner-teach`);
    p.window.fire({ type: 'hashchange' });
    await settle();
    assert.strictEqual(p.loads.length, 0, 'no page load');
    assert.strictEqual(p.location.pathname, `/app/speakers/${TODAY}`);
    assert.strictEqual(p.location.hash, '#owner-teach');
    assert.strictEqual(p.window.SolViews.current(), view, 'the same mount');
    assert.deepStrictEqual([...p.window.DateNav.days], [TODAY]);
    assert.ok(p.byId('spkOwnerBanner').textContent.length > 0, 'today starts the handoff');
  });

  await test('the status line\'s dismiss button still clears it after an in-page mount', async () => {
    const p = await arrive(`/app/speakers/${DAY}`);
    segmentItems(p)[0].click();
    await settle();
    const confirm = p.q('.spk-confirm-btn');
    assert.ok(confirm, 'a line can be confirmed');
    p.holds.push((call) => call.url === '/app/speakers/api/confirm-attribution');
    confirm.click();
    await settle();
    p.calls.find((c) => c.url === '/app/speakers/api/confirm-attribution').answer({ error: 'refused' }, 500);
    await settle();
    const dismiss = p.q('.spk-status-dismiss');
    assert.ok(dismiss, 'the status line is shown');
    const container = dismiss.closest('.spk-status-container');
    let thrown = null;
    try { dismiss.click(); } catch (error) { thrown = error; }
    assert.strictEqual(container.querySelector('.spk-status'), null, 'the dismiss clears it');
    // The handler's second `closest` runs on the button it has just taken
    // out of the page, as it does at the base. Recorded, not changed here.
    assert.ok(thrown === null || /null/.test(String(thrown && thrown.message)), String(thrown));
  });

  await test('a status line\'s timers end with the view: a confirmed line\'s fade is never run after leaving', async () => {
    const p = await boot(env);
    segmentItems(p)[0].click();
    await settle();
    // The segments read after the write would redraw the detail: hold it.
    p.holds.push((call) => call.url.startsWith('/app/speakers/api/segments/'));
    p.q('.spk-confirm-btn').click();
    await settle();
    assert.ok(p.q('.spk-status-success'), 'the line says it is confirmed');
    assert.ok(p.view.ctx.held().timeouts >= 1, 'its fade waits on a timer of the mount');
    p.leave();
    await settle();
    assert.strictEqual(p.clock.live(), 0, 'no timer is left');
  });
  //@@CASES5
  return cases;
};
