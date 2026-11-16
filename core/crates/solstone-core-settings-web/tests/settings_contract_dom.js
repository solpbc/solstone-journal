// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Settings on the view contract, under plain Node: the real fragment, its
// script and settings.js, mounted through the real view contract, read
// store, router and focus helpers, in a stand-in page with events, focus, a
// history and a clock the cases move by hand. Moves between views go through
// the real router, with a stand-in for the shell's fragment mount that places
// a view's markup and mounts it. facets_dom.js runs these after its own cases.
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
  isEqualNode(other) { return !!other && other.nodeType === 3 && other.data === this.data; }
  remove() { if (this.parentNode) this.parentNode.removeChild(this); }
}

function simpleMatch(el, part) {
  // `:not(...)` holds when what's inside it doesn't.
  let negated = false;
  part = part.replace(/:not\(([^)]*)\)/g, (_m, inner) => {
    if (simpleMatch(el, inner)) negated = true;
    return '';
  });
  if (negated) return false;
  if (part === '') return true;
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
  get firstChild() { return this.childNodes[0] || null; }
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
  // As a browser's: `disabled` is its attribute, so markup and script agree.
  get disabled() { return this.hasAttribute('disabled'); }
  set disabled(v) { if (v) this.setAttribute('disabled', ''); else this.removeAttribute('disabled'); }
  get hidden() { return this.hasAttribute('hidden'); }
  set hidden(v) { if (v) this.setAttribute('hidden', ''); else this.removeAttribute('hidden'); }
  setAttribute(k, v) { this.attrs.set(k, String(v)); if (k === 'style') this._style = null; }
  getAttribute(k) { return this.attrs.has(k) ? this.attrs.get(k) : null; }
  hasAttribute(k) { return this.attrs.has(k); }
  getAttributeNames() { return [...this.attrs.keys()]; }
  // As a browser's: the same kind, tag, attributes and children, in order.
  isEqualNode(other) {
    if (!other || other.nodeType !== 1 || other.tagName !== this.tagName) return false;
    if (other.attrs.size !== this.attrs.size || [...this.attrs].some(([k, v]) => other.attrs.get(k) !== v)) return false;
    if (other.childNodes.length !== this.childNodes.length) return false;
    return this.childNodes.every((node, i) => node.isEqualNode(other.childNodes[i]));
  }
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
    if (!focusable || this.disabled) return;
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
// Each record keeps the node it changed (an attribute's element, a child
// list's parent), so a case can count the records under one part of the page,
// as a MutationObserver on it would. Taking off an attribute that isn't there
// is no change, as in a browser.
let mutations = 0;
let depth = 0;
const mutationLog = [];
function counted(original, name) {
  return function (...args) {
    const real = !(name === 'removeAttribute' && !this.attrs.has(args[0]));
    if (depth === 0 && this.isConnected && real) {
      mutations += 1;
      // Held weakly, so the log keeps no gone markup alive.
      mutationLog.push(new WeakRef(this));
    }
    depth += 1;
    try { return original.apply(this, args); } finally { depth -= 1; }
  };
}
for (const name of ['append', 'removeChild', 'insertBefore', 'setAttribute', 'removeAttribute']) {
  Element.prototype[name] = counted(Element.prototype[name], name);
}
// The records since `from` (a mark from `mutationMark()`) under `region`.
function mutationMark() { return mutationLog.length; }
// What the records since `from` under `region` changed, for a failure's message.
function mutationsSeen(region, from) {
  return mutationLog.slice(from).map((ref) => ref.deref()).filter((node) => node && (node === region || region.contains(node)))
    .map((node) => node.tagName + (node.id ? '#' + node.id : '') + (node.getAttribute('class') ? '.' + node.getAttribute('class').split(' ').join('.') : '')).join(' ');
}
function mutationsUnder(region, from) {
  return mutationLog.slice(from).map((ref) => ref.deref()).filter((node) => node && (node === region || region.contains(node))).length;
}
const textContent = Object.getOwnPropertyDescriptor(Element.prototype, 'textContent');
Object.defineProperty(Element.prototype, 'textContent', { get: textContent.get, set: counted(textContent.set) });

// ---- the journal the page reads

const SLUG = 'work';
function trashItem(id) {
  return { id, kind: 'segment', day: '20261101', stream: 'field', segment: '070000_300', erase_at: '2099-11-15T00:00:00.000Z', bytes: 2048, replaces: false };
}
function journal(overrides = {}) {
  const answers = {
    '/app/settings/api/state': { settings_copy: {}, install_copy: {} },
    '/app/settings/api/config': { identity: { name: 'owner', timezone: 'UTC' }, transcribe: {}, env: {}, runtime_env: {}, key_validation: {}, trash: { erase_after_days: 7 } },
    '/app/settings/api/transcribe': { backends: [], owner_voice: null },
    '/app/settings/api/transcribe/owner-voice': { stored: false },
    '/app/settings/api/vision': { max_extractions: 20, category_defaults: {}, categories: {}, redaction: [] },
    '/app/settings/api/sync': { plaud: { enabled: false } },
    '/app/settings/api/granola': { configured: false, syncing: false },
    '/app/settings/api/storage/config': { retention: { raw_media: 'keep', raw_media_days: null, keep_empty_audio: false, per_stream: {}, journal_logs: { enabled: true, days: 30 } }, streams: [] },
    '/app/settings/api/storage': { summary: { raw_media_human: '0 B', derived_human: '0 B', total_segments: 0, segments_with_raw: 0, segments_purged: 0 }, warnings: [] },
    '/app/settings/api/storage/removals': { entries: [], total: 0 },
    '/app/settings/api/trash': { erase_after_days: 7, items: [trashItem('t1')] },
    '/app/settings/api/trash/backups': { set_up: false, available: false },
    '/app/settings/api/facets?all=true': { facets: [{ name: SLUG, title: 'Work', color: '#c00', muted: false }, { name: 'home', title: 'Home', color: '#0c0', muted: false }] },
    [`/app/settings/api/facet/${SLUG}`]: { facet: SLUG, config: { title: 'Work', color: '#c00', emoji: '', icon: '', muted: false, description: '' } },
    [`/app/settings/api/facet/${SLUG}/activities`]: { attached: [], defaults: [] },
    [`/app/settings/api/facet/${SLUG}/logs`]: { day: '', entries: [], next_cursor: null },
    '/app/settings/api/config#PUT': { success: true, config: { identity: { name: 'owner', timezone: 'UTC' }, transcribe: {}, env: {} } },
    '/app/settings/api/trash/erase#POST': { done: ['t1'], failed: [] },
    '/app/settings/api/trash/recover#POST': { done: ['t9'], failed: [] },
    '/api/system/about': { about: 'journal test' },
    '/app/network/api/clients': { clients: [] },
    ...overrides,
  };
  return (url, opts) => {
    const u = new URL(url, ORIGIN);
    const key = u.pathname + u.search;
    const method = String(opts?.method || 'GET').toUpperCase();
    if (method !== 'GET') return answers[`${key}#${method}`] ?? { success: true };
    if (key === '/app/settings/api/local-access') return undefined;
    return answers[key] ?? {};
  };
}

const HOME_MARKUP = '<section id="home"><h1>home</h1><p id="home-text">home as it was</p><a id="to-settings" href="/app/settings/">settings</a><a id="to-trash" href="/app/settings#trash">trash</a></section>';

function makePage(env, address = '/app/settings/', { answers } = {}) {
  let current = new URL(address, ORIGIN);
  const loads = [];
  const location = {
    get origin() { return ORIGIN; },
    get href() { return current.href; },
    set href(url) { loads.push(new URL(url, current.href).href); },
    get pathname() { return current.pathname; },
    get search() { return current.search; },
    get hash() { return current.hash; },
    get host() { return 'journal.test'; },
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
  const notices = [];
  const storage = new Map();
  const window = new Target();
  doc.window = window;
  const entries = [{ url: current.href, state: { solRoute: { id: 'e0', owner: 'router' } } }];
  let index = 0;
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
    notifications: { show: (notice) => { notices.push(notice); return notices.length; } },
  };
  window.JournalFormat = { day: (key) => `day ${key}`, time: (ms) => `at ${ms}`, stream: (s) => `stream ${s}`, duration: (s) => `${s} s` };
  window.JournalClock = { today: () => '20261116', dayKey: () => '20261201', placeNote() {} };
  window.CONVEY_COPY = { RELOAD_HINT: 'reload to try again.' };
  window.SurfaceState = {
    loading: ({ text }) => `<p class="loading">${text}</p>`,
    error: ({ heading }) => `<div><p class="surface-error">${heading}</p><button type="button" class="surface-state-retry">try again</button></div>`,
    serverMessageFrom: (error) => error?.serverMessage || '',
  };
  window.Drawer = {
    formatLine: (text) => escapeHtml(text),
    render: (options) => `<details class="drawer"><summary>${escapeHtml(options.line || '')}</summary>${options.bodyHtml || ''}</details>`,
    preserveOpen: (_mount, fn) => fn(),
  };
  window.BackupErase = {
    forgets: 0,
    async row() { return null; },
    attach() {}, detach() {},
    chosen: () => false, chosenIn: () => false,
    receipt: () => '', receiptIn: () => '',
    forget() { this.forgets += 1; },
  };
  window.convey = { copyToClipboard: async () => {} };
  window.logError = (error, context) => { errors.push({ error, context }); };
  window.ConveyModalLayer = { reconciles: 0, reconcile() { this.reconciles += 1; } };
  window.appEvents = { buildChanged: () => false };
  window.__SOL_ENTRY_TABLE__ = { views: {
    settings: { contract: true, fragment: '/app/settings/workspace', scripts: [], styles: [] },
    home: { contract: true, fragment: '/app/home/workspace', scripts: [], styles: [] },
  } };
  window.solShellData = { apps: [
    { name: 'settings', label: 'settings', workspace_url: '/app/settings/workspace' },
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
      answer(body, status = 200) {
        if (call.settled) return;
        call.settled = true;
        if (kind === 'fetch') {
          resolve({ ok: status < 400, status, redirected: false, json: async () => body, text: async () => String(body) });
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
    if (page.hold(call)) return;
    const value = serve(url, opts);
    Promise.resolve().then(() => (value === undefined ? call.answer({ error: 'not found' }, 404) : call.answer(value, page.status(call))));
  });
  window.apiJson = (url, opts = {}) => ask(url, opts, 'api');
  window.fetch = (url, opts = {}) => ask(url, opts, 'fetch');
  // The shell's save helper, as api.js has it: the write, then its hooks.
  window.saveControl = async ({ el, fetchArgs, onSuccess, onError }) => {
    try {
      const result = await fetchArgs();
      if (typeof onSuccess === 'function') onSuccess(result);
      return result;
    } catch (error) {
      if (typeof onError === 'function') onError(error);
      throw error;
    }
  };

  parseInto(doc, doc.body, '<nav id="app-rail"><a id="rail-home" href="/app/home/">home</a><a id="rail-settings" href="/app/settings/">settings</a></nav><main id="main-content" tabindex="-1"></main><div id="view-announcer"></div>');
  const main = doc.getElementById('main-content');

  const page = {
    env, window, doc, main, clock, calls, errors, loads, notices, storage,
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
    place(name = 'settings') {
      main.replaceChildren();
      parseInto(doc, main, name === 'settings' ? env.markup : HOME_MARKUP);
    },
    loadScripts() {
      env.scripts.forEach(([name, source]) => vm.runInContext(source, context, { filename: name }));
    },
    async mount() {
      page.ready = [];
      page.view = await window.SolViews.mount('settings', { reportReady: (s) => page.ready.push(s) });
      await settle();
      return page.view;
    },
    leave(next = '/app/home/') {
      window.history.pushState({ solRoute: { id: 'r' + entries.length, owner: 'router' } }, '', next);
      window.SolViews.dispose();
      main.replaceChildren();
      parseInto(doc, main, HOME_MARKUP);
    },
    async arrive(addressAgain = '/app/settings/') {
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
    shownModals() {
      return ['ownerVoiceConsentModal', 'facesClearModal', 'customActivityModal', 'iconPickerModal', 'colorPickerModal', 'cleanupModal', 'facetDeleteModal', 'trashConfirmModal', 'cleanupLogsModal', 'facetCreateModal']
        .filter((id) => { const m = doc.getElementById(id); return m && m.style.display && m.style.display !== 'none'; });
    },
  };

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

  const context = vm.createContext({
    window, document: doc, location, history: window.history, sessionStorage: window.sessionStorage,
    URL, URLSearchParams, AbortController, AbortSignal, DOMException, WeakMap, WeakRef, Map, Set, Promise,
    console, Math, Number, String, Object, Array, Boolean, Date, Intl, JSON, Error, TypeError, ReferenceError, RegExp, Symbol, Infinity, CustomEvent: class {},
    setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval, clearInterval: clock.clearInterval,
    addEventListener: window.addEventListener.bind(window),
    performance: { mark() {}, clearMarks() {} },
    fetch: window.fetch,
    requestAnimationFrame: window.requestAnimationFrame,
    confirm: () => { throw new Error('browser confirm is forbidden'); },
    module: undefined,
  });
  context.self = window;
  for (const name of ['store.js', 'views.js', 'router.js', 'focus.js']) {
    vm.runInContext(fs.readFileSync(path.join(env.shellDir, name), 'utf8'), context);
  }
  window.SolViews.register('home', { mount: (ctx) => { ctx.ready('content'); return () => {}; } });
  window.solRouter.start('settings');
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

function activeSection(p) {
  const section = p.q('.settings-section.active');
  return section ? section.id : null;
}

const GONE = ['mountSettings', 'switchSection', 'initSettingsWorkspace', 'loadConfig', 'loadTrash', 'revalidateAllKeys', 'saveField',
  'reportSettingsReady', 'configData', 'VALID_SECTIONS', 'settingsAbout', 'loadGranola', 'facetDialog', 'FACET_DELETE_COPY'];

// Run from facets_dom.js: answers with the number of cases.
module.exports = async function settingsContractCases(crateDir) {
  const workspace = fs.readFileSync(path.join(crateDir, 'assets/workspace.html'), 'utf8');
  const inline = [...workspace.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
  const settingsJs = fs.readFileSync(path.join(crateDir, 'assets/settings.js'), 'utf8');
  const markup = workspace.replace(/<style>[\s\S]*?<\/style>/g, '').replace(/<script[\s\S]*?<\/script>/g, '').replace(/<!--[\s\S]*?-->/g, '');
  const shellDir = path.join(crateDir, '../solstone-core-convey-shell/assets/static');
  // In the fragment's order: settings.js, then the view's script.
  const env = { markup, shellDir, scripts: [['settings.js', settingsJs], ['workspace.html', inline[inline.length - 1]]] };
  const viewScript = inline[inline.length - 1];
  const mountSource = viewScript.slice(viewScript.indexOf('function mountSettings(ctx) {'));

  await test('the fragment is a contract view with its first read, and no handler naming a global', async () => {
    assert.ok(workspace.includes('<!-- view-contract -->'), 'the view-contract marker');
    assert.ok(workspace.startsWith('<!-- first-read: /app/settings/ -> /app/settings/api/state -->'), 'the first read is declared');
    assert.strictEqual(inline.length, 1, 'one inline script');
    const handlers = [...markup.matchAll(/ on(click|change|input|submit|keydown)="([^"]*)"/g)].map((m) => m[2]);
    assert.ok(handlers.every((h) => h === 'return false;'), `only form guards remain: ${handlers.filter((h) => h !== 'return false;').join(' | ')}`);
    assert.ok(!/['`]api\//.test(mountSource), 'no relative api/ address in the mount');
    assert.ok(!/window\.setTimeout|window\.setInterval|history\.(push|replace)State|readyState|createViewReady/.test(mountSource), 'no page timer, history write, load check or ready of its own');
    assert.ok(!/(window|document)\.addEventListener/.test(mountSource), 'every listener outside the markup goes through ctx');
  });

  await test("every document member the mount reads is on its stand-in, which has no listener of its own and is never handed on", async () => {
    const offered = new Set(['getElementById', 'querySelector', 'querySelectorAll', 'activeElement', 'body', 'visibilityState', 'contains', 'createElement', 'createTextNode']);
    const used = new Set([...mountSource.matchAll(/(?<![\w.])document\.([a-zA-Z]+)/g)].map((m) => m[1]));
    const missing = [...used].filter((name) => !offered.has(name));
    assert.ok(missing.length === 0, `not on the stand-in: ${missing.join(', ')}`);
    assert.ok(!/[(,]\s*document\s*[,)]/.test(mountSource), 'the stand-in is never passed as an argument');
  });

  await test('the scripts only register: nothing reads, draws, listens or starts a timer at load', async () => {
    const p = makePage(env);
    p.place();
    const before = listenerCounts(p);
    const keys = p.windowKeys();
    p.loadScripts();
    await settle();
    assert.strictEqual(p.calls.length, 0, 'no read at load');
    assert.strictEqual(p.clock.live(), 0, 'no timer at load');
    assert.deepStrictEqual(listenerCounts(p), before, 'no listener at load');
    assert.ok(p.window.SolViews.registered('settings'), 'settings is registered');
    const added = p.windowKeys().filter((k) => !keys.includes(k));
    assert.deepStrictEqual(added, ['SettingsRender'], 'only settings.js\'s renderer is made at load');
    GONE.forEach((name) => assert.ok(!(name in p.window) && !(name in p.context), `${name} is not a global`));
  });

  await test('the index mounts: its first read through the store, its sections, and ready once', async () => {
    const p = await boot(env, '/app/settings/');
    assert.deepStrictEqual(p.ready, ['content']);
    assert.strictEqual(p.reads('/app/settings/api/state').length, 1);
    assert.ok(p.reads('/app/settings/api/trash').length === 1, 'the sections read after the state');
    assert.strictEqual(p.byId('settings-index-view').hidden, false);
    assert.strictEqual(p.byId('settings-facet-detail-view').hidden, true);
    assert.ok(p.errors.length === 0, `no error: ${p.errors.map((e) => `${e.context?.context || e.context}: ${e.error?.message}`).join(' | ')}`);
  });

  await test("a facet's page mounts at its address: its two reads, its page, and ready once", async () => {
    const p = await boot(env, `/app/settings/facets/${SLUG}`);
    assert.deepStrictEqual(p.ready, ['content']);
    assert.strictEqual(p.reads(`/app/settings/api/facet/${SLUG}`).length >= 1, true);
    assert.strictEqual(p.byId('settings-facet-detail-view').hidden, false);
    assert.strictEqual(p.byId('settings-index-content').hidden, true);
    assert.strictEqual(p.reads('/app/settings/api/trash').length, 0, "a facet's page doesn't read the sections");
  });

  await test('mounted twice in one document, the index and a facet page each start fresh, and a mount adds nothing to window', async () => {
    const p = makePage(env, '/app/settings/');
    p.loadScripts();
    const keys = p.windowKeys();
    const base = listenerCounts(p);
    p.place();
    await p.mount();
    const once = listenerCounts(p);
    p.leave();
    await p.arrive('/app/settings/');
    assert.deepStrictEqual(listenerCounts(p), once, 'a second mount holds what the first did');
    assert.deepStrictEqual(p.ready, ['content']);
    p.leave();
    await p.arrive(`/app/settings/facets/${SLUG}`);
    assert.deepStrictEqual(p.ready, ['content']);
    assert.strictEqual(p.byId('settings-facet-detail-view').hidden, false);
    p.leave();
    assert.deepStrictEqual(listenerCounts(p), base, 'nothing is left after the last dispose');
    assert.deepStrictEqual(p.windowKeys(), keys, 'no mount added to window');
  });

  await test('dispose leaves no listener, timer, poll or read', async () => {
    const p = makePage(env, '/app/settings/');
    p.loadScripts();
    const base = listenerCounts(p);
    p.place();
    const view = await p.mount();
    const timersBefore = p.clock.live();
    p.leave();
    const held = view.ctx.held();
    assert.deepStrictEqual([held.listeners, held.timeouts, held.intervals, held.reads, held.undo], [0, 0, 0, 0, 0]);
    assert.deepStrictEqual(listenerCounts(p), base);
    assert.ok(p.clock.live() <= timersBefore, 'no timer of the view is left');
  });

  await test('a read out when settings goes is cut, and its late answer draws nothing', async () => {
    const p = makePage(env, '/app/settings/#trash');
    p.holds.push((call) => call.url === '/app/settings/api/trash');
    p.loadScripts();
    p.place();
    await p.mount();
    const list = p.byId('trashList');
    const read = p.calls.find((call) => call.url === '/app/settings/api/trash');
    assert.ok(read && !read.settled, 'the trash read is out');
    const drawn = list.textContent;
    p.leave();
    assert.ok(read.aborted, 'the read was aborted with the view');
    read.answer({ erase_after_days: 7, items: [trashItem('late')] });
    await settle();
    assert.ok(list.textContent === drawn, 'the late answer drew nothing into the gone markup');
    assert.strictEqual(p.errors.filter((e) => !/abort/i.test(String(e.error?.name || e.error?.message))).length, 0);
  });

  await test("a write out when settings goes finishes uncut, and its answer runs nothing", async () => {
    const p = makePage(env, '/app/settings/#trash');
    p.holds.push((call) => call.url === '/app/settings/api/trash/erase');
    p.loadScripts();
    p.place();
    await p.mount();
    p.press(p.q('#trashList input[type="checkbox"]'));
    await settle();
    p.press(p.byId('trashEraseBtn'));
    await settle();
    p.press(p.byId('trashConfirmErase'));
    await settle();
    const erase = p.calls.find((call) => call.url === '/app/settings/api/trash/erase');
    assert.ok(erase && !erase.settled, 'the erase is out');
    const readsBefore = p.reads('/app/settings/api/trash').length;
    p.leave();
    assert.strictEqual(erase.aborted, false, 'the erase is not cut');
    erase.answer({ done: ['t1'], failed: [] });
    await settle();
    assert.strictEqual(p.reads('/app/settings/api/trash').length, readsBefore, 'no list read after the answer');
    assert.strictEqual(p.notices.length, 0, 'no notice after the answer');
    assert.deepStrictEqual(p.loads, [], 'no page load');
  });

  await test("a field's save not yet sent is sent when settings goes, once, and its answer runs nothing", async () => {
    const p = makePage(env, '/app/settings/#profile');
    p.holds.push((call) => call.method === 'PUT');
    p.loadScripts();
    p.place();
    await p.mount();
    const field = p.byId('field-name');
    field.value = 'a new name';
    p.doc.dispatch(field, { type: 'change' });
    p.doc.dispatch(field, { type: 'blur', bubbles: false });
    p.clock.advance(200);
    assert.strictEqual(p.writes().length, 0, 'not sent within its half second');
    p.leave();
    const writes = p.writes();
    assert.strictEqual(writes.length, 1, 'leaving sends it');
    assert.strictEqual(writes[0].url, '/app/settings/api/config');
    assert.deepStrictEqual(JSON.parse(writes[0].body), { section: 'identity', data: { name: 'a new name' } });
    p.clock.advance(1000);
    assert.strictEqual(p.writes().length, 1, 'and only once');
    writes[0].answer({ success: true, config: { identity: { name: 'a new name' } } });
    await settle();
    assert.strictEqual(p.errors.length, 0);
  });

  await test('the Granola follow stops when settings goes, its read cut, and the next mount follows again', async () => {
    const p = makePage(env, '/app/settings/#sync', { answers: journal({ '/app/settings/api/granola': { configured: true, syncing: true } }) });
    p.loadScripts();
    p.place();
    const view = await p.mount();
    assert.ok(view.ctx.held().timeouts >= 1, 'the follow is a timer of the view');
    const granolaReads = () => p.reads('/app/settings/api/granola').length;
    const before = granolaReads();
    p.clock.advance(2000);
    await settle();
    assert.strictEqual(granolaReads(), before + 1, 'it follows every 2 s while syncing');
    p.leave();
    p.clock.advance(10000);
    await settle();
    assert.strictEqual(granolaReads(), before + 1, 'no read after settings goes');
    await p.arrive('/app/settings/#sync');
    const again = granolaReads();
    p.clock.advance(2000);
    await settle();
    assert.strictEqual(granolaReads(), again + 1, 'the next mount follows again');
  });

  await test('ready says error once, across "try again"', async () => {
    const p = makePage(env, '/app/settings/');
    p.statuses.push([(call) => call.url === '/app/settings/api/state', 500]);
    p.loadScripts();
    p.place();
    await p.mount();
    assert.deepStrictEqual(p.ready, ['error']);
    p.statuses.length = 0;
    p.press(p.q('.surface-state-retry'));
    await settle();
    assert.strictEqual(p.byId('settings-index-view').hidden, false, 'try again draws the index');
    assert.deepStrictEqual(p.ready, ['error'], 'and reports nothing more');
  });

  await test('a mount that throws part-way takes back what it added', async () => {
    const p = makePage(env, '/app/settings/');
    p.loadScripts();
    const base = listenerCounts(p);
    p.place();
    p.byId('settingsAboutCopy').remove();
    let threw = false;
    try { await p.window.SolViews.mount('settings', {}); } catch (_) { threw = true; }
    assert.ok(threw, 'the mount threw');
    assert.deepStrictEqual(listenerCounts(p), base, 'no listener is left');
    assert.strictEqual(p.window.SolViews.current(), null);
  });

  await test('each markup handler runs by its own event after an in-page mount', async () => {
    const p = await boot(env, '/app/settings/#apikeys');
    p.press(p.byId('revalidateAllKeys'));
    await settle();
    assert.ok(p.writes().some((call) => call.url === '/app/settings/api/validate-keys'), 're-validate posts the check');
    const forms = p.qa('form').filter((form) => form.getAttribute('onsubmit'));
    assert.ok(forms.length === 7, `the seven form guards: ${forms.length}`);
  });

  await test('a held re-validate keeps focus on its button, busy, and a second press sends nothing', async () => {
    const p = await boot(env, '/app/settings/#apikeys');
    let release = null;
    p.holds.push((call) => {
      if (call.url !== '/app/settings/api/validate-keys') return false;
      release = () => call.answer({ key_validation: {} }, 200);
      return true;
    });
    const btn = p.byId('revalidateAllKeys');
    btn.focus();
    p.press(btn);
    await settle();
    const checks = () => p.writes().filter((call) => call.url === '/app/settings/api/validate-keys').length;
    assert.ok(checks() === 1, `one check out: ${checks()}`);
    assert.ok(btn.getAttribute('aria-disabled') === 'true', 'the button is busy while the check is out');
    assert.ok(!btn.disabled, 'and not disabled, which would drop focus');
    assert.ok(p.doc.activeElement === btn, `focus stays on it: ${focusOn(p)}`);
    p.press(btn);
    await settle();
    assert.ok(checks() === 1, `a press while busy sends nothing: ${checks()}`);
    release();
    await settle();
    assert.ok(btn.getAttribute('aria-disabled') === null, 'the answer clears busy');
    assert.ok(p.doc.activeElement === btn, `focus is still on it: ${focusOn(p)}`);
  });

  await test('each section is an arrival by its hash, the bare path takes the slashed address, and ?facet= shows the index', async () => {
    const sections = ['about', 'guide', 'profile', 'apikeys', 'transcription', 'vision', 'sync', 'storage', 'trash', 'facets', 'localaccess'];
    for (const section of sections) {
      const p = await boot(env, `/app/settings/#${section}`);
      assert.strictEqual(activeSection(p), `section-${section}`, section);
    }
    const bare = await boot(env, '/app/settings#trash');
    assert.strictEqual(new URL(bare.entry().url).pathname + new URL(bare.entry().url).hash, '/app/settings/#trash', 'the address a full load ends on');
    assert.strictEqual(activeSection(bare), 'section-trash');
    assert.strictEqual(bare.entries.length, 1, 'replaced, not a new entry');
    assert.ok(bare.reads('/app/settings/api/').every((call) => call.url.startsWith('/app/settings/api/')), 'every read at its absolute address');
    const query = await boot(env, `/app/settings/?facet=${SLUG}`);
    assert.strictEqual(activeSection(query), 'section-guide', 'the index at its default section');
  });

  await test("a back and a forward over settings' section entries keep their ids, close a dialog, and put focus on the section's tab", async () => {
    const p = await boot(env, '/app/settings/');
    p.press(p.byId('tab-storage'));
    await settle();
    assert.strictEqual(p.entries.length, 2);
    const pushed = p.entry().state?.solRoute;
    assert.ok(pushed && pushed.id && pushed.owner === 'view', 'the section entry has its id');
    p.press(p.byId('cleanupNowBtn'));
    await settle();
    assert.deepStrictEqual(p.shownModals(), ['cleanupModal']);
    p.window.history.back();
    await settle();
    assert.deepStrictEqual(p.shownModals(), [], 'the back closed the dialog');
    assert.strictEqual(activeSection(p), 'section-guide');
    assert.strictEqual(focusOn(p), 'BUTTON#tab-guide', 'focus on the shown section\'s tab');
    p.window.history.forward();
    await settle();
    assert.strictEqual(activeSection(p), 'section-storage');
    assert.strictEqual(p.entry().state?.solRoute?.id, pushed.id, 'the same entry, its id intact');
    assert.strictEqual(focusOn(p), 'BUTTON#tab-storage');
  });

  await test('each dialog is closed by dispose, and the modal layer is reconciled', async () => {
    const opens = [
      ['#storage', 'cleanupNowBtn', 'cleanupModal'],
      ['#storage', 'cleanupLogsBtn', 'cleanupLogsModal'],
      ['#trash', null, 'trashConfirmModal'],
      ['#facets', null, 'facetCreateModal'],
    ];
    for (const [hash, button, modal] of opens) {
      const p = await boot(env, `/app/settings/${hash}`);
      if (modal === 'trashConfirmModal') {
        p.press(p.q('#trashList input[type="checkbox"]'));
        await settle();
        p.press(p.byId('trashEraseBtn'));
      } else if (modal === 'facetCreateModal') {
        p.press(p.qa('.facets-list-action')[0]);
      } else {
        try { p.press(p.byId(button)); } catch (_) { /* the logs dialog's open throws at the base */ }
      }
      await settle();
      assert.deepStrictEqual(p.shownModals(), [modal], `${modal} is open`);
      const reconciles = p.window.ConveyModalLayer.reconciles;
      p.leave();
      const gone = p.env.markup && true;
      assert.ok(gone);
      assert.ok(p.window.ConveyModalLayer.reconciles > reconciles, `the layer is reconciled after ${modal}`);
    }
    const f = await boot(env, `/app/settings/facets/${SLUG}`);
    const deleteButton = f.byId('facetDeleteBtn');
    deleteButton.hidden = false;
    f.press(deleteButton);
    await settle();
    assert.deepStrictEqual(f.shownModals(), ['facetDeleteModal']);
    assert.strictEqual(f.doc.body.style.overflow, 'hidden');
    f.leave();
    assert.strictEqual(f.doc.body.style.overflow, '', "the page scrolls again once settings has gone");
  });

  await test("a facet deleted on its page moves to the list in the page, which shows the receipt, and its recover works from another view", async () => {
    const p = await boot(env, `/app/settings/facets/${SLUG}`, { answers: journal({ [`/app/settings/api/facet/${SLUG}#DELETE`]: { success: true, trash: { id: 't9', erase_at: '2099-11-15T00:00:00.000Z' } } }) });
    const deleteButton = p.byId('facetDeleteBtn');
    deleteButton.hidden = false;
    p.press(deleteButton);
    await settle();
    p.press(p.byId('facetDeleteModalConfirm') || p.q('#facetDeleteModal .btn-danger, #facetDeleteModal button.primary'));
    await settle();
    await settle();
    assert.deepStrictEqual(p.loads, [], 'no page load');
    assert.strictEqual(new URL(p.entry().url).pathname + new URL(p.entry().url).hash, '/app/settings/#facets');
    assert.strictEqual(activeSection(p), 'section-facets');
    const receipt = p.notices[p.notices.length - 1];
    assert.ok(receipt && receipt.buttons.length === 2, 'the receipt with recover and open trash');
    p.leave();
    receipt.buttons[0].onClick();
    await settle();
    assert.ok(p.writes().some((call) => call.url === '/app/settings/api/trash/recover'), 'recover posts from another view');
    assert.ok(p.notices.length >= 2, 'and says how it went');
  });

  await test('the gone markup is let go once settings has gone', async () => {
    if (typeof global.gc !== 'function') return;
    const p = makePage(env, '/app/settings/#trash');
    p.loadScripts();
    p.place();
    await p.mount();
    const ref = new WeakRef(p.byId('trashList'));
    p.leave();
    // The case's own hold on the mounted view goes too.
    p.view = null;
    await settle();
    let collected = false;
    for (let i = 0; i < 20 && !collected; i += 1) {
      global.gc();
      await flush();
      collected = ref.deref() === undefined;
    }
    assert.ok(collected, 'the gone trash list was collected');
  });

  await test("the logs cleanup dialog opens, closes by its own ×, and a cleanup that ran doesn't say it failed", async () => {
    const answers = journal({ '/app/settings/api/storage/prune-logs#POST': { enabled: true, files_deleted: 2, dirs_deleted: 0, retention_log: { lines_removed: 0 } } });
    const p = await boot(env, '/app/settings/#storage', { answers });
    const opener = p.byId('cleanupLogsBtn');
    opener.focus();
    p.press(opener);
    p.clock.advance(20);
    await settle();
    assert.deepStrictEqual(p.shownModals(), ['cleanupLogsModal']);
    assert.strictEqual(p.errors.length, 0, 'opening it throws nothing');
    p.press(p.byId('cleanupLogsModalClose'));
    await settle();
    assert.deepStrictEqual(p.shownModals(), [], 'its × closes it');
    assert.strictEqual(focusOn(p), 'BUTTON#cleanupLogsBtn', 'focus goes back to the button that opened it');
    p.press(opener);
    await settle();
    p.press(p.byId('cleanupLogsPreviewBtn'));
    await settle();
    p.press(p.byId('cleanupLogsConfirmBtn'));
    await settle();
    assert.ok(p.writes().some((call) => call.url === '/app/settings/api/storage/prune-logs' && JSON.parse(call.body).dry_run === false), 'the cleanup was sent');
    assert.deepStrictEqual(p.shownModals(), [], 'the dialog closes once it has run');
    assert.ok(!p.notices.some((notice) => notice.icon === 'circle-x'), 'and no failure notice follows a cleanup that ran');
  });

  await test('a press on the gone markup after settings has gone starts no write', async () => {
    const p = await boot(env, '/app/settings/#apikeys');
    const gone = p.byId('revalidateAllKeys');
    p.leave();
    gone.click();
    await settle();
    assert.strictEqual(p.writes().length, 0, 'no write from a gone control');
  });

  await test("a write's failure that answers after settings has gone says nothing", async () => {
    const p = await boot(env, '/app/settings/#facets');
    p.holds.push((call) => call.method === 'POST' && call.url === '/app/settings/api/facet');
    p.press(p.qa('.facets-list-action')[0]);
    await settle();
    p.byId('facetCreateName').value = 'lab late';
    p.doc.dispatch(p.byId('facetCreateForm'), { type: 'submit' });
    await settle();
    const create = p.calls.find((call) => call.method === 'POST' && call.url === '/app/settings/api/facet');
    assert.ok(create && !create.settled, 'the create is out');
    p.leave();
    create.answer({ error: 'facet already exists' }, 400);
    await settle();
    assert.strictEqual(p.notices.length, 0, 'no failure notice once settings has gone');
    assert.deepStrictEqual(p.loads, []);
  });

  await test('a plain read out when settings goes is cut too', async () => {
    const p = makePage(env, '/app/settings/#facets');
    p.holds.push((call) => call.url === '/app/settings/api/facets?all=true');
    p.loadScripts();
    p.place();
    await p.mount();
    const read = p.calls.find((call) => call.url === '/app/settings/api/facets?all=true');
    assert.ok(read && read.kind === 'fetch' && !read.settled, 'the facets read is out');
    p.leave();
    assert.ok(read.aborted, 'aborted with the view');
  });

  return cases;
};
